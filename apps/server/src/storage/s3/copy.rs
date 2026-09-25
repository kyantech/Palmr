use super::{classify, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::ObjectStat;

impl S3Provider {
    pub(crate) async fn copy_object(
        &self,
        src: &ObjectKey,
        dst: &ObjectKey,
    ) -> Result<ObjectStat, StorageError> {
        self.internal()
            .copy_object()
            .bucket(self.bucket())
            .copy_source(copy_source(self.bucket(), src))
            .key(dst.as_str())
            .send()
            .await
            .map_err(|error| classify(Operation::CopyObject, error))?;
        self.head_object(dst).await
    }
}

pub(super) fn copy_source(bucket: &str, src: &ObjectKey) -> String {
    format!("{bucket}/{}", src.as_str())
}
