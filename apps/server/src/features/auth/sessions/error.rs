use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::cookies::CookieError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum SessionError {
    AuthRequired,
    Forbidden,
    RecentAuthRequired { method: &'static str },
    PasswordChangeRequired,
    TotpEnrollmentRequired,
    CsrfMissing,
    CsrfInvalid,
    NotFound,
    RepositoryInvariant { column: &'static str },
    Audit(AuditError),
    Crypto(CryptoError),
    Cookie(CookieError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl SessionError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AuthRequired => "session_auth_required",
            Self::Forbidden => "session_forbidden",
            Self::RecentAuthRequired { .. } => "session_recent_auth_required",
            Self::PasswordChangeRequired => "session_password_change_required",
            Self::TotpEnrollmentRequired => "session_totp_enrollment_required",
            Self::CsrfMissing => "session_csrf_missing",
            Self::CsrfInvalid => "session_csrf_invalid",
            Self::NotFound => "session_not_found",
            Self::RepositoryInvariant { .. } => "session_repository_invariant",
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "session_crypto",
            Self::Cookie(_) => "session_cookie",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "session_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::AuthRequired => ApiError::new(ErrorCode::AuthRequired),
            Self::Forbidden => ApiError::new(ErrorCode::Forbidden),
            Self::RecentAuthRequired { method } => {
                ApiError::new(ErrorCode::AuthRecentAuthRequired).with_detail("method", *method)
            }
            Self::PasswordChangeRequired => ApiError::new(ErrorCode::AuthPasswordChangeRequired),
            Self::TotpEnrollmentRequired => ApiError::new(ErrorCode::Auth2faEnrollmentRequired),
            Self::CsrfMissing => ApiError::new(ErrorCode::CsrfTokenMissing),
            Self::CsrfInvalid => ApiError::new(ErrorCode::CsrfTokenInvalid),
            Self::NotFound => ApiError::new(ErrorCode::SessionNotFound),
            Self::RepositoryInvariant { .. }
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Cookie(_)
            | Self::Db(_)
            | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthRequired => f.write_str("a valid active session is required"),
            Self::Forbidden => f.write_str("the session does not have the required role"),
            Self::RecentAuthRequired { .. } => {
                f.write_str("the session does not have a recent authentication proof")
            }
            Self::PasswordChangeRequired => {
                f.write_str("the session is restricted until the password is changed")
            }
            Self::TotpEnrollmentRequired => {
                f.write_str("the session is restricted until TOTP is enrolled")
            }
            Self::CsrfMissing => f.write_str("the state change carries no CSRF proof"),
            Self::CsrfInvalid => f.write_str("the CSRF token is not the one bound to the session"),
            Self::NotFound => f.write_str("the session does not exist"),
            Self::RepositoryInvariant { column } => {
                write!(f, "the sessions row holds an invalid value in {column}")
            }
            Self::Audit(error) => write!(f, "session audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "session credential operation failed: {error}"),
            Self::Cookie(error) => write!(f, "session cookie operation failed: {error}"),
            Self::Db(error) => write!(f, "session database operation failed: {error}"),
            Self::Time(error) => write!(f, "session timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<CryptoError> for SessionError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<AuditError> for SessionError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CookieError> for SessionError {
    fn from(error: CookieError) -> Self {
        Self::Cookie(error)
    }
}

impl From<DbError> for SessionError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for SessionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for SessionError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
