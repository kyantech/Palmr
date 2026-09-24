use std::num::NonZeroU64;
use std::time::Duration;

use axum::response::{IntoResponse, Response};
use http::header::RETRY_AFTER;
use http::HeaderValue;

use super::class::RateLimitClass;
use crate::domain::error_code::ErrorCode;
use crate::infra::http::error::ApiError;
use crate::infra::http::request_id::{tag_error, RequestId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(NonZeroU64);

impl RetryAfter {
    pub fn covering(wait: Duration) -> Self {
        let whole = wait
            .as_secs()
            .saturating_add(u64::from(wait.subsec_nanos() > 0));
        Self(NonZeroU64::new(whole).unwrap_or(NonZeroU64::MIN))
    }

    pub const fn seconds(self) -> u64 {
        self.0.get()
    }

    pub fn header_value(self) -> HeaderValue {
        HeaderValue::from(self.0.get())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throttled {
    scope: RateLimitClass,
    retry_after: RetryAfter,
}

impl Throttled {
    pub(super) const fn new(scope: RateLimitClass, retry_after: RetryAfter) -> Self {
        Self { scope, retry_after }
    }

    pub const fn scope(&self) -> RateLimitClass {
        self.scope
    }

    pub const fn retry_after(&self) -> RetryAfter {
        self.retry_after
    }

    pub fn api_error(&self) -> ApiError {
        ApiError::new(ErrorCode::RateLimited).with_detail("scope", self.scope.as_str())
    }

    pub fn into_response(self, request_id: Option<&RequestId>) -> Response {
        let mut response = tag_error(self.api_error(), request_id).into_response();
        response
            .headers_mut()
            .insert(RETRY_AFTER, self.retry_after.header_value());
        response
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rejected {
    Throttled(Throttled),
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitRejection {
    rejected: Rejected,
    request_id: Option<RequestId>,
}

impl RateLimitRejection {
    pub(super) fn throttled(throttled: Throttled, request_id: Option<&RequestId>) -> Self {
        Self {
            rejected: Rejected::Throttled(throttled),
            request_id: request_id.cloned(),
        }
    }

    pub(super) fn internal(request_id: Option<&RequestId>) -> Self {
        Self {
            rejected: Rejected::Internal,
            request_id: request_id.cloned(),
        }
    }
}

impl IntoResponse for RateLimitRejection {
    fn into_response(self) -> Response {
        match self.rejected {
            Rejected::Throttled(throttled) => throttled.into_response(self.request_id.as_ref()),
            Rejected::Internal => {
                tag_error(ApiError::internal(), self.request_id.as_ref()).into_response()
            }
        }
    }
}
