pub mod delete_blob;
pub mod sweep;
pub mod thumbnails;

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::thumbnails::ThumbnailCache;
use super::key::ObjectKey;
use super::provider::StorageProvider;
use super::ProviderKind;
use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::service::AuditService;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::infra::jobs::claim::{enqueue, Enqueued, MAX_ROWS_PER_TX};
use crate::infra::jobs::{
    ClaimedJob, DedupKey, Idempotency, InvalidDedupKey, JobKind, JobPayload, JobsError, NewJob,
    PayloadError, Registry,
};

pub enum StorageObject {}

pub type StorageObjectId = Id<StorageObject>;

enum QueueEntry {}

type QueueEntryId = Id<QueueEntry>;

pub const MAX_TOMBSTONE_BATCH: usize = MAX_ROWS_PER_TX as usize;
pub const ABANDONED_UPLOAD_GRACE: Duration = Duration::from_secs(60 * 60);
pub const DEDUP_PREFIX: &str = "storage_object:";

const SELECT_STATES: &str = "SELECT id, state FROM storage_objects
                              WHERE id IN (SELECT value FROM json_each(?1))";

const TOMBSTONE_ACTIVE: &str = "UPDATE storage_objects
                                   SET state = 'tombstoned', refcount = 0, tombstoned_at = ?2,
                                       updated_at = ?2
                                 WHERE id IN (SELECT value FROM json_each(?1)) AND state = 'active'
                             RETURNING id, size_bytes";

const INSERT_UNCOMMITTED: &str =
    "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state,
                                                               refcount, created_at, updated_at,
                                                               tombstoned_at)
                                  VALUES (?1, ?2, ?3, ?4, 'tombstoned', 0, ?5, ?5, ?5)
                                  ON CONFLICT (id) DO NOTHING";

const SELECT_UNCOMMITTED: &str =
    "SELECT state, object_key, provider FROM storage_objects WHERE id = ?1";

const INSERT_QUEUE: &str =
    "INSERT INTO file_deletion_queue (id, storage_object_id, reason, state, attempts,
                                                             requested_at, not_before)
                            VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?5)
                            ON CONFLICT (storage_object_id) DO NOTHING";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeletionReason {
    FileDeleted,
    FolderDeleted,
    ReceivedDeleted,
    ReverseShareDeleted,
    ReceivedRetentionExpired,
    UserDeleted,
    UploadAbandoned,
    UploadRejected,
    QuotaRejected,
    BrandingReplaced,
    AvatarReplaced,
    CopyFailed,
    OrphanSweep,
}

impl DeletionReason {
    pub const ALL: [Self; 13] = [
        Self::FileDeleted,
        Self::FolderDeleted,
        Self::ReceivedDeleted,
        Self::ReverseShareDeleted,
        Self::ReceivedRetentionExpired,
        Self::UserDeleted,
        Self::UploadAbandoned,
        Self::UploadRejected,
        Self::QuotaRejected,
        Self::BrandingReplaced,
        Self::AvatarReplaced,
        Self::CopyFailed,
        Self::OrphanSweep,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileDeleted => "file_deleted",
            Self::FolderDeleted => "folder_deleted",
            Self::ReceivedDeleted => "received_deleted",
            Self::ReverseShareDeleted => "reverse_share_deleted",
            Self::ReceivedRetentionExpired => "received_retention_expired",
            Self::UserDeleted => "user_deleted",
            Self::UploadAbandoned => "upload_abandoned",
            Self::UploadRejected => "upload_rejected",
            Self::QuotaRejected => "quota_rejected",
            Self::BrandingReplaced => "branding_replaced",
            Self::AvatarReplaced => "avatar_replaced",
            Self::CopyFailed => "copy_failed",
            Self::OrphanSweep => "orphan_sweep",
        }
    }

    pub const fn grace(self) -> Duration {
        match self {
            Self::UploadAbandoned => ABANDONED_UPLOAD_GRACE,
            _ => Duration::ZERO,
        }
    }
}

