use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::SetupError;
use crate::app::health::VERSION;
use crate::domain::email::Email;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::domain::username::Username;
use crate::features::auth::login::password_login_enabled;
use crate::features::branding::model::BrandingAsset;
use crate::features::branding::service::public_url;
use crate::features::settings::model::AppSettings;
use crate::features::users::model::User;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

pub const DEFAULT_APP_DESCRIPTION: &str = "Self-hosted file transfer";
pub const MAX_DISPLAY_TEXT_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub setup_completed: bool,
    pub app_name: String,
    pub app_description: String,
    #[schema(required = true)]
    pub logo_url: Option<&'static str>,
    #[schema(required = true)]
    pub favicon_url: Option<&'static str>,
    pub primary_color: String,
    pub default_locale: &'static str,
    pub supported_locales: Vec<&'static str>,
    pub password_login_enabled: bool,
    pub providers: Vec<BootstrapProvider>,
    pub powered_by_visible: bool,
    #[schema(required = true)]
    pub version: Option<&'static str>,
}

#[allow(
    dead_code,
    reason = "external identity providers populate this list once they exist"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapProvider {
    pub slug: String,
    pub display_name: String,
    pub icon_key: String,
    pub sort_order: i64,
}

impl Bootstrap {
    pub fn from_settings(settings: &AppSettings) -> Self {
        let branding = &settings.branding;
        Self {
            setup_completed: settings.setup_completed(),
            app_name: settings.app_name().to_owned(),
            app_description: settings.app_description().to_owned(),
            logo_url: public_url(branding, BrandingAsset::Logo),
            favicon_url: public_url(branding, BrandingAsset::Favicon),
            primary_color: branding.primary_color.clone(),
            default_locale: settings.default_locale().as_str(),
            supported_locales: LocaleCode::ALL.iter().map(|code| code.as_str()).collect(),
            password_login_enabled: password_login_enabled(settings),
            providers: Vec::new(),
            powered_by_visible: settings.general.powered_by_visible,
            version: settings.general.show_version.then_some(VERSION),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub setup_completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub password_min_length: Option<u32>,
}

impl SetupStatus {
    pub fn from_settings(settings: &AppSettings) -> Self {
        if settings.setup_completed() {
            Self {
                setup_completed: true,
                password_min_length: None,
            }
        } else {
            Self {
                setup_completed: false,
                password_min_length: Some(
                    AccountPasswordPolicy::from_settings(settings).min_length(),
                ),
            }
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupRequest {
    #[schema(example = "Palmr")]
    pub app_name: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub email: String,
    #[schema(format = Password)]
    pub password: String,
    #[schema(example = "en-US")]
    pub locale: String,
}

pub struct SetupInput {
    pub app_name: String,
    pub first_name: String,
    pub last_name: String,
    pub username: Username,
    pub email: Email,
    pub password: Secret<String>,
    pub locale: LocaleCode,
}

impl JsonRequest for SetupRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("appName", JsonKind::String),
        JsonField::required("firstName", JsonKind::String),
        JsonField::required("lastName", JsonKind::String),
        JsonField::required("username", JsonKind::String),
        JsonField::required("email", JsonKind::String),
        JsonField::required("password", JsonKind::String),
        JsonField::required("locale", JsonKind::String),
    ];
}

impl SetupInput {
    pub fn parse(request: SetupRequest) -> Result<Self, SetupError> {
        let SetupRequest {
            app_name,
            first_name,
            last_name,
            username,
            email,
            password,
            locale,
        } = request;
        let password = Secret::new(password);
        let mut invalid = Vec::new();
        let app_name = display_text(&app_name, "appName", &mut invalid);
        let first_name = display_text(&first_name, "firstName", &mut invalid);
        let last_name = display_text(&last_name, "lastName", &mut invalid);
        let username = checked(Username::parse(&username).ok(), "username", &mut invalid);
        let email = checked(Email::parse(&email).ok(), "email", &mut invalid);
        let locale = checked(locale.parse().ok(), "locale", &mut invalid);
        match (app_name, first_name, last_name, username, email, locale) {
            (
                Some(app_name),
                Some(first_name),
                Some(last_name),
                Some(username),
                Some(email),
                Some(locale),
            ) if invalid.is_empty() => Ok(Self {
                app_name,
                first_name,
                last_name,
                username,
                email,
                password,
                locale,
            }),
            _ => Err(SetupError::Invalid { fields: invalid }),
        }
    }
}

fn display_text(
    input: &str,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<String> {
    let trimmed = input.trim();
    let length = trimmed.chars().count();
    let valid =
        (1..=MAX_DISPLAY_TEXT_CHARS).contains(&length) && !trimmed.chars().any(char::is_control);
    checked(valid.then(|| trimmed.to_owned()), field, invalid)
}

fn checked<T>(
    parsed: Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<T> {
    if parsed.is_none() {
        invalid.push(field);
    }
    parsed
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupUser {
    pub id: String,
    pub username: String,
    pub email: String,
    #[schema(example = "admin")]
    pub role: &'static str,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupResponse {
    pub user: SetupUser,
    pub must_change_password: bool,
}

impl SetupResponse {
    pub fn from_user(user: &User) -> Self {
        Self {
            user: SetupUser {
                id: user.id.to_string(),
                username: user.username.clone(),
                email: user.email.clone(),
                role: user.role.as_str(),
                is_active: user.is_active,
            },
            must_change_password: user.must_change_password,
        }
    }
}
