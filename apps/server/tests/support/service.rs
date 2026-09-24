use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use palmr_server::lifecycle::{application_router, Readiness, StartupError};
use palmr_server::{Clock, OperatorConfig, TestClock};
use tower::ServiceExt;

pub fn svc_router(config: &OperatorConfig, clock: TestClock) -> Result<axum::Router, StartupError> {
    let clock: Arc<dyn Clock> = Arc::new(clock);
    application_router(config, &Readiness::new(), clock)
}

pub async fn svc_oneshot(router: axum::Router, request: Request<Body>) -> Response {
    let response: Result<Response, Infallible> = router.oneshot(request).await;
    match response {
        Ok(response) => response,
        Err(error) => match error {},
    }
}
