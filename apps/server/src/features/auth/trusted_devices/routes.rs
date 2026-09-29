use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::service::AuthService;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, AuthenticatedRecentAuth};
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::TrustedDeviceError;
use super::model::{TrustedDeviceId, TrustedDeviceList};
use super::service::presented_device;

pub const LIST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const REVOKE_ONE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

pub const REVOKE_ALL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LIST_ROUTE, routes!(list_trusted_devices))
        .route(REVOKE_ONE_ROUTE, routes!(revoke_trusted_device))
        .route(REVOKE_ALL_ROUTE, routes!(revoke_trusted_devices))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/trusted-devices",
    tag = "auth",
    params(
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor."),
        ("limit" = Option<u16>, Query, minimum = 1, maximum = 200, description = "Page size."),
        ("sort" = Option<String>, Query, description = "Sort: lastSeenAt:asc|desc")
    ),
    responses(
        (
            status = 200,
            description = "The caller's unrevoked, unexpired trusted devices and the current policy. Label and IP are display metadata only; `isCurrent` is derived from the presented `palmr_device` token.",
            body = TrustedDeviceList
        ),
        (status = 400, description = "Invalid cursor or query.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_trusted_devices(
    Extension(service): Extension<AuthService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let page = match service.trusted_device_page(raw_query.as_deref()) {
        Ok(page) => page,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let presented = presented_device(request.headers());
    match service
        .list_trusted_devices(&principal, presented.as_ref(), page)
        .await
    {
        Ok(list) => json_ok(&list, request_id.as_ref()),
        Err(error) => device_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/auth/trusted-devices/{id}",
    tag = "auth",
    params(("id" = String, Path, description = "Trusted device UUIDv7")),
    responses(
        (
            status = 204,
            description = "The device no longer satisfies the second factor at the next login. Sessions are not revoked. `palmr_device` is expired when it belonged to this device."
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication is required, the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 404, description = "`TRUSTED_DEVICE_NOT_FOUND`: unknown or foreign device.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn revoke_trusted_device(
    Extension(service): Extension<AuthService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<TrustedDeviceId>() else {
        return device_error(&TrustedDeviceError::NotFound, request_id.as_ref());
    };
    let presented = presented_device(request.headers());
    match service
        .revoke_trusted_device(
            &principal,
            id,
            presented.as_ref(),
            &client_metadata(&request),
        )
        .await
    {
        Ok(revocation) => no_content(&service, revocation.clears_cookie, request_id.as_ref()),
        Err(error) => device_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/auth/trusted-devices",
    tag = "auth",
    responses(
        (
            status = 204,
            description = "Every trusted device of the caller is revoked, including the current one, and `palmr_device` is expired. Sessions are not revoked."
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication is required, the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn revoke_trusted_devices(
    Extension(service): Extension<AuthService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service
        .revoke_all_trusted_devices(&principal, &client_metadata(&request))
        .await
    {
        Ok(_) => no_content(&service, true, request_id.as_ref()),
        Err(error) => device_error(&error, request_id.as_ref()),
    }
}

fn no_content(
    service: &AuthService,
    clear_cookie: bool,
    request_id: Option<&RequestId>,
) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    if clear_cookie
        && service
            .sessions()
            .cookie_policy()
            .expire_device(response.headers_mut())
            .is_err()
    {
        return tag_error(ApiError::internal(), request_id).into_response();
    }
    response
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

fn device_error(error: &TrustedDeviceError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "trusted-device request failed");
    }
    tag_error(api_error, request_id).into_response()
}
