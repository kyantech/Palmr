use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Bounds, Field, Kind};
use crate::features::settings::model::AppSettings;

pub const PASSWORD_MIN_LENGTH: Bounds = Bounds::unbounded_above(8);
pub const MAX_LOGIN_ATTEMPTS: Bounds = Bounds::unbounded_above(3);
pub const LOGIN_LOCKOUT_MINUTES: Bounds = Bounds::unbounded_above(1);
pub const SESSION_IDLE_DAYS: Bounds = Bounds::new(1, 30);
pub const SESSION_ABSOLUTE_DAYS: Bounds = Bounds::new(1, 90);
pub const RECENT_AUTH_MINUTES: Bounds = Bounds::new(1, 15);
pub const PASSWORD_RESET_VALIDITY_MINUTES: Bounds = Bounds::new(5, 1440);
pub const INVITE_VALIDITY_HOURS: Bounds = Bounds::new(1, 720);
pub const TRUSTED_DEVICE_DURATION_DAYS: Bounds = Bounds::new(1, 365);

pub const TWO_FACTOR_REQUIRED_KEY: &str = "two_factor_required";

pub const FIELDS: &[Field] = &[
    Field::new(
        "passwordMinLength",
        "password_min_length",
        Kind::Integer(PASSWORD_MIN_LENGTH),
    ),
    Field::new(
        "publicLinkPasswordMinLength",
        "public_link_password_min_length",
        Kind::Integer(PASSWORD_MIN_LENGTH),
    ),
    Field::new(
        "maxLoginAttempts",
        "max_login_attempts",
        Kind::Integer(MAX_LOGIN_ATTEMPTS),
    ),
    Field::new(
        "loginLockoutMinutes",
        "login_lockout_minutes",
        Kind::Integer(LOGIN_LOCKOUT_MINUTES),
    ),
    Field::new(
        "sessionIdleDays",
        "session_idle_days",
        Kind::Integer(SESSION_IDLE_DAYS),
    ),
    Field::new(
        "sessionAbsoluteDays",
        "session_absolute_days",
        Kind::Integer(SESSION_ABSOLUTE_DAYS),
    ),
    Field::new(
        "recentAuthMinutes",
        "recent_auth_minutes",
        Kind::Integer(RECENT_AUTH_MINUTES),
    ),
    Field::new(
        "passwordResetValidityMinutes",
        "password_reset_validity_minutes",
        Kind::Integer(PASSWORD_RESET_VALIDITY_MINUTES),
    ),
    Field::new(
        "inviteValidityHours",
        "invite_validity_hours",
        Kind::Integer(INVITE_VALIDITY_HOURS),
    ),
    Field::new("twoFactorRequired", TWO_FACTOR_REQUIRED_KEY, Kind::Flag),
    Field::new(
        "trustedDevicesEnabled",
        "trusted_devices_enabled",
        Kind::Flag,
    ),
    Field::new(
        "trustedDeviceDurationDays",
        "trusted_device_duration_days",
        Kind::Integer(TRUSTED_DEVICE_DURATION_DAYS),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySettings {
    #[schema(minimum = 8)]
    pub password_min_length: u32,
    #[schema(minimum = 8)]
    pub public_link_password_min_length: u32,
    #[schema(minimum = 3)]
    pub max_login_attempts: u32,
    #[schema(minimum = 1)]
    pub login_lockout_minutes: u32,
    #[schema(minimum = 1, maximum = 30)]
    pub session_idle_days: u32,
    #[schema(minimum = 1, maximum = 90)]
    pub session_absolute_days: u32,
    #[schema(minimum = 1, maximum = 15)]
    pub recent_auth_minutes: u32,
    #[schema(minimum = 5, maximum = 1440)]
    pub password_reset_validity_minutes: u32,
    #[schema(minimum = 1, maximum = 720)]
    pub invite_validity_hours: u32,
    pub two_factor_required: bool,
    pub trusted_devices_enabled: bool,
    #[schema(minimum = 1, maximum = 365)]
    pub trusted_device_duration_days: u32,
}

impl From<&AppSettings> for SecuritySettings {
    fn from(settings: &AppSettings) -> Self {
        let security = &settings.security;
        Self {
            password_min_length: security.password_min_length,
            public_link_password_min_length: security.public_link_password_min_length,
            max_login_attempts: security.max_login_attempts,
            login_lockout_minutes: security.login_lockout_minutes,
            session_idle_days: security.session_idle_days,
            session_absolute_days: security.session_absolute_days,
            recent_auth_minutes: security.recent_auth_minutes,
            password_reset_validity_minutes: security.password_reset_validity_minutes,
            invite_validity_hours: security.invite_validity_hours,
            two_factor_required: security.two_factor_required,
            trusted_devices_enabled: security.trusted_devices_enabled,
            trusted_device_duration_days: security.trusted_device_duration_days,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecurityPatch {
    /// Platform floor 8.
    #[schema(nullable = false, minimum = 8)]
    pub password_min_length: Option<u32>,
    /// Platform floor 8.
    #[schema(nullable = false, minimum = 8)]
    pub public_link_password_min_length: Option<u32>,
    /// Platform floor 3.
    #[schema(nullable = false, minimum = 3)]
    pub max_login_attempts: Option<u32>,
    /// Platform floor 1.
    #[schema(nullable = false, minimum = 1)]
    pub login_lockout_minutes: Option<u32>,
    #[schema(nullable = false, minimum = 1, maximum = 30)]
    pub session_idle_days: Option<u32>,
    #[schema(nullable = false, minimum = 1, maximum = 90)]
    pub session_absolute_days: Option<u32>,
    #[schema(nullable = false, minimum = 1, maximum = 15)]
    pub recent_auth_minutes: Option<u32>,
    #[schema(nullable = false, minimum = 5, maximum = 1440)]
    pub password_reset_validity_minutes: Option<u32>,
    #[schema(nullable = false, minimum = 1, maximum = 720)]
    pub invite_validity_hours: Option<u32>,
    #[schema(nullable = false)]
    pub two_factor_required: Option<bool>,
    #[schema(nullable = false)]
    pub trusted_devices_enabled: Option<bool>,
    #[schema(nullable = false, minimum = 1, maximum = 365)]
    pub trusted_device_duration_days: Option<u32>,
}
