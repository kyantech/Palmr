use super::profile::GIB;
use super::{classify, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::ObjectStat;

pub const SINGLE_COPY_MAX: u64 = 5 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRoute {
    CopyObject,
    UploadPartCopy,
}

impl CopyRoute {
    pub const fn for_size(size: u64, single_copy_max: u64) -> Self {
        if size <= single_copy_max {
            Self::CopyObject
        } else {
            Self::UploadPartCopy
        }
    }
}

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
