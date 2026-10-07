use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::naming::{CandidateError, InvalidName};
use crate::domain::time::InvalidTimestamp;
use crate::features::files::naming_insert::NamedInsertError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum FolderError {
    NotFound,
    DepthExceeded,
    InvalidName(InvalidName),
    NameConflict(CandidateError),
    Invalid { fields: Vec<&'static str> },
    RepositoryInvariant { column: &'static str },
    Db(DbError),
    Time(InvalidTimestamp),
}

impl FolderError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "folder_not_found",
            Self::DepthExceeded => "folder_depth_exceeded",
            Self::InvalidName(_) => "folder_name_invalid",
            Self::NameConflict(_) => "folder_name_conflict",
            Self::Invalid { .. } => "folder_request_invalid",
            Self::RepositoryInvariant { .. } => "folder_repository_invariant",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "folder_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::FolderNotFound),
            Self::DepthExceeded => ApiError::new(ErrorCode::FolderDepthExceeded),
            Self::InvalidName(_) => ApiError::new(ErrorCode::NameInvalid),
            Self::NameConflict(_) => ApiError::new(ErrorCode::FileNameConflict),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::Db(error) => ApiError::new(error.api_code()),
            Self::RepositoryInvariant { .. } | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for FolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the folder does not exist for this owner"),
            Self::DepthExceeded => f.write_str("the folder would exceed the maximum depth"),
            Self::InvalidName(error) => write!(f, "folder name is invalid: {error}"),
            Self::NameConflict(error) => write!(f, "folder name could not be allocated: {error}"),
            Self::Invalid { fields } => write!(f, "folder request is invalid: {fields:?}"),
            Self::RepositoryInvariant { column } => {
                write!(f, "the folders row holds an invalid value in {column}")
            }
            Self::Db(error) => write!(f, "folder database operation failed: {error}"),
            Self::Time(error) => write!(f, "folder timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for FolderError {}

impl From<DbError> for FolderError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for FolderError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for FolderError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<NamedInsertError<Self>> for FolderError {
    fn from(error: NamedInsertError<Self>) -> Self {
        match error {
            NamedInsertError::InvalidName(invalid) => Self::InvalidName(invalid),
            NamedInsertError::Conflict(conflict) => Self::NameConflict(conflict),
            NamedInsertError::Failed(source) => source,
        }
    }
}

impl From<FolderError> for ApiError {
    fn from(error: FolderError) -> Self {
        error.api_error()
    }
}
