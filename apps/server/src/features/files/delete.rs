use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{self, FileDeletedFacts, FileDeletionScope};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::folders::impact::{
    deletion_impact, ensure_selectable, DeletionImpact, Missing,
};
use crate::features::folders::visibility::folder_hidden_sql;
use crate::features::folders::{claim_folder_deletion_in_tx, ClaimOutcome, FolderError, FolderId};
use crate::features::quota::error::QuotaError;
use crate::features::quota::service::decrement_used;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, WriteTx};
use crate::infra::http::error::{ApiError, ItemFailureDetail};
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::infra::jobs::{InvalidDedupKey, JobsError};
use crate::storage::lifecycle::{
    tombstone, DeletionReason, LifecycleError, StorageObjectId, MAX_TOMBSTONE_BATCH,
};

use super::error::FileError;
use super::model::{unique_ids, FileId, MAX_BATCH_IDS};
use super::service::FileService;

pub const DELETE_BATCH_ROWS: usize = 500;

const _: () = assert!(DELETE_BATCH_ROWS <= MAX_TOMBSTONE_BATCH);

const FIND_FOR_DELETE: &str = concat!(
    "SELECT f.id, f.storage_object_id, f.size_bytes, f.name, ",
    folder_hidden_sql!("f.folder_id", "f.owner_id"),
    " AS hidden FROM files f WHERE f.id = ?1 AND f.owner_id = ?2"
);

const FIND_RECEIPT: &str =
    "SELECT EXISTS(SELECT 1 FROM deletion_receipts WHERE id = ?1 AND resource_kind = ?2 AND owner_id = ?3)";

const DETACH_SHARE_ITEMS: &str =
    "DELETE FROM share_items WHERE file_id IN (SELECT value FROM json_each(?1))";

const REVOKE_EMBEDS: &str = "DELETE FROM embed_grants
      WHERE owner_id = ?2 AND file_id IN (SELECT value FROM json_each(?1))";

const WRITE_FILE_RECEIPTS: &str = "INSERT OR IGNORE INTO deletion_receipts
        (id, resource_kind, owner_id, deleted_at)
      SELECT value, 'file', ?2, ?3 FROM json_each(?1)";

const REMOVE_FILES: &str =
    "DELETE FROM files WHERE owner_id = ?2 AND id IN (SELECT value FROM json_each(?1))";

const DEPTHS: &str = "SELECT id, depth FROM folders WHERE owner_id = ?1
      AND id IN (SELECT value FROM json_each(?2))";

#[derive(Debug)]
pub enum DeleteError {
    Db(DbError),
    Time(InvalidTimestamp),
    Quota(QuotaError),
    Lifecycle(LifecycleError),
    Audit(AuditError),
    Jobs(JobsError),
    Invariant { what: &'static str },
}

impl DeleteError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(_) => "delete_database_failed",
            Self::Time(_) => "delete_time_out_of_range",
            Self::Quota(error) => error.kind(),
            Self::Lifecycle(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Jobs(error) => error.kind(),
            Self::Invariant { .. } => "delete_invariant",
        }
    }

    pub const fn is_quota_integrity(&self) -> bool {
        matches!(
            self,
            Self::Quota(
                QuotaError::Underflow { .. }
                    | QuotaError::Overflow { .. }
                    | QuotaError::Integrity { .. }
                    | QuotaError::OwnerNotFound
            )
        )
    }
}

impl fmt::Display for DeleteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "deletion database operation failed: {error}"),
            Self::Time(error) => write!(f, "deletion timestamp is out of range: {error}"),
            Self::Quota(error) => write!(f, "deletion quota accounting failed: {error}"),
            Self::Lifecycle(error) => write!(f, "deletion tombstoning failed: {error}"),
            Self::Audit(error) => write!(f, "deletion audit failed: {error}"),
            Self::Jobs(error) => write!(f, "deletion job enqueue failed: {error}"),
            Self::Invariant { what } => write!(f, "deletion invariant violated: {what}"),
        }
    }
}

impl std::error::Error for DeleteError {}

impl From<DbError> for DeleteError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for DeleteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for DeleteError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<QuotaError> for DeleteError {
    fn from(error: QuotaError) -> Self {
        Self::Quota(error)
    }
}

