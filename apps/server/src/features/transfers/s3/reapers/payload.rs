use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::infra::jobs::NonRetryable;

pub(super) const PAYLOAD_INVALID: &str = "S3_REAPER_PAYLOAD_INVALID";

pub(super) type ExpiryCursor = (String, String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum AbortJob {
    Sweep {},
    Expire {
        after: Option<ExpiryCursor>,
        continuation: u32,
    },
    Abort {
        after: Option<String>,
        continuation: u32,
    },
    Prune {
        continuation: u32,
    },
    Multipart {
        multipart_id: String,
    },
    Orphan {
        bucket: String,
        key: String,
        upload_id: String,
        initiated_at: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum ReconcileJob {
    Sweep {},
    Rows {
        after: Option<ExpiryCursor>,
        continuation: u32,
    },
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(payload: &Value) -> anyhow::Result<T> {
    serde_json::from_value(payload.clone()).map_err(|_| NonRetryable::new(PAYLOAD_INVALID).into())
}
