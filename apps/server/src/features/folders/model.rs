use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::id::Id;
use crate::domain::relative_path::{DirectoryPath, MAX_SEGMENTS};
use crate::domain::time::Timestamp;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::infra::http::pagination::WireBytes;

use super::error::FolderError;

pub const MAX_FOLDER_DEPTH: i64 = 64;
pub const MAX_DESCRIPTION_CHARS: usize = 2000;
pub const TREE_DEFAULT_DEPTH: u8 = 3;
pub const TREE_MAX_DEPTH: u8 = 8;
pub const TREE_NODE_CAP: usize = 2000;

pub enum Folder {}
pub type FolderId = Id<Folder>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedFolder {
    pub id: FolderId,
    pub parent_id: Option<FolderId>,
    pub depth: u8,
}

#[derive(Debug, Clone)]
pub struct FolderRecord {
    pub id: FolderId,
    pub parent_id: Option<FolderId>,
    pub name: String,
    pub name_normalized: String,
    pub description: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FolderTotals {
    pub file_count: u64,
    pub subfolder_count: u64,
    pub total_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub id: FolderId,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub id: FolderId,
    pub parent_id: Option<FolderId>,
    pub name: String,
    pub level: u8,
    pub has_children: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderItem {
    pub id: String,
    pub name: String,
    #[schema(required = true)]
    pub description: Option<String>,
    #[schema(required = true)]
    pub parent_id: Option<String>,
    /// Files in the whole subtree rooted at this folder.
    #[schema(minimum = 0)]
    pub file_count: u64,
    /// Folders below this folder, at any depth; the folder itself is not counted.
    #[schema(minimum = 0)]
    pub subfolder_count: u64,
    /// Sum of the sizes of every file in the subtree.
    pub total_bytes: WireBytes,
    pub created_at: String,
    pub updated_at: String,
}

impl FolderItem {
    pub fn new(record: FolderRecord, totals: FolderTotals) -> Self {
        let (total_bytes, _) = ByteSize::try_from(totals.total_bytes)
            .map_or((WireBytes::MAX, false), WireBytes::clamped);
        Self {
            id: record.id.to_string(),
            name: record.name,
            description: record.description,
            parent_id: record.parent_id.map(|parent| parent.to_string()),
            file_count: totals.file_count,
            subfolder_count: totals.subfolder_count,
            total_bytes,
            created_at: record.created_at.to_string(),
            updated_at: record.updated_at.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderPathItem {
    pub id: String,
    pub name: String,
}

impl From<Crumb> for FolderPathItem {
    fn from(crumb: Crumb) -> Self {
        Self {
            id: crumb.id.to_string(),
            name: crumb.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderDetail {
    #[serde(flatten)]
    pub folder: FolderItem,
    /// Breadcrumbs from the root-level ancestor down to and including this folder.
    pub path: Vec<FolderPathItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderTreeNode {
    pub id: String,
    #[schema(required = true)]
    pub parent_id: Option<String>,
    pub name: String,
    /// `true` when the folder has at least one child folder, whether or not it is part of this response.
    pub has_children: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TruncationPoint {
    /// The last node returned. Every node after it in the response order was omitted.
    pub after_id: String,
    /// The 1-based level of that node, where level 1 is the top of the returned tree.
    #[schema(minimum = 1, maximum = 8)]
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderTree {
    /// Breadth-first by level, then by normalized name, then by id. A parent always precedes its children.
    pub nodes: Vec<FolderTreeNode>,
    pub truncated: bool,
    #[schema(required = true)]
    pub truncation_point: Option<TruncationPoint>,
}

impl FolderTree {
    pub fn from_rows(mut rows: Vec<TreeRow>) -> Self {
        let truncated = rows.len() > TREE_NODE_CAP;
        rows.truncate(TREE_NODE_CAP);
        let truncation_point = truncated
            .then(|| {
                rows.last().map(|last| TruncationPoint {
                    after_id: last.id.to_string(),
                    level: last.level,
                })
            })
            .flatten();
        Self {
            nodes: rows
                .into_iter()
                .map(|row| FolderTreeNode {
                    id: row.id.to_string(),
                    parent_id: row.parent_id.map(|parent| parent.to_string()),
                    name: row.name,
                    has_children: row.has_children,
                })
                .collect(),
            truncated,
            truncation_point,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateFolderRequest {
    #[schema(min_length = 1, max_length = 255, example = "2026")]
    pub name: String,
    #[schema(nullable = true, max_length = 2000)]
    pub description: Option<String>,
    /// Absent or `null` creates a root-level folder.
    #[schema(nullable = true, example = "0192f3a1-0000-7000-8000-000000000001")]
    pub parent_id: Option<String>,
}

impl JsonRequest for CreateFolderRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("name", JsonKind::String),
        JsonField::optional("description", JsonKind::String),
        JsonField::optional("parentId", JsonKind::String),
    ];
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateFolderRequest {
    /// Absent leaves the name unchanged. `null` is not a valid name.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, nullable = false, min_length = 1, max_length = 255)]
    pub name: Option<Option<String>>,
    /// Absent leaves the description unchanged; `null` clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, max_length = 2000)]
    pub description: Option<Option<String>>,
}

fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

impl JsonRequest for UpdateFolderRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("name", JsonKind::String),
        JsonField::optional("description", JsonKind::String),
    ];
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveFolderRequest {
    /// The destination folder. `null` moves the folder to the My Files root. The member is required.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, required = true, example = "0192f3a1-0000-7000-8000-000000000001")]
    pub parent_id: Option<Option<String>>,
}

impl JsonRequest for MoveFolderRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::optional("parentId", JsonKind::String)];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderMove {
    pub parent_id: Option<FolderId>,
}

impl FolderMove {
    pub fn parse(request: MoveFolderRequest) -> Result<Self, FolderError> {
        let parent_id = match request.parent_id {
            None => {
                return Err(FolderError::Invalid {
                    fields: vec!["parentId"],
                })
            }
            Some(None) => None,
            Some(Some(raw)) => Some(raw.parse::<FolderId>().map_err(|_| FolderError::NotFound)?),
        };
        Ok(Self { parent_id })
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnsurePathRequest {
    /// Absent or `null` ensures the chain under the My Files root.
    #[schema(nullable = true, example = "0192f3a1-0000-7000-8000-000000000001")]
    pub parent_id: Option<String>,
    /// Directory names from the outermost to the innermost. Each is a folder name: 1 to 255 bytes, no `/`, `\`, control characters, `.` or `..`. At most 32 segments; the NFC-normalized segments joined by `/` fit in 1024 bytes.
    #[schema(value_type = Vec<String>, min_items = 1, max_items = 32, example = json!(["Photos", "2026", "Iceland"]))]
    pub segments: Vec<Value>,
}

impl JsonRequest for EnsurePathRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("parentId", JsonKind::String),
        JsonField::required("segments", JsonKind::Array),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsurePath {
    pub parent_id: Option<FolderId>,
    pub path: DirectoryPath,
}

impl EnsurePath {
    pub fn parse(request: EnsurePathRequest) -> Result<Self, FolderError> {
        let parent_id = match request.parent_id {
            None => None,
            Some(raw) => Some(raw.parse::<FolderId>().map_err(|_| FolderError::NotFound)?),
        };
        if request.segments.is_empty() {
            return Err(FolderError::Invalid {
                fields: vec!["segments"],
            });
        }
        let mut segments = Vec::with_capacity(request.segments.len().min(MAX_SEGMENTS + 1));
        for segment in &request.segments {
            match segment {
                Value::String(text) => segments.push(text.as_str()),
                _ => {
                    return Err(FolderError::Invalid {
                        fields: vec!["segments"],
                    })
                }
            }
        }
        let path = DirectoryPath::from_segments(&segments).map_err(FolderError::InvalidPath)?;
        Ok(Self { parent_id, path })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnsurePathResponse {
    /// One folder id per requested segment, in request order.
    pub folder_ids: Vec<String>,
    /// The id of the last segment.
    pub leaf_folder_id: String,
    /// The ids this call created, in segment order. Empty when every folder already existed.
    pub created: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFolder {
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<FolderId>,
}

impl NewFolder {
    pub fn parse(request: CreateFolderRequest) -> Result<Self, FolderError> {
        let mut invalid = Vec::new();
        if let Some(description) = &request.description {
            bounded_description(description, &mut invalid);
        }
        let parent_id = match request.parent_id {
            None => None,
            Some(raw) => match raw.parse::<FolderId>() {
                Ok(parent) => Some(parent),
                Err(_) => return Err(FolderError::NotFound),
            },
        };
        if invalid.is_empty() {
            Ok(Self {
                name: request.name,
                description: request.description,
                parent_id,
            })
        } else {
            Err(FolderError::Invalid { fields: invalid })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderChange {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
}

impl FolderChange {
    pub fn parse(request: UpdateFolderRequest) -> Result<Self, FolderError> {
        let mut invalid = Vec::new();
        let name = match request.name {
            None => None,
            Some(Some(name)) => Some(name),
            Some(None) => {
                invalid.push("name");
                None
            }
        };
        if let Some(Some(description)) = &request.description {
            bounded_description(description, &mut invalid);
        }
        if invalid.is_empty() {
            Ok(Self {
                name,
                description: request.description,
            })
        } else {
            Err(FolderError::Invalid { fields: invalid })
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none()
    }
}

fn bounded_description(description: &str, invalid: &mut Vec<&'static str>) {
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        invalid.push("description");
    }
}
