use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::features::auth::error::LoginError;
use crate::features::auth::sessions::SessionError;
use crate::features::email::error::EmailError;
use crate::features::users::error::UserError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum PasswordResetError {
    Invalid { fields: Vec<&'static str> },
    SmtpUnavailable,
    PasswordLoginDisabled,
    TokenInvalid,
    TokenExpired,
    TokenUsed,
    PasswordPolicyViolation { min_length: u32 },
    HashTask,
    RepositoryInvariant { column: &'static str },
    Login(LoginError),
    User(UserError),
    Session(SessionError),
    Email(EmailError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl PasswordResetError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "password_reset_invalid",
            Self::SmtpUnavailable => "password_reset_smtp_unavailable",
            Self::PasswordLoginDisabled => "password_reset_password_login_disabled",
            Self::TokenInvalid => "password_reset_token_invalid",
            Self::TokenExpired => "password_reset_token_expired",
            Self::TokenUsed => "password_reset_token_used",
            Self::PasswordPolicyViolation { .. } => "password_reset_policy_violation",
            Self::HashTask => "password_reset_hash_task_failed",
            Self::RepositoryInvariant { .. } => "password_reset_repository_invariant",
            Self::Login(error) => error.kind(),
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Email(error) => error.code(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "password_reset_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "password_reset_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::SmtpUnavailable => ApiError::new(ErrorCode::FeatureUnavailableSmtp),
            Self::PasswordLoginDisabled => ApiError::new(ErrorCode::AuthPasswordLoginDisabled),
            Self::TokenInvalid => ApiError::new(ErrorCode::ResetTokenInvalid),
            Self::TokenExpired => ApiError::new(ErrorCode::ResetTokenExpired),
            Self::TokenUsed => ApiError::new(ErrorCode::ResetTokenUsed),
            Self::PasswordPolicyViolation { min_length }
            | Self::User(UserError::PasswordPolicyViolation { min_length }) => {
                ApiError::new(ErrorCode::PasswordPolicyViolation)
                    .with_detail("minLength", i64::from(*min_length))
            }
            Self::Login(error) => error.api_error(),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error))
            | Self::Email(EmailError::Database(error)) => ApiError::new(error.api_code()),
            Self::HashTask
            | Self::RepositoryInvariant { .. }
            | Self::User(_)
            | Self::Session(_)
            | Self::Email(_)
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for PasswordResetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => {
                write!(
                    f,
                    "the password reset fields {} are invalid",
                    fields.join(", ")
                )
            }
            Self::SmtpUnavailable => f.write_str("outbound e-mail is not configured"),
            Self::PasswordLoginDisabled => f.write_str("password login is disabled"),
            Self::TokenInvalid => {
                f.write_str("the reset token is malformed, unknown or invalidated")
            }
            Self::TokenExpired => f.write_str("the reset token has expired"),
            Self::TokenUsed => f.write_str("the reset token was already consumed"),
            Self::PasswordPolicyViolation { min_length } => write!(
                f,
                "the password must be at least {min_length} characters long"
            ),
            Self::HashTask => f.write_str("the password hashing task did not complete"),
            Self::RepositoryInvariant { column } => {
                write!(f, "a password reset row holds an invalid value in {column}")
            }
            Self::Login(error) => write!(f, "password reset account operation failed: {error}"),
            Self::User(error) => write!(f, "password reset user operation failed: {error}"),
            Self::Session(error) => write!(f, "password reset session operation failed: {error}"),
            Self::Email(error) => write!(f, "password reset e-mail could not be queued: {error}"),
            Self::Audit(error) => write!(f, "password reset audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "password reset credential operation failed: {error}"),
            Self::Db(error) => write!(f, "password reset database operation failed: {error}"),
            Self::Time(error) => write!(f, "password reset timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for PasswordResetError {}

impl From<LoginError> for PasswordResetError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl From<UserError> for PasswordResetError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for PasswordResetError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<EmailError> for PasswordResetError {
    fn from(error: EmailError) -> Self {
        Self::Email(error)
    }
}

impl From<AuditError> for PasswordResetError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for PasswordResetError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for PasswordResetError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for PasswordResetError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for PasswordResetError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
