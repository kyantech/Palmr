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
use crate::infra::http::extractors::{
    Authenticated, AuthenticatedRecentAuth, TotpEnrollmentCaller,
};
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};
use crate::infra::ratelimit::RateLimitGate;

use super::error::TotpError;
use super::model::{
    BackupCodesResponse, EnrollmentResponse, EnrollmentVerifyRequest, TwoFactorStatus,
};
use super::service::TotpService;

pub const STATUS_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const ENROLL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
)
.with_mandatory_totp_enrollment_waiver();

pub const VERIFY_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::AuthTotp,
    Transport::ControlPlane,
)
.with_mandatory_totp_enrollment_waiver();

pub const DISABLE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

pub const REGENERATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(STATUS_ROUTE, routes!(status))
        .route(ENROLL_ROUTE, routes!(enroll))
        .route(VERIFY_ROUTE, routes!(verify))
        .route(DISABLE_ROUTE, routes!(disable))
        .route(REGENERATE_ROUTE, routes!(regenerate))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/2fa",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's two-factor state; secrets and backup codes are never included.", body = TwoFactorStatus),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
    )
)]
async fn status(
    Extension(service): Extension<TotpService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.status(&principal).await {
        Ok(status) => json_ok(&status, request_id.as_ref()),
        Err(error) => totp_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/2fa/enroll",
    tag = "auth",
    responses(
        (
            status = 200,
            description = "A pending secret is created server-side and returned exactly once; it grants nothing until verified and expires after 10 minutes. Starting again replaces an unverified pending secret.",
            body = EnrollmentResponse
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication is required (waived only for a session whose restriction is `mfa_enrollment_required`), the session is otherwise restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 409, description = "Two-factor authentication is already enabled.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn enroll(
    Extension(service): Extension<TotpService>,
    TotpEnrollmentCaller(principal): TotpEnrollmentCaller,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.enroll(&principal).await {
        Ok(enrollment) => json_ok(&enrollment, request_id.as_ref()),
        Err(error) => totp_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/2fa/enroll/verify",
    tag = "auth",
    request_body(content = EnrollmentVerifyRequest, content_type = "application/json"),
    responses(
        (
            status = 200,
            description = "Two-factor authentication is enabled against the server-held pending secret. The ten backup codes are returned exactly once; every other session is revoked and the current session is rotated with fresh `palmr_session` and `palmr_csrf` cookies. An `mfa_enrollment_required` restriction is lifted by this change.",
            body = BackupCodesResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "The code did not verify, its time step was already used, or authentication is required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication is required (waived only for a session whose restriction is `mfa_enrollment_required`), the session is otherwise restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 409, description = "No unexpired pending enrollment matches `enrollmentId`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn verify(
    Extension(service): Extension<TotpService>,
    TotpEnrollmentCaller(principal): TotpEnrollmentCaller,
    gate: RateLimitGate,
    request: Request,
) -> Response {
    if let Err(rejection) = gate.admit() {
        return rejection.into_response();
    }
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let body = match json::read::<EnrollmentVerifyRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let enabled = match service.verify(&principal, body, &client).await {
        Ok(enabled) => enabled,
        Err(error) => return totp_error(&error, request_id.as_ref()),
    };
    let mut response = json_ok(&enabled.response, request_id.as_ref());
    if let Err(error) = service
        .auth()
        .sessions()
        .emit_cookies(response.headers_mut(), &enabled.session)
    {
        tracing::error!(
            kind = error.kind(),
            "rotated session cookies could not be emitted after enabling two-factor authentication"
        );
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/2fa/disable",
    tag = "auth",
    responses(
        (
            status = 204,
            description = "The TOTP secret and every backup code are deleted; every session of the account, including the current one, and every trusted device are revoked, and `palmr_session` and `palmr_csrf` are cleared."
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Instance policy requires two-factor authentication, recent authentication is required, the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 409, description = "Two-factor authentication is not enabled.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn disable(
    Extension(service): Extension<TotpService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    if let Err(error) = service
        .disable(&principal, &client_metadata(&request))
        .await
    {
        return totp_error(&error, request_id.as_ref());
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    if service
        .auth()
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
    post,
    path = "/api/v1/auth/2fa/backup-codes/regenerate",
    tag = "auth",
    responses(
        (
            status = 200,
            description = "Every previous backup code is invalidated and ten new codes are returned exactly once.",
            body = BackupCodesResponse
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication is required, the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 409, description = "Two-factor authentication is not enabled.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn regenerate(
    Extension(service): Extension<TotpService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service
        .regenerate_backup_codes(&principal, &client_metadata(&request))
        .await
    {
        Ok(codes) => json_ok(&codes, request_id.as_ref()),
        Err(error) => totp_error(&error, request_id.as_ref()),
    }
}

fn totp_error(error: &TotpError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "two-factor request failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn json_ok<T: serde::Serialize>(body: &T, request_id: Option<&RequestId>) -> Response {
    match serde_json::to_vec(body) {
        Ok(body) => (
            StatusCode::OK,
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
