use std::fmt;

use crate::domain::bytes::ByteSize;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

use super::model::ReservationState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exceeded {
    pub used: ByteSize,
    pub held: ByteSize,
    pub requested: ByteSize,
    pub quota: Option<ByteSize>,
}

#[derive(Debug)]
pub enum QuotaError {
    Exceeded(Exceeded),
    OwnerNotFound,
    OwnerInactive,
    SessionNotFound,
    OwnerContextMismatch,
    InvalidExpiry,
    ReservationNotFound,
    ReservationMismatch,
    SessionAlreadySettled,
    StateConflict { current: ReservationState },
    Overflow { operation: &'static str },
    Underflow { operation: &'static str },
    Integrity { column: &'static str },
    Db(DbError),
    Time(InvalidTimestamp),
}

impl QuotaError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Exceeded(_) => "quota_exceeded",
            Self::OwnerNotFound => "quota_owner_not_found",
            Self::OwnerInactive => "quota_owner_inactive",
            Self::SessionNotFound => "quota_session_not_found",
            Self::OwnerContextMismatch => "quota_owner_context_mismatch",
            Self::InvalidExpiry => "quota_invalid_expiry",
            Self::ReservationNotFound => "quota_reservation_not_found",
            Self::ReservationMismatch => "quota_reservation_mismatch",
            Self::SessionAlreadySettled => "quota_session_already_settled",
            Self::StateConflict { .. } => "quota_state_conflict",
            Self::Overflow { .. } => "quota_arithmetic_overflow",
            Self::Underflow { .. } => "quota_arithmetic_underflow",
            Self::Integrity { .. } => "quota_integrity",
            Self::Db(_) => "quota_database_failed",
            Self::Time(_) => "quota_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Exceeded(exceeded) => {
                let error = ApiError::new(ErrorCode::QuotaExceeded)
                    .with_detail("usedBytes", exceeded.used.to_i64())
                    .with_detail("heldBytes", exceeded.held.to_i64())
                    .with_detail("requestedBytes", exceeded.requested.to_i64());
                match exceeded.quota {
                    Some(quota) => error.with_detail("quotaBytes", quota.to_i64()),
                    None => error,
                }
            }
            Self::OwnerInactive => ApiError::new(ErrorCode::AuthAccountInactive),
            Self::Db(error) => ApiError::new(error.api_code()),
            Self::OwnerNotFound
            | Self::SessionNotFound
            | Self::OwnerContextMismatch
            | Self::InvalidExpiry
            | Self::ReservationNotFound
            | Self::ReservationMismatch
            | Self::SessionAlreadySettled
            | Self::StateConflict { .. }
            | Self::Overflow { .. }
            | Self::Underflow { .. }
            | Self::Integrity { .. }
            | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for QuotaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exceeded(exceeded) => write!(
                f,
                "quota would be exceeded: used {} + held {} + requested {}",
                exceeded.used, exceeded.held, exceeded.requested
            ),
            Self::OwnerNotFound => f.write_str("the quota owner does not exist"),
            Self::OwnerInactive => f.write_str("the quota owner account is inactive"),
            Self::SessionNotFound => f.write_str("the transfer session does not exist"),
            Self::OwnerContextMismatch => {
                f.write_str("the transfer session does not belong to this owner and context")
            }
            Self::InvalidExpiry => f.write_str("the reservation expiry is not in the future"),
            Self::ReservationNotFound => {
                f.write_str("the transfer session has no quota reservation")
            }
            Self::ReservationMismatch => f.write_str(
                "the transfer session already holds a reservation with different parameters",
            ),
            Self::SessionAlreadySettled => {
                f.write_str("the transfer session reservation is already settled")
            }
            Self::StateConflict { current } => write!(
                f,
                "the reservation is {} and cannot take this transition",
                current.as_str()
            ),
            Self::Overflow { operation } => {
                write!(f, "quota arithmetic overflowed during {operation}")
            }
            Self::Underflow { operation } => {
                write!(f, "quota arithmetic underflowed during {operation}")
            }
            Self::Integrity { column } => {
                write!(f, "a quota row holds an invalid value in {column}")
            }
            Self::Db(error) => write!(f, "quota database operation failed: {error}"),
            Self::Time(error) => write!(f, "quota timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for QuotaError {}

impl From<DbError> for QuotaError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for QuotaError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for QuotaError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<QuotaError> for ApiError {
    fn from(error: QuotaError) -> Self {
        error.api_error()
    }
}
