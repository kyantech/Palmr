use std::fmt;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::features::auth::sessions::SessionError;
use crate::features::settings::SettingsError;
use crate::features::users::error::UserError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug)]
pub enum SetupError {
    AlreadyCompleted,
    Invalid { fields: Vec<&'static str> },
    User(UserError),
    Session(SessionError),
    Settings(SettingsError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
    HashingTask,
    MalformedSetupFlag,
}

impl SetupError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AlreadyCompleted => "setup_already_completed",
            Self::Invalid { .. } => "setup_invalid",
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Settings(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "setup_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "setup_time_out_of_range",
            Self::HashingTask => "setup_hashing_task_failed",
            Self::MalformedSetupFlag => "setup_malformed_flag",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::AlreadyCompleted => ApiError::new(ErrorCode::SetupAlreadyCompleted),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::User(UserError::EmailTaken) => ApiError::new(ErrorCode::UserEmailTaken),
            Self::User(UserError::UsernameTaken) => ApiError::new(ErrorCode::UserUsernameTaken),
            Self::User(UserError::PasswordPolicyViolation { min_length }) => {
                ApiError::new(ErrorCode::PasswordPolicyViolation)
                    .with_detail("minLength", i64::from(*min_length))
            }
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Settings(SettingsError::Db(error))
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error)) => ApiError::new(error.api_code()),
            Self::User(_)
            | Self::Session(_)
            | Self::Settings(_)
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Time(_)
            | Self::HashingTask
            | Self::MalformedSetupFlag => ApiError::internal(),
        }
    }
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCompleted => f.write_str("setup has already been completed"),
            Self::Invalid { fields } => {
                write!(f, "the setup fields {} are invalid", fields.join(", "))
            }
            Self::User(error) => write!(f, "setup user operation failed: {error}"),
            Self::Session(error) => write!(f, "setup session operation failed: {error}"),
            Self::Settings(error) => write!(f, "setup settings operation failed: {error}"),
            Self::Audit(error) => write!(f, "setup audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "setup credential operation failed: {error}"),
            Self::Db(error) => write!(f, "setup database operation failed: {error}"),
            Self::Time(error) => write!(f, "setup timestamp is out of range: {error}"),
            Self::HashingTask => f.write_str("the password hashing task did not complete"),
            Self::MalformedSetupFlag => {
                f.write_str("the persisted setup_completed flag is not a boolean")
            }
        }
    }
}

impl std::error::Error for SetupError {}

impl From<UserError> for SetupError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for SetupError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<SettingsError> for SetupError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

impl From<AuditError> for SetupError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for SetupError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for SetupError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for SetupError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for SetupError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
