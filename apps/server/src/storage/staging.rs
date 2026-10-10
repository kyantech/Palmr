use std::sync::Arc;

use async_trait::async_trait;

use super::error::StorageError;
use super::provider::StorageProvider;

pub use super::local::UploadId;

#[async_trait]
pub trait StagingStorage: Send + Sync {
    async fn ensure(&self, id: &UploadId) -> Result<Box<dyn StagingAppend>, StorageError>;

    async fn write_hint(&self, id: &UploadId, hint: &[u8]) -> Result<(), StorageError>;

    async fn staged_len(&self, id: &UploadId) -> Result<Option<u64>, StorageError>;

    async fn remove(&self, id: &UploadId) -> Result<bool, StorageError>;
}

#[async_trait]
pub trait StagingAppend: Send {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError>;

    async fn flush(&mut self) -> Result<(), StorageError>;

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
}
