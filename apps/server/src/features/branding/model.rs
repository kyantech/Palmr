use crate::infra::http::shell::FRESH_INSTALL_APP_NAME;

pub const FRESH_INSTALL_PRIMARY_COLOR: &str = "#1668dc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "custom and disabled favicons are selected once branding modes are persisted"
    )
)]
pub enum FaviconState {
    Default,
    Custom,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestSettings<'a> {
    pub app_name: &'a str,
    pub primary_color: &'a str,
    pub favicon: FaviconState,
}

impl ManifestSettings<'static> {
    pub const FRESH_INSTALL: Self = Self {
        app_name: FRESH_INSTALL_APP_NAME,
        primary_color: FRESH_INSTALL_PRIMARY_COLOR,
        favicon: FaviconState::Default,
    };
}
