use std::fmt;

use super::thumbnails::ThumbnailRemoval;
use super::{parse_provider, DeleteBlobPayload, LifecycleContext, LifecycleError, StorageObjectId};
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::infra::db::DbError;
use crate::infra::jobs::{ClaimedJob, NonRetryable};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::ProviderKind;

pub const MAX_QUEUE_ERROR_BYTES: usize = 512;

pub const CODE_PAYLOAD_INVALID: &str = "STORAGE_DELETE_PAYLOAD_INVALID";
pub const CODE_NOT_TOMBSTONED: &str = "STORAGE_OBJECT_NOT_TOMBSTONED";
pub const CODE_INVALID_KEY: &str = "STORAGE_INVALID_KEY";
pub const CODE_PROVIDER_MISMATCH: &str = "STORAGE_PROVIDER_MISMATCH";

const SELECT_WORK: &str = "SELECT q.state, o.object_key, o.provider, o.state, o.refcount
                             FROM file_deletion_queue q
                             JOIN storage_objects o ON o.id = q.storage_object_id
                            WHERE q.storage_object_id = ?1";

const MARK_DELETING: &str = "UPDATE file_deletion_queue
                                SET state = 'deleting', attempts = attempts + 1, last_attempt_at = ?2
                              WHERE storage_object_id = ?1";

const MARK_FAILED: &str = "UPDATE file_deletion_queue
                              SET state = 'failed', last_error = ?2, last_attempt_at = ?3
                            WHERE storage_object_id = ?1 AND state <> 'done'";

const CONFIRM_OBJECT: &str = "UPDATE storage_objects
                                 SET state = 'deleted', deleted_at = ?2, updated_at = ?2
                               WHERE id = ?1 AND state = 'tombstoned' AND refcount = 0";

const CONFIRM_QUEUE: &str = "UPDATE file_deletion_queue
                                SET state = 'done', completed_at = ?2, last_error = NULL
                              WHERE storage_object_id = ?1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted { existed: bool },
    AlreadyDone,
    NothingQueued,
}

#[derive(Debug)]
pub enum DeleteBlobError {
    Lifecycle(LifecycleError),
    Refused(&'static str),
    ProviderMismatch {
        expected: ProviderKind,
        found: ProviderKind,
    },
    Storage(StorageError),
}

impl DeleteBlobError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Lifecycle(error) => error.kind(),
            Self::Refused(code) => code,
            Self::ProviderMismatch { .. } => CODE_PROVIDER_MISMATCH,
            Self::Storage(error) => error.kind(),
        }
    }
}

impl fmt::Display for DeleteBlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lifecycle(error) => error.fmt(f),
            Self::Refused(code) => write!(f, "physical deletion refused: {code}"),
            Self::ProviderMismatch { expected, found } => write!(
                f,
                "storage object belongs to provider {found}, configured provider is {expected}"
            ),
            Self::Storage(error) => write!(f, "physical deletion failed: {}", error.kind()),
        }
    }
}

impl std::error::Error for DeleteBlobError {}

