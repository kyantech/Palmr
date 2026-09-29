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
use crate::infra::http::idempotency::IdempotencyError;

#[derive(Debug)]
pub enum InviteError {
    Invalid { fields: Vec<&'static str> },
    SmtpUnavailable,
    NotFound,
    Expired,
    AlreadyUsed,
    Revoked,
    EmailTaken,
    SessionRefused,
    HashTask,
    RepositoryInvariant { column: &'static str },
    Login(LoginError),
    User(UserError),
    Session(SessionError),
    Email(EmailError),
    Audit(AuditError),
    Crypto(CryptoError),
    Idempotency(IdempotencyError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl InviteError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "invite_invalid",
            Self::SmtpUnavailable => "invite_smtp_unavailable",
            Self::NotFound => "invite_not_found",
            Self::Expired => "invite_expired",
            Self::AlreadyUsed => "invite_already_used",
            Self::Revoked => "invite_revoked",
            Self::EmailTaken => "invite_email_taken",
            Self::SessionRefused => "invite_session_refused",
            Self::HashTask => "invite_hash_task_failed",
            Self::RepositoryInvariant { .. } => "invite_repository_invariant",
            Self::Login(error) => error.kind(),
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Email(error) => error.code(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "invite_crypto",
            Self::Idempotency(error) => error.kind(),
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "invite_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::SmtpUnavailable => ApiError::new(ErrorCode::FeatureUnavailableSmtp),
            Self::NotFound => ApiError::new(ErrorCode::InviteNotFound),
            Self::Expired => ApiError::new(ErrorCode::InviteExpired),
            Self::AlreadyUsed => ApiError::new(ErrorCode::InviteAlreadyUsed),
            Self::Revoked => ApiError::new(ErrorCode::InviteRevoked),
            Self::EmailTaken | Self::User(UserError::EmailTaken) => {
                ApiError::new(ErrorCode::UserEmailTaken)
            }
            Self::User(UserError::UsernameTaken) => ApiError::new(ErrorCode::UserUsernameTaken),
            Self::User(UserError::PasswordPolicyViolation { min_length }) => {
                ApiError::new(ErrorCode::PasswordPolicyViolation)
                    .with_detail("minLength", i64::from(*min_length))
            }
            Self::Login(error) => error.api_error(),
            Self::Idempotency(error) => ApiError::new(error.api_code()),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error))
            | Self::Email(EmailError::Database(error)) => ApiError::new(error.api_code()),
            Self::SessionRefused
            | Self::HashTask
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

impl fmt::Display for InviteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => {
                write!(f, "the invite fields {} are invalid", fields.join(", "))
            }
            Self::SmtpUnavailable => f.write_str("outbound e-mail is not configured"),
            Self::NotFound => f.write_str("the invite does not exist"),
            Self::Expired => f.write_str("the invite has expired"),
            Self::AlreadyUsed => f.write_str("the invite was already accepted"),
            Self::Revoked => f.write_str("the invite was revoked"),
            Self::EmailTaken => {
                f.write_str("the e-mail address already has an account or a pending invite")
            }
            Self::SessionRefused => f.write_str("the invited account could not be signed in"),
            Self::HashTask => f.write_str("the password hashing task did not complete"),
            Self::RepositoryInvariant { column } => {
                write!(f, "an invite row holds an invalid value in {column}")
            }
            Self::Login(error) => write!(f, "invite account operation failed: {error}"),
            Self::User(error) => write!(f, "invite user operation failed: {error}"),
            Self::Session(error) => write!(f, "invite session operation failed: {error}"),
            Self::Email(error) => write!(f, "invite e-mail could not be queued: {error}"),
            Self::Audit(error) => write!(f, "invite audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "invite credential operation failed: {error}"),
            Self::Idempotency(error) => write!(f, "invite replay record failed: {error}"),
            Self::Db(error) => write!(f, "invite database operation failed: {error}"),
            Self::Time(error) => write!(f, "invite timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for InviteError {}

impl From<LoginError> for InviteError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl From<UserError> for InviteError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for InviteError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<EmailError> for InviteError {
    fn from(error: EmailError) -> Self {
        Self::Email(error)
    }
}

impl From<AuditError> for InviteError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for InviteError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<IdempotencyError> for InviteError {
    fn from(error: IdempotencyError) -> Self {
        Self::Idempotency(error)
    }
}

impl From<DbError> for InviteError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for InviteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for InviteError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
