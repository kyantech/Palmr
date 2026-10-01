use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Field, Kind};
use crate::features::settings::model::AppSettings;

pub const FIELDS: &[Field] = &[
    Field::new("appName", "app_name", Kind::Name),
    Field::new("appDescription", "app_description", Kind::Description),
    Field::new("defaultLocale", "default_locale", Kind::Locale),
    Field::new("hideVersion", "show_version", Kind::InvertedFlag),
    Field::new("poweredByVisible", "powered_by_visible", Kind::Flag),
    Field::new(
        "thumbnailSourceLimit",
        "thumbnail_source_limit",
        Kind::ThumbnailLimit,
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    #[schema(example = "Palmr", min_length = 1, max_length = 100)]
    pub app_name: String,
    #[schema(example = "Self-hosted file transfer", max_length = 300)]
    pub app_description: String,
    #[schema(example = "en-US")]
    pub default_locale: String,
    pub hide_version: bool,
    pub powered_by_visible: bool,
    #[schema(example = "64MiB")]
    pub thumbnail_source_limit: ThumbnailSourceLimitName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum ThumbnailSourceLimitName {
    #[serde(rename = "64MiB")]
    #[schema(rename = "64MiB")]
    MiB64,
    #[serde(rename = "128MiB")]
    #[schema(rename = "128MiB")]
    MiB128,
    #[serde(rename = "256MiB")]
    #[schema(rename = "256MiB")]
    MiB256,
    #[serde(rename = "512MiB")]
    #[schema(rename = "512MiB")]
    MiB512,
    #[serde(rename = "unlimited")]
    #[schema(rename = "unlimited")]
    Unlimited,
}

impl From<&AppSettings> for GeneralSettings {
    fn from(settings: &AppSettings) -> Self {
        use crate::features::settings::model::ThumbnailSourceLimit as Limit;
        let general = &settings.general;
        Self {
            app_name: general.app_name.clone(),
            app_description: general.app_description.clone(),
            default_locale: general.default_locale.as_str().to_owned(),
            hide_version: !general.show_version,
            powered_by_visible: general.powered_by_visible,
            thumbnail_source_limit: match general.thumbnail_source_limit {
                Limit::MiB64 => ThumbnailSourceLimitName::MiB64,
                Limit::MiB128 => ThumbnailSourceLimitName::MiB128,
                Limit::MiB256 => ThumbnailSourceLimitName::MiB256,
                Limit::MiB512 => ThumbnailSourceLimitName::MiB512,
                Limit::Unlimited => ThumbnailSourceLimitName::Unlimited,
            },
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeneralPatch {
    /// Trimmed, 1–100 characters, no control characters.
    #[schema(nullable = false, example = "Palmr", min_length = 1, max_length = 100)]
    pub app_name: Option<String>,
    /// Trimmed, at most 300 characters, no control characters. The empty string clears the description.
    #[schema(nullable = false, max_length = 300)]
    pub app_description: Option<String>,
    /// One of the 23 supported locales.
    #[schema(nullable = false, example = "en-US")]
    pub default_locale: Option<String>,
    #[schema(nullable = false)]
    pub hide_version: Option<bool>,
    #[schema(nullable = false)]
    pub powered_by_visible: Option<bool>,
    #[schema(nullable = false)]
    pub thumbnail_source_limit: Option<ThumbnailSourceLimitName>,
}
