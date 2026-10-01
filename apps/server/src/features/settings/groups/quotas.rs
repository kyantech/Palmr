use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Bounds, Field, Kind, MAX_SAFE_BYTES};
use crate::domain::bytes::ByteSize;
use crate::features::settings::model::AppSettings;

pub const BYTES: Bounds = Bounds::new(0, MAX_SAFE_BYTES);

pub const FIELDS: &[Field] = &[
    Field::new(
        "defaultUserQuotaBytes",
        "default_user_quota_bytes",
        Kind::OptionalInteger(BYTES),
    ),
    Field::new(
        "maxFileSizeBytes",
        "max_file_size_bytes",
        Kind::OptionalInteger(BYTES),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSettings {
    /// `null` is Unlimited.
    #[schema(required = true, minimum = 0, maximum = 9_007_199_254_740_991_u64)]
    pub default_user_quota_bytes: Option<u64>,
    /// `null` is Unlimited.
    #[schema(required = true, minimum = 0, maximum = 9_007_199_254_740_991_u64)]
    pub max_file_size_bytes: Option<u64>,
}

impl From<&AppSettings> for QuotaSettings {
    fn from(settings: &AppSettings) -> Self {
        Self {
            default_user_quota_bytes: settings.quotas.default_user_quota_bytes.map(ByteSize::get),
            max_file_size_bytes: settings.quotas.max_file_size_bytes.map(ByteSize::get),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaPatch {
    /// Absent leaves the value unchanged; explicit `null` stores Unlimited.
    #[schema(value_type = Option<u64>, nullable = true, minimum = 0, maximum = 9_007_199_254_740_991_u64)]
    pub default_user_quota_bytes: Option<Option<u64>>,
    /// Absent leaves the value unchanged; explicit `null` stores Unlimited.
    #[schema(value_type = Option<u64>, nullable = true, minimum = 0, maximum = 9_007_199_254_740_991_u64)]
    pub max_file_size_bytes: Option<Option<u64>>,
}
