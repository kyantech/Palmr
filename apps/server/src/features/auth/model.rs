use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::features::auth::sessions::{SessionRestriction, SessionSummary};
use crate::features::users::model::UserId;

pub const AVATAR_URL: &str = "/api/v1/profile/avatar";

#[derive(Debug, Clone)]
pub struct AccountView {
    pub id: UserId,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub email: String,
    pub pending_email: Option<String>,
    pub role: Role,
    pub is_active: bool,
    pub has_avatar: bool,
    pub has_local_password: bool,
    pub totp_enabled: bool,
    pub locale: String,
    pub theme: String,
    pub accent: String,
    pub created_at: Timestamp,
    pub identity_link_count: u32,
}

impl AccountView {
    fn avatar_url(&self) -> Option<&'static str> {
        self.has_avatar.then_some(AVATAR_URL)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginUser {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub email: String,
    #[schema(example = "admin")]
    pub role: &'static str,
    pub is_active: bool,
    #[schema(required = true, example = "/api/v1/profile/avatar")]
    pub avatar_url: Option<&'static str>,
    #[schema(example = "en-US")]
    pub locale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub user: LoginUser,
    pub must_change_password: bool,
    pub mfa_enrollment_required: bool,
}

impl LoginResponse {
    pub fn new(account: &AccountView, restriction: SessionRestriction) -> Self {
        Self {
            user: LoginUser {
                id: account.id.to_string(),
                first_name: account.first_name.clone(),
                last_name: account.last_name.clone(),
                username: account.username.clone(),
                email: account.email.clone(),
                role: account.role.as_str(),
                is_active: account.is_active,
                avatar_url: account.avatar_url(),
                locale: account.locale.clone(),
            },
            must_change_password: restriction == SessionRestriction::MustChangePassword,
            mfa_enrollment_required: restriction == SessionRestriction::MustEnrollTotp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestrictionName {
    MustChangePassword,
    MfaEnrollmentRequired,
}

impl RestrictionName {
    pub const fn of(restriction: SessionRestriction) -> Option<Self> {
        match restriction {
            SessionRestriction::None => None,
            SessionRestriction::MustChangePassword => Some(Self::MustChangePassword),
            SessionRestriction::MustEnrollTotp => Some(Self::MfaEnrollmentRequired),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeUser {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub email: String,
    #[schema(required = true)]
    pub pending_email: Option<String>,
    #[schema(example = "admin")]
    pub role: &'static str,
    pub is_active: bool,
    #[schema(required = true, example = "/api/v1/profile/avatar")]
    pub avatar_url: Option<&'static str>,
    #[schema(example = "pt-BR")]
    pub locale: String,
    #[schema(example = "system")]
    pub theme: String,
    #[schema(example = "blue")]
    pub accent: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeSession {
    pub id: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub recent_auth_until: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeCapabilities {
    pub can_change_password: bool,
    pub has_local_password: bool,
    pub two_factor_enabled: bool,
    pub identity_link_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user: MeUser,
    pub session: MeSession,
    #[schema(required = true)]
    pub restriction: Option<RestrictionName>,
    pub capabilities: MeCapabilities,
}

pub struct MeParts {
    pub account: AccountView,
    pub session: SessionSummary,
    pub recent_auth_until: Timestamp,
    pub restriction: SessionRestriction,
    pub password_login_enabled: bool,
}

impl MeResponse {
    pub fn new(parts: MeParts) -> Self {
        let MeParts {
            account,
            session,
            recent_auth_until,
            restriction,
            password_login_enabled,
        } = parts;
        Self {
            capabilities: MeCapabilities {
                can_change_password: account.has_local_password && password_login_enabled,
                has_local_password: account.has_local_password,
                two_factor_enabled: account.totp_enabled,
                identity_link_count: account.identity_link_count,
            },
            session: MeSession {
                id: session.id.to_string(),
                created_at: session.created_at.to_string(),
                last_seen_at: session.last_seen_at.to_string(),
                expires_at: session.idle_expires_at.to_string(),
                recent_auth_until: recent_auth_until.to_string(),
            },
            restriction: RestrictionName::of(restriction),
            user: MeUser::from(account),
        }
    }
}

impl From<AccountView> for MeUser {
    fn from(account: AccountView) -> Self {
        Self {
            avatar_url: account.avatar_url(),
            id: account.id.to_string(),
            first_name: account.first_name,
            last_name: account.last_name,
            username: account.username,
            email: account.email,
            pending_email: account.pending_email,
            role: account.role.as_str(),
            is_active: account.is_active,
            locale: account.locale,
            theme: account.theme,
            accent: account.accent,
            created_at: account.created_at.to_string(),
        }
    }
}
