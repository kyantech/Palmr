use std::os::fd::AsFd;

use super::LocalProvider;
use crate::storage::provider::{
    Capacity, CapacityReport, LocalStorageDescriptor, StorageDescriptor,
};
use crate::storage::ProviderKind;

const GIB: u64 = 1024 * 1024 * 1024;
const LOW_SPACE_PERCENT: u64 = 50;

pub fn is_low_space(capacity: &Capacity) -> bool {
    let floor = GIB;
    let percentage = capacity.total_bytes / LOW_SPACE_PERCENT;
    capacity.available_bytes < floor.max(percentage)
}

impl LocalProvider {
    pub fn capacity(&self) -> CapacityReport {
        match self.ops.filesystem_stats(self.objects.as_fd()) {
            Ok(capacity) => CapacityReport::Available(capacity),
            Err(error) => {
                tracing::warn!(
                    errno = error.raw_os_error(),
                    error_kind = %error.kind(),
                    "the storage capacity figure is unavailable; the filesystem reported an error"
                );
                CapacityReport::Unavailable
            }
        }
    }

    pub fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: ProviderKind::Local,
            local: Some(LocalStorageDescriptor {
                capacity: self.capacity(),
            }),
        }
    }
}
