use serde_json::Value;

use super::error::SettingsError;
use crate::domain::bytes::ByteSize;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    General,
    Branding,
    Smtp,
    Security,
    Quotas,
    PublicLinks,
    Retention,
    Audit,
}

impl Group {
    pub const ALL: [Self; 8] = [
        Self::General,
        Self::Branding,
        Self::Smtp,
        Self::Security,
        Self::Quotas,
        Self::PublicLinks,
        Self::Retention,
        Self::Audit,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Branding => "branding",
            Self::Smtp => "smtp",
            Self::Security => "security",
            Self::Quotas => "quotas",
            Self::PublicLinks => "public_links",
            Self::Retention => "retention",
            Self::Audit => "audit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    String,
    Integer,
    Boolean,
    Json,
    Secret,
}

impl ValueType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Json => "json",
            Self::Secret => "secret",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    SetupCompleted,
    AppName,
    AppDescription,
    DefaultLocale,
    ShowVersion,
    PoweredByVisible,
    ThumbnailSourceLimit,
    PrimaryColor,
    LogoMode,
    FaviconMode,
    LoginBackgroundMode,
    OgImageMode,
    EmailLogoMode,
    SmtpEnabled,
    SmtpHost,
    SmtpPort,
    SmtpSecurity,
    SmtpUsername,
    SmtpPassword,
    SmtpFromName,
    SmtpFromEmail,
    SmtpAllowSelfSignedCertificate,
    SmtpNoAuth,
    PasswordMinLength,
    PublicLinkPasswordMinLength,
    MaxLoginAttempts,
    LoginLockoutMinutes,
    SessionIdleDays,
    SessionAbsoluteDays,
    RecentAuthMinutes,
    PasswordResetValidityMinutes,
    InviteValidityHours,
    TwoFactorRequired,
    TrustedDevicesEnabled,
    TrustedDeviceDurationDays,
    DefaultUserQuotaBytes,
    MaxFileSizeBytes,
    MaxPublicLinkLifetimeDays,
    ReceivedRetentionDays,
    AuditRetentionDays,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub key: &'static str,
    pub group: Group,
    pub value_type: ValueType,
    pub slot: Slot,
}

macro_rules! spec {
    ($key:literal, $group:ident, $value_type:ident, $slot:ident) => {
        SettingSpec {
            key: $key,
            group: Group::$group,
            value_type: ValueType::$value_type,
            slot: Slot::$slot,
        }
    };
}

