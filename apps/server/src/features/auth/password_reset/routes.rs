use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};
use crate::infra::ratelimit::RateLimitGate;

use super::error::PasswordResetError;
use super::model::{
    AcceptedResponse, ForgotInput, ForgotPasswordRequest, PresentedToken, ResetCheckRequest,
    ResetCheckResponse, ResetInput, ResetPasswordRequest,
};
use super::service::{AccountBudget, PasswordResetService};

pub const FORGOT_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthReset,
    Transport::ControlPlane,
);

pub const RESET_CHECK_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthToken,
    Transport::ControlPlane,
);

pub const RESET_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthToken,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(FORGOT_ROUTE, routes!(forgot))
        .route(RESET_CHECK_ROUTE, routes!(reset_check))
        .route(RESET_ROUTE, routes!(reset))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/password/forgot",
    tag = "auth",
    request_body(
        content = ForgotPasswordRequest,
        content_type = "application/json",
        description = "`identifier` is an e-mail address or a username. The body carries no URL of any kind: the e-mailed link is always the configured public base URL plus `/reset-password/{token}`."
    ),
    responses(
        (
            status = 202,
            description = "Accepted. The answer is identical whether or not the identifier names an account, and whatever that account's state; when a reset is issued, the e-mail is queued durably and sent later.",
            body = AcceptedResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 409, description = "`FEATURE_UNAVAILABLE_SMTP` when the instance has no usable outbound e-mail configuration.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited per client address or per requested account.", body = ApiErrorBody),
    )
)]
async fn forgot(
    Extension(service): Extension<PasswordResetService>,
    gate: RateLimitGate,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let input = match json::read::<ForgotPasswordRequest>(request.into_body()).await {
        Ok(body) => match ForgotInput::parse(body) {
            Ok(input) => input,
            Err(error) => return reset_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let account = match service.rate_limit_account(&input.identifier).await {
        Ok(account) => account,
        Err(error) => return reset_error(&error, request_id.as_ref()),
    };
    let budget = match gate.with_account(account).admit() {
        Ok(()) => AccountBudget::Available,
        Err(rejection) if rejection.is_throttled() => AccountBudget::Exhausted,
        Err(rejection) => return rejection.into_response(),
    };
    match service.forgot(input, budget, &client).await {
        Ok(()) => json(
            StatusCode::ACCEPTED,
            &AcceptedResponse { accepted: true },
            request_id.as_ref(),
        ),
        Err(error) => reset_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/password/reset/check",
    tag = "auth",
    request_body(content = ResetCheckRequest, content_type = "application/json"),
    responses(
        (
            status = 200,
            description = "The token is live. The answer describes only the token, never the account it belongs to.",
            body = ResetCheckResponse
        ),
        (status = 400, description = "`RESET_TOKEN_INVALID` for a malformed, unknown or invalidated token, or the body is not parseable JSON.", body = ApiErrorBody),
        (status = 410, description = "`RESET_TOKEN_EXPIRED` or `RESET_TOKEN_USED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn reset_check(
    Extension(service): Extension<PasswordResetService>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let token = match json::read::<ResetCheckRequest>(request.into_body()).await {
        Ok(body) => match PresentedToken::parse(&body.token) {
            Ok(token) => token,
            Err(error) => return reset_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.check(&token).await {
        Ok(checked) => json(StatusCode::OK, &checked, request_id.as_ref()),
        Err(error) => reset_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/password/reset",
    tag = "auth",
    request_body(content = ResetPasswordRequest, content_type = "application/json"),
    responses(
        (
            status = 204,
            description = "The password is replaced. Every session and trusted device of the account is revoked, the account lockout is cleared and no session is created; two-factor enrollment is unchanged."
        ),
        (status = 400, description = "`RESET_TOKEN_INVALID` for a malformed, unknown or invalidated token, or the body is not parseable JSON.", body = ApiErrorBody),
        (status = 403, description = "`AUTH_PASSWORD_LOGIN_DISABLED` while password login is disabled.", body = ApiErrorBody),
        (status = 410, description = "`RESET_TOKEN_EXPIRED` or `RESET_TOKEN_USED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`PASSWORD_POLICY_VIOLATION` with `details.minLength`, or the request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn reset(Extension(service): Extension<PasswordResetService>, request: Request) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let input = match json::read::<ResetPasswordRequest>(request.into_body()).await {
        Ok(body) => match ResetInput::parse(body) {
            Ok(input) => input,
            Err(error) => return reset_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.reset(input, &client).await {
        Ok(()) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
            response
        }
        Err(error) => reset_error(&error, request_id.as_ref()),
    }
}

fn reset_error(error: &PasswordResetError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "password reset request failed");
    }
    tag_error(api_error, request_id).into_response()
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
