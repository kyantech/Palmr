use std::io::Write as _;
use std::os::fd::AsFd;

use async_trait::async_trait;
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use super::paths::{open_dir, open_regular, UploadId, STAGING_BLOB, STAGING_META};
use super::read::stat_regular;
use super::StagingWriter;
use super::{blocking, classify, sync_directory, LocalProvider, Step, DIRECTORY_MODE, FILE_MODE};
use crate::storage::error::StorageError;
use crate::storage::staging::{StagingAppend, StagingStorage};

const MAX_HINT_BYTES: usize = 4_096;

impl LocalProvider {
    pub fn ensure_staging(&self, upload_id: &UploadId) -> Result<StagingWriter, StorageError> {
        let name = upload_id.as_str();
        let created =
            match rustix::fs::mkdirat(&self.uploads, name, Mode::from_raw_mode(DIRECTORY_MODE)) {
                Ok(()) => true,
                Err(Errno::EXIST) => false,
                Err(errno) => return Err(classify(errno, name)),
            };
        if created {
            sync_directory(
                self.ops.as_ref(),
                self.uploads.as_fd(),
                Step::SyncCreatedDir,
            )?;
        }
        let dir = open_dir(self.uploads.as_fd(), name)?;
        let file = open_regular(
            dir.as_fd(),
            STAGING_BLOB,
            OFlags::WRONLY | OFlags::CREATE | OFlags::APPEND,
            Mode::from_raw_mode(FILE_MODE),
        )?;
        sync_directory(self.ops.as_ref(), dir.as_fd(), Step::SyncCreatedDir)?;
        Ok(self.writer(file))
    }

    pub fn write_staging_hint(
        &self,
        upload_id: &UploadId,
        hint: &[u8],
    ) -> Result<(), StorageError> {
        if hint.len() > MAX_HINT_BYTES {
            return Err(StorageError::Config(format!(
                "a staging hint is limited to {MAX_HINT_BYTES} bytes"
            )));
        }
        let dir = open_dir(self.uploads.as_fd(), upload_id.as_str())?;
        let mut file = open_regular(
            dir.as_fd(),
            STAGING_META,
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC,
            Mode::from_raw_mode(FILE_MODE),
        )?;
        file.write_all(hint)?;
        self.ops
            .fsync(file.as_fd(), Step::SyncTemp)
            .map_err(StorageError::from)?;
        sync_directory(self.ops.as_ref(), dir.as_fd(), Step::SyncCreatedDir)
    }

    pub fn staged_len(&self, upload_id: &UploadId) -> Result<Option<u64>, StorageError> {
        let dir = match open_dir(self.uploads.as_fd(), upload_id.as_str()) {
            Ok(dir) => dir,
            Err(StorageError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        match stat_regular(dir.as_fd(), STAGING_BLOB) {
            Ok(stat) => Ok(Some(stat.size)),
            Err(StorageError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl StagingAppend for StagingWriter {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        blocking(|| self.append(chunk))
    }

    async fn flush(&mut self) -> Result<(), StorageError> {
        blocking(|| self.sync())
    }

    async fn len(&mut self) -> Result<u64, StorageError> {
        self.staged_len()
    }
}

#[async_trait]
impl StagingStorage for LocalProvider {
    async fn ensure(&self, id: &UploadId) -> Result<Box<dyn StagingAppend>, StorageError> {
        let writer = blocking(|| self.ensure_staging(id))?;
        Ok(Box::new(writer))
    }

    async fn write_hint(&self, id: &UploadId, hint: &[u8]) -> Result<(), StorageError> {
        blocking(|| self.write_staging_hint(id, hint))
    }

    async fn staged_len(&self, id: &UploadId) -> Result<Option<u64>, StorageError> {
        blocking(|| Self::staged_len(self, id))
    }

    async fn remove(&self, id: &UploadId) -> Result<bool, StorageError> {
        blocking(|| self.remove_staging(id))
    }
}
