use serde::Serialize;
use utoipa::ToSchema;

use super::authorize::{AuthorizeContext, Authorized};
use super::callback::ExternalLoginService;
use super::error::ExternalLoginError;
use super::link_repo::{self, ListedLink};
use super::model::{IdentityLinkId, IdentityProvider};
use super::password_login::{assert_safe_sso_after_change, Projection};
use super::resolve::{
    insert_link, refused, ExternalIdentity, LinkInsert, LinkMethod, ResolveInput,
};
use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::auth::sessions::{AuthenticatedPrincipal, RevokedReason};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::users::model::{User, UserId};
use crate::features::users::repo as users;
use crate::infra::db::WriteTx;
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    CursorKey, Page, PageRequest, QueryParams, SortAllowlist, SortDirection, SortField,
    SortKeyKind, SortValue, TotalCount,
};

pub const LINK_RETURN_TO: &str = "/settings/security";
pub const LINK_TRANSACTION: &str = "identity_providers.link";
pub const UNLINK_TRANSACTION: &str = "identity_providers.unlink";

static LINK_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "linkedAt",
    link_repo::SORT_COLUMN,
    SortKeyKind::Text,
)];
pub static LINK_SORT: SortAllowlist = SortAllowlist::new(&LINK_SORT_FIELDS, 0, SortDirection::Asc)
    .with_id_column(link_repo::ID_COLUMN);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IdentityLinkItem {
    #[schema(example = "0192fe3a-5c4b-7e21-9a02-3f8c1d6e4b90")]
    pub id: String,
    pub provider_slug: String,
    pub provider_display_name: String,
    pub external_subject: String,
    #[schema(required = true)]
    pub email_at_link: Option<String>,
    pub linked_at: String,
    #[schema(required = true)]
    pub last_used_at: Option<String>,
}

impl From<ListedLink> for IdentityLinkItem {
    fn from(link: ListedLink) -> Self {
        Self {
            id: link.id.to_string(),
            provider_slug: link.provider_slug,
            provider_display_name: link.provider_display_name,
            external_subject: link.subject,
            email_at_link: link.email_at_link,
            linked_at: link.linked_at.to_string(),
            last_used_at: link.last_used_at.map(|at| at.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlinkScope {
    SelfService,
    Admin,
}

impl UnlinkScope {
    const fn audit_actor(self) -> &'static str {
        match self {
            Self::SelfService => "self",
            Self::Admin => "admin",
        }
    }
}

pub struct UnlinkCommand<'a> {
    pub actor: &'a AuthenticatedPrincipal,
    pub target: UserId,
    pub link: IdentityLinkId,
    pub scope: UnlinkScope,
    pub client: &'a ClientMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unlinked {
    pub actor_credentials_revoked: bool,
}

pub(super) struct LinkCompletion<'a> {
    pub provider: &'a IdentityProvider,
    pub identity: &'a ExternalIdentity,
    pub principal: &'a AuthenticatedPrincipal,
    pub client: &'a ClientMetadata,
}

impl ExternalLoginService {
    pub async fn start_link(
        &self,
        principal: &AuthenticatedPrincipal,
        slug: &str,
    ) -> Result<Authorized, ExternalLoginError> {
        let context = AuthorizeContext::link(principal.user_id, Some(LINK_RETURN_TO.to_owned()));
        Ok(self.providers().authorize(slug, context).await?)
    }

    pub fn link_query(&self, raw_query: Option<&str>) -> Result<PageRequest, ApiError> {
        let params = QueryParams::parse(raw_query);
        PageRequest::from_query(&params, &LINK_SORT, self.providers().keys())
    }

