use serde::Serialize;
use utoipa::ToSchema;

use crate::app::health::VERSION;
use crate::domain::locale::LocaleCode;
use crate::features::branding::model::BrandingAsset;
use crate::features::branding::service::public_url;
use crate::features::settings::model::AppSettings;

const PASSWORD_LOGIN_ENABLED: bool = true;

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
            password_login_enabled: PASSWORD_LOGIN_ENABLED,
            providers: Vec::new(),
            powered_by_visible: settings.general.powered_by_visible,
            version: settings.general.show_version.then_some(VERSION),
        }
    }
}