pub const SETTINGS_REGISTRY: &[SettingSpec] = &[
    spec!("setup_completed", General, Boolean, SetupCompleted),
    spec!("app_name", General, String, AppName),
    spec!("app_description", General, String, AppDescription),
    spec!("default_locale", General, String, DefaultLocale),
    spec!("show_version", General, Boolean, ShowVersion),
    spec!("powered_by_visible", General, Boolean, PoweredByVisible),
    spec!(
        "thumbnail_source_limit",
        General,
        String,
        ThumbnailSourceLimit
    ),
    spec!("primary_color", Branding, String, PrimaryColor),
    spec!("logo_mode", Branding, String, LogoMode),
    spec!("favicon_mode", Branding, String, FaviconMode),
    spec!(
        "login_background_mode",
        Branding,
        String,
        LoginBackgroundMode
    ),
    spec!("og_image_mode", Branding, String, OgImageMode),
    spec!("email_logo_mode", Branding, String, EmailLogoMode),
    spec!("smtp_enabled", Smtp, Boolean, SmtpEnabled),
    spec!("smtp_host", Smtp, String, SmtpHost),
    spec!("smtp_port", Smtp, Integer, SmtpPort),
    spec!("smtp_security", Smtp, String, SmtpSecurity),
    spec!("smtp_username", Smtp, String, SmtpUsername),
    spec!("smtp_password", Smtp, Secret, SmtpPassword),
    spec!("smtp_from_name", Smtp, String, SmtpFromName),
    spec!("smtp_from_email", Smtp, String, SmtpFromEmail),
    spec!(
        "smtp_allow_self_signed_certificate",
        Smtp,
        Boolean,
        SmtpAllowSelfSignedCertificate
    ),
    spec!("smtp_no_auth", Smtp, Boolean, SmtpNoAuth),
    spec!("password_min_length", Security, Integer, PasswordMinLength),
    spec!(
        "public_link_password_min_length",
        Security,
        Integer,
        PublicLinkPasswordMinLength
    ),
    spec!("max_login_attempts", Security, Integer, MaxLoginAttempts),
    spec!(
        "login_lockout_minutes",
        Security,
        Integer,
        LoginLockoutMinutes
    ),
    spec!("session_idle_days", Security, Integer, SessionIdleDays),
    spec!(
        "session_absolute_days",
        Security,
        Integer,
        SessionAbsoluteDays
    ),
    spec!("recent_auth_minutes", Security, Integer, RecentAuthMinutes),
    spec!(
        "password_reset_validity_minutes",
        Security,
        Integer,
        PasswordResetValidityMinutes
    ),
    spec!(
        "invite_validity_hours",
        Security,
        Integer,
        InviteValidityHours
    ),
    spec!("two_factor_required", Security, Boolean, TwoFactorRequired),
    spec!(
        "trusted_devices_enabled",
        Security,
        Boolean,
        TrustedDevicesEnabled
    ),
    spec!(
        "trusted_device_duration_days",
        Security,
        Integer,
        TrustedDeviceDurationDays
    ),
    spec!(
        "default_user_quota_bytes",
        Quotas,
        Integer,
        DefaultUserQuotaBytes
    ),
    spec!("max_file_size_bytes", Quotas, Integer, MaxFileSizeBytes),
    spec!(
        "max_public_link_lifetime_days",
        PublicLinks,
        Integer,
        MaxPublicLinkLifetimeDays
    ),
    spec!(
        "received_retention_days",
        Retention,
        Integer,
        ReceivedRetentionDays
    ),
    spec!("audit_retention_days", Audit, Integer, AuditRetentionDays),
];

pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS_REGISTRY.iter().find(|spec| spec.key == key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailSourceLimit {
    MiB64,
    MiB128,
    MiB256,
    MiB512,
    Unlimited,
}

impl ThumbnailSourceLimit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MiB64 => "64MiB",
            Self::MiB128 => "128MiB",
            Self::MiB256 => "256MiB",
            Self::MiB512 => "512MiB",
            Self::Unlimited => "unlimited",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "64MiB" => Some(Self::MiB64),
            "128MiB" => Some(Self::MiB128),
            "256MiB" => Some(Self::MiB256),
            "512MiB" => Some(Self::MiB512),
            "unlimited" => Some(Self::Unlimited),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetMode {
    Default,
    Custom,
    Disabled,
}

impl AssetMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Custom => "custom",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "default" => Some(Self::Default),
            "custom" => Some(Self::Custom),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailLogoMode {
    Inherit,
    Custom,
    None,
}

impl EmailLogoMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Custom => "custom",
            Self::None => "none",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "inherit" => Some(Self::Inherit),
            "custom" => Some(Self::Custom),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    Starttls,
    Implicit,
    None,
}

impl SmtpSecurity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starttls => "starttls",
            Self::Implicit => "implicit",
            Self::None => "none",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "starttls" => Some(Self::Starttls),
            "implicit" => Some(Self::Implicit),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderFieldPolicy {
    Hidden,
    Optional,
    Required,
}

impl SenderFieldPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }
}

#[derive(Debug)]
pub struct GeneralSettings {
    pub setup_completed: bool,
    pub app_name: String,
    pub app_description: String,
    pub default_locale: LocaleCode,
    pub show_version: bool,
    pub powered_by_visible: bool,
    pub thumbnail_source_limit: ThumbnailSourceLimit,
}

