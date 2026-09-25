use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;
use tokio::fs::File as TokioFile;
use tokio::io::AsyncReadExt as _;
use tokio_util::io::{ReaderStream, StreamReader};

use super::paths::{open_regular, ObjectLocation};
use super::{classify, measure, modified_at, LocalProvider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{ObjectBody, ObjectStat};

const READ_CHUNK_BYTES: usize = 256 * 1024;

impl LocalProvider {
    pub fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.stat_at(&ObjectLocation::of(key))
    }

    pub fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.exists_at(&ObjectLocation::of(key))
    }

    pub fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.open_read_at(&ObjectLocation::of(key))
    }

    pub fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.open_range_at(&ObjectLocation::of(key), start, len)
    }

    pub(super) fn stat_at(
        &self,
        location: &ObjectLocation<'_>,
    ) -> Result<ObjectStat, StorageError> {
        let leaf_dir = self.leaf_dir(location)?;
        stat_regular(leaf_dir.as_fd(), location.leaf)
    }

    pub(super) fn exists_at(&self, location: &ObjectLocation<'_>) -> Result<bool, StorageError> {
        match self.stat_at(location) {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) fn open_read_at(
        &self,
        location: &ObjectLocation<'_>,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        let (stat, file) = self.open_file_at(location)?;
        Ok((stat, body(file, None)))
    }

    pub(super) fn open_range_at(
        &self,
        location: &ObjectLocation<'_>,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        let (stat, file) = self.open_file_at(location)?;
        if start >= stat.size {
            return Err(StorageError::RangeNotSatisfiable { size: stat.size });
        }
        let effective_len = len.min(stat.size - start);
        self.ops.seek(&file, start)?;
        Ok((stat, body(file, Some(effective_len))))
    }

    pub(super) fn open_file(&self, key: &ObjectKey) -> Result<(ObjectStat, File), StorageError> {
        self.open_file_at(&ObjectLocation::of(key))
    }

    fn open_file_at(
        &self,
        location: &ObjectLocation<'_>,
    ) -> Result<(ObjectStat, File), StorageError> {
        let leaf_dir = self.leaf_dir(location)?;
        let file = open_regular(
            leaf_dir.as_fd(),
            location.leaf,
            OFlags::RDONLY,
            Mode::empty(),
        )?;
        let stat = measure(&file)?;
        Ok((stat, file))
    }
}

pub(super) fn stat_regular(dir: BorrowedFd<'_>, name: &str) -> Result<ObjectStat, StorageError> {
    let stat = match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Err(StorageError::NotFound),
        Err(errno) => return Err(classify(errno, name)),
    };
    if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
        Ok(ObjectStat {
            size: u64::try_from(stat.st_size).unwrap_or(0),
            modified_at: modified_at(stat.st_mtime, stat.st_mtime_nsec),
            etag: None,
        })
    } else {
        tracing::error!(
            storage_error = "not_regular_file",
            entry = name,
            "a storage entry is not a regular file"
        );
        Err(StorageError::PermissionDenied)
    }
}

fn body(file: File, len: Option<u64>) -> ObjectBody {
    let file = TokioFile::from_std(file);
    let reader: Box<dyn tokio::io::AsyncRead + Send + Unpin> = match len {
        Some(len) => Box::new(file.take(len)),
        None => Box::new(file),
    };
    Box::pin(StreamReader::new(ReaderStream::with_capacity(
        reader,
        READ_CHUNK_BYTES,
    )))
}
