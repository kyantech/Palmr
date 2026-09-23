use std::any::Any;

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::catch_panic::{CatchPanic, ResponseForPanic};

use super::error::ApiError;
use super::request_id::{tag_error, RequestId};

#[derive(Clone)]
struct InternalErrorForPanic {
    request_id: Option<RequestId>,
}

impl ResponseForPanic for InternalErrorForPanic {
    type ResponseBody = Body;

    fn response_for_panic(&mut self, _payload: Box<dyn Any + Send + 'static>) -> Response {
        tracing::error!("request processing panicked");
        tag_error(ApiError::internal(), self.request_id.as_ref()).into_response()
    }
}

pub async fn catch_panic(request: Request, next: Next) -> Response {
    let respond = InternalErrorForPanic {
        request_id: RequestId::of(&request),
    };
    match CatchPanic::custom(next, respond).oneshot(request).await {
        Ok(response) => response.into_response(),
        Err(infallible) => match infallible {},
    }
}