#[derive(Debug)]
pub struct BrandingSettings {
    pub primary_color: String,
    pub logo_mode: AssetMode,
    pub favicon_mode: AssetMode,
    pub login_background_mode: AssetMode,
    pub og_image_mode: AssetMode,
    pub email_logo_mode: EmailLogoMode,
}

#[derive(Debug)]
pub struct SmtpSettings {
    pub enabled: bool,
    pub host: Option<String>,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<Secret<String>>,
    pub from_name: Option<String>,
    pub from_email: Option<String>,
    pub allow_self_signed_certificate: bool,
    pub no_auth: bool,
}

#[derive(Debug)]
pub struct SecuritySettings {
    pub password_min_length: u32,
    pub public_link_password_min_length: u32,
    pub max_login_attempts: u32,
    pub login_lockout_minutes: u32,
    pub session_idle_days: u32,
    pub session_absolute_days: u32,
    pub recent_auth_minutes: u32,
    pub password_reset_validity_minutes: u32,
    pub invite_validity_hours: u32,
    pub two_factor_required: bool,
    pub trusted_devices_enabled: bool,
    pub trusted_device_duration_days: u32,
}

#[derive(Debug)]
pub struct QuotaSettings {
    pub default_user_quota_bytes: Option<ByteSize>,
    pub max_file_size_bytes: Option<ByteSize>,
}

#[derive(Debug)]
pub struct PublicLinkSettings {
    pub max_public_link_lifetime_days: Option<u32>,
}

#[derive(Debug)]
pub struct RetentionSettings {
    pub received_retention_days: Option<u32>,
}

#[derive(Debug)]
pub struct AuditSettings {
    pub audit_retention_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentedDefaults {
    pub reverse_share_max_file_size_bytes: Option<ByteSize>,
    pub reverse_share_max_file_count: Option<u32>,
    pub share_expiration_days: Option<u32>,
    pub reverse_share_expiration_days: Option<u32>,
    pub reverse_share_name_field: SenderFieldPolicy,
    pub reverse_share_email_field: SenderFieldPolicy,
    pub reverse_share_description_field: SenderFieldPolicy,
    pub reverse_share_owner_notification: bool,
    pub share_recipient_notification: bool,
    pub provider_auto_provision: bool,
}

impl DocumentedDefaults {
    pub const FRESH_INSTALL: Self = Self {
        reverse_share_max_file_size_bytes: None,
        reverse_share_max_file_count: None,
        share_expiration_days: None,
        reverse_share_expiration_days: None,
        reverse_share_name_field: SenderFieldPolicy::Optional,
        reverse_share_email_field: SenderFieldPolicy::Optional,
        reverse_share_description_field: SenderFieldPolicy::Optional,
        reverse_share_owner_notification: true,
        share_recipient_notification: true,
        provider_auto_provision: false,
    };
}

#[derive(Debug)]
pub struct AppSettings {
    pub general: GeneralSettings,
    pub branding: BrandingSettings,
    pub smtp: SmtpSettings,
    pub security: SecuritySettings,
    pub quotas: QuotaSettings,
    pub public_links: PublicLinkSettings,
    pub retention: RetentionSettings,
    pub audit: AuditSettings,
    pub documented: DocumentedDefaults,
}

impl AppSettings {
    pub fn defaults() -> Self {
        Self {
            general: GeneralSettings {
                setup_completed: false,
                app_name: "Palmr".to_owned(),
                app_description: "Self-hosted file transfer".to_owned(),
                default_locale: LocaleCode::EnUs,
                show_version: true,
                powered_by_visible: true,
                thumbnail_source_limit: ThumbnailSourceLimit::MiB64,
            },
            branding: BrandingSettings {
                primary_color: "#1668dc".to_owned(),
                logo_mode: AssetMode::Default,
                favicon_mode: AssetMode::Default,
                login_background_mode: AssetMode::Default,
                og_image_mode: AssetMode::Default,
                email_logo_mode: EmailLogoMode::Inherit,
            },
            smtp: SmtpSettings {
                enabled: false,
                host: None,
                port: 587,
                security: SmtpSecurity::Starttls,
                username: None,
                password: None,
                from_name: None,
                from_email: None,
                allow_self_signed_certificate: false,
                no_auth: false,
            },
            security: SecuritySettings {
                password_min_length: 8,
                public_link_password_min_length: 8,
                max_login_attempts: 5,
                login_lockout_minutes: 10,
                session_idle_days: 7,
                session_absolute_days: 30,
                recent_auth_minutes: 5,
                password_reset_validity_minutes: 60,
                invite_validity_hours: 24,
                two_factor_required: false,
                trusted_devices_enabled: true,
                trusted_device_duration_days: 30,
            },
            quotas: QuotaSettings {
                default_user_quota_bytes: None,
                max_file_size_bytes: None,
            },
            public_links: PublicLinkSettings {
                max_public_link_lifetime_days: None,
            },
            retention: RetentionSettings {
                received_retention_days: None,
            },
            audit: AuditSettings {
                audit_retention_days: 90,
            },
            documented: DocumentedDefaults::FRESH_INSTALL,
        }
    }

