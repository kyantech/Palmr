use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use super::paths::{
    ensure_leaf_dir, open_dir, open_regular, ObjectLocation, UploadId, STAGING_BLOB,
};
use super::{
    best_effort, confirm_placed, measure, refuse_existing, sync_directory, Destination,
    LocalProvider, Step, FILE_MODE,
};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::ObjectStat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeRoute {
    Rename,
    CrossDeviceCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finalized {
    pub stat: ObjectStat,
    pub route: FinalizeRoute,
}

impl LocalProvider {
    pub fn finalize_staged(
        &self,
        upload_id: &UploadId,
        final_key: &ObjectKey,
    ) -> Result<Finalized, StorageError> {
        let staging_dir = open_dir(self.uploads.as_fd(), upload_id.as_str())?;
        let staging = open_regular(
            staging_dir.as_fd(),
            STAGING_BLOB,
            OFlags::RDONLY,
            Mode::empty(),
        )?;
        self.ops
            .fsync(staging.as_fd(), Step::SyncStaging)
            .map_err(StorageError::from)?;

        let location = ObjectLocation::of(final_key);
        let leaf_dir = ensure_leaf_dir(self.ops.as_ref(), self.root(location.root), &location)?;
        let destination = Destination {
            leaf_dir: leaf_dir.as_fd(),
            name: location.leaf,
        };
        refuse_existing(&destination)?;

        match self.ops.rename(
            staging_dir.as_fd(),
            STAGING_BLOB,
            destination.leaf_dir,
            destination.name,
            Step::RenameStaging,
        ) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                return self.copy_across_devices(
                    upload_id,
                    staging_dir.as_fd(),
                    &staging,
                    &destination,
                );
            }
            Err(error) => return Err(StorageError::from(error)),
        }

        confirm_placed(&staging, &destination)?;
        sync_directory(self.ops.as_ref(), destination.leaf_dir, Step::SyncLeaf)?;
        Ok(Finalized {
            stat: measure(&staging)?,
            route: FinalizeRoute::Rename,
        })
    }

    fn copy_across_devices(
        &self,
        upload_id: &UploadId,
        staging_dir: BorrowedFd<'_>,
        staging: &File,
        destination: &Destination<'_>,
    ) -> Result<Finalized, StorageError> {
        tracing::warn!(
            event = "storage.finalize.cross_device_copy",
            upload_id = %upload_id,
            "upload staging and object storage are on different filesystems; finalizing by bounded copy"
        );
        let temp_name = upload_id.temp_name();
        let temp = self.create_temp(destination.leaf_dir, &temp_name)?;

        let placed = self
            .fill_temp(staging, &temp)
            .and_then(|()| {
                self.ops
                    .fsync(temp.as_fd(), Step::SyncTemp)
                    .map_err(StorageError::from)
            })
            .and_then(|()| {
                self.ops
                    .rename(
                        destination.leaf_dir,
                        &temp_name,
                        destination.leaf_dir,
                        destination.name,
                        Step::RenameTemp,
                    )
                    .map_err(StorageError::from)
            });
        if let Err(error) = placed {
            best_effort(
                self.ops
                    .unlink(destination.leaf_dir, &temp_name, Step::UnlinkTemp),
                Step::UnlinkTemp,
                &[Errno::NOENT],
            );
            return Err(error);
        }

        confirm_placed(&temp, destination)?;
        sync_directory(self.ops.as_ref(), destination.leaf_dir, Step::SyncLeaf)?;
        let stat = measure(&temp)?;
        self.discard_staging_blob(staging_dir, upload_id);
        Ok(Finalized {
            stat,
            route: FinalizeRoute::CrossDeviceCopy,
        })
    }

    pub(super) fn create_temp(
        &self,
        leaf_dir: BorrowedFd<'_>,
        temp_name: &str,
    ) -> Result<File, StorageError> {
        let create = || {
            open_regular(
                leaf_dir,
                temp_name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                Mode::from_raw_mode(FILE_MODE),
            )
        };
        match create() {
            Err(StorageError::AlreadyExists) => {
                tracing::warn!(
                    event = "storage.finalize.stale_temp",
                    "removing a temporary object left by an interrupted finalization of this upload"
                );
                self.ops
                    .unlink(leaf_dir, temp_name, Step::UnlinkTemp)
                    .map_err(StorageError::from)?;
                create()
            }
            other => other,
        }
    }

    fn fill_temp(&self, mut staging: &File, temp: &File) -> Result<(), StorageError> {
        let expected = staging.metadata()?.len();
        let mut buffer = vec![0_u8; self.buffer_bytes];
        let mut copied = 0_u64;
        loop {
            let read = match staging.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(StorageError::from(error)),
            };
            self.ops.write_all(temp, &buffer[..read], Step::WriteTemp)?;
            copied += u64::try_from(read).unwrap_or(u64::MAX);
        }
        if copied == expected {
            Ok(())
        } else {
            Err(StorageError::Io(io::Error::other(
                "the staging file changed size while it was being finalized",
            )))
        }
    }
}
