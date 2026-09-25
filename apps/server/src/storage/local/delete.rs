use std::io;
use std::os::fd::AsFd;

use super::paths::ObjectLocation;
use super::{LocalProvider, Step};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;

impl LocalProvider {
    pub fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.delete_at(&ObjectLocation::of(key))
    }

    pub(super) fn delete_at(&self, location: &ObjectLocation<'_>) -> Result<bool, StorageError> {
        let leaf_dir = match self.leaf_dir(location) {
            Ok(leaf_dir) => leaf_dir,
            Err(StorageError::NotFound) => return Ok(false),
            Err(error) => return Err(error),
        };
        match self
            .ops
            .unlink(leaf_dir.as_fd(), location.leaf, Step::UnlinkObject)
        {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StorageError::from(error)),
        }
    }
}
