use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sqlx::{Row, SqliteConnection};

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{
    self, FileDeletedFacts, FileDeletionScope, FolderDeletedFacts,
};
use crate::features::audit::model::{Actor, AuditEvent, Outcome, Target, TargetType};
use crate::features::audit::service::AuditService;
use crate::features::files::delete::{
    receipt_exists, remove_files_in_tx, DeleteError, DeletionCaller, DoomedFile, DELETE_BATCH_ROWS,
};
use crate::features::users::model::UserId;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{
    ClaimedJob, DedupKey, Idempotency, JobKind, JobPayload, NewJob, NonRetryable, Registry,
};
use crate::storage::lifecycle::DeletionReason;

use super::error::FolderError;
use super::impact::{deletion_impact, ensure_selectable, DeletionImpact};
use super::model::FolderId;
use super::repo;
use super::service::FolderService;

pub const BATCHES_PER_RUN: u32 = 20;
pub const TRANSFER_RETRY_DELAY: Duration = Duration::from_secs(5 * 60);
pub const DEDUP_PREFIX: &str = "folder-del:";
pub const CODE_PAYLOAD_INVALID: &str = "FOLDER_DELETE_PAYLOAD_INVALID";
pub const CODE_QUOTA_INTEGRITY: &str = "FOLDER_DELETE_QUOTA_INTEGRITY";

macro_rules! subtree {
    () => {
        "WITH RECURSIVE walk(id, level) AS (
    SELECT id, 0 FROM folders WHERE id = ?1 AND owner_id = ?2
    UNION ALL
    SELECT c.id, w.level + 1 FROM folders c JOIN walk w ON c.parent_id = w.id
     WHERE c.owner_id = ?2 AND w.level < 64
)
"
    };
}

const SELECT_FILES: &str = concat!(
    subtree!(),
    "SELECT fi.id, fi.storage_object_id, fi.size_bytes FROM files fi
 WHERE fi.owner_id = ?2 AND fi.folder_id IN (SELECT id FROM walk)
 ORDER BY fi.id LIMIT ?3"
);

const SELECT_FOLDERS: &str = concat!(
    subtree!(),
    "SELECT f.id, f.depth FROM folders f JOIN walk w ON f.id = w.id
 WHERE f.id <> ?1
 ORDER BY f.depth DESC, f.id LIMIT ?3"
);

const ACTIVE_TRANSFERS: &str = concat!(
    subtree!(),
    "SELECT EXISTS(SELECT 1 FROM transfer_sessions t
 WHERE t.target_folder_id IN (SELECT id FROM walk)
   AND t.state NOT IN ('completed', 'canceled', 'expired'))"
);

const DETACH_TRANSFERS: &str = "UPDATE transfer_sessions
      SET deleted_target_folder_id = target_folder_id, target_folder_id = NULL
      WHERE target_folder_id IN (SELECT value FROM json_each(?1))
        AND state IN ('completed', 'canceled', 'expired')";

const LOAD_INTENT: &str = "SELECT owner_id, files_deleted, folders_deleted, bytes_released
      FROM folder_deletions WHERE folder_id = ?1";

const MARK_DELETING: &str =
    "UPDATE folders SET deleting = 1 WHERE id = ?1 AND owner_id = ?2 AND deleting = 0";

const RECORD_INTENT: &str =
    "INSERT INTO folder_deletions (folder_id, owner_id, claimed_at, updated_at)
      VALUES (?1, ?2, ?3, ?3)";

const ADD_FILE_PROGRESS: &str = "UPDATE folder_deletions
      SET files_deleted = files_deleted + ?2, bytes_released = bytes_released + ?3, updated_at = ?4
      WHERE folder_id = ?1";

const ADD_FOLDER_PROGRESS: &str = "UPDATE folder_deletions
      SET folders_deleted = folders_deleted + ?2, updated_at = ?3 WHERE folder_id = ?1";

