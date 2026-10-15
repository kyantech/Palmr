use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::time::Timestamp;
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{JobKind, JobPayload, NewJob};
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{
    ListCursor, MultipartHandle, MultipartStorage, PendingMultipart, PutHint,
};
use crate::storage::s3::service_code;

use super::super::super::error::TransferError;
use super::hooks::Point;
use super::orphan::OrphanTarget;
use super::payload::AbortJob;
use super::summary::Summary;
use super::{jobs_error, repo, S3ReaperContext, MAX_SCAN_PAGES};

const ORPHAN_TRANSACTION: &str = "transfers.s3_queue_orphans";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    ObjectsPrefix,
    BucketRoot,
}

impl Strategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObjectsPrefix => "objects_prefix",
            Self::BucketRoot => "bucket_root",
        }
    }

    const fn prefix(self) -> &'static str {
        match self {
            Self::ObjectsPrefix => "objects/",
            Self::BucketRoot => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Complete(Strategy),
    ExactKeyOnly,
    Unsupported,
    Incomplete(&'static str),
    NoMultipartStorage,
}

impl Coverage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete(_) => "complete",
            Self::ExactKeyOnly => "exact_key_only",
            Self::Unsupported => "unsupported",
            Self::Incomplete(_) => "incomplete",
            Self::NoMultipartStorage => "no_multipart_storage",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanRecord {
    pub coverage: Coverage,
    pub finished_at: Timestamp,
    pub pages: u32,
}

#[derive(Clone, Default)]
pub struct ScanStatus(Arc<Mutex<Option<ScanRecord>>>);

impl ScanStatus {
    #[cfg(test)]
    pub fn last(&self) -> Option<ScanRecord> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, record: ScanRecord) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(record);
    }
}

