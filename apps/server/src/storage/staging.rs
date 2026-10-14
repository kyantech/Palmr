use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::error::StorageError;
use super::key::ObjectKey;
use super::provider::StorageProvider;

pub use super::local::UploadId;

pub const MIME_PREFIX_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedObject {
    pub size: u64,
    pub prefix: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceRoute {
    Rename,
    CrossDeviceCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub size: u64,
    pub route: PlaceRoute,
}

#[async_trait]
pub trait StagingStorage: Send + Sync {
    async fn ensure(&self, id: &UploadId) -> Result<Box<dyn StagingAppend>, StorageError>;

    async fn write_hint(&self, id: &UploadId, hint: &[u8]) -> Result<(), StorageError>;

    async fn staged_len(&self, id: &UploadId) -> Result<Option<u64>, StorageError>;

    async fn remove(&self, id: &UploadId) -> Result<bool, StorageError>;

    async fn measure(&self, id: &UploadId) -> Result<Option<StagedObject>, StorageError>;

    async fn inspect(&self, key: &ObjectKey) -> Result<Option<StagedObject>, StorageError>;

    async fn staged_matches(&self, id: &UploadId, key: &ObjectKey) -> Result<bool, StorageError>;

    async fn place(
        &self,
        id: &UploadId,
        key: &ObjectKey,
        cancel: &CancellationToken,
    ) -> Result<Placed, StorageError>;
}

#[async_trait]
pub trait StagingAppend: Send {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError>;

    async fn flush(&mut self) -> Result<(), StorageError>;

    async fn truncate(&mut self, len: u64) -> Result<(), StorageError>;

    async fn len(&mut self) -> Result<u64, StorageError>;
}

#[cfg(test)]
pub(crate) fn temporary_local(root: &std::path::Path) -> Arc<dyn StagingStorage> {
    for dir in ["storage/objects", "uploads", "branding"] {
        std::fs::create_dir_all(root.join(dir)).unwrap_or_else(|error| panic!("{error}"));
    }
    Arc::new(
        super::local::LocalProvider::open(root, 262_144).unwrap_or_else(|error| panic!("{error}")),
    )
}

#[derive(Clone)]
pub struct ProviderStaging {
    provider: Arc<dyn StorageProvider>,
}

impl ProviderStaging {
    pub fn of(provider: &Arc<dyn StorageProvider>) -> Option<Arc<dyn StagingStorage>> {
        provider.as_staging()?;
        Some(Arc::new(Self {
            provider: Arc::clone(provider),
        }))
    }

    fn staging(&self) -> Result<&dyn StagingStorage, StorageError> {
        self.provider
            .as_staging()
            .ok_or_else(|| StorageError::Config("the storage provider has no staging area".into()))
    }
}

#[async_trait]
impl StagingStorage for ProviderStaging {
    async fn ensure(&self, id: &UploadId) -> Result<Box<dyn StagingAppend>, StorageError> {
        self.staging()?.ensure(id).await
    }

    async fn write_hint(&self, id: &UploadId, hint: &[u8]) -> Result<(), StorageError> {
        self.staging()?.write_hint(id, hint).await
    }

    async fn staged_len(&self, id: &UploadId) -> Result<Option<u64>, StorageError> {
        self.staging()?.staged_len(id).await
    }

    async fn remove(&self, id: &UploadId) -> Result<bool, StorageError> {
        self.staging()?.remove(id).await
    }

    async fn measure(&self, id: &UploadId) -> Result<Option<StagedObject>, StorageError> {
        self.staging()?.measure(id).await
    }

    async fn inspect(&self, key: &ObjectKey) -> Result<Option<StagedObject>, StorageError> {
        self.staging()?.inspect(key).await
    }

    async fn staged_matches(&self, id: &UploadId, key: &ObjectKey) -> Result<bool, StorageError> {
        self.staging()?.staged_matches(id, key).await
    }

    async fn place(
        &self,
        id: &UploadId,
        key: &ObjectKey,
        cancel: &CancellationToken,
    ) -> Result<Placed, StorageError> {
        self.staging()?.place(id, key, cancel).await
    }
}
