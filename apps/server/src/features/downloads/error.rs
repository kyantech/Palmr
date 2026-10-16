use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;
use crate::storage::error::StorageError;

#[derive(Debug)]
pub enum DownloadError {
    NotFound,
    RangeNotSatisfiable {
        size: u64,
    },
    ObjectMissing,
    SizeMismatch {
        recorded: u64,
        actual: u64,
    },
    ProviderMismatch {
        recorded: String,
        configured: &'static str,
    },
    CorruptRecord {
        column: &'static str,
    },
    Backpressure {
        retry_after: u64,
    },
    Storage(StorageError),
    Db(DbError),
}

impl DownloadError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "download_file_not_found",
            Self::RangeNotSatisfiable { .. } => "download_range_not_satisfiable",
            Self::ObjectMissing => "download_object_missing",
            Self::SizeMismatch { .. } => "download_size_mismatch",
            Self::ProviderMismatch { .. } => "download_provider_mismatch",
            Self::CorruptRecord { .. } => "download_record_invalid",
            Self::Backpressure { .. } => "download_backpressure",
            Self::Storage(_) => "download_storage_failed",
            Self::Db(_) => "download_database_failed",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::FileNotFound),
            Self::RangeNotSatisfiable { .. } => ApiError::new(ErrorCode::RangeNotSatisfiable),
            Self::ObjectMissing => ApiError::new(ErrorCode::StorageObjectMissing),
            Self::SizeMismatch { .. } => ApiError::new(ErrorCode::StorageSizeMismatch),
            Self::ProviderMismatch { .. } => ApiError::new(ErrorCode::StorageProviderMismatch),
            Self::CorruptRecord { .. } => ApiError::internal(),
            Self::Backpressure { .. } => ApiError::new(ErrorCode::TransferBackpressure),
            Self::Storage(error) => ApiError::new(storage_code(error)),
            Self::Db(error) => ApiError::new(error.api_code()),
        }
    }

    pub fn from_storage(error: StorageError) -> Self {
        match error {
            StorageError::NotFound => Self::ObjectMissing,
            StorageError::RangeNotSatisfiable { size } => Self::RangeNotSatisfiable { size },
            other => Self::Storage(other),
        }
    }
}

fn storage_code(error: &StorageError) -> ErrorCode {
    use crate::features::transfers::classify::{classify_storage, StorageSite};
    classify_storage(error, StorageSite::Stream).code
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the file does not exist for this owner"),
            Self::RangeNotSatisfiable { size } => {
                write!(f, "the requested range lies outside {size} bytes")
            }
            Self::ObjectMissing => f.write_str("the storage backend has no such object"),
            Self::SizeMismatch { recorded, actual } => write!(
                f,
                "the stored object holds {actual} bytes but {recorded} were recorded"
            ),
            Self::ProviderMismatch {
                recorded,
                configured,
            } => write!(
                f,
                "the object belongs to provider {recorded} but {configured} is configured"
            ),
            Self::CorruptRecord { column } => {
                write!(f, "the download record holds an invalid value in {column}")
            }
            Self::Backpressure { retry_after } => {
                write!(f, "all download slots are busy; retry in {retry_after}s")
            }
            Self::Storage(error) => write!(f, "storage failed: {error}"),
            Self::Db(error) => write!(f, "download database operation failed: {error}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<DbError> for DownloadError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for DownloadError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}
