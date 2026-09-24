use std::fmt;

use super::model::{LinkError, ParamsError};
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::jobs::{InvalidDedupKey, JobsError, PayloadError};

pub const EMAIL_NOT_CONFIGURED: &str = "EMAIL_NOT_CONFIGURED";
pub const EMAIL_CONFIG_INVALID: &str = "EMAIL_CONFIG_INVALID";
pub const EMAIL_DELIVERY_FAILED: &str = "EMAIL_DELIVERY_FAILED";
pub const EMAIL_REJECTED: &str = "EMAIL_REJECTED";
pub const EMAIL_TEMPLATE_FAILED: &str = "EMAIL_TEMPLATE_FAILED";
pub const EMAIL_TOKEN_UNREADABLE: &str = "EMAIL_TOKEN_UNREADABLE";

#[derive(Debug)]
pub enum EmailError {
    Database(DbError),
    Jobs(JobsError),
    Payload(PayloadError),
    Crypto(CryptoError),
    Params(ParamsError),
    Link(LinkError),
    KindMismatch,
    TokenNotAllowed,
    ParamsTooLarge,
    InvalidBatchKey,
}

impl EmailError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Database(_) => "EMAIL_STORAGE_FAILED",
            Self::Jobs(_) | Self::Payload(_) => "EMAIL_ENQUEUE_FAILED",
            Self::Crypto(_) => "EMAIL_SEAL_FAILED",
            Self::Params(_) | Self::KindMismatch | Self::TokenNotAllowed | Self::ParamsTooLarge => {
                "EMAIL_PARAMS_INVALID"
            }
            Self::Link(_) => "EMAIL_LINK_INVALID",
            Self::InvalidBatchKey => "EMAIL_PARAMS_INVALID",
        }
    }
}

impl fmt::Display for EmailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "e-mail outbox database operation failed: {error}"),
            Self::Jobs(error) => write!(f, "e-mail delivery job could not be enqueued: {error}"),
            Self::Payload(error) => write!(f, "e-mail delivery job payload is invalid: {error}"),
            Self::Crypto(error) => write!(f, "e-mail token could not be sealed: {error}"),
            Self::Params(error) => write!(f, "e-mail parameters are invalid: {error}"),
            Self::Link(error) => write!(f, "e-mail link is invalid: {error}"),
            Self::KindMismatch => f.write_str("e-mail parameters do not match the outbox kind"),
            Self::TokenNotAllowed => {
                f.write_str("this e-mail kind does not carry a sealed one-time token")
            }
            Self::ParamsTooLarge => f.write_str("e-mail parameters exceed the stored limit"),
            Self::InvalidBatchKey => f.write_str("e-mail batch key is invalid"),
        }
    }
}

impl std::error::Error for EmailError {}

impl From<DbError> for EmailError {
    fn from(error: DbError) -> Self {
        Self::Database(error)
    }
}

impl From<sqlx::Error> for EmailError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(DbError::from(error))
    }
}

impl From<JobsError> for EmailError {
    fn from(error: JobsError) -> Self {
        Self::Jobs(error)
    }
}

impl From<CryptoError> for EmailError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<ParamsError> for EmailError {
    fn from(error: ParamsError) -> Self {
        Self::Params(error)
    }
}

impl From<LinkError> for EmailError {
    fn from(error: LinkError) -> Self {
        Self::Link(error)
    }
}

impl From<PayloadError> for EmailError {
    fn from(error: PayloadError) -> Self {
        Self::Payload(error)
    }
}

impl From<InvalidDedupKey> for EmailError {
    fn from(error: InvalidDedupKey) -> Self {
        Self::Jobs(JobsError::from(error))
    }
}
