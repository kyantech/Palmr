use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::client_key::ClientFileKey;
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::domain::naming::NameCandidate;
use crate::domain::relative_path::{DirectoryPath, RelativePath};
use crate::features::folders::{FolderError, FolderId};
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::infra::http::pagination::{WireBytes, MAX_WIRE_BYTES};

use super::error::TransferError;
use super::state::{TransferItemState, TransferSessionState};

pub use crate::features::quota::model::TransferSessionId;

pub enum SessionItem {}

pub type SessionItemId = Id<SessionItem>;

pub const MAX_FILES_PER_SESSION: usize = 2_000;
pub const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const TUS_CREATE_URL: &str = "/api/v1/uploads/tus";
pub const MAX_CONTENT_TYPE_CHARS: usize = 255;
pub const MAX_PRESIGN_BATCH: u32 = 16;
pub const PRESIGN_TTL_SECONDS: u32 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransferProvider {
    Local,
    S3,
}

impl TransferProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "local" => Some(Self::Local),
            "s3" => Some(Self::S3),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadKind {
    Tus,
    S3Multipart,
    S3Single,
}

impl UploadKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tus => "tus",
            Self::S3Multipart => "s3_multipart",
            Self::S3Single => "s3_single",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "tus" => Some(Self::Tus),
            "s3_multipart" => Some(Self::S3Multipart),
            "s3_single" => Some(Self::S3Single),
            _ => None,
        }
    }

    pub const fn protocol(self) -> TransferProtocol {
        match self {
            Self::Tus => TransferProtocol::Tus,
            Self::S3Multipart => TransferProtocol::S3Multipart,
            Self::S3Single => TransferProtocol::S3Single,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub enum TransferProtocol {
    #[serde(rename = "tus")]
    Tus,
    #[serde(rename = "s3-multipart")]
    S3Multipart,
    #[serde(rename = "s3-single")]
    S3Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferTargetKind {
    MyFiles,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferTarget {
    pub kind: TransferTargetKind,
    /// The My Files folder the upload lands in. `null` or absent targets the My Files root. The target is fixed when the session is created and can never be changed.
    #[schema(nullable = true, example = "0192f3a1-0000-7000-8000-000000000001")]
    pub folder_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferFileRequest {
    /// The caller's own opaque key for this file, unique within the request: 1 to 128 characters, none of them a control character. It identifies the item in later responses and is never used as a storage key.
    #[schema(example = "c1")]
    pub client_id: String,
    /// The file name, 1 to 255 bytes, with no `/`, `\`, control characters, `.` or `..`.
    #[schema(example = "video.mkv")]
    pub name: String,
    /// The declared size in bytes, up to 9007199254740991. `null` or absent means the size is not known in advance.
    #[serde(default)]
    #[schema(value_type = Option<i64>, nullable = true, minimum = 0, maximum = 9_007_199_254_740_991_i64, example = 53_687_091_200_i64)]
    pub size_bytes: Option<Value>,
    /// The full path of the file relative to the selected upload root, including the file name, with `/` as the separator. Absent or empty places the file directly in the target folder. When present its last segment must equal `name`.
    #[serde(default)]
    #[schema(example = "Trip/Day 1/video.mkv")]
    pub relative_path: Option<String>,
    /// Advisory MIME type. It is validated for shape only and never decides how the content is stored or served.
    #[serde(default)]
    #[schema(example = "video/x-matroska")]
    pub declared_content_type: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTransferSessionRequest {
    pub target: TransferTarget,
    /// One to 2 000 files. More than 2 000 is `BATCH_TOO_LARGE`.
    #[schema(value_type = Vec<TransferFileRequest>, min_items = 1, max_items = 2000)]
    pub files: Vec<Value>,
}

impl JsonRequest for CreateTransferSessionRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("target", JsonKind::Object),
        JsonField::required("files", JsonKind::Array),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub ordinal: u32,
    pub client_key: ClientFileKey,
    pub name: NameCandidate,
    pub directory: DirectoryPath,
    pub size: Option<ByteSize>,
}

impl PlannedFile {
    pub fn relative_path(&self) -> String {
        self.directory.joined()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSession {
    pub target: Option<FolderId>,
    pub files: Vec<PlannedFile>,
}

impl ValidatedSession {
    pub fn parse(request: CreateTransferSessionRequest) -> Result<Self, TransferError> {
        let CreateTransferSessionRequest { target, files } = request;
        match target.kind {
            TransferTargetKind::MyFiles => {}
        }
        if files.is_empty() {
            return Err(invalid(&["files"]));
        }
        if files.len() > MAX_FILES_PER_SESSION {
            return Err(TransferError::BatchTooLarge);
        }
        let target = match target.folder_id {
            None => None,
            Some(raw) => Some(
                raw.parse::<FolderId>()
                    .map_err(|_| TransferError::Folder(FolderError::NotFound))?,
            ),
        };
        let mut seen = HashSet::with_capacity(files.len());
        let mut planned = Vec::with_capacity(files.len());
        for (ordinal, value) in files.into_iter().enumerate() {
            let request: TransferFileRequest =
                serde_json::from_value(value).map_err(|_| invalid(&["files"]))?;
            let ordinal = u32::try_from(ordinal).map_err(|_| TransferError::BatchTooLarge)?;
            let file = PlannedFile::parse(ordinal, request)?;
            if !seen.insert(file.client_key.clone()) {
                return Err(invalid(&["clientId"]));
            }
            planned.push(file);
        }
        Ok(Self {
            target,
            files: planned,
        })
    }
}

impl PlannedFile {
    fn parse(ordinal: u32, request: TransferFileRequest) -> Result<Self, TransferError> {
        let client_key =
            ClientFileKey::parse(&request.client_id).map_err(|_| invalid(&["clientId"]))?;
        let composed: String = request.name.nfc().collect();
        let name = NameCandidate::new(composed).map_err(|_| TransferError::NameInvalid)?;
        let directory = match request.relative_path.as_deref() {
            None | Some("") => DirectoryPath::root(),
            Some(wire) => {
                let path = RelativePath::parse(wire).map_err(|_| invalid(&["relativePath"]))?;
                if path.leaf().display() != name.display() {
                    return Err(invalid(&["name", "relativePath"]));
                }
                path.directory().clone()
            }
        };
        let size = match request.size_bytes {
            None | Some(Value::Null) => None,
            Some(Value::Number(number)) => {
                let bytes = number
                    .as_u64()
                    .and_then(|bytes| ByteSize::try_from(bytes).ok())
                    .filter(|bytes| bytes.to_i64() <= MAX_WIRE_BYTES)
                    .ok_or_else(|| invalid(&["sizeBytes"]))?;
                Some(bytes)
            }
            Some(_) => return Err(invalid(&["sizeBytes"])),
        };
        if let Some(content_type) = &request.declared_content_type {
            let well_formed = !content_type.is_empty()
                && content_type.len() <= MAX_CONTENT_TYPE_CHARS
                && content_type.chars().all(|c| !c.is_control());
            if !well_formed {
                return Err(invalid(&["declaredContentType"]));
            }
        }
        Ok(Self {
            ordinal,
            client_key,
            name,
            directory,
            size,
        })
    }
}

fn invalid(fields: &[&'static str]) -> TransferError {
    TransferError::Invalid {
        fields: fields.to_vec(),
    }
}

pub fn wire_bytes(size: ByteSize) -> WireBytes {
    WireBytes::clamped(size).0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferItemError {
    pub code: ErrorCode,
    #[schema(required = true)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferTusPlan {
    /// Where the TUS upload for this item is created.
    pub create_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferS3Plan {
    /// The planned part size. `null` while the size is unknown.
    #[schema(required = true)]
    pub part_size_bytes: Option<WireBytes>,
    /// The planned part count. `null` while the size is unknown.
    #[schema(required = true, minimum = 1)]
    pub part_count: Option<u32>,
    #[schema(minimum = 1)]
    pub max_presign_batch: u32,
    #[schema(minimum = 1)]
    pub presign_ttl_seconds: u32,
    /// Parts persisted as uploaded. Present only once a multipart upload exists for the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 0)]
    pub completed_parts: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferFileView {
    pub item_id: String,
    pub client_id: String,
    pub name: String,
    /// The full relative path including the name; `null` for a file placed directly in the target folder.
    #[schema(required = true)]
    pub relative_path: Option<String>,
    /// `created` is the durable `pending` state: planned, with no protocol resource yet.
    pub state: TransferItemState,
    pub protocol: TransferProtocol,
    #[schema(required = true)]
    pub size_bytes: Option<WireBytes>,
    /// Persisted, authoritative progress. It is `0` until a protocol resource records bytes.
    pub uploaded_bytes: WireBytes,
    /// The resulting file. Non-null only once the item is `completed` and its file row exists.
    #[schema(required = true)]
    pub file_id: Option<String>,
    #[schema(required = true)]
    pub error: Option<TransferItemError>,
    #[schema(minimum = 0)]
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tus: Option<TransferTusPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s3: Option<TransferS3Plan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferSessionView {
    pub id: String,
    pub state: TransferSessionState,
    pub provider: TransferProvider,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: String,
    /// The sum of the declared sizes of the files whose size is known.
    pub total_bytes: WireBytes,
    pub uploaded_bytes: WireBytes,
    /// The quota currently held for the unfinished files; `0` once the reservation settles.
    pub reserved_bytes: WireBytes,
    #[schema(required = true)]
    pub error: Option<TransferItemError>,
    pub files: Vec<TransferFileView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransferSessionSummary {
    pub id: String,
    pub state: TransferSessionState,
    pub provider: TransferProvider,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: String,
    pub total_bytes: WireBytes,
    pub uploaded_bytes: WireBytes,
    pub reserved_bytes: WireBytes,
    #[schema(minimum = 0)]
    pub file_count: u32,
    #[schema(minimum = 0)]
    pub completed_file_count: u32,
    #[schema(required = true)]
    pub error: Option<TransferItemError>,
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{CreateTransferSessionRequest, ValidatedSession, MAX_FILES_PER_SESSION};
    use crate::features::transfers::error::TransferError;

    fn request(files: Vec<Value>) -> CreateTransferSessionRequest {
        serde_json::from_value(json!({
            "target": { "kind": "my_files", "folderId": null },
            "files": files,
        }))
        .unwrap()
    }

    fn file(id: &str, name: &str) -> Value {
        json!({ "clientId": id, "name": name, "sizeBytes": 10 })
    }

    fn fields(error: TransferError) -> Vec<&'static str> {
        match error {
            TransferError::Invalid { fields } => fields,
            other => panic!("expected a validation error, got {other}"),
        }
    }

    #[test]
    fn unit_session_request_rejects_key_like_and_unknown_fields() {
        for extra in [
            "objectKey",
            "storageObjectId",
            "finalObjectId",
            "finalObjectKey",
            "bucket",
            "stagingPath",
            "uploadId",
            "multipartUploadId",
            "ownerId",
            "userId",
        ] {
            let mut item = file("c1", "a.txt");
            item[extra] = json!("x");
            let parsed = ValidatedSession::parse(request(vec![item]));
            assert_eq!(fields(parsed.unwrap_err()), ["files"], "{extra}");

            let body = json!({
                "target": { "kind": "my_files", "folderId": null },
                "files": [file("c1", "a.txt")],
                extra: "x",
            });
            assert!(serde_json::from_value::<CreateTransferSessionRequest>(body).is_err());

            let body = json!({
                "target": { "kind": "my_files", "folderId": null, extra: "x" },
                "files": [file("c1", "a.txt")],
            });
            assert!(serde_json::from_value::<CreateTransferSessionRequest>(body).is_err());
        }
    }

    #[test]
    fn unit_session_request_file_count_bounds() {
        assert_eq!(
            fields(ValidatedSession::parse(request(Vec::new())).unwrap_err()),
            ["files"]
        );
        let many = |count: usize| -> Vec<Value> {
            (0..count)
                .map(|index| file(&format!("c{index}"), "a.txt"))
                .collect()
        };
        assert_eq!(
            ValidatedSession::parse(request(many(MAX_FILES_PER_SESSION)))
                .unwrap()
                .files
                .len(),
            MAX_FILES_PER_SESSION
        );
        assert!(matches!(
            ValidatedSession::parse(request(many(MAX_FILES_PER_SESSION + 1))),
            Err(TransferError::BatchTooLarge)
        ));
        let mut oversized_and_malformed = many(MAX_FILES_PER_SESSION + 1);
        oversized_and_malformed[0] = json!("not an object");
        assert!(
            matches!(
                ValidatedSession::parse(request(oversized_and_malformed)),
                Err(TransferError::BatchTooLarge)
            ),
            "the count is checked before any item is examined"
        );
    }

    #[test]
    fn unit_session_request_duplicate_client_ids_are_rejected() {
        let error =
            ValidatedSession::parse(request(vec![file("c1", "a.txt"), file("c1", "b.txt")]))
                .unwrap_err();
        assert_eq!(fields(error), ["clientId"]);
    }

    #[test]
    fn unit_session_request_size_rules() {
        let sized = |size: Value| {
            ValidatedSession::parse(request(vec![
                json!({ "clientId": "c1", "name": "a.txt", "sizeBytes": size }),
            ]))
        };
        assert_eq!(sized(json!(0)).unwrap().files[0].size.unwrap().to_i64(), 0);
        assert_eq!(
            sized(json!(9_007_199_254_740_991_i64)).unwrap().files[0]
                .size
                .unwrap()
                .to_i64(),
            9_007_199_254_740_991
        );
        assert_eq!(sized(json!(null)).unwrap().files[0].size, None);
        for bad in [
            json!(-1),
            json!(9_007_199_254_740_992_i64),
            json!(u64::MAX),
            json!(1.5),
            json!("10"),
            json!(true),
            json!([1]),
        ] {
            assert_eq!(
                fields(sized(bad.clone()).unwrap_err()),
                ["sizeBytes"],
                "{bad}"
            );
        }
        let absent =
            ValidatedSession::parse(request(vec![json!({ "clientId": "c1", "name": "a" })]));
        assert_eq!(absent.unwrap().files[0].size, None);
    }

    #[test]
    fn unit_session_request_path_and_name_rules() {
        let with = |name: &str, path: Option<&str>| {
            let mut item = json!({ "clientId": "c1", "name": name, "sizeBytes": 1 });
            if let Some(path) = path {
                item["relativePath"] = json!(path);
            }
            ValidatedSession::parse(request(vec![item]))
        };
        let nested = with("video.mkv", Some("Trip/Day 1/video.mkv")).unwrap();
        let file = &nested.files[0];
        assert_eq!(file.directory.joined(), "Trip/Day 1");
        assert_eq!(file.name.display(), "video.mkv");
        assert_eq!(file.relative_path(), "Trip/Day 1");

        let root = with("a.txt", None).unwrap();
        assert!(root.files[0].directory.is_empty());
        assert_eq!(root.files[0].relative_path(), "");
        assert!(with("a.txt", Some("")).unwrap().files[0]
            .directory
            .is_empty());
        assert!(with("a.txt", Some("a.txt")).unwrap().files[0]
            .directory
            .is_empty());

        let backslashes = with("f.txt", Some("Dir\\Sub\\f.txt")).unwrap();
        assert_eq!(backslashes.files[0].directory.joined(), "Dir/Sub");

        let composed = with("cafe\u{301}.txt", Some("cafe\u{301}/cafe\u{301}.txt")).unwrap();
        assert_eq!(composed.files[0].name.display(), "caf\u{e9}.txt");
        assert_eq!(composed.files[0].directory.joined(), "caf\u{e9}");

        for bad in [
            "/abs/f.txt",
            "dir/",
            "a//b.txt",
            "../f.txt",
            "dir/./f.txt",
            "C:/f.txt",
            "\\\\server\\share\\f.txt",
            "dir/nul\0/f.txt",
            "dir/\u{7}/f.txt",
        ] {
            let error = with("f.txt", Some(bad)).unwrap_err();
            assert_eq!(fields(error), ["relativePath"], "{bad:?}");
        }
        let too_deep = vec!["d"; 33].join("/") + "/f.txt";
        assert_eq!(
            fields(with("f.txt", Some(&too_deep)).unwrap_err()),
            ["relativePath"]
        );
        let too_long = format!("{}/f.txt", "d".repeat(250).repeat(5));
        assert_eq!(
            fields(with("f.txt", Some(&too_long)).unwrap_err()),
            ["relativePath"]
        );
        assert_eq!(
            fields(with("other.txt", Some("dir/f.txt")).unwrap_err()),
            ["name", "relativePath"]
        );
        for bad_name in ["", ".", "..", "a/b", "a\\b", "tab\t", &"x".repeat(256)] {
            assert!(
                matches!(with(bad_name, None), Err(TransferError::NameInvalid)),
                "{bad_name:?}"
            );
        }
    }

    #[test]
    fn unit_session_request_client_ids_and_content_types() {
        for bad in ["", "tab\tid", "nul\0id", &"a".repeat(129)] {
            let error = ValidatedSession::parse(request(vec![file(bad, "a.txt")])).unwrap_err();
            assert_eq!(fields(error), ["clientId"], "{bad:?}");
        }
        let typed = |content_type: Value| {
            ValidatedSession::parse(request(vec![json!({
                "clientId": "c1", "name": "a.txt", "declaredContentType": content_type
            })]))
        };
        assert!(typed(json!("application/pdf")).is_ok());
        for bad in [json!(""), json!("a\nb"), json!("x".repeat(256)), json!(7)] {
            assert!(typed(bad).is_err());
        }
    }

    #[test]
    fn unit_session_request_target_kind_and_folder_id() {
        let body = |target: Value| {
            serde_json::from_value::<CreateTransferSessionRequest>(json!({
                "target": target,
                "files": [file("c1", "a.txt")],
            }))
        };
        assert!(body(json!({ "kind": "reverse_share" })).is_err());
        assert!(body(json!({ "kind": "my_files" })).is_ok());
        let malformed = body(json!({ "kind": "my_files", "folderId": "not-a-uuid" })).unwrap();
        assert!(matches!(
            ValidatedSession::parse(malformed),
            Err(TransferError::Folder(
                crate::features::folders::FolderError::NotFound
            ))
        ));
    }
}
