use serde::Serialize;
use unicode_segmentation::UnicodeSegmentation;
use utoipa::ToSchema;

use super::model::{FaviconState, ManifestSettings};

pub const MANIFEST_CONTENT_TYPE: &str = "application/manifest+json; charset=utf-8";
pub const MANIFEST_BACKGROUND_COLOR: &str = "#ffffff";
pub const SHORT_NAME_MAX_GRAPHEMES: usize = 12;

// Relative, so the browser resolves it against the manifest's own URL: a
// deployment under a PALMR_BASE_URL sub-path keeps its prefix without the
// server ever building an absolute URL.
const FAVICON_ICONS: &[ManifestIcon] = &[ManifestIcon {
    src: "api/v1/public/branding/favicon",
    sizes: "512x512",
    mime_type: "image/png",
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct ManifestIcon {
    src: &'static str,
    sizes: &'static str,
    #[serde(rename = "type")]
    #[schema(rename = "type")]
    mime_type: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct WebAppManifest<'a> {
    name: &'a str,
    short_name: &'a str,
    theme_color: &'a str,
    background_color: &'static str,
    icons: &'static [ManifestIcon],
}

impl<'a> WebAppManifest<'a> {
    pub fn new(settings: &ManifestSettings<'a>) -> Self {
        Self {
            name: settings.app_name,
            short_name: short_name(settings.app_name),
            theme_color: settings.primary_color,
            background_color: MANIFEST_BACKGROUND_COLOR,
            icons: icons(settings.favicon),
        }
    }
}

pub fn short_name(app_name: &str) -> &str {
    let name = app_name.trim();
    match name.grapheme_indices(true).nth(SHORT_NAME_MAX_GRAPHEMES) {
        Some((end, _)) => name[..end].trim_end(),
        None => name,
    }
}

const fn icons(favicon: FaviconState) -> &'static [ManifestIcon] {
    match favicon {
        FaviconState::Default | FaviconState::Custom => FAVICON_ICONS,
        FaviconState::Disabled => &[],
    }
}
