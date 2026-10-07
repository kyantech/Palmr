use serde::Serialize;
use utoipa::ToSchema;

use super::authorize::{AuthorizeContext, Authorized};
use super::callback::{ExternalLoginService, REAUTH_COMPLETE_PATH};
use super::error::{ExternalLoginError, ProviderError};
use super::link_repo;
use super::model::IdentityProvider;
use super::resolve::{refused, ExternalIdentity, LinkState};
use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::auth::sessions::{AuthenticatedPrincipal, SessionError};
use crate::infra::crypto::token::Token;
use crate::infra::crypto::CryptoError;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReauthResponse {
    pub accepted: bool,
    pub external_reauth_url: String,
    /// Opaque 256-bit base64url correlation identifier of this one challenge.
    /// It names the same-origin `BroadcastChannel` that carries the completion
    /// notification; it grants nothing and refreshes nothing.
    #[schema(example = "AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s")]
    pub external_reauth_channel: String,
}

pub const REAUTH_TRANSACTION: &str = "identity_providers.reauth";
pub const REAUTH_CHANNEL_PARAMETER: &str = "channel";

pub struct ReauthStarted {
    pub authorized: Authorized,
    pub channel: String,
}

pub fn mint_channel() -> Result<String, CryptoError> {
    Ok(Token::mint()?.encode().expose_secret().clone())
}

pub fn completion_target(channel: &str) -> String {
    format!("{REAUTH_COMPLETE_PATH}?{REAUTH_CHANNEL_PARAMETER}={channel}")
}

/// The channel carried by a stored completion target, only when the target is
/// exactly the server-built form `/auth/reauth-complete?channel=<256-bit
/// base64url>`; anything else is not a channel.
pub fn channel_of_target(target: &str) -> Option<&str> {
    let channel = target
        .strip_prefix(REAUTH_COMPLETE_PATH)?
        .strip_prefix('?')?
        .strip_prefix(REAUTH_CHANNEL_PARAMETER)?
        .strip_prefix('=')?;
    Token::decode(channel).ok().map(|_| channel)
}

pub(super) struct ReauthCompletion<'a> {
    pub provider: &'a IdentityProvider,
    pub identity: &'a ExternalIdentity,
    pub principal: &'a AuthenticatedPrincipal,
}

impl ExternalLoginService {
    pub async fn start_reauth(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<ReauthStarted, ExternalLoginError> {
        let link = link_repo::find_for_session(
            self.providers().pools().reader(),
            &principal.session_id.to_string(),
        )
        .await?
        .filter(|link| link.user_id == principal.user_id && link.state == LinkState::Active)
        .ok_or_else(|| refused(ErrorCode::ProviderLinkNotFound))?;
        let channel = mint_channel().map_err(ProviderError::from)?;
        let context = AuthorizeContext::reauth(principal.user_id, &channel);
        let authorized = self
            .providers()
            .authorize(&link.provider_slug, context)
            .await?;
        Ok(ReauthStarted {
            authorized,
            channel,
        })
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

#[cfg(test)]
mod tests {
    use super::{channel_of_target, completion_target, mint_channel};
    use crate::infra::crypto::token::{Token, ENCODED_TOKEN_LEN};

    #[test]
    fn unit_reauth_channel_is_256_bit_base64url_and_unique() {
        let channels: Vec<String> = (0..64).map(|_| mint_channel().unwrap()).collect();
        for (index, channel) in channels.iter().enumerate() {
            assert_eq!(channel.len(), ENCODED_TOKEN_LEN);
            assert!(channel
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
            assert_eq!(
                Token::decode(channel).unwrap().expose_secret().len() * 8,
                256
            );
            assert!(!channels[index + 1..].contains(channel));
        }
    }

    #[test]
    fn unit_reauth_completion_target_round_trips_only_the_exact_form() {
        let channel = mint_channel().unwrap();
        let target = completion_target(&channel);
        assert_eq!(target, format!("/auth/reauth-complete?channel={channel}"));
        assert_eq!(channel_of_target(&target), Some(channel.as_str()));

        for rejected in [
            String::new(),
            "/overview".to_owned(),
            "/auth/reauth-complete".to_owned(),
            "/auth/reauth-complete?channel=".to_owned(),
            "/auth/reauth-complete?channel=short".to_owned(),
            format!("/auth/reauth-complete?channel={channel}&x=1"),
            format!("/auth/reauth-complete?channel={channel}x"),
            format!("/auth/reauth-complete?status=success&channel={channel}"),
            format!("/auth/reauth-complete/?channel={channel}"),
            format!("//auth/reauth-complete?channel={channel}"),
            format!("/auth/reauth-complete?channel={}", "!".repeat(43)),
            format!("/auth/reauth-complete?channel=%2F{}", &channel[3..]),
        ] {
            assert_eq!(channel_of_target(&rejected), None, "{rejected}");
        }
    }
}
