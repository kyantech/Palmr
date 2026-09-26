use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::features::auth::sessions::SessionError;
use crate::features::users::error::UserError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;
use crate::infra::ratelimit::RetryAfter;

#[derive(Debug)]
pub enum LoginError {
    Invalid { fields: Vec<&'static str> },
    InvalidCredentials,
    Locked { retry_after: RetryAfter },
    PasswordLoginDisabled,
    SecondFactorUnavailable,
    ExternalReauthUnavailable,
    RepositoryInvariant { column: &'static str },
    VerificationTask,
    User(UserError),
    Session(SessionError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl LoginError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "login_invalid",
            Self::InvalidCredentials => "login_invalid_credentials",
            Self::Locked { .. } => "login_locked",
            Self::PasswordLoginDisabled => "login_password_disabled",
            Self::SecondFactorUnavailable => "login_second_factor_unavailable",
            Self::ExternalReauthUnavailable => "reauth_external_unavailable",
            Self::RepositoryInvariant { .. } => "login_repository_invariant",
            Self::VerificationTask => "login_verification_task_failed",
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "login_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "login_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::InvalidCredentials => ApiError::new(ErrorCode::AuthInvalidCredentials),
            Self::Locked { .. } => ApiError::new(ErrorCode::AuthLocked),
            Self::PasswordLoginDisabled => ApiError::new(ErrorCode::AuthPasswordLoginDisabled),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error)) => ApiError::new(error.api_code()),
            Self::Session(error) => error.api_error(),
            Self::SecondFactorUnavailable
            | Self::ExternalReauthUnavailable
            | Self::RepositoryInvariant { .. }
            | Self::VerificationTask
            | Self::User(_)
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Time(_) => ApiError::internal(),
        }
    }

    pub const fn retry_after(&self) -> Option<RetryAfter> {
        match self {
            Self::Locked { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => {
                write!(f, "the login fields {} are invalid", fields.join(", "))
            }
            Self::InvalidCredentials => f.write_str("the credentials did not authenticate"),
            Self::Locked { .. } => f.write_str("the account is temporarily locked"),
            Self::PasswordLoginDisabled => f.write_str("password login is disabled"),
            Self::SecondFactorUnavailable => f.write_str(
                "the account requires a second factor and the second login step is not available",
            ),
            Self::ExternalReauthUnavailable => f.write_str(
                "the account has no local password and external re-authentication is not available",
            ),
            Self::RepositoryInvariant { column } => {
                write!(
                    f,
                    "an authentication row holds an invalid value in {column}"
                )
            }
            Self::VerificationTask => {
                f.write_str("the password verification task did not complete")
            }
            Self::User(error) => write!(f, "login user operation failed: {error}"),
            Self::Session(error) => write!(f, "login session operation failed: {error}"),
            Self::Audit(error) => write!(f, "login audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "login credential operation failed: {error}"),
            Self::Db(error) => write!(f, "login database operation failed: {error}"),
            Self::Time(error) => write!(f, "login timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for LoginError {}

impl From<UserError> for LoginError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for LoginError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<AuditError> for LoginError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for LoginError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for LoginError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for LoginError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for LoginError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
