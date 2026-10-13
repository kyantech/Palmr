use std::fmt;

use crate::domain::bytes::ByteSize;
use crate::domain::error_code::ErrorCode;
use crate::features::quota::error::QuotaError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;
use crate::storage::error::StorageError;

use super::super::error::TransferError;
use super::headers::PROTOCOL_VERSION;

#[derive(Debug)]
pub enum TusError {
    VersionUnsupported,
    ExtensionUnsupported,
    Header {
        key: &'static str,
    },
    LengthBeyondRange,
    TooLarge {
        max: Option<ByteSize>,
    },
    LengthMismatch {
        requested: Option<ByteSize>,
        planned: Option<ByteSize>,
    },
    Overrun {
        declared: ByteSize,
    },
    IdleTimeout,
    OffsetMismatch {
        expected: u64,
    },
    Locked,
    ChecksumMismatch {
        offset: u64,
    },
    Backpressure {
        retry_after: u64,
    },
    UnsupportedMedia,
    ShuttingDown,
    NotAvailable,
    NotFound,
    Gone,
    MethodNotAllowed,
    Storage(StorageError),
    WriteFailed,
    Transfer(TransferError),
}

impl TusError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::VersionUnsupported => "tus_version_unsupported",
            Self::ExtensionUnsupported => "tus_extension_unsupported",
            Self::Header { .. } => "tus_header_invalid",
            Self::LengthBeyondRange => "tus_length_beyond_range",
            Self::TooLarge { .. } => "tus_file_too_large",
            Self::LengthMismatch { .. } => "tus_length_mismatch",
            Self::Overrun { .. } => "tus_body_exceeds_length",
            Self::IdleTimeout => "tus_idle_timeout",
            Self::OffsetMismatch { .. } => "tus_offset_mismatch",
            Self::Locked => "tus_upload_locked",
            Self::ChecksumMismatch { .. } => "tus_checksum_mismatch",
            Self::Backpressure { .. } => "tus_backpressure",
            Self::UnsupportedMedia => "tus_unsupported_media",
            Self::ShuttingDown => "tus_shutting_down",
            Self::NotAvailable => "tus_not_available",
            Self::NotFound => "tus_upload_not_found",
            Self::Gone => "tus_upload_gone",
            Self::MethodNotAllowed => "tus_method_not_allowed",
            Self::Storage(error) => error.kind(),
            Self::WriteFailed => "tus_write_failed",
            Self::Transfer(error) => error.kind(),
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::VersionUnsupported => ApiError::new(ErrorCode::TusVersionUnsupported)
                .with_detail("supportedVersion", PROTOCOL_VERSION),
            Self::ExtensionUnsupported => ApiError::new(ErrorCode::TusExtensionUnsupported),
            Self::Header { key } => {
                ApiError::new(ErrorCode::UploadMetadataInvalid).with_detail("key", *key)
            }
            Self::LengthBeyondRange => {
                ApiError::new(ErrorCode::FileTooLarge).with_detail("reason", "object_size")
            }
            Self::TooLarge { max } => {
                let error =
                    ApiError::new(ErrorCode::FileTooLarge).with_detail("reason", "max_file_size");
                match max {
                    Some(max) => error.with_detail("maxBytes", max.to_i64()),
                    None => error,
                }
            }
            Self::LengthMismatch { requested, planned } => {
                let mut error = ApiError::new(ErrorCode::UploadLengthMismatch);
                if let Some(requested) = requested {
                    error = error.with_detail("declaredBytes", requested.to_i64());
                }
                if let Some(planned) = planned {
                    error = error.with_detail("plannedBytes", planned.to_i64());
                }
                error
            }
            Self::Overrun { declared } => ApiError::new(ErrorCode::FileTooLarge)
                .with_detail("reason", "upload_length")
                .with_detail("declaredBytes", declared.to_i64()),
            Self::IdleTimeout => ApiError::new(ErrorCode::TransferIdleTimeout),
            Self::OffsetMismatch { expected } => ApiError::new(ErrorCode::UploadOffsetMismatch)
                .with_detail(
                    "expectedOffset",
                    i64::try_from(*expected).unwrap_or(i64::MAX),
                ),
            Self::Locked => ApiError::new(ErrorCode::UploadLocked),
            Self::ChecksumMismatch { offset } => ApiError::new(ErrorCode::UploadChecksumMismatch)
                .with_detail("algorithm", "sha256")
                .with_detail("offset", i64::try_from(*offset).unwrap_or(i64::MAX)),
            Self::Backpressure { .. } => ApiError::new(ErrorCode::TransferBackpressure),
            Self::UnsupportedMedia => ApiError::new(ErrorCode::UnsupportedMediaType),
            Self::ShuttingDown => ApiError::new(ErrorCode::ServiceUnavailable),
            Self::NotAvailable | Self::NotFound => ApiError::new(ErrorCode::NotFound),
            Self::Gone => ApiError::new(ErrorCode::UploadSessionExpired),
            Self::MethodNotAllowed => ApiError::new(ErrorCode::MethodNotAllowed),
            Self::Storage(error) => ApiError::new(error.api_code()),
            Self::WriteFailed => ApiError::new(ErrorCode::StorageWriteFailed),
            Self::Transfer(error) => error.api_error(),
        }
    }
}