impl fmt::Display for DeletionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum LifecycleError {
    Db(DbError),
    Jobs(JobsError),
    Time(InvalidTimestamp),
    Payload(PayloadError),
    BatchTooLarge { requested: usize },
    UnknownObjects { requested: usize, found: usize },
    CommittedObject,
    KeyMismatch,
    SizeOutOfRange,
    CorruptRow(&'static str),
}

impl LifecycleError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(error) => error.kind().as_str(),
            Self::Jobs(error) => error.kind(),
            Self::Time(_) => "time_out_of_range",
            Self::Payload(_) => "invalid_payload",
            Self::BatchTooLarge { .. } => "batch_too_large",
            Self::UnknownObjects { .. } => "unknown_storage_object",
            Self::CommittedObject => "committed_object",
            Self::KeyMismatch => "key_mismatch",
            Self::SizeOutOfRange => "size_out_of_range",
            Self::CorruptRow(_) => "corrupt_row",
        }
    }
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "storage lifecycle database operation failed: {error}"),
            Self::Jobs(error) => write!(f, "storage lifecycle job operation failed: {error}"),
            Self::Time(error) => write!(f, "storage lifecycle time is out of range: {error}"),
            Self::Payload(error) => write!(f, "storage lifecycle job payload is invalid: {error}"),
            Self::BatchTooLarge { requested } => write!(
                f,
                "{requested} storage objects exceed the tombstone batch limit of {MAX_TOMBSTONE_BATCH}"
            ),
            Self::UnknownObjects { requested, found } => write!(
                f,
                "{requested} storage objects were named for tombstoning but only {found} exist"
            ),
            Self::CommittedObject => f.write_str(
                "an uncommitted object id already belongs to an active storage object",
            ),
            Self::KeyMismatch => f.write_str(
                "an uncommitted object id is already recorded under a different key or provider",
            ),
            Self::SizeOutOfRange => f.write_str("a measured object size exceeds the database range"),
            Self::CorruptRow(column) => write!(f, "storage_objects row has an invalid {column}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

impl From<DbError> for LifecycleError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for LifecycleError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<JobsError> for LifecycleError {
    fn from(error: JobsError) -> Self {
        Self::Jobs(error)
    }
}

impl From<InvalidTimestamp> for LifecycleError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<PayloadError> for LifecycleError {
    fn from(error: PayloadError) -> Self {
        Self::Payload(error)
    }
}

impl From<InvalidDedupKey> for LifecycleError {
    fn from(error: InvalidDedupKey) -> Self {
        Self::Jobs(JobsError::from(error))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TombstoneReport {
    pub tombstoned: u64,
    pub tombstoned_bytes: u64,
    pub already_tombstoned: u64,
    pub already_deleted: u64,
    pub queued: u64,
    pub jobs_enqueued: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncommittedOutcome {
    Queued { queued: bool, job_enqueued: bool },
    AlreadyDeleted,
}

#[derive(Debug, Clone)]
pub struct PlacedObject<'a> {
    pub id: StorageObjectId,
    pub key: &'a ObjectKey,
    pub provider: ProviderKind,
    pub measured_size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeleteBlobPayload {
    storage_object_id: String,
}

pub fn dedup_key(id: StorageObjectId) -> Result<DedupKey, InvalidDedupKey> {
    DedupKey::new(format!("{DEDUP_PREFIX}{id}"))
}

pub fn parse_provider(text: &str) -> Option<ProviderKind> {
    match text {
        "local" => Some(ProviderKind::Local),
        "s3" => Some(ProviderKind::S3),
        _ => None,
    }
}

fn json_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    Value::from_iter(items).to_string()
}

pub async fn tombstone(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    ids: &[StorageObjectId],
    reason: DeletionReason,
) -> Result<TombstoneReport, LifecycleError> {
    let mut report = TombstoneReport::default();
    if ids.is_empty() {
        return Ok(report);
    }
    if ids.len() > MAX_TOMBSTONE_BATCH {
        return Err(LifecycleError::BatchTooLarge {
            requested: ids.len(),
        });
    }
    let now = Timestamp::try_from(clock.now())?;
    let requested: BTreeSet<String> = ids.iter().map(ToString::to_string).collect();
    let list = json_list(requested.iter().map(String::as_str));

    let states: Vec<(String, String)> = sqlx::query_as(SELECT_STATES)
        .bind(&list)
        .fetch_all(tx.executor())
        .await?;
    if states.len() != requested.len() {
        return Err(LifecycleError::UnknownObjects {
            requested: requested.len(),
            found: states.len(),
        });
    }

    let flipped: Vec<(String, i64)> = sqlx::query_as(TOMBSTONE_ACTIVE)
        .bind(&list)
        .bind(now.to_string())
        .fetch_all(tx.executor())
        .await?;
    report.tombstoned = u64::try_from(flipped.len()).unwrap_or(u64::MAX);
    report.tombstoned_bytes = flipped
        .iter()
        .map(|(_, size)| u64::try_from(*size).unwrap_or(0))
        .fold(0_u64, u64::saturating_add);

    for (id, state) in &states {
        match state.as_str() {
            "deleted" => {
                report.already_deleted += 1;
                continue;
            }
            "tombstoned" => report.already_tombstoned += 1,
            "active" => {}
            _ => return Err(LifecycleError::CorruptRow("state")),
        }
        let id: StorageObjectId = id.parse().map_err(|_| LifecycleError::CorruptRow("id"))?;
        let (queued, enqueued) = queue_deletion(tx, clock, id, reason, now).await?;
        report.queued += u64::from(queued);
        report.jobs_enqueued += u64::from(enqueued);
    }
    Ok(report)
}

pub async fn tombstone_uncommitted(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    object: &PlacedObject<'_>,
    reason: DeletionReason,
) -> Result<UncommittedOutcome, LifecycleError> {
    let now = Timestamp::try_from(clock.now())?;
    let size = i64::try_from(object.measured_size.unwrap_or(0))
        .map_err(|_| LifecycleError::SizeOutOfRange)?;
    let id = object.id.to_string();
    sqlx::query(INSERT_UNCOMMITTED)
        .bind(&id)
        .bind(object.key.as_str())
        .bind(object.provider.as_str())
        .bind(size)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;

    let (state, key, provider): (String, String, String) = sqlx::query_as(SELECT_UNCOMMITTED)
        .bind(&id)
        .fetch_one(tx.executor())
        .await?;
    if key != object.key.as_str() || provider != object.provider.as_str() {
        return Err(LifecycleError::KeyMismatch);
    }
    match state.as_str() {
        "tombstoned" => {
            let (queued, job_enqueued) = queue_deletion(tx, clock, object.id, reason, now).await?;
            Ok(UncommittedOutcome::Queued {
                queued,
                job_enqueued,
            })
        }
        "deleted" => Ok(UncommittedOutcome::AlreadyDeleted),
        "active" => Err(LifecycleError::CommittedObject),
        _ => Err(LifecycleError::CorruptRow("state")),
    }
}

async fn queue_deletion(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    id: StorageObjectId,
    reason: DeletionReason,
    now: Timestamp,
) -> Result<(bool, bool), LifecycleError> {
    let not_before = Timestamp::try_from(now.get() + reason.grace())?;
    let queued = sqlx::query(INSERT_QUEUE)
        .bind(QueueEntryId::generate(clock).to_string())
        .bind(id.to_string())
        .bind(reason.as_str())
        .bind(now.to_string())
        .bind(not_before.to_string())
        .execute(tx.executor())
        .await?
        .rows_affected()
        == 1;
    let payload = JobPayload::new(&DeleteBlobPayload {
        storage_object_id: id.to_string(),
    })?;
    let job = NewJob::new(JobKind::StorageDeleteBlob, payload)
        .run_at(not_before)
        .dedup_key(dedup_key(id)?);
    let enqueued = matches!(enqueue(tx, clock, &job).await?, Enqueued::Inserted(_));
    Ok((queued, enqueued))
}

#[derive(Clone)]
pub struct LifecycleContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    provider: Arc<dyn StorageProvider>,
    configured: ProviderKind,
    thumbnails: ThumbnailCache,
    audit: AuditService,
    orphan_reap: bool,
}

impl LifecycleContext {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        provider: Arc<dyn StorageProvider>,
        thumbnails: ThumbnailCache,
        audit: AuditService,
        orphan_reap: bool,
    ) -> Self {
        let configured = provider.describe().provider;
        Self {
            pools,
            clock,
            provider,
            configured,
            thumbnails,
            audit,
            orphan_reap,
        }
    }
}

impl fmt::Debug for LifecycleContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifecycleContext")
            .field("configured", &self.configured)
            .field("orphan_reap", &self.orphan_reap)
            .finish_non_exhaustive()
    }
}

pub fn register_jobs(registry: Registry, context: LifecycleContext) -> Registry {
    let delete = context.clone();
    registry
        .register(
            JobKind::StorageDeleteBlob,
            Idempotency::key("storage_object:<id> queue row and tombstoned state"),
            move |job: ClaimedJob| {
                let context = delete.clone();
                async move { delete_blob::run(job, context).await }
            },
        )
        .register(
            JobKind::StorageOrphanSweep,
            Idempotency::key("storage.orphan_sweep daily bucket"),
            move |job: ClaimedJob| {
                let context = context.clone();
                async move { sweep::run(job, context).await }
            },
        )
}

#[cfg(test)]
mod tests;
