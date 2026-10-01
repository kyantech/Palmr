use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Bounds, Field, Kind};
use crate::features::settings::model::AppSettings;

pub const LIFETIME_DAYS: Bounds = Bounds::unbounded_above(1);

pub const FIELDS: &[Field] = &[Field::new(
    "maxPublicLinkLifetimeDays",
    "max_public_link_lifetime_days",
    Kind::OptionalInteger(LIFETIME_DAYS),
)];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicLinkSettings {
    /// `null` means no maximum.
    #[schema(required = true, minimum = 1)]
    pub max_public_link_lifetime_days: Option<u32>,
}

impl From<&AppSettings> for PublicLinkSettings {
    fn from(settings: &AppSettings) -> Self {
        Self {
            max_public_link_lifetime_days: settings.public_links.max_public_link_lifetime_days,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicLinkPatch {
    /// Absent leaves the value unchanged; explicit `null` removes the maximum.
    #[schema(value_type = Option<u32>, nullable = true, minimum = 1)]
    pub max_public_link_lifetime_days: Option<Option<u32>>,
}
