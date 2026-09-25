use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::SessionService;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, SignOutCaller};
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::LoginError;
use super::lockout::AttemptClient;
use super::login::{LoginInput, LoginRequest};
use super::model::{LoginResponse, MeResponse};
use super::service::{AuthService, LoginContext};

pub const LOGIN_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthLogin,
    Transport::ControlPlane,
);

pub const LOGOUT_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
)
.with_idempotent_sign_out();

pub const ME_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LOGIN_ROUTE, routes!(login))
        .route(LOGOUT_ROUTE, routes!(logout))
        .route(ME_ROUTE, routes!(me))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body(content = LoginRequest, content_type = "application/json"),
    responses(
        (
            status = 200,
            description = "Signed in; `palmr_session` and `palmr_csrf` are set and any presented session is revoked.",
            body = LoginResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "The credentials did not authenticate.", body = ApiErrorBody),
        (status = 403, description = "Password login is disabled, or the origin is not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited, or the account is locked after the password was proven.", body = ApiErrorBody),
    )
)]
async fn login(Extension(service): Extension<AuthService>, request: Request) -> Response {
    let request_id = RequestId::of(&request);
    let session = SessionService::client(&request);
    let context = LoginContext {
        attempt: AttemptClient {
            ip: session.ip_address.clone(),
            user_agent: session.user_agent.clone(),
            request_id: request_id.as_ref().map(|id| id.as_str().to_owned()),
        },
        session,
        audit: client_metadata(&request),
        presented_session: SessionService::presented_token(request.headers()),
    };
    let input = match json::read::<LoginRequest>(request.into_body()).await {
        Ok(body) => match LoginInput::parse(body) {
            Ok(input) => input,
            Err(error) => return login_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };

    let logged_in = match service.login(input, context).await {
        Ok(logged_in) => logged_in,
        Err(error) => return login_error(&error, request_id.as_ref()),
    };
    let mut response = json(StatusCode::OK, &logged_in.response, request_id.as_ref());
    if let Err(error) = service
        .sessions()
        .emit_cookies(response.headers_mut(), &logged_in.session)
    {
        tracing::error!(
            kind = error.kind(),
            "login session cookies could not be emitted"
        );
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 204, description = "The current session is revoked, or none remained; `palmr_session` and `palmr_csrf` are cleared."),
        (status = 403, description = "A presented session lacks its CSRF proof, or the origin is not allowed.", body = ApiErrorBody),
    )
)]
async fn logout(
    Extension(service): Extension<AuthService>,
    caller: SignOutCaller,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    if let SignOutCaller::Session(principal) = caller {
        if let Err(error) = service.logout(&principal, &client_metadata(&request)).await {
            return login_error(&error, request_id.as_ref());
        }
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    if service
        .sessions()
        .cookie_policy()
        .expire_session_pair(response.headers_mut())
        .is_err()
    {
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's current authentication state.", body = MeResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
    )
)]
async fn me(
    Extension(service): Extension<AuthService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.me(&principal).await {
        Ok(me) => json(StatusCode::OK, &me, request_id.as_ref()),
        Err(error) => login_error(&error, request_id.as_ref()),
    }
}

fn login_error(error: &LoginError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "authentication request failed");
    }
    let mut response = tag_error(api_error, request_id).into_response();
    if let Some(retry_after) = error.retry_after() {
        response
            .headers_mut()
            .insert(RETRY_AFTER, retry_after.header_value());
    }
    response
}

fn json<T: serde::Serialize>(
    status: StatusCode,
    body: &T,
    request_id: Option<&RequestId>,
) -> Response {
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
        Err(_) => tag_error(ApiError::internal(), request_id).into_response(),
    }
}
