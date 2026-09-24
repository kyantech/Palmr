use std::fmt;

use crate::domain::time::InvalidTimestamp;
use crate::infra::db::DbError;
use crate::infra::jobs::JobsError;

use super::model::{AuditAction, WritePath};

#[derive(Debug)]
pub enum AuditError {
    Db(DbError),
    Time(InvalidTimestamp),
    Jobs(JobsError),
    WrongWritePath {
        action: AuditAction,
        expected: WritePath,
    },
    InvalidSchedule,
}

impl AuditError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "audit_time_out_of_range",
            Self::Jobs(_) => "audit_job_enqueue_failed",
            Self::WrongWritePath { .. } => "audit_wrong_write_path",
            Self::InvalidSchedule => "audit_invalid_schedule",
        }
    }
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "audit database operation failed: {error}"),
            Self::Time(error) => write!(f, "audit timestamp is out of range: {error}"),
            Self::Jobs(error) => write!(f, "audit job enqueue failed: {error}"),
            Self::WrongWritePath { action, expected } => write!(
                f,
                "audit action {} is written through the {} path",
                action.as_str(),
                expected.as_str()
            ),
            Self::InvalidSchedule => f.write_str("the audit retention period is invalid"),
        }
    }
}

impl std::error::Error for AuditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::Time(error) => Some(error),
            Self::Jobs(error) => Some(error),
            Self::WrongWritePath { .. } | Self::InvalidSchedule => None,
        }
    }
}

impl From<DbError> for AuditError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for AuditError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for AuditError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<JobsError> for AuditError {
    fn from(error: JobsError) -> Self {
        Self::Jobs(error)
    }
}
