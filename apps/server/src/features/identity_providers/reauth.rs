use serde::Serialize;
use utoipa::ToSchema;

use super::authorize::{AuthorizeContext, Authorized};
use super::callback::ExternalLoginService;
use super::error::ExternalLoginError;
use super::link_repo;
use super::model::IdentityProvider;
use super::resolve::{refused, ExternalIdentity, LinkState};
use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::auth::sessions::{AuthenticatedPrincipal, SessionError};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReauthResponse {
    pub accepted: bool,
    pub external_reauth_url: String,
}

pub const REAUTH_TRANSACTION: &str = "identity_providers.reauth";

pub(super) struct ReauthCompletion<'a> {
    pub provider: &'a IdentityProvider,
    pub identity: &'a ExternalIdentity,
    pub principal: &'a AuthenticatedPrincipal,
}

impl ExternalLoginService {
    pub async fn start_reauth(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<Authorized, ExternalLoginError> {
        let link = link_repo::find_for_session(
            self.providers().pools().reader(),
            &principal.session_id.to_string(),
        )
        .await?
        .filter(|link| link.user_id == principal.user_id && link.state == LinkState::Active)
        .ok_or_else(|| refused(ErrorCode::ProviderLinkNotFound))?;
        let context = AuthorizeContext::reauth(principal.user_id, None);
        Ok(self
            .providers()
            .authorize(&link.provider_slug, context)
            .await?)
    }

    pub(super) async fn complete_reauth(
        &self,
        completion: ReauthCompletion<'_>,
    ) -> Result<(), ExternalLoginError> {
        let clock = self.providers().clock();
        let principal = completion.principal;
        let session_id = principal.session_id.to_string();
        self.providers()
            .pools()
            .write_tx(clock, REAUTH_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(clock.now())?;
                let session = link_repo::session_in_tx(tx, &session_id, now)
                    .await?
                    .filter(|session| session.live && session.user_id == principal.user_id)
                    .ok_or_else(|| refused(ErrorCode::ProviderStateInvalid))?;
                let link_id = session
                    .identity_link_id
                    .ok_or_else(|| refused(ErrorCode::ProviderLinkNotFound))?;
                let link = link_repo::find_in_tx(tx, link_id)
                    .await?
                    .filter(|link| link.state == LinkState::Active)
                    .ok_or_else(|| refused(ErrorCode::ProviderLinkNotFound))?;
                if link.user_id != principal.user_id
                    || link.provider_id != completion.provider.id
                    || link.subject != completion.identity.subject
                {
                    return Err(refused(ErrorCode::AuthRecentAuthRequired));
                }
                self.auth()
                    .sessions()
                    .mark_reauthenticated_in_tx(tx, principal.session_id)
                    .await
                    .map_err(|error| match error {
                        SessionError::AuthRequired => refused(ErrorCode::ProviderStateInvalid),
                        other => other.into(),
                    })
            })
            .await
    }
}
