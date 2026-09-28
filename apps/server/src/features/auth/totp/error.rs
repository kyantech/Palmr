use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::features::auth::sessions::SessionError;
use crate::features::users::error::UserError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum TotpError {
    Invalid { fields: Vec<&'static str> },
    AlreadyEnabled,
    NotEnrolled,
    RequiredByPolicy,
    EnrollmentPendingMissing,
    CodeInvalid,
    RepositoryInvariant { column: &'static str },
    User(UserError),
    Session(SessionError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl TotpError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "totp_invalid",
            Self::AlreadyEnabled => "totp_already_enabled",
            Self::NotEnrolled => "totp_not_enrolled",
            Self::RequiredByPolicy => "totp_required_by_policy",
            Self::EnrollmentPendingMissing => "totp_enrollment_pending_missing",
            Self::CodeInvalid => "totp_code_invalid",
            Self::RepositoryInvariant { .. } => "totp_repository_invariant",
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "totp_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "totp_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::AlreadyEnabled => ApiError::new(ErrorCode::TotpAlreadyEnabled),
            Self::NotEnrolled => ApiError::new(ErrorCode::TotpNotEnrolled),
            Self::RequiredByPolicy => ApiError::new(ErrorCode::TotpRequiredByPolicy),
            Self::EnrollmentPendingMissing => {
                ApiError::new(ErrorCode::TotpEnrollmentPendingMissing)
            }
            Self::CodeInvalid => ApiError::new(ErrorCode::Auth2faInvalid),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error)) => ApiError::new(error.api_code()),
            Self::Session(error) => error.api_error(),
            Self::RepositoryInvariant { .. }
            | Self::User(_)
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for TotpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => {
                write!(f, "the two-factor fields {} are invalid", fields.join(", "))
            }
            Self::AlreadyEnabled => f.write_str("two-factor authentication is already enabled"),
            Self::NotEnrolled => f.write_str("two-factor authentication is not enabled"),
            Self::RequiredByPolicy => {
                f.write_str("two-factor authentication is required by instance policy")
            }
            Self::EnrollmentPendingMissing => {
                f.write_str("no unexpired pending enrollment matches the request")
            }
            Self::CodeInvalid => f.write_str("the two-factor code did not verify"),
            Self::RepositoryInvariant { column } => {
                write!(f, "a two-factor row holds an invalid value in {column}")
            }
            Self::User(error) => write!(f, "two-factor user operation failed: {error}"),
            Self::Session(error) => write!(f, "two-factor session operation failed: {error}"),
            Self::Audit(error) => write!(f, "two-factor audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "two-factor secret operation failed: {error}"),
            Self::Db(error) => write!(f, "two-factor database operation failed: {error}"),
            Self::Time(error) => write!(f, "two-factor timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for TotpError {}

impl From<UserError> for TotpError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for TotpError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<AuditError> for TotpError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for TotpError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for TotpError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for TotpError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for TotpError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
