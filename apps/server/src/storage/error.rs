use std::error::Error;
use std::io;

use super::key::InvalidKey;
use super::ProviderKind;
use crate::domain::error_code::ErrorCode;
use crate::infra::http::error::ApiError;

pub type BoxedSource = Box<dyn Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
#[error("transient storage provider failure")]
pub struct Retryable(#[source] BoxedSource);

impl Retryable {
    pub fn new(source: impl Into<BoxedSource>) -> Self {
        Self(source.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage object not found")]
    NotFound,
    #[error("requested range is not satisfiable for an object of {size} bytes")]
    RangeNotSatisfiable { size: u64 },
    #[error("storage object already exists")]
    AlreadyExists,
    #[error("object key does not match the storage key grammar")]
    InvalidKey,
    #[error("storage device is out of space or quota")]
    QuotaOnDevice,
    #[error("storage permission denied")]
    PermissionDenied,
    #[error("storage provider unavailable")]
    ProviderUnavailable(#[source] Retryable),
    #[error("storage configuration is invalid: {0}")]
    Config(String),
    #[error("storage object belongs to provider {found}, configured provider is {expected}")]
    ProviderMismatch {
        expected: ProviderKind,
        found: ProviderKind,
    },
    #[error("storage i/o failure")]
    Io(#[source] io::Error),
    #[error("storage provider request failed")]
    S3(#[source] BoxedSource),
}

impl StorageError {
    pub const fn api_code(&self) -> ErrorCode {
        match self {
            Self::NotFound => ErrorCode::FileNotFound,
            Self::RangeNotSatisfiable { .. } => ErrorCode::RangeNotSatisfiable,
            Self::AlreadyExists | Self::InvalidKey | Self::Config(_) => ErrorCode::InternalError,
            Self::QuotaOnDevice => ErrorCode::StorageFull,
            Self::PermissionDenied | Self::ProviderUnavailable(_) | Self::Io(_) | Self::S3(_) => {
                ErrorCode::StorageUnavailable
            }
            Self::ProviderMismatch { .. } => ErrorCode::StorageProviderMismatch,
        }
    }

    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::QuotaOnDevice
            | Self::PermissionDenied
            | Self::ProviderUnavailable(_)
            | Self::Io(_)
            | Self::S3(_) => true,
            Self::NotFound
            | Self::RangeNotSatisfiable { .. }
            | Self::AlreadyExists
            | Self::InvalidKey
            | Self::Config(_)
            | Self::ProviderMismatch { .. } => false,
        }
    }

    const fn is_defect(&self) -> bool {
        matches!(
            self,
            Self::AlreadyExists | Self::InvalidKey | Self::Config(_)
        )
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::RangeNotSatisfiable { .. } => "range_not_satisfiable",
            Self::AlreadyExists => "already_exists",
            Self::InvalidKey => "invalid_key",
            Self::QuotaOnDevice => "quota_on_device",
            Self::PermissionDenied => "permission_denied",
            Self::ProviderUnavailable(_) => "provider_unavailable",
            Self::Config(_) => "config",
            Self::ProviderMismatch { .. } => "provider_mismatch",
            Self::Io(_) => "io",
            Self::S3(_) => "s3",
        }
    }
}

pub fn not_found_is_deleted(outcome: Result<(), StorageError>) -> Result<(), StorageError> {
    match outcome {
        Err(StorageError::NotFound) => Ok(()),
        other => other,
    }
}

impl From<InvalidKey> for StorageError {
    fn from(_: InvalidKey) -> Self {
        Self::InvalidKey
    }
}

impl From<io::Error> for StorageError {
    fn from(source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::QuotaOnDevice,
            _ => Self::Io(source),
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        if error.is_defect() {
            tracing::error!(
                storage_error = error.kind(),
                "a storage defect reached a request boundary"
            );
        }
        Self::new(error.api_code())
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use rustix::io::Errno;
    use serde_json::Value;

    use super::{not_found_is_deleted, Retryable, StorageError};
    use crate::domain::error_code::ErrorCode;
    use crate::infra::http::error::{ApiError, ApiErrorBody};
    use crate::storage::key::ObjectKey;
    use crate::storage::ProviderKind;

    const SENTINEL: &str = "palmr-storage-leak-sentinel";

    fn every_variant() -> Vec<StorageError> {
        vec![
            StorageError::NotFound,
            StorageError::RangeNotSatisfiable { size: 1_024 },
            StorageError::AlreadyExists,
            StorageError::InvalidKey,
            StorageError::QuotaOnDevice,
            StorageError::PermissionDenied,
            StorageError::ProviderUnavailable(Retryable::new(io::Error::other(SENTINEL))),
            StorageError::Config(format!("PALMR_S3_SECRET_ACCESS_KEY={SENTINEL}")),
            StorageError::ProviderMismatch {
                expected: ProviderKind::Local,
                found: ProviderKind::S3,
            },
            StorageError::Io(io::Error::other(format!("/data/storage/{SENTINEL}"))),
            StorageError::S3(Box::new(io::Error::other(SENTINEL))),
        ]
    }

    fn expected(error: &StorageError) -> (&'static str, u16, bool) {
        match error {
            StorageError::NotFound => ("FILE_NOT_FOUND", 404, false),
            StorageError::RangeNotSatisfiable { .. } => ("RANGE_NOT_SATISFIABLE", 416, false),
            StorageError::AlreadyExists | StorageError::InvalidKey | StorageError::Config(_) => {
                ("INTERNAL_ERROR", 500, false)
            }
            StorageError::QuotaOnDevice => ("STORAGE_FULL", 507, true),
            StorageError::PermissionDenied
            | StorageError::ProviderUnavailable(_)
            | StorageError::Io(_)
            | StorageError::S3(_) => ("STORAGE_UNAVAILABLE", 503, true),
            StorageError::ProviderMismatch { .. } => ("STORAGE_PROVIDER_MISMATCH", 500, false),
        }
    }

    #[test]
    fn unit_storage_error_mapping() {
        for error in every_variant() {
            let (code, status, retryable) = expected(&error);
            assert_eq!(error.api_code().as_str(), code, "{error:?}");
            assert_eq!(error.is_retryable(), retryable, "{error:?}");

            let api = ApiError::from(error);
            assert_eq!(api.code().as_str(), code);
            assert_eq!(api.status().as_u16(), status);
            assert!(api.details().is_empty());
            let body: Value = serde_json::to_value(ApiErrorBody::from(api)).unwrap();
            assert!(!body.to_string().contains(SENTINEL), "{body}");
            assert!(!body.to_string().contains("PALMR_"), "{body}");
        }

        let invalid = ObjectKey::parse("objects/../../etc/shadow").unwrap_err();
        let invalid = StorageError::from(invalid);
        assert!(matches!(invalid, StorageError::InvalidKey));
        assert_eq!(invalid.api_code(), ErrorCode::InternalError);
        assert_ne!(invalid.api_code(), ErrorCode::ValidationError);
        assert!(!invalid.api_code().status().is_client_error());
        assert!(!invalid.is_retryable());

        let mismatch = StorageError::ProviderMismatch {
            expected: ProviderKind::S3,
            found: ProviderKind::Local,
        };
        assert_ne!(mismatch.api_code(), ErrorCode::FileNotFound);
        assert_ne!(mismatch.api_code(), ErrorCode::NotFound);
        assert_ne!(mismatch.api_code().status().as_u16(), 404);
        assert!(mismatch.to_string().contains("local") && mismatch.to_string().contains("s3"));

        let full = StorageError::QuotaOnDevice.api_code();
        assert_eq!(full.as_str(), "STORAGE_FULL");
        assert_ne!(full.as_str(), "QUOTA_EXCEEDED");
        assert!(full.retryable());

        assert!(not_found_is_deleted(Err(StorageError::NotFound)).is_ok());
        assert!(not_found_is_deleted(Ok(())).is_ok());
        assert!(matches!(
            not_found_is_deleted(Err(StorageError::PermissionDenied)),
            Err(StorageError::PermissionDenied)
        ));
        assert!(matches!(
            not_found_is_deleted(Err(StorageError::InvalidKey)),
            Err(StorageError::InvalidKey)
        ));

        let classified = [
            (io::ErrorKind::NotFound, "FILE_NOT_FOUND"),
            (io::ErrorKind::AlreadyExists, "INTERNAL_ERROR"),
            (io::ErrorKind::PermissionDenied, "STORAGE_UNAVAILABLE"),
            (io::ErrorKind::StorageFull, "STORAGE_FULL"),
            (io::ErrorKind::QuotaExceeded, "STORAGE_FULL"),
            (io::ErrorKind::TimedOut, "STORAGE_UNAVAILABLE"),
        ];
        for (kind, code) in classified {
            let error = StorageError::from(io::Error::from(kind));
            assert_eq!(error.api_code().as_str(), code, "{kind:?}");
        }
        let errnos = [
            (Errno::NOSPC, "STORAGE_FULL"),
            (Errno::DQUOT, "STORAGE_FULL"),
            (Errno::ACCESS, "STORAGE_UNAVAILABLE"),
            (Errno::PERM, "STORAGE_UNAVAILABLE"),
            (Errno::NOENT, "FILE_NOT_FOUND"),
            (Errno::IO, "STORAGE_UNAVAILABLE"),
        ];
        for (errno, code) in errnos {
            let error = StorageError::from(io::Error::from_raw_os_error(errno.raw_os_error()));
            assert_eq!(error.api_code().as_str(), code, "{errno:?}");
        }
    }
}