    pub async fn list_links(
        &self,
        user_id: UserId,
        page: PageRequest,
    ) -> Result<Page<IdentityLinkItem>, ExternalLoginError> {
        let (links, total) =
            link_repo::list(self.providers().pools().reader(), user_id, &page).await?;
        let page = page.into_page(
            links,
            self.providers().keys(),
            |link, _| CursorKey::new(SortValue::Text(link.linked_at.to_string()), link.id),
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page.items.into_iter().map(IdentityLinkItem::from).collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn user_exists(&self, user_id: UserId) -> Result<bool, ExternalLoginError> {
        Ok(
            crate::features::users::admin_repo::exists(self.providers().pools().reader(), user_id)
                .await?,
        )
    }

    pub async fn unlink(&self, command: UnlinkCommand<'_>) -> Result<Unlinked, ExternalLoginError> {
        let clock = self.providers().clock();
        let revoked_actor = command.actor.user_id == command.target;
        self.providers()
            .pools()
            .write_tx(clock, UNLINK_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(clock.now())?;
                let target = users::find_by_id_in_tx(tx, command.target)
                    .await?
                    .ok_or_else(|| refused(ErrorCode::UserNotFound))?;
                let link = link_repo::find_scoped_in_tx(tx, target.id, command.link)
                    .await?
                    .ok_or_else(|| refused(ErrorCode::ProviderLinkNotFound))?;
                assert_unlink_allowed(tx, command.scope, &target, link.id).await?;

                let sessions = self
                    .auth()
                    .sessions()
                    .revoke_all_in_tx(tx, target.id, RevokedReason::IdentityProviderUnlinked)
                    .await?;
                let devices = trusted_devices::revoke_all_in_tx(tx, target.id, now).await?;
                if !link_repo::delete_scoped(tx, target.id, link.id).await? {
                    return Err(refused(ErrorCode::ProviderLinkNotFound));
                }

                let event = AuditEvent::new(
                    actions::identity_link_removed(
                        command.scope.audit_actor(),
                        &link.provider_id.to_string(),
                        &target.id.to_string(),
                        sessions,
                        devices,
                    ),
                    Actor::user(&command.actor.user_id.to_string(), &command.actor.username),
                    Outcome::Success,
                    now,
                )
                .with_target(
                    Target::new(TargetType::IdentityLink)
                        .id(&link.id.to_string())
                        .label(&link.provider_slug),
                )
                .with_client(command.client.clone());
                self.providers().audit().record_in_tx(tx, &event).await?;
                Ok(())
            })
            .await?;
        Ok(Unlinked {
            actor_credentials_revoked: revoked_actor,
        })
    }

    pub(super) async fn complete_link(
        &self,
        completion: LinkCompletion<'_>,
    ) -> Result<(), ExternalLoginError> {
        let clock = self.providers().clock();
        let locale = self.providers().settings().load().default_locale();
        let principal = completion.principal;
        let session_id = principal.session_id.to_string();
        self.providers()
            .pools()
            .write_tx(clock, LINK_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(clock.now())?;
                let session = link_repo::session_in_tx(tx, &session_id, now)
                    .await?
                    .filter(|session| session.live && session.user_id == principal.user_id)
                    .ok_or_else(|| refused(ErrorCode::ProviderStateInvalid))?;
                if !self.recent_auth_open(session.last_auth_at, now)? {
                    return Err(refused(ErrorCode::AuthRecentAuthRequired));
                }
                let user = users::find_by_id_in_tx(tx, principal.user_id)
                    .await?
                    .filter(|user| user.is_active)
                    .ok_or_else(|| refused(ErrorCode::ProviderStateInvalid))?;

                let input = ResolveInput {
                    provider: completion.provider,
                    identity: completion.identity,
                    clock,
                    audit: self.providers().audit(),
                    client: completion.client,
                    locale,
                };
                let email = completion.identity.usable_email();
                match insert_link(
                    tx,
                    &input,
                    &user,
                    email.as_ref(),
                    completion.identity.email_verified,
                    LinkMethod::Manual,
                )
                .await?
                {
                    LinkInsert::Created(_) => Ok(()),
                    LinkInsert::SubjectTaken(link) if link.user_id == user.id => Ok(()),
                    LinkInsert::SubjectTaken(_) | LinkInsert::UserAlreadyLinked => {
                        Err(refused(ErrorCode::ProviderIdentityAlreadyLinked))
                    }
                }
            })
            .await
    }

    pub(super) fn recent_auth_open(
        &self,
        last_auth_at: Timestamp,
        now: Timestamp,
    ) -> Result<bool, ExternalLoginError> {
        let until = self.auth().sessions().recent_auth_until(last_auth_at)?;
        Ok(last_auth_at <= now && now <= until)
    }
}

async fn assert_unlink_allowed(
    tx: &mut WriteTx<'_>,
    scope: UnlinkScope,
    target: &User,
    link: IdentityLinkId,
) -> Result<(), ExternalLoginError> {
    if scope == UnlinkScope::SelfService {
        let remaining = link_repo::count_for_user_in_tx(tx, target.id).await?;
        if target.password_hash.is_none() && remaining <= 1 {
            return Err(refused(ErrorCode::IdentityLinkLastLoginPath));
        }
    }
    assert_safe_sso_after_change(tx, Projection::removing_link(link)).await?;
    Ok(())
}
