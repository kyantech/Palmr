use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::infra::http::error::ApiError;

const SQLITE_BUSY: i32 = 5;
const SQLITE_CONSTRAINT_CHECK: i32 = 275;
const SQLITE_CONSTRAINT_FOREIGNKEY: i32 = 787;
const SQLITE_CONSTRAINT_PRIMARYKEY: i32 = 1555;
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;
const PRIMARY_RESULT_CODE_MASK: i32 = 0xff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbErrorKind {
    Busy,
    UniqueViolation,
    CheckViolation,
    ForeignKeyViolation,
    Other,
}

impl DbErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::UniqueViolation => "unique_violation",
            Self::CheckViolation => "check_violation",
            Self::ForeignKeyViolation => "foreign_key_violation",
            Self::Other => "other",
        }
    }

    fn classify(error: &sqlx::Error) -> Self {
        match error {
            sqlx::Error::PoolTimedOut => Self::Busy,
            sqlx::Error::Database(database) => database
                .code()
                .and_then(|code| code.parse::<i32>().ok())
                .map_or(Self::Other, Self::from_extended_code),
            _ => Self::Other,
        }
    }

    const fn from_extended_code(code: i32) -> Self {
        match code {
            SQLITE_CONSTRAINT_UNIQUE | SQLITE_CONSTRAINT_PRIMARYKEY => Self::UniqueViolation,
            SQLITE_CONSTRAINT_CHECK => Self::CheckViolation,
            SQLITE_CONSTRAINT_FOREIGNKEY => Self::ForeignKeyViolation,
            _ if code & PRIMARY_RESULT_CODE_MASK == SQLITE_BUSY => Self::Busy,
            _ => Self::Other,
        }
    }
}

#[derive(Debug)]
pub enum DbError {
    Busy(sqlx::Error),
    UniqueViolation(sqlx::Error),
    CheckViolation(sqlx::Error),
    ForeignKeyViolation(sqlx::Error),
    Other(sqlx::Error),
}

impl DbError {
    pub const fn kind(&self) -> DbErrorKind {
        match self {
            Self::Busy(_) => DbErrorKind::Busy,
            Self::UniqueViolation(_) => DbErrorKind::UniqueViolation,
            Self::CheckViolation(_) => DbErrorKind::CheckViolation,
            Self::ForeignKeyViolation(_) => DbErrorKind::ForeignKeyViolation,
            Self::Other(_) => DbErrorKind::Other,
        }
    }

    pub const fn source_error(&self) -> &sqlx::Error {
        match self {
            Self::Busy(source)
            | Self::UniqueViolation(source)
            | Self::CheckViolation(source)
            | Self::ForeignKeyViolation(source)
            | Self::Other(source) => source,
        }
    }

    pub const fn api_code(&self) -> ErrorCode {
        match self {
            Self::Busy(_) => ErrorCode::DatabaseBusy,
            Self::UniqueViolation(_)
            | Self::CheckViolation(_)
            | Self::ForeignKeyViolation(_)
            | Self::Other(_) => ErrorCode::InternalError,
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(source: sqlx::Error) -> Self {
        match DbErrorKind::classify(&source) {
            DbErrorKind::Busy => Self::Busy(source),
            DbErrorKind::UniqueViolation => Self::UniqueViolation(source),
            DbErrorKind::CheckViolation => Self::CheckViolation(source),
            DbErrorKind::ForeignKeyViolation => Self::ForeignKeyViolation(source),
            DbErrorKind::Other => Self::Other(source),
        }
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        Self::new(error.api_code())
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "database error ({}): {}",
            self.kind().as_str(),
            self.source_error()
        )
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source_error())
    }
}
