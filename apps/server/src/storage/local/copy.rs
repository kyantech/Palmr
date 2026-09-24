use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsFd;

use rustix::io::Errno;

use super::paths::{ensure_leaf_dir, temp_name, ObjectLocation};
use super::{
    best_effort, confirm_placed, measure, refuse_existing, sync_directory, CloneStep,
    CopyRangeStep, Destination, LocalProvider, Step, REFLINK_SUPPORTED, REFLINK_UNSUPPORTED,
};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::ObjectStat;

const MAX_COPY_RANGE_REQUEST: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyProgress {
    Complete,
    Partial(u64),
}

impl LocalProvider {
    pub fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        let (source_stat, source) = self.open_file(src)?;
        let location = ObjectLocation::of(dst);
        let leaf_dir = ensure_leaf_dir(self.ops.as_ref(), self.root(location.root), &location)?;
        let destination = Destination {
            leaf_dir: leaf_dir.as_fd(),
            name: location.leaf,
        };
        refuse_existing(&destination)?;
        let temp_label = temp_name(location.leaf);
        let temp = self.create_temp(destination.leaf_dir, &temp_label)?;

        let placed = self
            .materialize(&source, source_stat.size, &temp)
            .and_then(|()| verify_size(&temp, source_stat.size))
            .and_then(|()| {
                self.ops
                    .fsync(temp.as_fd(), Step::SyncTemp)
                    .map_err(StorageError::from)
            })
            .and_then(|()| {
                self.ops
                    .rename(
                        destination.leaf_dir,
                        &temp_label,
                        destination.leaf_dir,
                        destination.name,
                        Step::RenameTemp,
                    )
                    .map_err(StorageError::from)
            });
        if let Err(error) = placed {
            best_effort(
                self.ops
                    .unlink(destination.leaf_dir, &temp_label, Step::UnlinkTemp),
                Step::UnlinkTemp,
                &[Errno::NOENT],
            );
            return Err(error);
        }

        confirm_placed(&temp, &destination)?;
        sync_directory(self.ops.as_ref(), destination.leaf_dir, Step::SyncLeaf)?;
        measure(&temp)
    }

    fn materialize(&self, source: &File, size: u64, temp: &File) -> Result<(), StorageError> {
        if size == 0 {
            return Ok(());
        }
        if self.reflink_into(source, temp)? {
            return Ok(());
        }
        match self.copy_range_into(source, size, temp)? {
            CopyProgress::Complete => Ok(()),
            CopyProgress::Partial(done) => self.stream_remaining(source, temp, done, size - done),
        }
    }

    fn reflink_into(&self, source: &File, temp: &File) -> Result<bool, StorageError> {
        if self.reflink_state() == REFLINK_UNSUPPORTED {
            return Ok(false);
        }
        match self.ops.try_reflink(temp, source) {
            Ok(CloneStep::Cloned) => {
                self.set_reflink_state(REFLINK_SUPPORTED);
                Ok(true)
            }
            Ok(CloneStep::Unsupported) => {
                self.set_reflink_state(REFLINK_UNSUPPORTED);
                Ok(false)
            }
            Err(error) => Err(StorageError::from(error)),
        }
    }

    fn copy_range_into(
        &self,
        source: &File,
        size: u64,
        temp: &File,
    ) -> Result<CopyProgress, StorageError> {
        let mut done = 0_u64;
        while done < size {
            let request =
                usize::try_from((size - done).min(MAX_COPY_RANGE_REQUEST)).unwrap_or(usize::MAX);
            match self.ops.copy_range_step(source, done, temp, done, request) {
                Ok(CopyRangeStep::Copied(0)) => return Ok(CopyProgress::Partial(done)),
                Ok(CopyRangeStep::Copied(copied)) => {
                    done = done.saturating_add(u64::try_from(copied).unwrap_or(u64::MAX));
                    if done >= size {
                        return Ok(CopyProgress::Complete);
                    }
                }
                Ok(CopyRangeStep::Unsupported) => return Ok(CopyProgress::Partial(done)),
                Err(error) => return Err(StorageError::from(error)),
            }
        }
        Ok(CopyProgress::Complete)
    }

    fn stream_remaining(
        &self,
        source: &File,
        temp: &File,
        offset: u64,
        remaining: u64,
    ) -> Result<(), StorageError> {
        if remaining == 0 {
            return Ok(());
        }
        if offset != 0 {
            self.ops.seek(source, offset)?;
            self.ops.seek(temp, offset)?;
        }
        let mut buffer = vec![0_u8; self.buffer_bytes];
        let mut limited = source.take(remaining);
        let mut copied = 0_u64;
        loop {
            let read = match limited.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(StorageError::from(error)),
            };
            self.ops.write_all(temp, &buffer[..read], Step::WriteTemp)?;
            copied = copied.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        if copied == remaining {
            Ok(())
        } else {
            Err(StorageError::Io(io::Error::other(
                "the source object changed size while it was being copied",
            )))
        }
    }
}

fn verify_size(temp: &File, expected: u64) -> Result<(), StorageError> {
    if measure(temp)?.size == expected {
        Ok(())
    } else {
        Err(StorageError::Io(io::Error::other(
            "the copied object changed size while it was being copied",
        )))
    }
}
