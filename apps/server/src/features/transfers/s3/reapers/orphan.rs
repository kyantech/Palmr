use sha2::{Digest, Sha256};

use crate::domain::time::Timestamp;
use crate::infra::jobs::DedupKey;
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::MultipartHandle;

use super::super::super::error::TransferError;
use super::hooks::Point;
use super::{repo, S3ReaperContext, ORPHAN_MIN_AGE};

const MAX_UPLOAD_ID_CHARS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrphanTarget {
    pub(super) bucket: String,
    pub(super) key: ObjectKey,
    pub(super) upload_id: String,
    pub(super) initiated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OrphanOutcome {
    Aborted,
    Known,
    TooYoung,
}

impl OrphanTarget {
    pub(super) fn parse(
        bucket: &str,
        key: &str,
        upload_id: &str,
        initiated_at: &str,
    ) -> Result<Self, TransferError> {
        let invalid = |what| TransferError::Invariant { what };
        let key = ObjectKey::parse(key).map_err(|_| invalid("orphan_object_key"))?;
        if key.namespace() != KeyNamespace::Objects {
            return Err(invalid("orphan_object_key"));
        }
        let well_formed = !upload_id.is_empty()
            && upload_id.chars().count() <= MAX_UPLOAD_ID_CHARS
            && upload_id.chars().all(|c| !c.is_control());
        if !well_formed || bucket.is_empty() {
            return Err(invalid("orphan_identity"));
        }
        Ok(Self {
            bucket: bucket.to_owned(),
            key,
            upload_id: upload_id.to_owned(),
            initiated_at: initiated_at
                .parse()
                .map_err(|_| invalid("orphan_initiated_at"))?,
        })
    }

    pub(super) fn dedup_key(&self) -> Result<DedupKey, TransferError> {
        let mut digest = Sha256::new();
        digest.update(self.bucket.as_bytes());
        digest.update([0]);
        digest.update(self.key.as_str().as_bytes());
        digest.update([0]);
        digest.update(self.upload_id.as_bytes());
        let hex: String = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        DedupKey::new(format!("s3orphan:{hex}")).map_err(|_| TransferError::Invariant {
            what: "orphan_job_key",
        })
    }

    pub(super) fn old_enough(&self, now: Timestamp) -> Result<bool, TransferError> {
        let cutoff = Timestamp::try_from(self.initiated_at.get() + ORPHAN_MIN_AGE)?;
        Ok(now >= cutoff)
    }
}

pub(super) async fn abort_orphan(
    context: &S3ReaperContext,
    target: &OrphanTarget,
) -> Result<OrphanOutcome, TransferError> {
    let multipart = context
        .storage
        .multipart()
        .ok_or(TransferError::StorageUnavailable)?;
    if context.storage.bucket() != Some(target.bucket.as_str()) {
        return Err(TransferError::Invariant {
            what: "orphan_bucket",
        });
    }
    if !target.old_enough(context.now()?)? {
        return Ok(OrphanOutcome::TooYoung);
    }
    let known = {
        let mut connection = context.reader().await?;
        repo::provider_identity_known(
            &mut connection,
            &target.bucket,
            target.key.as_str(),
            &target.upload_id,
        )
        .await?
    };
    if known {
        return Ok(OrphanOutcome::Known);
    }
    context.reached(Point::OrphanBeforeAbort).await?;
    let handle = MultipartHandle::new(target.key.clone(), target.upload_id.clone());
    match multipart.abort_multipart(&handle).await {
        Ok(()) | Err(StorageError::NotFound) => Ok(OrphanOutcome::Aborted),
        Err(error) => Err(TransferError::Storage(error)),
    }
}
