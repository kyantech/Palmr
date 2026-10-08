use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::naming::{CandidateError, InvalidName};
use crate::domain::time::InvalidTimestamp;
use crate::features::folders::FolderError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

use super::model::MAX_BATCH_IDS;

#[derive(Debug)]
pub enum FileError {
    NotFound,
    Folder(FolderError),
    InvalidName(InvalidName),
    NameConflict(CandidateError),
    BatchTooLarge,
    Invalid { fields: Vec<&'static str> },
    RepositoryInvariant { column: &'static str },
    Db(DbError),
    Time(InvalidTimestamp),
}

impl FileError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "file_not_found",
            Self::Folder(_) => "file_folder_failed",
            Self::InvalidName(_) => "file_name_invalid",
            Self::NameConflict(_) => "file_name_conflict",
            Self::BatchTooLarge => "file_batch_too_large",
            Self::Invalid { .. } => "file_request_invalid",
            Self::RepositoryInvariant { .. } => "file_repository_invariant",
            Self::Db(_) => "file_database_failed",
            Self::Time(_) => "file_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::FileNotFound),
            Self::Folder(error) => error.api_error(),
            Self::InvalidName(_) => ApiError::new(ErrorCode::NameInvalid),
            Self::NameConflict(_) => ApiError::new(ErrorCode::FileNameConflict),
            Self::BatchTooLarge => ApiError::new(ErrorCode::BatchTooLarge)
                .with_detail("maxItems", i64::try_from(MAX_BATCH_IDS).unwrap_or(i64::MAX)),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::Db(error) => ApiError::new(error.api_code()),
            Self::RepositoryInvariant { .. } | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the file does not exist for this owner"),
            Self::Folder(error) => write!(f, "{error}"),
            Self::InvalidName(error) => write!(f, "file name is invalid: {error}"),
            Self::NameConflict(error) => write!(f, "file name could not be allocated: {error}"),
            Self::BatchTooLarge => f.write_str("the batch contains too many ids"),
            Self::Invalid { fields } => write!(f, "file request is invalid: {fields:?}"),
            Self::RepositoryInvariant { column } => {
                write!(f, "the files row holds an invalid value in {column}")
            }
            Self::Db(error) => write!(f, "file database operation failed: {error}"),
            Self::Time(error) => write!(f, "file timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<DbError> for FileError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for FileError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for FileError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<FolderError> for FileError {
    fn from(error: FolderError) -> Self {
        Self::Folder(error)
    }
}

impl From<FileError> for ApiError {
    fn from(error: FileError) -> Self {
        error.api_error()
    }
}
