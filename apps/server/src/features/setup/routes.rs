use axum::body::Body;
use axum::extract::{Extension, Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, USER_AGENT};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use super::error::SetupError;
use super::model::{Bootstrap, SetupInput, SetupRequest, SetupResponse, SetupStatus};
use super::service::SetupService;
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::SessionService;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::json;
use crate::infra::http::proxy::ResolvedClient;
use crate::infra::http::request_id::{tag_error, RequestId};

pub const BOOTSTRAP_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::PublicRead,
    Transport::ControlPlane,
)
.with_anonymous_csrf();

pub const SETUP_STATUS_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::PublicRead,
    Transport::ControlPlane,
);

pub const SETUP_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Setup,
    RateLimitClass::AuthLogin,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(BOOTSTRAP_ROUTE, routes!(bootstrap))
        .route(SETUP_STATUS_ROUTE, routes!(setup_status))
        .route(SETUP_ROUTE, routes!(complete_setup))
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

#[utoipa::path(
    get,
    path = "/api/v1/setup/status",
    tag = "instance",
    responses(
        (
            status = 200,
            description = "Whether first-run setup is complete, and the account password policy while it is not.",
            body = SetupStatus
        ),
    )
)]
async fn setup_status(State(state): State<AppState>, request: Request) -> Response {
    let status = SetupStatus::from_settings(&state.settings().load());
    json(StatusCode::OK, &status, &request)
}

#[utoipa::path(
    post,
    path = "/api/v1/setup",
    tag = "instance",
    request_body(content = SetupRequest, content_type = "application/json"),
    responses(
        (
            status = 201,
            description = "The first Admin was created and signed in; `palmr_session` and `palmr_csrf` are set.",
            body = SetupResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 409, description = "Setup is already complete, or the identity is taken.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation or the password policy.", body = ApiErrorBody),
    )
)]
async fn complete_setup(Extension(service): Extension<SetupService>, request: Request) -> Response {
    let request_id = RequestId::of(&request);
    let client = SessionService::client(&request);
    let audit_client = audit_client(&request);
    let input = match json::read::<SetupRequest>(request.into_body()).await {
        Ok(body) => match SetupInput::parse(body) {
            Ok(input) => input,
            Err(error) => return setup_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };

    let completed = match service.complete(input, client, audit_client).await {
        Ok(completed) => completed,
        Err(error) => return setup_error(&error, request_id.as_ref()),
    };
    let payload = match serde_json::to_vec(&SetupResponse::from_user(&completed.user)) {
        Ok(payload) => payload,
        Err(_) => {
            tracing::error!("the setup response could not be serialized");
            return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
        }
    };
    let mut response = Response::new(Body::from(payload));
    *response.status_mut() = StatusCode::CREATED;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE));
    headers.insert(CACHE_CONTROL, NO_STORE);
    if let Err(error) = service.sessions().emit_cookies(headers, &completed.session) {
        tracing::error!(
            kind = error.kind(),
            "setup session cookies could not be emitted"
        );
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

fn audit_client(request: &Request) -> ClientMetadata {
    let user_agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok());
    request
        .extensions()
        .get::<ResolvedClient>()
        .map_or_else(ClientMetadata::none, |client| {
            ClientMetadata::from_request(client, RequestId::of(request).as_ref(), user_agent)
        })
}

fn setup_error(error: &SetupError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "first-run setup failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn json<T: serde::Serialize>(status: StatusCode, body: &T, request: &Request) -> Response {
    match serde_json::to_vec(body) {
        Ok(body) => (
            status,
            [
                (CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE)),
                (CACHE_CONTROL, NO_STORE),
            ],
            body,
        )
            .into_response(),
        Err(_) => tag_error(ApiError::internal(), RequestId::of(request).as_ref()).into_response(),
    }
}
