use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsFd;

use rustix::fs::{Mode, OFlags};

use super::paths::{open_dir, open_regular, UploadId, STAGING_BLOB};
use super::{measure, LocalProvider, Step};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::staging::{StagedObject, MIME_PREFIX_BYTES};

impl LocalProvider {
    pub fn measure_staged(&self, id: &UploadId) -> Result<Option<StagedObject>, StorageError> {
        let Some(file) = self.open_staged(id)? else {
            return Ok(None);
        };
        self.ops
            .fsync(file.as_fd(), Step::SyncStaging)
            .map_err(StorageError::from)?;
        staged_object(&file).map(Some)
    }

    pub fn inspect_object(&self, key: &ObjectKey) -> Result<Option<StagedObject>, StorageError> {
        match self.open_file(key) {
            Ok((_, file)) => staged_object(&file).map(Some),
            Err(StorageError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn staged_matches_object(
        &self,
        id: &UploadId,
        key: &ObjectKey,
    ) -> Result<bool, StorageError> {
        let Some(staged) = self.open_staged(id)? else {
            return Ok(false);
        };
        let (_, placed) = match self.open_file(key) {
            Ok(opened) => opened,
            Err(StorageError::NotFound) => return Ok(false),
            Err(error) => return Err(error),
        };
        if measure(&staged)?.size != measure(&placed)?.size {
            return Ok(false);
        }
        let mut left = vec![0_u8; self.buffer_bytes];
        let mut right = vec![0_u8; self.buffer_bytes];
        loop {
            let read = read_block(&staged, &mut left)?;
            let other = read_block(&placed, &mut right[..read])?;
            if read != other || left[..read] != right[..read] {
                return Ok(false);
            }
            if read == 0 {
                return Ok(true);
            }
        }
    }

    fn open_staged(&self, id: &UploadId) -> Result<Option<File>, StorageError> {
        let dir = match open_dir(self.uploads.as_fd(), id.as_str()) {
            Ok(dir) => dir,
            Err(StorageError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        match open_regular(dir.as_fd(), STAGING_BLOB, OFlags::RDONLY, Mode::empty()) {
            Ok(file) => Ok(Some(file)),
            Err(StorageError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn staged_object(file: &File) -> Result<StagedObject, StorageError> {
    let size = measure(file)?.size;
    let wanted = usize::try_from(size.min(MIME_PREFIX_BYTES as u64)).unwrap_or(MIME_PREFIX_BYTES);
    let mut prefix = vec![0_u8; wanted];
    let filled = read_block(file, &mut prefix)?;
    prefix.truncate(filled);
    Ok(StagedObject { size, prefix })
}

fn read_block(mut file: &File, buffer: &mut [u8]) -> Result<usize, StorageError> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(StorageError::from(error)),
        }
    }
    Ok(filled)
}