enum Pass {
    Complete { saw_sentinel: bool },
    Refused,
    Incomplete(&'static str),
}

pub const SENTINEL_PREFIX: &str = "objects/00/00/0000";

fn sentinel_key() -> ObjectKey {
    let drawn = ObjectKey::allocate(KeyNamespace::Objects);
    let tail = &drawn.as_str()[drawn.as_str().len() - 28..];
    ObjectKey::parse(&format!("{SENTINEL_PREFIX}{tail}")).unwrap_or(drawn)
}

fn refused(error: &StorageError) -> bool {
    matches!(error, StorageError::PermissionDenied)
        || matches!(
            service_code(error),
            Some("NotImplemented" | "XNotImplemented" | "MethodNotAllowed")
        )
}

pub(super) async fn scan_provider(context: &S3ReaperContext, summary: &mut Summary) -> Coverage {
    let mut pages = 0_u32;
    let coverage = enumerate(context, summary, &mut pages).await;
    match coverage {
        Coverage::Complete(_) | Coverage::NoMultipartStorage => {}
        Coverage::ExactKeyOnly | Coverage::Unsupported => {
            summary.listing_unsupported = true;
            tracing::warn!(
                coverage = coverage.as_str(),
                "this storage provider cannot enumerate multipart uploads by namespace: provider-side orphans from a crash between CreateMultipartUpload and its database row are not discovered automatically, and the database-driven abort of known uploads is unaffected; a bucket lifecycle rule for incomplete multipart uploads is the recommended protection"
            );
        }
        Coverage::Incomplete(reason) => {
            summary.listing_incomplete = true;
            tracing::warn!(
                reason,
                "the provider multipart enumeration did not finish; no conclusion is drawn from it and the next run scans again"
            );
        }
    }
    if let Ok(now) = context.now() {
        context.status.record(ScanRecord {
            coverage,
            finished_at: now,
            pages,
        });
    }
    coverage
}

async fn enumerate(context: &S3ReaperContext, summary: &mut Summary, pages: &mut u32) -> Coverage {
    let Some(multipart) = context.storage.multipart() else {
        return Coverage::NoMultipartStorage;
    };
    if context.storage.bucket().is_none() {
        return Coverage::NoMultipartStorage;
    }
    let sentinel_key = sentinel_key();
    let sentinel = match multipart
        .create_multipart(&sentinel_key, PutHint::default())
        .await
    {
        Ok(handle) => handle,
        Err(error) if refused(&error) => return Coverage::Unsupported,
        Err(error) => {
            tracing::warn!(
                storage_error = error.kind(),
                "the enumeration probe upload could not be created"
            );
            return Coverage::Incomplete("probe_create_failed");
        }
    };
    let mut coverage = Coverage::ExactKeyOnly;
    for strategy in [Strategy::ObjectsPrefix, Strategy::BucketRoot] {
        match pass(context, multipart, strategy, &sentinel, summary, pages).await {
            Pass::Complete { saw_sentinel: true } => {
                coverage = Coverage::Complete(strategy);
                break;
            }
            Pass::Complete {
                saw_sentinel: false,
            } => {}
            Pass::Refused => {
                coverage = Coverage::Unsupported;
                break;
            }
            Pass::Incomplete(reason) => {
                coverage = Coverage::Incomplete(reason);
                break;
            }
        }
    }
    if let Err(error) = multipart.abort_multipart(&sentinel).await {
        if !matches!(error, StorageError::NotFound) {
            tracing::warn!(
                storage_error = error.kind(),
                "the enumeration probe upload could not be aborted; the orphan sweep reclaims it after 24 hours"
            );
        }
    }
    coverage
}

async fn pass(
    context: &S3ReaperContext,
    multipart: &dyn MultipartStorage,
    strategy: Strategy,
    sentinel: &MultipartHandle,
    summary: &mut Summary,
    pages: &mut u32,
) -> Pass {
    let mut cursor: Option<ListCursor> = None;
    let mut saw_sentinel = false;
    loop {
        if context.stopping() {
            return Pass::Incomplete("shutdown");
        }
        if *pages >= MAX_SCAN_PAGES {
            return Pass::Incomplete("page_budget");
        }
        let page = match multipart
            .list_multipart_uploads(strategy.prefix(), cursor.clone())
            .await
        {
            Ok(page) => page,
            Err(error) if refused(&error) => return Pass::Refused,
            Err(error) => {
                tracing::warn!(
                    storage_error = error.kind(),
                    strategy = strategy.as_str(),
                    "a multipart listing page could not be read"
                );
                return Pass::Incomplete("listing_failed");
            }
        };
        *pages += 1;
        if context.reached(Point::ScanPageRead).await.is_err() {
            return Pass::Incomplete("interrupted");
        }
        match queue_orphans(context, &page.uploads, sentinel, summary).await {
            Ok(saw) => saw_sentinel |= saw,
            Err(error) => {
                tracing::warn!(
                    error_kind = error.kind(),
                    "provider-side orphans of a listing page could not be queued"
                );
                return Pass::Incomplete("queue_failed");
            }
        }
        match page.next {
            None => return Pass::Complete { saw_sentinel },
            Some(next) => {
                if cursor.as_ref() == Some(&next) {
                    return Pass::Incomplete("repeated_cursor");
                }
                cursor = Some(next);
            }
        }
    }
}

async fn queue_orphans(
    context: &S3ReaperContext,
    uploads: &[PendingMultipart],
    sentinel: &MultipartHandle,
    summary: &mut Summary,
) -> Result<bool, TransferError> {
    let Some(bucket) = context.storage.bucket() else {
        return Ok(false);
    };
    let now = context.now()?;
    let mut saw_sentinel = false;
    let mut candidates: Vec<OrphanTarget> = Vec::new();
    for upload in uploads {
        if upload.handle.key().namespace() != KeyNamespace::Objects {
            continue;
        }
        if upload.handle == *sentinel {
            saw_sentinel = true;
            continue;
        }
        summary.scanned += 1;
        let initiated_at = Timestamp::try_from(upload.initiated_at)?;
        let target = OrphanTarget {
            bucket: bucket.to_owned(),
            key: upload.handle.key().clone(),
            upload_id: upload.handle.upload_id().to_owned(),
            initiated_at,
        };
        if target.old_enough(now)? {
            candidates.push(target);
        } else {
            summary.provider_orphans_too_young += 1;
        }
    }
    if candidates.is_empty() {
        return Ok(saw_sentinel);
    }
    let clock = context.clock.as_ref();
    let queued = context
        .pools
        .write_tx(clock, ORPHAN_TRANSACTION, async |tx| {
            let keys: Vec<&str> = candidates
                .iter()
                .map(|target| target.key.as_str())
                .collect();
            let known = repo::known_pairs(tx.executor(), bucket, &keys).await?;
            let mut orphans = 0_u64;
            let mut inserted = 0_u64;
            for target in &candidates {
                let owned = known
                    .iter()
                    .any(|(key, upload)| key == target.key.as_str() && *upload == target.upload_id);
                if owned {
                    continue;
                }
                orphans += 1;
                let payload = JobPayload::new(&AbortJob::Orphan {
                    bucket: target.bucket.clone(),
                    key: target.key.as_str().to_owned(),
                    upload_id: target.upload_id.clone(),
                    initiated_at: target.initiated_at.to_string(),
                })
                .map_err(|_| TransferError::Invariant {
                    what: "orphan_job_payload",
                })?;
                let queued = enqueue(
                    tx,
                    clock,
                    &NewJob::new(JobKind::S3AbortAbandonedMultipart, payload)
                        .dedup_key(target.dedup_key()?),
                )
                .await
                .map_err(jobs_error)?;
                if matches!(queued, crate::infra::jobs::claim::Enqueued::Inserted(_)) {
                    inserted += 1;
                }
            }
            Ok::<_, TransferError>((orphans, inserted))
        })
        .await?;
    summary.provider_orphans += queued.0;
    summary.queued += queued.1;
    Ok(saw_sentinel)
}
