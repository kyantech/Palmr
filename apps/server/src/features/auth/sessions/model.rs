use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::id::Id;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;

pub enum Session {}
pub type SessionId = Id<Session>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    MfaPending,
    Active,
    Revoked,
    Expired,
}

impl SessionState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mfa_pending" => Some(Self::MfaPending),
            "active" => Some(Self::Active),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    PasswordTotp,
    PasswordBackupCode,
    PasswordTrustedDevice,
    External,
    Invite,
    Reset,
}

impl AuthMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PasswordTotp => "password_totp",
            Self::PasswordBackupCode => "password_backup_code",
            Self::PasswordTrustedDevice => "password_trusted_device",
            Self::External => "external",
            Self::Invite => "invite",
            Self::Reset => "reset",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "password" => Some(Self::Password),
            "password_totp" => Some(Self::PasswordTotp),
            "password_backup_code" => Some(Self::PasswordBackupCode),
            "password_trusted_device" => Some(Self::PasswordTrustedDevice),
            "external" => Some(Self::External),
            "invite" => Some(Self::Invite),
            "reset" => Some(Self::Reset),
            _ => None,
        }
    }

    pub const fn recent_auth_hint(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::PasswordTotp | Self::PasswordBackupCode | Self::PasswordTrustedDevice => {
                "password+totp"
            }
            Self::External => "external",
            Self::Invite | Self::Reset => "password",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokedReason {
    Logout,
    UserRequest,
    AdminRequest,
    PasswordChanged,
    PasswordReset,
    RoleChanged,
    Deactivated,
    Deleted,
    MfaAbandoned,
    Rotated,
    PolicyChanged,
    TrustedDeviceRevoked,
}

impl RevokedReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Logout => "logout",
            Self::UserRequest => "user_request",
            Self::AdminRequest => "admin_request",
            Self::PasswordChanged => "password_changed",
            Self::PasswordReset => "password_reset",
            Self::RoleChanged => "role_changed",
            Self::Deactivated => "deactivated",
            Self::Deleted => "deleted",
            Self::MfaAbandoned => "mfa_abandoned",
            Self::Rotated => "rotated",
            Self::PolicyChanged => "policy_changed",
            Self::TrustedDeviceRevoked => "trusted_device_revoked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRestriction {
    None,
    MustChangePassword,
    MustEnrollTotp,
}

#[derive(Clone)]
pub struct NewSession {
    pub user_id: UserId,
    pub auth_method: AuthMethod,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Clone)]
pub struct MintedSession {
    pub id: SessionId,
    pub session_token: Secret<String>,
    pub csrf_token: Secret<String>,
    pub idle_expires_at: Timestamp,
    pub absolute_expires_at: Timestamp,
}

/// Fresh browser credentials prepared before entering a caller-owned write transaction.
///
/// This lets password, role, and MFA state changes rotate a session atomically without
/// performing randomness or other external work while SQLite's single writer is held.
pub struct PreparedSessionCredentials {
    pub(super) session_token: Secret<String>,
    pub(super) csrf_token: Secret<String>,
    pub(super) token_hash: TokenDigest,
    pub(super) csrf_token_hash: TokenDigest,
}

#[derive(Clone)]
pub struct SessionRecord {
    pub id: SessionId,
    pub user_id: UserId,
    pub token_hash: TokenDigest,
    pub csrf_token_hash: TokenDigest,
    pub state: SessionState,
    pub auth_method: AuthMethod,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub last_auth_at: Timestamp,
    pub idle_expires_at: Timestamp,
    pub absolute_expires_at: Timestamp,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Clone)]
pub struct ResolvedSession {
    pub session: SessionRecord,
    pub username: String,
    pub role: Role,
    pub is_active: bool,
    pub must_change_password: bool,
    pub totp_enabled: bool,
}

#[derive(Clone)]
pub struct SessionSummary {
    pub id: SessionId,
    pub auth_method: AuthMethod,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub idle_expires_at: Timestamp,
    pub absolute_expires_at: Timestamp,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    pub user_id: UserId,
    pub username: String,
    pub session_id: SessionId,
    pub role: Role,
    pub restriction: SessionRestriction,
    pub last_auth_at: Timestamp,
    pub recent_auth: bool,
    pub auth_method: AuthMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    Password,
    External,
}

impl From<AuthMethod> for SessionOrigin {
    fn from(method: AuthMethod) -> Self {
        match method {
            AuthMethod::External => Self::External,
            AuthMethod::Password
            | AuthMethod::PasswordTotp
            | AuthMethod::PasswordBackupCode
            | AuthMethod::PasswordTrustedDevice
            | AuthMethod::Invite
            | AuthMethod::Reset => Self::Password,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionItem {
    pub id: String,
    pub is_current: bool,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
    #[schema(required = true)]
    pub ip_address: Option<String>,
    #[schema(required = true)]
    pub user_agent: Option<String>,
    pub origin: SessionOrigin,
}

impl SessionItem {
    pub fn from_summary(record: SessionSummary, current: SessionId) -> Self {
        Self {
            id: record.id.to_string(),
            is_current: record.id == current,
            created_at: record.created_at.to_string(),
            last_seen_at: record.last_seen_at.to_string(),
            expires_at: record.idle_expires_at.to_string(),
            absolute_expires_at: record.absolute_expires_at.to_string(),
            ip_address: record.ip_address,
            user_agent: record.user_agent,
            origin: record.auth_method.into(),
        }
    }
}
