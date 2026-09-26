use crate::domain::bytes::ByteSize;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageCapabilities {
    pub supports_presigned_get: bool,
    pub supports_presigned_put: bool,
    pub supports_server_side_copy: bool,
    pub supports_multipart: bool,

    pub max_object_size: u64,
    pub min_part_size: u64,
    pub max_part_size: u64,
    pub max_parts: u32,

    pub requires_checksum_headers: bool,
}

impl StorageCapabilities {
    pub const LOCAL: Self = Self {
        supports_presigned_get: false,
        supports_presigned_put: false,
        supports_server_side_copy: true,
        supports_multipart: false,
        max_object_size: u64::MAX,
        min_part_size: 0,
        max_part_size: 0,
        max_parts: 0,
        requires_checksum_headers: false,
    };

    pub const S3_DEFAULT: Self = Self {
        supports_presigned_get: true,
        supports_presigned_put: true,
        supports_server_side_copy: true,
        supports_multipart: true,
        max_object_size: 5 * TIB,
        min_part_size: 5 * MIB,
        max_part_size: 5 * GIB,
        max_parts: 10_000,
        requires_checksum_headers: false,
    };

    pub fn effective_max_file_size(&self, configured: Option<ByteSize>) -> Option<ByteSize> {
        let provider = ByteSize::try_from(self.max_object_size).ok();
        match (configured, provider) {
            (Some(configured), Some(provider)) => Some(configured.min(provider)),
            (configured, provider) => configured.or(provider),
        }
    }

    pub const fn upload_data_plane(&self) -> UploadDataPlane {
        if self.supports_multipart {
            UploadDataPlane::Multipart
        } else {
            UploadDataPlane::Tus
        }
    }

    pub const fn part_upload(&self) -> PartUpload {
        if self.requires_checksum_headers {
            PartUpload::ServerProxied
        } else {
            PartUpload::BrowserDirect
        }
    }

    pub const fn download_data_plane(&self) -> DownloadDataPlane {
        if self.supports_presigned_get {
            DownloadDataPlane::PresignedRedirect
        } else {
            DownloadDataPlane::Streamed
        }
    }

    pub const fn copy_strategy(&self) -> CopyStrategy {
        if self.supports_server_side_copy {
            CopyStrategy::ServerSide
        } else {
            CopyStrategy::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadDataPlane {
    Multipart,
    Tus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartUpload {
    BrowserDirect,
    ServerProxied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadDataPlane {
    PresignedRedirect,
    Streamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStrategy {
    ServerSide,
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::StorageCapabilities;
    use crate::domain::bytes::ByteSize;

    #[test]
    fn unit_effective_max_file_size_is_min_of_policy_and_provider() {
        let bytes = |value: u64| ByteSize::try_from(value).unwrap();
        let local = StorageCapabilities::LOCAL;
        assert_eq!(local.effective_max_file_size(None), None);
        assert_eq!(
            local.effective_max_file_size(Some(bytes(10))),
            Some(bytes(10))
        );
        let s3 = StorageCapabilities::S3_DEFAULT;
        assert_eq!(
            s3.effective_max_file_size(None),
            Some(bytes(5_497_558_138_880))
        );
        assert_eq!(s3.effective_max_file_size(Some(bytes(10))), Some(bytes(10)));
        assert_eq!(
            s3.effective_max_file_size(Some(bytes(u64::from(u32::MAX) << 12))),
            Some(bytes(5_497_558_138_880))
        );
    }

    #[test]
    fn unit_storage_capabilities_baseline_profiles() {
        let local = StorageCapabilities::LOCAL;
        assert!(!local.supports_presigned_get);
        assert!(!local.supports_presigned_put);
        assert!(local.supports_server_side_copy);
        assert!(!local.supports_multipart);
        assert_eq!(local.max_object_size, u64::MAX);
        assert_eq!(
            (local.min_part_size, local.max_part_size, local.max_parts),
            (0, 0, 0)
        );
        assert!(!local.requires_checksum_headers);

        let s3 = StorageCapabilities::S3_DEFAULT;
        assert!(s3.supports_presigned_get);
        assert!(s3.supports_presigned_put);
        assert!(s3.supports_server_side_copy);
        assert!(s3.supports_multipart);
        assert_eq!(s3.max_object_size, 5_497_558_138_880);
        assert_eq!(s3.min_part_size, 5_242_880);
        assert_eq!(s3.max_part_size, 5_368_709_120);
        assert_eq!(s3.max_parts, 10_000);
        assert!(!s3.requires_checksum_headers);
    }
}
