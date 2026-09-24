use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use super::paths::{open_dir, open_regular, UploadId, STAGING_BLOB, STAGING_META};
use super::{
    best_effort, classify, sync_directory, LocalProvider, Step, DIRECTORY_MODE, FILE_MODE,
};
use crate::storage::error::StorageError;

#[derive(Debug)]
pub struct StagingWriter {
    file: File,
    buffer_bytes: usize,
}

impl StagingWriter {
    pub fn staged_len(&self) -> Result<u64, StorageError> {
        Ok(self.file.metadata()?.len())
    }

    pub fn append(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        self.file.write_all(chunk).map_err(StorageError::from)
    }

    pub fn append_from(&mut self, source: &mut dyn Read, limit: u64) -> Result<u64, StorageError> {
        let mut buffer = vec![0_u8; self.buffer_bytes];
        let mut source = source.take(limit);
        let mut total = 0_u64;
        loop {
            let read = match source.read(&mut buffer) {
                Ok(0) => return Ok(total),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(StorageError::from(error)),
            };
            self.file.write_all(&buffer[..read])?;
            total += u64::try_from(read).unwrap_or(u64::MAX);
        }
    }

    pub fn sync(&self) -> Result<(), StorageError> {
        self.file.sync_all().map_err(StorageError::from)
    }
}

impl LocalProvider {
    pub fn create_staging(&self, upload_id: &UploadId) -> Result<StagingWriter, StorageError> {
        let name = upload_id.as_str();
        match rustix::fs::mkdirat(&self.uploads, name, Mode::from_raw_mode(DIRECTORY_MODE)) {
            Ok(()) => {}
            Err(Errno::EXIST) => return Err(StorageError::AlreadyExists),
            Err(errno) => return Err(classify(errno, name)),
        }
        sync_directory(
            self.ops.as_ref(),
            self.uploads.as_fd(),
            Step::SyncCreatedDir,
        )?;
        let dir = open_dir(self.uploads.as_fd(), name)?;
        let file = open_regular(
            dir.as_fd(),
            STAGING_BLOB,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::APPEND,
            Mode::from_raw_mode(FILE_MODE),
        )?;
        sync_directory(self.ops.as_ref(), dir.as_fd(), Step::SyncCreatedDir)?;
        Ok(self.writer(file))
    }

    pub fn open_staging(&self, upload_id: &UploadId) -> Result<StagingWriter, StorageError> {
        let dir = open_dir(self.uploads.as_fd(), upload_id.as_str())?;
        let file = open_regular(
            dir.as_fd(),
            STAGING_BLOB,
            OFlags::WRONLY | OFlags::APPEND,
            Mode::empty(),
        )?;
        Ok(self.writer(file))
    }

    pub fn remove_staging(&self, upload_id: &UploadId) -> Result<bool, StorageError> {
        let dir = match open_dir(self.uploads.as_fd(), upload_id.as_str()) {
            Ok(dir) => dir,
            Err(StorageError::NotFound) => return Ok(false),
            Err(error) => return Err(error),
        };
        for (entry, step) in [
            (STAGING_BLOB, Step::UnlinkStaging),
            (STAGING_META, Step::UnlinkStaging),
        ] {
            match self.ops.unlink(dir.as_fd(), entry, step) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(StorageError::from(error))
                }
                _ => {}
            }
        }
        match self.ops.remove_dir(
            self.uploads.as_fd(),
            upload_id.as_str(),
            Step::RemoveStagingDir,
        ) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(StorageError::from(error)),
            _ => Ok(true),
        }
    }

    pub(super) fn discard_staging_blob(&self, staging_dir: BorrowedFd<'_>, upload_id: &UploadId) {
        best_effort(
            self.ops
                .unlink(staging_dir, STAGING_BLOB, Step::UnlinkStaging),
            Step::UnlinkStaging,
            &[Errno::NOENT],
        );
        best_effort(
            self.ops.remove_dir(
                self.uploads.as_fd(),
                upload_id.as_str(),
                Step::RemoveStagingDir,
            ),
            Step::RemoveStagingDir,
            &[Errno::NOENT, Errno::NOTEMPTY, Errno::EXIST],
        );
    }

    fn writer(&self, file: File) -> StagingWriter {
        StagingWriter {
            file,
            buffer_bytes: self.buffer_bytes,
        }
    }
}
