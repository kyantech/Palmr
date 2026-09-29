use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum TrustedDeviceError {
    NotFound,
    RepositoryInvariant { column: &'static str },
    Audit(AuditError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl TrustedDeviceError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "trusted_device_not_found",
            Self::RepositoryInvariant { .. } => "trusted_device_repository_invariant",
            Self::Audit(error) => error.kind(),
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "trusted_device_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::TrustedDeviceNotFound),
            Self::Db(error) | Self::Audit(AuditError::Db(error)) => ApiError::new(error.api_code()),
            Self::RepositoryInvariant { .. } | Self::Audit(_) | Self::Time(_) => {
                ApiError::internal()
            }
        }
    }
}

impl fmt::Display for TrustedDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the trusted device does not exist for this account"),
            Self::RepositoryInvariant { column } => {
                write!(
                    f,
                    "a trusted_devices row holds an invalid value in {column}"
                )
            }
            Self::Audit(error) => write!(f, "trusted-device audit record failed: {error}"),
            Self::Db(error) => write!(f, "trusted-device database operation failed: {error}"),
            Self::Time(error) => write!(f, "trusted-device timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for TrustedDeviceError {}

impl From<AuditError> for TrustedDeviceError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<DbError> for TrustedDeviceError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for TrustedDeviceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for TrustedDeviceError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