impl From<LifecycleError> for DeleteError {
    fn from(error: LifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<JobsError> for DeleteError {
    fn from(error: JobsError) -> Self {
        Self::Jobs(error)
    }
}

impl From<InvalidDedupKey> for DeleteError {
    fn from(error: InvalidDedupKey) -> Self {
        Self::Jobs(JobsError::from(error))
    }
}

impl From<AuditError> for DeleteError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

#[derive(Debug, Clone)]
pub struct DeletionCaller {
    pub owner: UserId,
    pub username: String,
    pub client: ClientMetadata,
}

impl DeletionCaller {
    pub fn actor(&self) -> Actor {
        Actor::user(&self.owner.to_string(), &self.username)
    }
}

#[derive(Debug, Clone)]
pub struct DoomedFile {
    pub id: String,
    pub storage_object_id: StorageObjectId,
    pub size_bytes: ByteSize,
}

impl DoomedFile {
    pub fn from_row(row: &SqliteRow) -> Result<Self, DeleteError> {
        let invariant = |what| DeleteError::Invariant { what };
        let object: String = row
            .try_get("storage_object_id")
            .map_err(|_| invariant("storage_object_id"))?;
        let size: i64 = row
            .try_get("size_bytes")
            .map_err(|_| invariant("size_bytes"))?;
        Ok(Self {
            id: row.try_get("id").map_err(|_| invariant("id"))?,
            storage_object_id: object.parse().map_err(|_| invariant("storage_object_id"))?,
            size_bytes: ByteSize::try_from(size).map_err(|_| invariant("size_bytes"))?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Removal {
    pub files: u64,
    pub bytes: u64,
    pub shares_detached: u64,
    pub embeds_revoked: u64,
}

fn json_list<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<String, DeleteError> {
    let ids: Vec<&str> = ids.into_iter().collect();
    serde_json::to_string(&ids).map_err(|_| DeleteError::Invariant { what: "ids" })
}

pub(crate) async fn remove_files_in_tx(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    owner: UserId,
    files: &[DoomedFile],
    reason: DeletionReason,
    at: Timestamp,
) -> Result<Removal, DeleteError> {
    if files.is_empty() {
        return Ok(Removal::default());
    }
    if files.len() > DELETE_BATCH_ROWS {
        return Err(DeleteError::Invariant {
            what: "batch_too_large",
        });
    }
    let owner_text = owner.to_string();
    let ids = json_list(files.iter().map(|file| file.id.as_str()))?;
    let mut bytes = ByteSize::ZERO;
    for file in files {
        bytes = bytes
            .checked_add(file.size_bytes)
            .ok_or(QuotaError::Overflow {
                operation: "deleted_bytes",
            })?;
    }

    let shares_detached = sqlx::query(DETACH_SHARE_ITEMS)
        .bind(&ids)
        .execute(tx.executor())
        .await?
        .rows_affected();
    let embeds_revoked = sqlx::query(REVOKE_EMBEDS)
        .bind(&ids)
        .bind(&owner_text)
        .execute(tx.executor())
        .await?
        .rows_affected();
    sqlx::query(WRITE_FILE_RECEIPTS)
        .bind(&ids)
        .bind(&owner_text)
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    let removed = sqlx::query(REMOVE_FILES)
        .bind(&ids)
        .bind(&owner_text)
        .execute(tx.executor())
        .await?
        .rows_affected();
    if usize::try_from(removed).ok() != Some(files.len()) {
        return Err(DeleteError::Invariant {
            what: "files_removed",
        });
    }
    let objects: Vec<StorageObjectId> = files.iter().map(|file| file.storage_object_id).collect();
    tombstone(tx, clock, &objects, reason).await?;
    decrement_used(tx, owner, bytes).await?;
    Ok(Removal {
        files: removed,
        bytes: bytes.get(),
        shares_detached,
        embeds_revoked,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDeletion {
    Deleted,
    AlreadyDeleted,
}

pub(crate) async fn receipt_exists(
    connection: &mut SqliteConnection,
    kind: &'static str,
    owner: UserId,
    id: &str,
) -> Result<bool, DeleteError> {
    let found: i64 = sqlx::query_scalar(FIND_RECEIPT)
        .bind(id)
        .bind(kind)
        .bind(owner.to_string())
        .fetch_one(connection)
        .await?;
    Ok(found != 0)
}

async fn delete_file_in_tx(
    tx: &mut WriteTx<'_>,
    service: &FileService,
    caller: &DeletionCaller,
    id: FileId,
    at: Timestamp,
) -> Result<FileDeletion, FileError> {
    let row = sqlx::query(FIND_FOR_DELETE)
        .bind(id.to_string())
        .bind(caller.owner.to_string())
        .fetch_optional(tx.executor())
        .await?;
    let Some(row) = row else {
        return if receipt_exists(tx.executor(), "file", caller.owner, &id.to_string()).await? {
            Ok(FileDeletion::AlreadyDeleted)
        } else {
            Err(FileError::NotFound)
        };
    };
    let hidden: i64 = row
        .try_get("hidden")
        .map_err(|_| FileError::RepositoryInvariant { column: "hidden" })?;
    if hidden != 0 {
        return Ok(FileDeletion::AlreadyDeleted);
    }
    let name: String = row
        .try_get("name")
        .map_err(|_| FileError::RepositoryInvariant { column: "name" })?;
    let file = DoomedFile::from_row(&row)?;
    let removal = remove_files_in_tx(
        tx,
        service.clock.as_ref(),
        caller.owner,
        std::slice::from_ref(&file),
        DeletionReason::FileDeleted,
        at,
    )
    .await?;
    let event = AuditEvent::new(
        actions::file_deleted(&FileDeletedFacts {
            scope: FileDeletionScope::File,
            files: removal.files,
            bytes_released: removal.bytes,
            shares_detached: removal.shares_detached,
            embeds_revoked: removal.embeds_revoked,
        }),
        caller.actor(),
        Outcome::Success,
        at,
    )
    .with_target(Target::new(TargetType::File).id(&file.id).label(&name))
    .with_client(caller.client.clone());
    service
        .audit
        .record_in_tx(tx, &event)
        .await
        .map_err(DeleteError::from)?;
    Ok(FileDeletion::Deleted)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchSelectionRequest {
    /// Files to select. Together with `folderIds` at most 500 ids and at least one; no id may repeat. Absent is the same as empty.
    #[serde(default)]
    #[schema(max_items = 500, example = json!(["0192f3a1-0000-7000-8000-000000000001"]))]
    pub file_ids: Vec<String>,
    /// Folders to select, each with its whole subtree. Absent is the same as empty.
    #[serde(default)]
    #[schema(max_items = 500)]
    pub folder_ids: Vec<String>,
}

impl JsonRequest for BatchSelectionRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("fileIds", JsonKind::Array),
        JsonField::optional("folderIds", JsonKind::Array),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSelection {
    pub files: Vec<FileId>,
    pub folders: Vec<FolderId>,
}

impl BatchSelection {
    pub fn parse(request: BatchSelectionRequest) -> Result<Self, FileError> {
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
        Ok(Self {
            files: unique_ids::<FileId>(&request.file_ids, || FileError::NotFound, "fileIds")?,
            folders: unique_ids::<FolderId>(
                &request.folder_ids,
                || FileError::Folder(FolderError::NotFound),
                "folderIds",
            )?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchDeleteFailure {
    pub id: String,
    pub code: ErrorCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BatchDeleteResult {
    /// Ids whose deletion is durable. A folder is listed once its deletion claim has committed: the subtree is already invisible and its rows and bytes are removed in the background.
    pub succeeded: Vec<String>,
    pub failed: Vec<BatchDeleteFailure>,
}

impl BatchDeleteResult {
    pub fn zero_success_error(&self) -> Option<ApiError> {
        if !self.succeeded.is_empty() {
            return None;
        }
        let first = self.failed.first()?.code;
        let code = if self.failed.iter().all(|failure| failure.code == first) {
            first
        } else {
            ErrorCode::BatchDeleteFailed
        };
        let failures: Vec<ItemFailureDetail> = self
            .failed
            .iter()
            .map(|failure| ItemFailureDetail {
                id: failure.id.clone(),
                code: failure.code,
            })
            .collect();
        Some(ApiError::new(code).with_detail("failed", failures))
    }
}

fn failure_code(error: &FileError) -> ErrorCode {
    error.api_error().code()
}

impl FileService {
    pub async fn delete_file(
        &self,
        caller: &DeletionCaller,
        id: FileId,
    ) -> Result<FileDeletion, FileError> {
        let at = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), "files.delete", async |tx| {
                delete_file_in_tx(tx, self, caller, id, at).await
            })
            .await
    }

    pub async fn file_impact(
        &self,
        owner: UserId,
        id: FileId,
    ) -> Result<DeletionImpact, FileError> {
        self.selection_impact(
            owner,
            &BatchSelection {
                files: vec![id],
                folders: Vec::new(),
            },
        )
        .await
    }

    pub async fn selection_impact(
        &self,
        owner: UserId,
        selection: &BatchSelection,
    ) -> Result<DeletionImpact, FileError> {
        let reader = self.pools.reader().executor();
        match ensure_selectable(reader, owner, &selection.files, &selection.folders).await? {
            Some(Missing::File) => return Err(FileError::NotFound),
            Some(Missing::Folder) => return Err(FileError::Folder(FolderError::NotFound)),
            None => {}
        }
        let now = Timestamp::try_from(self.clock.now())?;
        Ok(deletion_impact(reader, owner, &selection.files, &selection.folders, now).await?)
    }

    pub async fn batch_delete(
        &self,
        caller: &DeletionCaller,
        selection: &BatchSelection,
    ) -> Result<BatchDeleteResult, FileError> {
        let depths = self.folder_depths(caller.owner, &selection.folders).await?;
        let mut folder_order: Vec<FolderId> = selection.folders.clone();
        folder_order.sort_by_key(|id| depths.get(&id.to_string()).copied().unwrap_or(i64::MAX));

        let mut outcomes: HashMap<String, Result<(), ErrorCode>> = HashMap::new();
        for id in folder_order {
            let at = Timestamp::try_from(self.clock.now())?;
            let result = self
                .pools
                .write_tx(self.clock.as_ref(), "folders.delete_claim", async |tx| {
                    claim_folder_deletion_in_tx(tx, self.clock.as_ref(), caller.owner, id, at).await
                })
                .await;
            outcomes.insert(
                id.to_string(),
                match result {
                    Ok(
                        ClaimOutcome::Claimed
                        | ClaimOutcome::AlreadyDeleting
                        | ClaimOutcome::AlreadyDeleted,
                    ) => Ok(()),
                    Err(error) => Err(error.api_error().code()),
                },
            );
        }
        for id in &selection.files {
            let result = self.delete_file(caller, *id).await;
            outcomes.insert(
                id.to_string(),
                match result {
                    Ok(_) => Ok(()),
                    Err(error) => Err(failure_code(&error)),
                },
            );
        }

        let mut report = BatchDeleteResult {
            succeeded: Vec::new(),
            failed: Vec::new(),
        };
        let requested = selection
            .files
            .iter()
            .map(ToString::to_string)
            .chain(selection.folders.iter().map(ToString::to_string));
        for id in requested {
            match outcomes.remove(&id) {
                Some(Ok(())) => report.succeeded.push(id),
                Some(Err(code)) => report.failed.push(BatchDeleteFailure { id, code }),
                None => {
                    return Err(FileError::RepositoryInvariant {
                        column: "batch_outcome",
                    })
                }
            }
        }
        Ok(report)
    }

    async fn folder_depths(
        &self,
        owner: UserId,
        folders: &[FolderId],
    ) -> Result<HashMap<String, i64>, FileError> {
        if folders.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<String> = folders.iter().map(ToString::to_string).collect();
        let ids = serde_json::to_string(&ids)
            .map_err(|_| FileError::RepositoryInvariant { column: "ids" })?;
        let rows: Vec<(String, i64)> = sqlx::query_as(DEPTHS)
            .bind(owner.to_string())
            .bind(ids)
            .fetch_all(self.pools.reader().executor())
            .await?;
        Ok(rows.into_iter().collect())
    }
}

impl From<DeleteError> for FileError {
    fn from(error: DeleteError) -> Self {
        Self::Delete(error)
    }
}