impl fmt::Display for TusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionUnsupported => f.write_str("the TUS protocol version is not supported"),
            Self::ExtensionUnsupported => f.write_str("the TUS extension is not supported"),
            Self::Header { key } => write!(f, "the TUS header {key} is missing or malformed"),
            Self::LengthBeyondRange => f.write_str("the declared upload length is out of range"),
            Self::TooLarge { .. } => f.write_str("the upload exceeds the maximum file size"),
            Self::LengthMismatch { .. } => {
                f.write_str("the upload length does not match the planned file")
            }
            Self::Overrun { .. } => f.write_str("the upload body exceeds the declared length"),
            Self::IdleTimeout => f.write_str("the upload body stalled"),
            Self::OffsetMismatch { .. } => {
                f.write_str("the upload offset does not match the server state")
            }
            Self::Locked => f.write_str("another request is writing to the upload"),
            Self::ChecksumMismatch { .. } => {
                f.write_str("the uploaded chunk failed checksum verification")
            }
            Self::Backpressure { .. } => {
                f.write_str("the server is handling the maximum number of concurrent uploads")
            }
            Self::UnsupportedMedia => {
                f.write_str("the request content type is not application/offset+octet-stream")
            }
            Self::ShuttingDown => f.write_str("the server is shutting down"),
            Self::NotAvailable => f.write_str("local resumable uploads are not available"),
            Self::NotFound => f.write_str("the upload does not exist"),
            Self::Gone => f.write_str("the upload expired or was terminated"),
            Self::MethodNotAllowed => f.write_str("the method is not allowed for this upload"),
            Self::Storage(error) => write!(f, "upload staging failed: {error}"),
            Self::WriteFailed => f.write_str("a write to upload staging failed"),
            Self::Transfer(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TusError {}

impl From<TransferError> for TusError {
    fn from(error: TransferError) -> Self {
        Self::Transfer(error)
    }
}

impl From<DbError> for TusError {
    fn from(error: DbError) -> Self {
        Self::Transfer(TransferError::Db(error))
    }
}

impl From<sqlx::Error> for TusError {
    fn from(error: sqlx::Error) -> Self {
        Self::Transfer(TransferError::from(error))
    }
}

impl From<QuotaError> for TusError {
    fn from(error: QuotaError) -> Self {
        Self::Transfer(TransferError::from(error))
    }
}

impl From<crate::features::folders::FolderError> for TusError {
    fn from(error: crate::features::folders::FolderError) -> Self {
        Self::Transfer(TransferError::Folder(error))
    }
}

impl From<StorageError> for TusError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<TusError> for ApiError {
    fn from(error: TusError) -> Self {
        error.api_error()
    }
}