const DETACH_FOLDER_SHARE_ITEMS: &str =
    "DELETE FROM share_items WHERE folder_id IN (SELECT value FROM json_each(?1))";

const WRITE_FOLDER_RECEIPTS: &str = "INSERT OR IGNORE INTO deletion_receipts
        (id, resource_kind, owner_id, deleted_at)
      SELECT value, 'folder', ?2, ?3 FROM json_each(?1)";

const REMOVE_FOLDERS: &str =
    "DELETE FROM folders WHERE owner_id = ?2 AND id IN (SELECT value FROM json_each(?1))";

const SETTLE_NESTED_INTENTS: &str =
    "DELETE FROM folder_deletions WHERE owner_id = ?2 AND folder_id IN (SELECT value FROM json_each(?1))";

const FIND_ROOT_NAME: &str = "SELECT name FROM folders WHERE id = ?1 AND owner_id = ?2";

const DROP_INTENT: &str = "DELETE FROM folder_deletions WHERE folder_id = ?1";

const OWNER_LABEL: &str = "SELECT username FROM users WHERE id = ?1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed,
    AlreadyDeleting,
    AlreadyDeleted,
}

pub async fn claim_folder_deletion_in_tx(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    owner: UserId,
    id: FolderId,
    at: Timestamp,
) -> Result<ClaimOutcome, FolderError> {
    let Some(folder) = repo::find_owned(tx.executor(), owner, id).await? else {
        return if receipt_exists(tx.executor(), "folder", owner, &id.to_string()).await? {
            Ok(ClaimOutcome::AlreadyDeleted)
        } else {
            Err(FolderError::NotFound)
        };
    };
    if folder.hidden {
        return Ok(ClaimOutcome::AlreadyDeleting);
    }
    let marked = sqlx::query(MARK_DELETING)
        .bind(id.to_string())
        .bind(owner.to_string())
        .execute(tx.executor())
        .await?
        .rows_affected();
    if marked != 1 {
        return Err(FolderError::RepositoryInvariant { column: "deleting" });
    }
    sqlx::query(RECORD_INTENT)
        .bind(id.to_string())
        .bind(owner.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    let payload = JobPayload::new(&json!({
        "folder_id": id.to_string(),
        "owner_id": owner.to_string(),
    }))
    .map_err(|_| FolderError::RepositoryInvariant { column: "payload" })?;
    let key = DedupKey::new(format!("{DEDUP_PREFIX}{id}")).map_err(DeleteError::from)?;
    let job = NewJob::new(JobKind::FoldersDeleteTree, payload).dedup_key(key);
    enqueue(tx, clock, &job).await.map_err(DeleteError::from)?;
    Ok(ClaimOutcome::Claimed)
}

impl FolderService {
    pub async fn delete_folder(
        &self,
        caller: &DeletionCaller,
        id: FolderId,
    ) -> Result<ClaimOutcome, FolderError> {
        let at = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), "folders.delete_claim", async |tx| {
                claim_folder_deletion_in_tx(tx, self.clock.as_ref(), caller.owner, id, at).await
            })
            .await
    }

    pub async fn folder_impact(
        &self,
        owner: UserId,
        id: FolderId,
    ) -> Result<DeletionImpact, FolderError> {
        let reader = self.pools.reader().executor();
        if ensure_selectable(reader, owner, &[], &[id])
            .await?
            .is_some()
        {
            return Err(FolderError::NotFound);
        }
        let now = Timestamp::try_from(self.clock.now())?;
        deletion_impact(reader, owner, &[], &[id], now).await
    }
}

#[derive(Clone)]
pub struct DeleteTreeContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    audit: AuditService,
    batches_per_run: u32,
}

impl DeleteTreeContext {
    pub fn new(pools: DbPools, clock: Arc<dyn Clock>, audit: AuditService) -> Self {
        Self {
            pools,
            clock,
            audit,
            batches_per_run: BATCHES_PER_RUN,
        }
    }

