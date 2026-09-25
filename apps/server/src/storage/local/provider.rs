use async_trait::async_trait;
use tokio::io::AsyncReadExt as _;

use super::paths::{ObjectLocation, UploadId};
use super::{blocking, LocalProvider, StagingWriter};
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::StorageError;
use crate::storage::health::{ProbeDepth, SelfTestReport};
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    ListCursor, ListPage, ObjectBody, ObjectStat, PutHint, StorageDescriptor, StorageProvider,
};

impl LocalProvider {
    #[cfg(test)]
    pub(crate) fn temporary() -> Self {
        let root = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        for dir in ["storage/objects", "uploads", "branding"] {
            std::fs::create_dir_all(root.path().join(dir))
                .unwrap_or_else(|error| panic!("{error}"));
        }
        let mut provider =
            Self::open(root.path(), 262_144).unwrap_or_else(|error| panic!("{error}"));
        provider._owned_root = Some(root);
        provider
    }

    pub(super) async fn put_at(
        &self,
        location: &ObjectLocation<'_>,
        body: ObjectBody,
        declared_len: Option<u64>,
    ) -> Result<ObjectStat, StorageError> {
        let upload_id = UploadId::generate();
        let mut writer = self.create_staging(&upload_id)?;
        let placed = self
            .stage(&mut writer, body, declared_len)
            .await
            .and_then(|()| blocking(|| writer.sync()))
            .and_then(|()| blocking(|| self.finalize_at(&upload_id, location)));
        drop(writer);
        if let Err(error) = self.remove_staging(&upload_id) {
            tracing::warn!(
                storage_error = %error,
                "a staging directory could not be removed after a direct write; the staging sweep reclaims it"
            );
        }
        placed.map(|finalized| finalized.stat)
    }

    async fn stage(
        &self,
        writer: &mut StagingWriter,
        mut body: ObjectBody,
        declared_len: Option<u64>,
    ) -> Result<(), StorageError> {
        let mut buffer = vec![0_u8; self.buffer_bytes];
        let mut staged = 0_u64;
        loop {
            let read = body.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            staged = staged.saturating_add(read as u64);
            if declared_len.is_some_and(|declared| staged > declared) {
                return Err(StorageError::SizeMismatch {
                    expected: declared_len.unwrap_or_default(),
                    actual: staged,
                });
            }
            writer.append(&buffer[..read])?;
        }
        match declared_len {
            Some(expected) if expected != staged => Err(StorageError::SizeMismatch {
                expected,
                actual: staged,
            }),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl StorageProvider for LocalProvider {
    fn caps(&self) -> &StorageCapabilities {
        &StorageCapabilities::LOCAL
    }

    fn describe(&self) -> StorageDescriptor {
        Self::describe(self)
    }

    async fn put_stream(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        self.put_at(&ObjectLocation::of(key), body, hint.declared_len)
            .await
    }

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        Self::open_read(self, key)
    }

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        Self::open_range(self, key, start, len)
    }

    async fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        Self::stat(self, key)
    }

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        Self::delete(self, key)
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        Self::exists(self, key)
    }

    async fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        blocking(|| Self::copy(self, src, dst))
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        blocking(|| Self::list_page(self, prefix, cursor, page_size))
    }

    async fn self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        self.run_self_test(depth).await
    }
}
