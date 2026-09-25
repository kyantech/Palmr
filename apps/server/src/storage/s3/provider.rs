use async_trait::async_trait;

use super::assembly::{plan_rejected, WriteRoute};
use super::copy::CopyRoute;
use super::multipart::invalid;
use super::profile::ProfileLimits;
use super::{Operation, S3Provider};
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::StorageError;
use crate::storage::health::{ProbeDepth, SelfTestReport};
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    ListCursor, ListPage, MultipartStorage, ObjectBody, ObjectStat, PresignStorage, PutHint,
    StorageDescriptor, StorageProvider,
};
use crate::storage::ProviderKind;

pub const fn capabilities(limits: &ProfileLimits) -> StorageCapabilities {
    StorageCapabilities {
        supports_presigned_get: limits.supports_presigned_get,
        supports_presigned_put: !limits.requires_part_checksums,
        supports_server_side_copy: limits.supports_server_side_copy,
        supports_multipart: true,
        max_object_size: limits.max_object,
        min_part_size: limits.min_part,
        max_part_size: limits.max_part,
        max_parts: limits.max_parts,
        requires_checksum_headers: limits.requires_part_checksums,
    }
}

impl S3Provider {
    pub(super) fn effective_caps(&self) -> &StorageCapabilities {
        self.verified_caps.get().unwrap_or(&self.caps)
    }

    pub(super) fn relax_checksum_requirement(&self) -> bool {
        if !self.caps.requires_checksum_headers {
            return false;
        }
        let relaxed = StorageCapabilities {
            requires_checksum_headers: false,
            supports_presigned_put: true,
            ..self.caps
        };
        self.verified_caps.set(relaxed).is_ok()
    }
}

fn measured(expected: u64, stat: ObjectStat) -> Result<ObjectStat, StorageError> {
    if stat.size == expected {
        Ok(stat)
    } else {
        Err(StorageError::SizeMismatch {
            expected,
            actual: stat.size,
        })
    }
}

#[async_trait]
impl StorageProvider for S3Provider {
    fn caps(&self) -> &StorageCapabilities {
        self.effective_caps()
    }

    fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: ProviderKind::S3,
            local: None,
        }
    }

    async fn put_stream(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        let Some(len) = hint.declared_len else {
            return Err(invalid(
                Operation::PutObject,
                "an S3 write requires a declared length",
            ));
        };
        let route = WriteRoute::for_len(len, &self.limits())
            .map_err(|error| plan_rejected(Operation::PutObject, error))?;
        let written = match route {
            WriteRoute::PutObject => {
                self.put_object_single(key, body, len, hint.content_type.as_deref())
                    .await?
            }
            WriteRoute::Multipart(plan) => self.put_multipart(key, body, len, plan, hint).await?,
        };
        measured(len, written)
    }

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.get_object(key).await
    }

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.get_object_range(key, start, len).await
    }

    async fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.head_object(key).await
    }

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        let existed = self.object_exists(key).await?;
        self.delete_object(key).await?;
        Ok(existed)
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.object_exists(key).await
    }

    async fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        let source = self.head_object(src).await?;
        let copied = match CopyRoute::for_size(source.size, self.single_copy_max) {
            CopyRoute::CopyObject => self.copy_object(src, dst).await?,
            CopyRoute::UploadPartCopy => self.copy_multipart(src, dst, source.size).await?,
        };
        measured(source.size, copied)
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        self.list_objects_page(prefix, cursor, page_size).await
    }

    async fn self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        self.run_self_test(depth).await
    }

    fn as_multipart(&self) -> Option<&dyn MultipartStorage> {
        self.effective_caps().supports_multipart.then_some(self)
    }

    fn as_presign(&self) -> Option<&dyn PresignStorage> {
        self.effective_caps().supports_presigned_get.then_some(self)
    }
}
