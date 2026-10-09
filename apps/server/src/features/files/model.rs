use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::folders::{FolderError, FolderId, FolderItem, FolderPathItem};
use crate::infra::http::json::{present, JsonField, JsonKind, JsonRequest};
use crate::infra::http::pagination::{Page, WireBytes};

use super::error::FileError;

pub const MAX_DESCRIPTION_CHARS: usize = 2000;
pub const MAX_BATCH_IDS: usize = 500;

pub enum File {}
pub type FileId = Id<File>;

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: FileId,
    pub folder_id: Option<FolderId>,
    pub name: String,
    pub name_normalized: String,
    pub description: Option<String>,
    pub size_bytes: i64,
    pub mime_type: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSource {
    pub folder_id: Option<FolderId>,
    pub name: String,
    pub description: Option<String>,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileItem {
    /// Always `file`. Distinguishes a file from a folder in a mixed listing.
    pub kind: FileKind,
    pub id: String,
    pub name: String,
    #[schema(required = true)]
    pub description: Option<String>,
    pub size_bytes: WireBytes,
    /// The content type Palmr stored when the file was finalized. It is never the type the browser declared.
    pub content_type: String,
    /// The folder that holds the file; `null` for My Files root.
    #[schema(required = true)]
    pub folder_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<FileRecord> for FileItem {
    fn from(record: FileRecord) -> Self {
        let (size_bytes, _) = ByteSize::try_from(record.size_bytes)
            .map_or((WireBytes::MAX, false), WireBytes::clamped);
        Self {
            kind: FileKind::File,
            id: record.id.to_string(),
            name: record.name,
            description: record.description,
            size_bytes,
            content_type: record.mime_type,
            folder_id: record.folder_id.map(|folder| folder.to_string()),
            created_at: record.created_at.to_string(),
            updated_at: record.updated_at.to_string(),
        }
    }
}

/// A folder or a file, told apart by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(discriminator(
    property_name = "kind",
    mapping(
        ("folder" = "#/components/schemas/FolderItem"),
        ("file" = "#/components/schemas/FileItem")
    )
))]
pub enum BrowseItem {
    Folder(FolderItem),
    File(FileItem),
}

/// A file found by global search, with the folders that contain it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileItem {
    #[serde(flatten)]
    pub file: FileItem,
    /// The folders that contain the file, from the root-level ancestor down to the containing folder. Empty for a file at the My Files root. The file itself is not part of the path.
    pub path: Vec<FolderPathItem>,
}

/// The page `GET /files` returns: a browse page without `q`, a search page with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum FilesPage {
    Browse(Page<BrowseItem>),
    Search(Page<SearchFileItem>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    #[serde(flatten)]
    pub file: FileItem,
    /// The stored name when it differs from the requested one because a sibling already had it; otherwise `null`.
    #[schema(required = true)]
    pub renamed_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovedItem {
    pub id: String,
    /// The name the item has after the move.
    pub name: String,
    /// The stored name when it differs from the name before the move; otherwise `null`.
    #[schema(required = true)]
    pub renamed_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchMoveResult {
    /// Every requested file, in request order.
    pub files: Vec<MovedItem>,
    /// Every requested folder, in request order.
    pub folders: Vec<MovedItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NameCheck {
    pub available: bool,
    /// The first currently free deterministic name when `available` is `false`; otherwise `null`.
    #[schema(required = true)]
    pub suggested_name: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateFileRequest {
    /// Absent leaves the name unchanged. `null` is not a valid name.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, nullable = false, min_length = 1, max_length = 255)]
    pub name: Option<Option<String>>,
    /// Absent leaves the description unchanged; `null` clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, max_length = 2000)]
    pub description: Option<Option<String>>,
}

impl JsonRequest for UpdateFileRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("name", JsonKind::String),
        JsonField::optional("description", JsonKind::String),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileChange {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
}

impl FileChange {
    pub fn parse(request: UpdateFileRequest) -> Result<Self, FileError> {
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
            if description.chars().count() > MAX_DESCRIPTION_CHARS {
                invalid.push("description");
            }
        }
        if invalid.is_empty() {
            Ok(Self {
                name,
                description: request.description,
            })
        } else {
            Err(FileError::Invalid { fields: invalid })
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveFileRequest {
    /// The destination folder. `null` moves the file to the My Files root. The member is required.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, required = true, example = "0192f3a1-0000-7000-8000-000000000001")]
    pub folder_id: Option<Option<String>>,
}

impl JsonRequest for MoveFileRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::optional("folderId", JsonKind::String)];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMove {
    pub folder_id: Option<FolderId>,
}

impl FileMove {
    pub fn parse(request: MoveFileRequest) -> Result<Self, FileError> {
        Ok(Self {
            folder_id: destination(request.folder_id, "folderId")?,
        })
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchMoveRequest {
    /// Files to move. Together with `folderIds` at most 500 ids; no id may repeat.
    #[schema(max_items = 500, example = json!(["0192f3a1-0000-7000-8000-000000000001"]))]
    pub file_ids: Vec<String>,
    /// Folders to move along with the files. Absent is the same as empty.
    #[serde(default)]
    #[schema(max_items = 500)]
    pub folder_ids: Vec<String>,
    /// The destination folder. `null` moves everything to the My Files root. The member is required.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, required = true, example = "0192f3a1-0000-7000-8000-000000000002")]
    pub target_folder_id: Option<Option<String>>,
}

impl JsonRequest for BatchMoveRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("fileIds", JsonKind::Array),
        JsonField::optional("folderIds", JsonKind::Array),
        JsonField::optional("targetFolderId", JsonKind::String),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMove {
    pub files: Vec<FileId>,
    pub folders: Vec<FolderId>,
    pub target: Option<FolderId>,
}

impl BatchMove {
    pub fn parse(request: BatchMoveRequest) -> Result<Self, FileError> {
        if request
            .file_ids
            .len()
            .saturating_add(request.folder_ids.len())
            > MAX_BATCH_IDS
        {
            return Err(FileError::BatchTooLarge);
        }
        if request.file_ids.is_empty() && request.folder_ids.is_empty() {
            return Err(FileError::Invalid {
                fields: vec!["fileIds"],
            });
        }
        let target = destination(request.target_folder_id, "targetFolderId")?;
        let files = unique_ids::<FileId>(&request.file_ids, || FileError::NotFound, "fileIds")?;
        let folders = unique_ids::<FolderId>(
            &request.folder_ids,
            || FileError::Folder(FolderError::NotFound),
            "folderIds",
        )?;
        Ok(Self {
            files,
            folders,
            target,
        })
    }
}

pub(super) fn unique_ids<T>(
    raw: &[String],
    unknown: impl Fn() -> FileError,
    field: &'static str,
) -> Result<Vec<T>, FileError>
where
    T: FromStr + Copy + Eq + Hash,
{
    let mut seen = HashSet::with_capacity(raw.len());
    let mut ids = Vec::with_capacity(raw.len());
    for text in raw {
        let Ok(id) = text.parse::<T>() else {
            return Err(unknown());
        };
        if !seen.insert(id) {
            return Err(FileError::Invalid {
                fields: vec![field],
            });
        }
        ids.push(id);
    }
    Ok(ids)
}

fn destination(
    raw: Option<Option<String>>,
    field: &'static str,
) -> Result<Option<FolderId>, FileError> {
    match raw {
        None => Err(FileError::Invalid {
            fields: vec![field],
        }),
        Some(None) => Ok(None),
        Some(Some(text)) => text
            .parse::<FolderId>()
            .map(Some)
            .map_err(|_| FileError::Folder(FolderError::NotFound)),
    }
}
