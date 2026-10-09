use std::fmt;

use crate::domain::bytes::ByteSize;
use crate::domain::client_key::ClientFileKey;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::folders::FolderError;
use crate::features::quota::error::QuotaError;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;
use crate::infra::http::idempotency::IdempotencyError;
use crate::storage::lifecycle::LifecycleError;

use super::model::MAX_FILES_PER_SESSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooLargeReason {
    MaxFileSize,
    ObjectSize,
    PartCount,
    ProxyPartCount,
}

impl TooLargeReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxFileSize => "max_file_size",
            Self::ObjectSize => "object_size",
            Self::PartCount => "part_count",
            Self::ProxyPartCount => "proxy_part_count",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTooLarge {
    pub client_key: ClientFileKey,
    pub declared: ByteSize,
    pub max: Option<ByteSize>,
    pub provider_max: Option<ByteSize>,
    pub reason: TooLargeReason,
}

#[derive(Debug)]
pub enum TransferError {
    Invalid { fields: Vec<&'static str> },
    BatchTooLarge,
    NameInvalid,
    FileTooLarge(FileTooLarge),
    Folder(FolderError),
    StorageUnavailable,
    Quota(QuotaError),
    AccountInactive,
    SessionNotFound,
    StateInvalid,
    Expired,
    Lifecycle(LifecycleError),
    Idempotency(IdempotencyError),
    Invariant { what: &'static str },
    Db(DbError),
    Time(InvalidTimestamp),
}

impl TransferError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "transfer_request_invalid",
            Self::BatchTooLarge => "transfer_batch_too_large",
            Self::NameInvalid => "transfer_name_invalid",
            Self::FileTooLarge(_) => "transfer_file_too_large",
            Self::Folder(error) => error.kind(),
            Self::StorageUnavailable => "transfer_storage_unavailable",
            Self::Quota(error) => error.kind(),
            Self::AccountInactive => "transfer_account_inactive",
            Self::SessionNotFound => "transfer_session_not_found",
            Self::StateInvalid => "transfer_session_state_invalid",
            Self::Expired => "transfer_session_expired",
            Self::Lifecycle(_) => "transfer_lifecycle_failed",
            Self::Idempotency(error) => error.kind(),
            Self::Invariant { .. } => "transfer_repository_invariant",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "transfer_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::BatchTooLarge => ApiError::new(ErrorCode::BatchTooLarge).with_detail(
                "maxItems",
                i64::try_from(MAX_FILES_PER_SESSION).unwrap_or(i64::MAX),
            ),
            Self::NameInvalid => ApiError::new(ErrorCode::NameInvalid),
            Self::FileTooLarge(large) => {
                let mut error = ApiError::new(ErrorCode::FileTooLarge)
                    .with_detail("itemClientId", &large.client_key)
                    .with_detail("declaredBytes", large.declared.to_i64())
                    .with_detail("reason", large.reason.as_str());
                if let Some(max) = large.max {
                    error = error.with_detail("maxBytes", max.to_i64());
                }
                if let Some(provider) = large.provider_max {
                    error = error.with_detail("providerMaxObjectBytes", provider.to_i64());
                }
                error
            }
            Self::Folder(error) => error.api_error(),
            Self::StorageUnavailable => ApiError::new(ErrorCode::StorageUnavailable),
            Self::Quota(error) => error.api_error(),
            Self::AccountInactive => ApiError::new(ErrorCode::AuthAccountInactive),
            Self::SessionNotFound => ApiError::new(ErrorCode::TransferSessionNotFound),
            Self::StateInvalid => ApiError::new(ErrorCode::TransferSessionStateInvalid),
            Self::Expired => ApiError::new(ErrorCode::TransferSessionExpired),
            Self::Db(error) => ApiError::new(error.api_code()),
            Self::Idempotency(error) => ApiError::new(error.api_code()),
            Self::Lifecycle(_) | Self::Invariant { .. } | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => write!(f, "transfer request is invalid: {fields:?}"),
            Self::BatchTooLarge => write!(
                f,
                "a transfer session holds at most {MAX_FILES_PER_SESSION} files"
            ),
            Self::NameInvalid => f.write_str("a transfer item name is not valid"),
            Self::FileTooLarge(large) => write!(
                f,
                "a transfer item exceeds the maximum size ({})",
                large.reason.as_str()
            ),
            Self::Folder(error) => write!(f, "transfer target folder failed: {error}"),
            Self::StorageUnavailable => f.write_str("the storage backend is unavailable"),
            Self::Quota(error) => write!(f, "transfer quota admission failed: {error}"),
            Self::AccountInactive => f.write_str("the account is not active"),
            Self::SessionNotFound => f.write_str("the transfer session does not exist"),
            Self::StateInvalid => {
                f.write_str("the transfer session cannot take this transition from its state")
            }
            Self::Expired => f.write_str("the transfer session has expired"),
            Self::Lifecycle(error) => write!(f, "transfer cleanup intent failed: {error}"),
            Self::Idempotency(error) => write!(f, "transfer replay record failed: {error}"),
            Self::Invariant { what } => {
                write!(f, "a transfer row holds an invalid value in {what}")
            }
            Self::Db(error) => write!(f, "transfer database operation failed: {error}"),
            Self::Time(error) => write!(f, "transfer timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<DbError> for TransferError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for TransferError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for TransferError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<FolderError> for TransferError {
    fn from(error: FolderError) -> Self {
        Self::Folder(error)
    }
}

impl From<QuotaError> for TransferError {
    fn from(error: QuotaError) -> Self {
        match error {
            QuotaError::OwnerInactive => Self::AccountInactive,
            other => Self::Quota(other),
        }
    }
}

impl From<LifecycleError> for TransferError {
    fn from(error: LifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<IdempotencyError> for TransferError {
    fn from(error: IdempotencyError) -> Self {
        Self::Idempotency(error)
    }
}

impl From<TransferError> for ApiError {
    fn from(error: TransferError) -> Self {
        error.api_error()
    }
}
