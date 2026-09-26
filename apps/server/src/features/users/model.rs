use crate::domain::bytes::ByteSize;
use crate::domain::email::Email;
use crate::domain::id::Id;
use crate::domain::normalize::normalize;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::domain::username::Username;

pub type UserId = Id<User>;

pub const MAX_DISPLAY_TEXT_CHARS: usize = 100;

pub fn display_text(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let length = trimmed.chars().count();
    let valid =
        (1..=MAX_DISPLAY_TEXT_CHARS).contains(&length) && !trimmed.chars().any(char::is_control);
    valid.then(|| trimmed.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaOverride {
    Inherit,
    Unlimited,
    Bytes(ByteSize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidQuotaOverride;

impl QuotaOverride {
    pub const fn mode(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Unlimited => "unlimited",
            Self::Bytes(_) => "bytes",
        }
    }

    pub const fn quota_bytes(self) -> Option<ByteSize> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Inherit | Self::Unlimited => None,
        }
    }

    pub fn from_columns(
        mode: &str,
        quota_bytes: Option<i64>,
    ) -> Result<Self, InvalidQuotaOverride> {
        match (mode, quota_bytes) {
            ("inherit", None) => Ok(Self::Inherit),
            ("unlimited", None) => Ok(Self::Unlimited),
            ("bytes", Some(bytes)) => ByteSize::try_from(bytes)
                .map(Self::Bytes)
                .map_err(|_| InvalidQuotaOverride),
            _ => Err(InvalidQuotaOverride),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedIdentifier(String);

impl NormalizedIdentifier {
    pub fn from_input(input: &str) -> Self {
        Self(normalize(input))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&Email> for NormalizedIdentifier {
    fn from(email: &Email) -> Self {
        Self(email.normalized().to_owned())
    }
}

impl From<&Username> for NormalizedIdentifier {
    fn from(username: &Username) -> Self {
        Self(username.normalized().to_owned())
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub email_normalized: String,
    pub username: String,
    pub username_normalized: String,
    pub first_name: String,
    pub last_name: String,
    pub password_hash: Option<Secret<String>>,
    pub password_updated_at: Option<Timestamp>,
    pub must_change_password: bool,
    pub role: Role,
    pub is_active: bool,
    pub deactivated_at: Option<Timestamp>,
    pub totp_enabled: bool,
    pub quota: QuotaOverride,
    pub used_bytes: ByteSize,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub created_by: Option<UserId>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: Email,
    pub username: Username,
    pub first_name: String,
    pub last_name: String,
    pub password_hash: Option<Secret<String>>,
    pub must_change_password: bool,
    pub role: Role,
    pub is_active: bool,
    pub quota: QuotaOverride,
    pub created_by: Option<UserId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminState {
    pub role: Role,
    pub is_active: bool,
}

impl AdminState {
    pub const fn is_active_admin(self) -> bool {
        self.is_active && matches!(self.role, Role::Admin)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageRow {
    pub used_bytes: ByteSize,
    pub my_files_bytes: ByteSize,
    pub received_bytes: ByteSize,
    pub reserved_bytes: ByteSize,
    pub quota: QuotaOverride,
}
