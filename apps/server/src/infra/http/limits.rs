use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::CONTENT_LENGTH;
use http::HeaderMap;
use http_body_util::{BodyExt, LengthLimitError, Limited};

use super::error::ApiError;
use super::request_id::{tag_error, RequestId};
use crate::domain::error_code::ErrorCode;

pub const CONTROL_PLANE_DEADLINE: Duration = Duration::from_secs(30);
pub const CONTROL_PLANE_BODY_LIMIT: BodyLimit = BodyLimit(2 * 1024 * 1024);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimit(usize);

impl BodyLimit {
    pub const fn new(max_bytes: usize) -> Self {
        Self(max_bytes)
    }

    pub const fn max_bytes(self) -> usize {
        self.0
    }

    fn rejects_declared_length(self, headers: &HeaderMap) -> bool {
        headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|declared| u64::try_from(self.0).is_ok_and(|max| declared > max))
    }

    fn rejection(self, request_id: Option<&RequestId>) -> Response {
        let max_bytes = i64::try_from(self.0).unwrap_or(i64::MAX);
        let error =
            ApiError::new(ErrorCode::RequestBodyTooLarge).with_detail("maxBytes", max_bytes);
        tag_error(error, request_id).into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPlaneLimits {
    pub deadline: Duration,
    pub body: BodyLimit,
}

impl Default for ControlPlaneLimits {
    fn default() -> Self {
        Self {
            deadline: CONTROL_PLANE_DEADLINE,
            body: CONTROL_PLANE_BODY_LIMIT,
        }
    }
}

pub async fn enforce_deadline(
    State(deadline): State<Duration>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = RequestId::of(&request);
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(response) => response,
        Err(_elapsed) => tag_error(
            ApiError::new(ErrorCode::RequestTimeout),
            request_id.as_ref(),
        )
        .into_response(),
    }
}

pub async fn limit_body(State(limit): State<BodyLimit>, request: Request, next: Next) -> Response {
    let request_id = RequestId::of(&request);
    if limit.rejects_declared_length(request.headers()) {
        return limit.rejection(request_id.as_ref());
    }

    let exceeded = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&exceeded);
    let request = request.map(|body| {
        Body::new(Limited::new(body, limit.max_bytes()).map_err(move |error| {
            if error.is::<LengthLimitError>() {
                observed.store(true, Ordering::Relaxed);
            }
            error
        }))
    });

    let response = next.run(request).await;
    if exceeded.load(Ordering::Relaxed) {
        limit.rejection(request_id.as_ref())
    } else {
        response
    }
}
