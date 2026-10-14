use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::id::Id;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::infra::http::pagination::WireBytes;

use super::super::model::wire_bytes;

pub enum Multipart {}

pub type MultipartId = Id<Multipart>;

pub const MAX_REPORTED_PARTS: usize = 1_000;
pub const MAX_ETAG_CHARS: usize = 128;
pub const MAX_PROVIDER_PARTS: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultipartState {
    Created,
    InProgress,
    Completing,
    Completed,
    Aborted,
    Abandoned,
}

impl MultipartState {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "created" => Some(Self::Created),
            "in_progress" => Some(Self::InProgress),
            "completing" => Some(Self::Completing),
            "completed" => Some(Self::Completed),
            "aborted" => Some(Self::Aborted),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct S3Ref {
    pub id: MultipartId,
    pub upload_id: String,
    pub bucket: String,
    pub object_key: String,
    pub total: Option<ByteSize>,
    pub part_size: u64,
    pub part_count: u32,
    pub state: MultipartState,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateMultipartResponse {
    /// The Palmr handle of the multipart upload. It is not the provider's upload id and addresses nothing by itself: every route is addressed by session and item.
    pub multipart_id: String,
    /// The size of every part except the last.
    pub part_size_bytes: WireBytes,
    #[schema(minimum = 1)]
    pub part_count: u32,
    #[schema(minimum = 1)]
    pub max_presign_batch: u32,
    #[schema(minimum = 1)]
    pub presign_ttl_seconds: u32,
    /// `true` when the storage profile requires checksum headers that a browser cannot produce. Parts then go through the server-proxied path and part URLs are not issued.
    pub checksum_headers_required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresignRequest {
    /// One to 16 part numbers in `1..=partCount`, without repeats.
    #[schema(value_type = Vec<i64>, min_items = 1, max_items = 16, example = json!([1, 2, 3]))]
    pub part_numbers: Vec<Value>,
}

impl JsonRequest for PresignRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("partNumbers", JsonKind::Array)];
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresignedPart {
    #[schema(minimum = 1)]
    pub part_number: u32,
    /// A presigned `PUT` for exactly this part. It signs no header besides `host`: send the part bytes and nothing else. Never store it.
    pub url: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresignResponse {
    pub parts: Vec<PresignedPart>,
    #[schema(minimum = 1)]
    pub ttl_seconds: u32,
    pub expires_at: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportedPart {
    #[schema(minimum = 1, example = 1)]
    pub part_number: i64,
    /// The `ETag` response header of the part upload, verbatim, quotes included.
    #[schema(example = "\"9b2cf535f27731c974343645a3985328\"")]
    pub etag: String,
    #[schema(minimum = 1, example = 8_388_608)]
    pub size_bytes: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportPartsRequest {
    /// One to 1 000 parts the storage provider acknowledged. A hint only: reconciliation against the provider decides what exists.
    #[schema(min_items = 1, max_items = 1000)]
    pub parts: Vec<ReportedPart>,
}

impl JsonRequest for ReportPartsRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("parts", JsonKind::Array)];
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletedPartView {
    #[schema(minimum = 1)]
    pub part_number: u32,
    pub etag: String,
    pub size_bytes: WireBytes,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartsResponse {
    /// The parts the storage provider holds, with the sizes it reports.
    pub completed_parts: Vec<CompletedPartView>,
    /// The parts of the plan the provider does not hold, ascending. Only these are signed and uploaded again.
    pub missing_part_numbers: Vec<u32>,
    /// The sum of the provider-reported sizes of the held parts.
    pub uploaded_bytes: WireBytes,
    pub part_size_bytes: WireBytes,
    #[schema(minimum = 1)]
    pub part_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CompletedState {
    Completed,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteResponse {
    pub file_id: String,
    pub size_bytes: WireBytes,
    pub content_type: String,
    pub state: CompletedState,
}

pub fn part_bytes(bytes: u64) -> WireBytes {
    wire_bytes(ByteSize::try_from(bytes).unwrap_or(ByteSize::ZERO))
}