    #[must_use]
    #[cfg(test)]
    pub const fn with_batches_per_run(mut self, batches: u32) -> Self {
        self.batches_per_run = batches;
        self
    }
}

pub fn register_jobs(registry: Registry, context: DeleteTreeContext) -> Registry {
    registry.register(
        JobKind::FoldersDeleteTree,
        Idempotency::key("folder-del:<root id> intent row; every batch is its own transaction"),
        move |job: ClaimedJob| {
            let context = context.clone();
            async move { run(job, context).await }
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Files { files: u64 },
    Folders { folders: u64 },
    Deferred,
    Completed,
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drain {
    Finished,
    Continue,
    Deferred,
}

async fn run(job: ClaimedJob, context: DeleteTreeContext) -> anyhow::Result<()> {
    let root = job
        .payload()
        .get("folder_id")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<FolderId>().ok());
    let Some(root) = root else {
        tracing::error!(job_id = %job.id(), "folders.delete_tree payload is not a folder id");
        return Err(NonRetryable::new(CODE_PAYLOAD_INVALID).into());
    };
    match drain(&context, root, context.batches_per_run).await {
        Ok(Drain::Finished) => Ok(()),
        Ok(Drain::Continue) => schedule_continuation(&context, root, None)
            .await
            .map_err(anyhow::Error::from),
        Ok(Drain::Deferred) => schedule_continuation(&context, root, Some(job.id().to_string()))
            .await
            .map_err(anyhow::Error::from),
        Err(error) if error.is_quota_integrity() => {
            tracing::error!(
                error_kind = error.kind(),
                folder_id = %root,
                "folder deletion stopped on a quota integrity failure"
            );
            Err(NonRetryable::new(CODE_QUOTA_INTEGRITY).into())
        }
        Err(error) => {
            tracing::warn!(
                error_kind = error.kind(),
                folder_id = %root,
                "folder deletion batch failed; the job runtime retries it"
            );
            Err(error.into())
        }
    }
}

pub async fn drain(
    context: &DeleteTreeContext,
    root: FolderId,
    max_batches: u32,
) -> Result<Drain, DeleteError> {
    for _ in 0..max_batches {
        match step(context, root).await? {
            Step::Completed | Step::Gone => return Ok(Drain::Finished),
            Step::Deferred => return Ok(Drain::Deferred),
            Step::Files { .. } | Step::Folders { .. } => {}
        }
        tokio::task::yield_now().await;
    }
    Ok(Drain::Continue)
}

async fn schedule_continuation(
    context: &DeleteTreeContext,
    root: FolderId,
    waiting_on: Option<String>,
) -> Result<(), DeleteError> {
    let clock = context.clock.as_ref();
    let run_at = waiting_on
        .as_ref()
        .map(|_| Timestamp::try_from(clock.now() + TRANSFER_RETRY_DELAY))
        .transpose()?;
    context
        .pools
        .write_tx(clock, "folders.delete_tree_continue", async |tx| {
            let Some(intent) = load_intent(tx.executor(), root).await? else {
                return Ok(());
            };
            let payload = JobPayload::new(&json!({
                "folder_id": root.to_string(),
                "owner_id": intent.owner.to_string(),
            }))
            .map_err(|_| DeleteError::Invariant { what: "payload" })?;
            let key = match &waiting_on {
                Some(job) => format!("{DEDUP_PREFIX}{root}:wait:{job}"),
                None => format!(
                    "{DEDUP_PREFIX}{root}:{}:{}",
                    intent.files_deleted, intent.folders_deleted
                ),
            };
            let mut job =
                NewJob::new(JobKind::FoldersDeleteTree, payload).dedup_key(DedupKey::new(key)?);
            if let Some(run_at) = run_at {
                job = job.run_at(run_at);
            }
            enqueue(tx, clock, &job).await?;
            Ok(())
        })
        .await
}

#[derive(Debug, Clone, Copy)]
struct Intent {
    owner: UserId,
    files_deleted: u64,
    folders_deleted: u64,
    bytes_released: u64,
}

async fn load_intent(
    connection: &mut SqliteConnection,
    root: FolderId,
) -> Result<Option<Intent>, DeleteError> {
    let invariant = |what| DeleteError::Invariant { what };
    let row = sqlx::query(LOAD_INTENT)
        .bind(root.to_string())
        .fetch_optional(connection)
        .await?;
    row.map(|row| {
        let owner: String = row.try_get("owner_id").map_err(|_| invariant("owner_id"))?;
        let count = |column: &'static str| -> Result<u64, DeleteError> {
            let value: i64 = row.try_get(column).map_err(|_| invariant(column))?;
            u64::try_from(value).map_err(|_| invariant(column))
        };
        Ok(Intent {
            owner: owner.parse().map_err(|_| invariant("owner_id"))?,
            files_deleted: count("files_deleted")?,
            folders_deleted: count("folders_deleted")?,
            bytes_released: count("bytes_released")?,
        })
    })
    .transpose()
}

pub async fn step(context: &DeleteTreeContext, root: FolderId) -> Result<Step, DeleteError> {
    let at = Timestamp::try_from(context.clock.now())?;
    context
        .pools
        .write_tx(
            context.clock.as_ref(),
            "folders.delete_tree_batch",
            async |tx| step_in_tx(context, tx, root, at).await,
        )
        .await
}

async fn step_in_tx(
    context: &DeleteTreeContext,
    tx: &mut WriteTx<'_>,
    root: FolderId,
    at: Timestamp,
) -> Result<Step, DeleteError> {
    let Some(intent) = load_intent(tx.executor(), root).await? else {
        return Ok(Step::Gone);
    };
    let owner = intent.owner;
    let limit = i64::try_from(DELETE_BATCH_ROWS).unwrap_or(i64::MAX);

    let rows = sqlx::query(SELECT_FILES)
        .bind(root.to_string())
        .bind(owner.to_string())
        .bind(limit)
        .fetch_all(tx.executor())
        .await?;
    if !rows.is_empty() {
        let files = rows
            .iter()
            .map(DoomedFile::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let removal = remove_files_in_tx(
            tx,
            context.clock.as_ref(),
            owner,
            &files,
            DeletionReason::FolderDeleted,
            at,
        )
        .await?;
        sqlx::query(ADD_FILE_PROGRESS)
            .bind(root.to_string())
            .bind(i64::try_from(removal.files).unwrap_or(i64::MAX))
            .bind(i64::try_from(removal.bytes).unwrap_or(i64::MAX))
            .bind(at.to_string())
            .execute(tx.executor())
            .await?;
        let label = owner_label(tx.executor(), owner).await?;
        let event = AuditEvent::new(
            actions::file_deleted(&FileDeletedFacts {
                scope: FileDeletionScope::FolderTree,
                files: removal.files,
                bytes_released: removal.bytes,
                shares_detached: removal.shares_detached,
                embeds_revoked: removal.embeds_revoked,
            }),
            Actor::user(&owner.to_string(), &label),
            Outcome::Success,
            at,
        )
        .with_target(Target::new(TargetType::Folder).id(&root.to_string()));
        context.audit.record_in_tx(tx, &event).await?;
        return Ok(Step::Files {
            files: removal.files,
        });
    }

    let live_transfer: i64 = sqlx::query_scalar(ACTIVE_TRANSFERS)
        .bind(root.to_string())
        .bind(owner.to_string())
        .fetch_one(tx.executor())
        .await?;
    if live_transfer != 0 {
        return Ok(Step::Deferred);
    }

    let rows = sqlx::query(SELECT_FOLDERS)
        .bind(root.to_string())
        .bind(owner.to_string())
        .bind(limit)
        .fetch_all(tx.executor())
        .await?;
    if !rows.is_empty() {
        let ids = rows
            .iter()
            .map(|row| {
                let id: String = row
                    .try_get("id")
                    .map_err(|_| DeleteError::Invariant { what: "folder_id" })?;
                let depth: i64 = row
                    .try_get("depth")
                    .map_err(|_| DeleteError::Invariant { what: "depth" })?;
                Ok((id, depth))
            })
            .collect::<Result<Vec<_>, DeleteError>>()?;
        let removed = remove_folders_in_tx(tx, owner, &ids, at).await?;
        sqlx::query(ADD_FOLDER_PROGRESS)
            .bind(root.to_string())
            .bind(i64::try_from(removed).unwrap_or(i64::MAX))
            .bind(at.to_string())
            .execute(tx.executor())
            .await?;
        return Ok(Step::Folders { folders: removed });
    }

    let present = sqlx::query(FIND_ROOT_NAME)
        .bind(root.to_string())
        .bind(owner.to_string())
        .fetch_optional(tx.executor())
        .await?;
    let Some(present) = present else {
        sqlx::query(DROP_INTENT)
            .bind(root.to_string())
            .execute(tx.executor())
            .await?;
        return Ok(Step::Gone);
    };
    let name: String = present
        .try_get("name")
        .map_err(|_| DeleteError::Invariant { what: "name" })?;
    remove_folders_in_tx(tx, owner, &[(root.to_string(), 0)], at).await?;
    sqlx::query(DROP_INTENT)
        .bind(root.to_string())
        .execute(tx.executor())
        .await?;
    let label = owner_label(tx.executor(), owner).await?;
    let event = AuditEvent::new(
        actions::folder_deleted(&FolderDeletedFacts {
            files: intent.files_deleted,
            folders: intent.folders_deleted.saturating_add(1),
            bytes_released: intent.bytes_released,
        }),
        Actor::user(&owner.to_string(), &label),
        Outcome::Success,
        at,
    )
    .with_target(
        Target::new(TargetType::Folder)
            .id(&root.to_string())
            .label(&name),
    );
    context.audit.record_in_tx(tx, &event).await?;
    Ok(Step::Completed)
}

async fn owner_label(
    connection: &mut SqliteConnection,
    owner: UserId,
) -> Result<String, DeleteError> {
    let label: Option<String> = sqlx::query_scalar(OWNER_LABEL)
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    Ok(label.unwrap_or_else(|| owner.to_string()))
}

async fn remove_folders_in_tx(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    folders: &[(String, i64)],
    at: Timestamp,
) -> Result<u64, DeleteError> {
    let owner_text = owner.to_string();
    let every: Vec<&str> = folders.iter().map(|(id, _)| id.as_str()).collect();
    let list =
        serde_json::to_string(&every).map_err(|_| DeleteError::Invariant { what: "folder_ids" })?;
    sqlx::query(DETACH_FOLDER_SHARE_ITEMS)
        .bind(&list)
        .execute(tx.executor())
        .await?;
    sqlx::query(WRITE_FOLDER_RECEIPTS)
        .bind(&list)
        .bind(&owner_text)
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    sqlx::query(SETTLE_NESTED_INTENTS)
        .bind(&list)
        .bind(&owner_text)
        .execute(tx.executor())
        .await?;
    sqlx::query(DETACH_TRANSFERS)
        .bind(&list)
        .execute(tx.executor())
        .await?;
    let mut removed = 0_u64;
    let mut ordered: Vec<&(String, i64)> = folders.iter().collect();
    ordered.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    for level in ordered.chunk_by(|left, right| left.1 == right.1) {
        let ids: Vec<&str> = level.iter().map(|(id, _)| id.as_str()).collect();
        let level_list = serde_json::to_string(&ids)
            .map_err(|_| DeleteError::Invariant { what: "folder_ids" })?;
        removed += sqlx::query(REMOVE_FOLDERS)
            .bind(&level_list)
            .bind(&owner_text)
            .execute(tx.executor())
            .await?
            .rows_affected();
    }
    if usize::try_from(removed).ok() != Some(folders.len()) {
        return Err(DeleteError::Invariant {
            what: "folders_removed",
        });
    }
    Ok(removed)
}
