use std::sync::Arc;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::{from_fn, Next};
use axum::response::Response;
use axum::routing::MethodRouter;
use tracing::field::{display, Empty};
use tracing::{Instrument, Span};

use super::request_id::RequestId;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestLog {
    #[default]
    Standard,
    Polled,
}

impl RequestLog {
    pub(crate) fn apply<S>(self, handler: MethodRouter<S>) -> MethodRouter<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        match self {
            Self::Standard => handler,
            Self::Polled => handler.route_layer(from_fn(mark_polled_response)),
        }
    }
}

#[derive(Clone, Copy)]
struct PolledResponse(());

async fn mark_polled_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.extensions_mut().insert(PolledResponse(()));
    response
}

pub async fn trace_request(
    State(clock): State<Arc<dyn Clock>>,
    request: Request,
    next: Next,
) -> Response {
    let span = tracing::info_span!(
        "http_request",
        request_id = Empty,
        method = %request.method(),
        route = Empty,
        client_ip = Empty,
        status = Empty,
        duration_ms = Empty,
        error_code = Empty,
    );
    if let Some(request_id) = request.extensions().get::<RequestId>() {
        span.record("request_id", request_id.as_str());
    }

    let started = clock.monotonic();
    let response = next.run(request).instrument(span.clone()).await;
    let elapsed = clock.monotonic().saturating_duration_since(started);

    span.record("status", response.status().as_u16());
    span.record(
        "duration_ms",
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    );
    if let Some(code) = response.extensions().get::<ErrorCode>() {
        span.record("error_code", code.as_str());
    }
    let routine_poll =
        response.status().is_success() && response.extensions().get::<PolledResponse>().is_some();
    span.in_scope(|| {
        if routine_poll {
            tracing::debug!("request completed");
        } else {
            tracing::info!("request completed");
        }
    });
    response
}

pub async fn record_route(request: Request, next: Next) -> Response {
    if let Some(route) = request.extensions().get::<MatchedPath>() {
        Span::current().record("route", display(route.as_str()));
    }
    next.run(request).await
}
