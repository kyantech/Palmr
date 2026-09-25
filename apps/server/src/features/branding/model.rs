use crate::features::settings::model::{AppSettings, AssetMode};
#[cfg(test)]
use crate::infra::http::shell::FRESH_INSTALL_APP_NAME;

#[cfg(test)]
pub const FRESH_INSTALL_PRIMARY_COLOR: &str = "#1668dc";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrandingAsset {
    Logo,
    Favicon,
    LoginBackground,
    OgImage,
    EmailLogo,
}

impl BrandingAsset {
    pub const ALL: [Self; 5] = [
        Self::Logo,
        Self::Favicon,
        Self::LoginBackground,
        Self::OgImage,
        Self::EmailLogo,
    ];

    pub fn from_segment(segment: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|asset| asset.segment() == segment)
    }

    pub const fn segment(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
            Self::LoginBackground => "login-background",
            Self::OgImage => "og-image",
            Self::EmailLogo => "email-logo",
        }
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
            Self::LoginBackground => "login_background",
            Self::OgImage => "og_default_image",
            Self::EmailLogo => "email_logo",
        }
    }

    pub const fn public_path(self) -> &'static str {
        match self {
            Self::Logo => "/api/v1/public/branding/logo",
            Self::Favicon => "/api/v1/public/branding/favicon",
            Self::LoginBackground => "/api/v1/public/branding/login-background",
            Self::OgImage => "/api/v1/public/branding/og-image",
            Self::EmailLogo => "/api/v1/public/branding/email-logo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundledAsset {
    Logo,
    Favicon,
    LoginBackground,
    OgImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetResolution {
    Bundled(BundledAsset),
    Custom(BrandingAsset),
    Disabled,
}

impl AssetResolution {
    pub const fn is_served(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaviconState {
    Default,
    Custom,
    Disabled,
}

impl From<AssetMode> for FaviconState {
    fn from(mode: AssetMode) -> Self {
        match mode {
            AssetMode::Default => Self::Default,
            AssetMode::Custom => Self::Custom,
            AssetMode::Disabled => Self::Disabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestSettings<'a> {
    pub app_name: &'a str,
    pub primary_color: &'a str,
    pub favicon: FaviconState,
}

impl<'a> ManifestSettings<'a> {
    pub fn from_settings(settings: &'a AppSettings) -> Self {
        Self {
            app_name: settings.app_name(),
            primary_color: &settings.branding.primary_color,
            favicon: settings.branding.favicon_mode.into(),
        }
    }
}

#[cfg(test)]
impl ManifestSettings<'static> {
    pub const FRESH_INSTALL: Self = Self {
        app_name: FRESH_INSTALL_APP_NAME,
        primary_color: FRESH_INSTALL_PRIMARY_COLOR,
        favicon: FaviconState::Default,
    };
}