    pub fn setup_completed(&self) -> bool {
        self.general.setup_completed
    }

    pub fn app_name(&self) -> &str {
        &self.general.app_name
    }

    pub fn app_description(&self) -> &str {
        &self.general.app_description
    }

    pub fn default_locale(&self) -> LocaleCode {
        self.general.default_locale
    }

    pub fn smtp_password(&self) -> Option<&Secret<String>> {
        self.smtp.password.as_ref()
    }
}

pub fn apply_value(
    settings: &mut AppSettings,
    spec: &SettingSpec,
    value: &Value,
) -> Result<(), SettingsError> {
    let malformed = || SettingsError::MalformedValue {
        key: spec.key.to_owned(),
    };
    match spec.slot {
        Slot::SetupCompleted => {
            settings.general.setup_completed = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::AppName => {
            settings.general.app_name = string(value).ok_or_else(malformed)?.to_owned();
        }
        Slot::AppDescription => {
            settings.general.app_description = string(value).ok_or_else(malformed)?.to_owned();
        }
        Slot::DefaultLocale => {
            settings.general.default_locale = string(value)
                .and_then(|text| text.parse().ok())
                .ok_or_else(malformed)?;
        }
        Slot::ShowVersion => {
            settings.general.show_version = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::PoweredByVisible => {
            settings.general.powered_by_visible = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::ThumbnailSourceLimit => {
            settings.general.thumbnail_source_limit = string(value)
                .and_then(ThumbnailSourceLimit::parse)
                .ok_or_else(malformed)?;
        }
        Slot::PrimaryColor => {
            settings.branding.primary_color = string(value).ok_or_else(malformed)?.to_owned();
        }
        Slot::LogoMode => {
            settings.branding.logo_mode = string(value)
                .and_then(AssetMode::parse)
                .ok_or_else(malformed)?;
        }
        Slot::FaviconMode => {
            settings.branding.favicon_mode = string(value)
                .and_then(AssetMode::parse)
                .ok_or_else(malformed)?;
        }
        Slot::LoginBackgroundMode => {
            settings.branding.login_background_mode = string(value)
                .and_then(AssetMode::parse)
                .ok_or_else(malformed)?;
        }
        Slot::OgImageMode => {
            settings.branding.og_image_mode = string(value)
                .and_then(AssetMode::parse)
                .ok_or_else(malformed)?;
        }
        Slot::EmailLogoMode => {
            settings.branding.email_logo_mode = string(value)
                .and_then(EmailLogoMode::parse)
                .ok_or_else(malformed)?;
        }
        Slot::SmtpEnabled => {
            settings.smtp.enabled = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::SmtpHost => {
            settings.smtp.host = optional_string(value).ok_or_else(malformed)?;
        }
        Slot::SmtpPort => {
            settings.smtp.port =
                u16::try_from(integer(value).ok_or_else(malformed)?).map_err(|_| malformed())?;
        }
        Slot::SmtpSecurity => {
            settings.smtp.security = string(value)
                .and_then(SmtpSecurity::parse)
                .ok_or_else(malformed)?;
        }
        Slot::SmtpUsername => {
            settings.smtp.username = optional_string(value).ok_or_else(malformed)?;
        }
        Slot::SmtpPassword => return Err(malformed()),
        Slot::SmtpFromName => {
            settings.smtp.from_name = optional_string(value).ok_or_else(malformed)?;
        }
        Slot::SmtpFromEmail => {
            settings.smtp.from_email = optional_string(value).ok_or_else(malformed)?;
        }
        Slot::SmtpAllowSelfSignedCertificate => {
            settings.smtp.allow_self_signed_certificate = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::SmtpNoAuth => {
            settings.smtp.no_auth = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::PasswordMinLength => {
            settings.security.password_min_length = bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::PublicLinkPasswordMinLength => {
            settings.security.public_link_password_min_length =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::MaxLoginAttempts => {
            settings.security.max_login_attempts = bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::LoginLockoutMinutes => {
            settings.security.login_lockout_minutes =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::SessionIdleDays => {
            settings.security.session_idle_days = bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::SessionAbsoluteDays => {
            settings.security.session_absolute_days =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::RecentAuthMinutes => {
            settings.security.recent_auth_minutes = bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::PasswordResetValidityMinutes => {
            settings.security.password_reset_validity_minutes =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::InviteValidityHours => {
            settings.security.invite_validity_hours =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::TwoFactorRequired => {
            settings.security.two_factor_required = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::TrustedDevicesEnabled => {
            settings.security.trusted_devices_enabled = value.as_bool().ok_or_else(malformed)?;
        }
        Slot::TrustedDeviceDurationDays => {
            settings.security.trusted_device_duration_days =
                bounded_integer(value).ok_or_else(malformed)?;
        }
        Slot::DefaultUserQuotaBytes => {
            settings.quotas.default_user_quota_bytes =
                optional_bytes(value).ok_or_else(malformed)?;
        }
        Slot::MaxFileSizeBytes => {
            settings.quotas.max_file_size_bytes = optional_bytes(value).ok_or_else(malformed)?;
        }
        Slot::MaxPublicLinkLifetimeDays => {
            settings.public_links.max_public_link_lifetime_days =
                optional_u32(value).ok_or_else(malformed)?;
        }
        Slot::ReceivedRetentionDays => {
            settings.retention.received_retention_days =
                optional_u32(value).ok_or_else(malformed)?;
        }
        Slot::AuditRetentionDays => {
            settings.audit.audit_retention_days = bounded_integer(value).ok_or_else(malformed)?;
        }
    }
    Ok(())
}

pub fn apply_secret(
    settings: &mut AppSettings,
    spec: &SettingSpec,
    value: Secret<String>,
) -> Result<(), SettingsError> {
    match spec.slot {
        Slot::SmtpPassword => {
            settings.smtp.password = Some(value);
            Ok(())
        }
        _ => Err(SettingsError::MalformedValue {
            key: spec.key.to_owned(),
        }),
    }
}

fn string(value: &Value) -> Option<&str> {
    value.as_str()
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64()
}

fn optional_string(value: &Value) -> Option<Option<String>> {
    if value.is_null() {
        Some(None)
    } else {
        value.as_str().map(|text| Some(text.to_owned()))
    }
}

fn bounded_integer(value: &Value) -> Option<u32> {
    value.as_i64().and_then(|number| u32::try_from(number).ok())
}

fn optional_u32(value: &Value) -> Option<Option<u32>> {
    if value.is_null() {
        Some(None)
    } else {
        bounded_integer(value).map(Some)
    }
}

fn optional_bytes(value: &Value) -> Option<Option<ByteSize>> {
    if value.is_null() {
        Some(None)
    } else {
        value
            .as_i64()
            .and_then(|number| ByteSize::try_from(number).ok())
            .map(Some)
    }
}
