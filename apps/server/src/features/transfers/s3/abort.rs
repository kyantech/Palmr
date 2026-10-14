use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::DbPools;
use crate::storage::key::ObjectKey;
use crate::storage::provider::MultipartHandle;

use super::super::admission::TransferStorage;
use super::super::error::TransferError;
use super::model::S3Ref;
use super::repo;

const FINISH_TRANSACTION: &str = "transfers.s3_abort_finish";

pub async fn abort_at_provider(
    pools: &DbPools,
    clock: &dyn Clock,
    storage: &TransferStorage,
    s3: &S3Ref,
) -> Result<(), TransferError> {
    let multipart = storage
        .multipart()
        .ok_or(TransferError::StorageUnavailable)?;
    if storage.bucket() != Some(s3.bucket.as_str()) {
        return Err(TransferError::Invariant {
            what: "multipart_bucket",
        });
    }
    let key = ObjectKey::parse(&s3.object_key).map_err(|_| TransferError::Invariant {
        what: "multipart_object_key",
    })?;
    let handle = MultipartHandle::new(key, s3.upload_id.clone());
    multipart.abort_multipart(&handle).await?;
    let now = Timestamp::try_from(clock.now())?;
    pools
        .write_tx(clock, FINISH_TRANSACTION, async |tx| {
            repo::finish_abort(tx.executor(), s3.id, now).await
        })
        .await?;
    Ok(())
}