impl From<LifecycleError> for DeleteBlobError {
    fn from(error: LifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<sqlx::Error> for DeleteBlobError {
    fn from(error: sqlx::Error) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

impl From<DbError> for DeleteBlobError {
    fn from(error: DbError) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

impl From<InvalidTimestamp> for DeleteBlobError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

enum Plan {
    Delete(ObjectKey),
    Done,
    Nothing,
    Refuse(&'static str),
    Mismatch(ProviderKind),
}

pub fn queue_error(error: &StorageError) -> String {
    let text = format!("{} {}", error.api_code().as_str(), error.kind());
    if text.len() <= MAX_QUEUE_ERROR_BYTES {
        return text;
    }
    let mut end = MAX_QUEUE_ERROR_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub(super) async fn run(job: ClaimedJob, context: LifecycleContext) -> anyhow::Result<()> {
    let id = serde_json::from_value::<DeleteBlobPayload>(job.payload().clone())
        .ok()
        .and_then(|payload| payload.storage_object_id.parse::<StorageObjectId>().ok());
    let Some(id) = id else {
        tracing::error!(job_id = %job.id(), "storage.delete_blob payload is not a storage object id");
        return Err(NonRetryable::new(CODE_PAYLOAD_INVALID).into());
    };
    match delete_object(&context, id).await {
        Ok(_) => Ok(()),
        Err(DeleteBlobError::Refused(code)) => Err(NonRetryable::new(code).into()),
        Err(error) => {
            tracing::warn!(
                storage_object_id = %id,
                error_kind = error.kind(),
                "physical deletion did not complete; the job runtime retries it"
            );
            Err(error.into())
        }
    }
}

pub async fn delete_object(
    context: &LifecycleContext,
    id: StorageObjectId,
) -> Result<DeleteOutcome, DeleteBlobError> {
    let plan = begin(context, id).await?;
    let key = match plan {
        Plan::Delete(key) => key,
        Plan::Done => return Ok(DeleteOutcome::AlreadyDone),
        Plan::Nothing => {
            tracing::debug!(storage_object_id = %id, "no deletion is queued for this storage object");
            return Ok(DeleteOutcome::NothingQueued);
        }
        Plan::Refuse(code) => {
            tracing::error!(
                storage_object_id = %id,
                error_code = code,
                "physical deletion refused; the storage object is left untouched"
            );
            return Err(DeleteBlobError::Refused(code));
        }
        Plan::Mismatch(found) => {
            tracing::error!(
                storage_object_id = %id,
                error_code = CODE_PROVIDER_MISMATCH,
                recorded_provider = found.as_str(),
                configured_provider = context.configured.as_str(),
                "physical deletion skipped because the object belongs to another storage provider"
            );
            return Err(DeleteBlobError::ProviderMismatch {
                expected: context.configured,
                found,
            });
        }
    };

    let existed = match context.provider.delete(&key).await {
        Ok(existed) => existed,
        Err(StorageError::NotFound) => false,
        Err(StorageError::InvalidKey) => {
            record_failure(context, id, CODE_INVALID_KEY).await?;
            tracing::error!(
                storage_object_id = %id,
                error_code = CODE_INVALID_KEY,
                "the storage provider rejected a recorded object key"
            );
            return Err(DeleteBlobError::Refused(CODE_INVALID_KEY));
        }
        Err(error) => {
            record_failure(context, id, &queue_error(&error)).await?;
            return Err(DeleteBlobError::Storage(error));
        }
    };

    confirm(context, id).await?;
    tracing::info!(storage_object_id = %id, existed, "storage object deleted");
    match context.thumbnails.remove(&key).await {
        ThumbnailRemoval::Failed(kind) => tracing::warn!(
            storage_object_id = %id,
            io_error = %kind,
            "a derived thumbnail could not be removed; it is regenerable and left for cache cleanup"
        ),
        ThumbnailRemoval::Removed | ThumbnailRemoval::Absent | ThumbnailRemoval::NotApplicable => {}
    }
    Ok(DeleteOutcome::Deleted { existed })
}

async fn begin(context: &LifecycleContext, id: StorageObjectId) -> Result<Plan, DeleteBlobError> {
    let clock = context.clock.as_ref();
    let configured = context.configured;
    context
        .pools
        .write_tx(clock, "storage.delete_blob.begin", async |tx| {
            let now = Timestamp::try_from(clock.now())?.to_string();
            let row: Option<(String, String, String, String, i64)> = sqlx::query_as(SELECT_WORK)
                .bind(id.to_string())
                .fetch_optional(tx.executor())
                .await?;
            let Some((queue_state, key, provider, state, refcount)) = row else {
                return Ok(Plan::Nothing);
            };
            if queue_state == "done" {
                return Ok(Plan::Done);
            }
            let plan = if state != "tombstoned" || refcount != 0 {
                Plan::Refuse(CODE_NOT_TOMBSTONED)
            } else {
                match parse_provider(&provider) {
                    Some(found) if found == configured => match ObjectKey::parse(&key) {
                        Ok(key) => Plan::Delete(key),
                        Err(_) => Plan::Refuse(CODE_INVALID_KEY),
                    },
                    Some(found) => Plan::Mismatch(found),
                    None => Plan::Refuse(CODE_PROVIDER_MISMATCH),
                }
            };
            let (statement, error) = match &plan {
                Plan::Delete(_) => (MARK_DELETING, None),
                Plan::Refuse(code) => (MARK_FAILED, Some(*code)),
                Plan::Mismatch(_) => (MARK_FAILED, Some(CODE_PROVIDER_MISMATCH)),
                Plan::Done | Plan::Nothing => return Ok(plan),
            };
            let query = sqlx::query(statement).bind(id.to_string());
            let query = match error {
                Some(code) => query.bind(code).bind(&now),
                None => query.bind(&now),
            };
            query.execute(tx.executor()).await?;
            Ok::<Plan, DeleteBlobError>(plan)
        })
        .await
}

async fn record_failure(
    context: &LifecycleContext,
    id: StorageObjectId,
    error: &str,
) -> Result<(), DeleteBlobError> {
    let clock = context.clock.as_ref();
    context
        .pools
        .write_tx(clock, "storage.delete_blob.failure", async |tx| {
            sqlx::query(MARK_FAILED)
                .bind(id.to_string())
                .bind(error)
                .bind(Timestamp::try_from(clock.now())?.to_string())
                .execute(tx.executor())
                .await?;
            Ok::<(), DeleteBlobError>(())
        })
        .await
}

async fn confirm(context: &LifecycleContext, id: StorageObjectId) -> Result<(), DeleteBlobError> {
    let clock = context.clock.as_ref();
    context
        .pools
        .write_tx(clock, "storage.delete_blob.confirm", async |tx| {
            let now = Timestamp::try_from(clock.now())?.to_string();
            sqlx::query(CONFIRM_OBJECT)
                .bind(id.to_string())
                .bind(&now)
                .execute(tx.executor())
                .await?;
            sqlx::query(CONFIRM_QUEUE)
                .bind(id.to_string())
                .bind(&now)
                .execute(tx.executor())
                .await?;
            Ok::<(), DeleteBlobError>(())
        })
        .await
}
