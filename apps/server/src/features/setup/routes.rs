use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use super::model::Bootstrap;
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::infra::http::error::{ApiError, JSON_CONTENT_TYPE};
use crate::infra::http::request_id::{tag_error, RequestId};

pub const BOOTSTRAP_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::PublicRead,
    Transport::ControlPlane,
)
.with_anonymous_csrf();

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new().route(BOOTSTRAP_ROUTE, routes!(bootstrap))
}

#[utoipa::path(
    get,
    path = "/api/v1/bootstrap",
    tag = "instance",
    responses(
        (
            status = 200,
            description = "The instance description the SPA needs before it knows the visitor.",
            body = Bootstrap
        ),
    )
)]
async fn bootstrap(State(state): State<AppState>, request: Request) -> Response {
    let settings = state.settings().load();
    match serde_json::to_vec(&Bootstrap::from_settings(&settings)) {
        Ok(body) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE)),
                (CACHE_CONTROL, NO_STORE),
            ],
            body,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "the bootstrap payload could not be serialized");
            tag_error(ApiError::internal(), RequestId::of(&request).as_ref()).into_response()
        }
    }
}
