use std::fmt;

use crate::domain::time::InvalidTimestamp;
use crate::infra::db::DbError;

#[derive(Debug)]
pub enum UserError {
    NotFound,
    EmailTaken,
    UsernameTaken,
    PasswordPolicyViolation { min_length: u32 },
    LastAdminProtected,
    RepositoryInvariant { column: &'static str },
    Db(DbError),
    Time(InvalidTimestamp),
}

impl UserError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "user_not_found",
            Self::EmailTaken => "user_email_taken",
            Self::UsernameTaken => "user_username_taken",
            Self::PasswordPolicyViolation { .. } => "user_password_policy_violation",
            Self::LastAdminProtected => "user_last_admin_protected",
            Self::RepositoryInvariant { .. } => "user_repository_invariant",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "user_time_out_of_range",
        }
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the user does not exist"),
            Self::EmailTaken => f.write_str("the e-mail address is already in use"),
            Self::UsernameTaken => f.write_str("the username is already in use"),
            Self::PasswordPolicyViolation { min_length } => write!(
                f,
                "the password must be at least {min_length} characters long"
            ),
            Self::LastAdminProtected => {
                f.write_str("the action would leave the instance without an active administrator")
            }
            Self::RepositoryInvariant { column } => {
                write!(f, "the users row holds an invalid value in {column}")
            }
            Self::Db(error) => write!(f, "user database operation failed: {error}"),
            Self::Time(error) => write!(f, "user timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for UserError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::Time(error) => Some(error),
            Self::NotFound
            | Self::EmailTaken
            | Self::UsernameTaken
            | Self::PasswordPolicyViolation { .. }
            | Self::LastAdminProtected
            | Self::RepositoryInvariant { .. } => None,
        }
    }
}

impl From<DbError> for UserError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for UserError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for UserError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}
