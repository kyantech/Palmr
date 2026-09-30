use axum::body::Body;
use axum::extract::{Extension, Path, Request};
use axum::response::{IntoResponse, Response};
use http::header::CACHE_CONTROL;
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::ApiErrorBody;
use crate::infra::http::extractors::Admin;
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::email_change::{
    ChangeEmailInput, ChangeEmailRequest, EmailChangeError, EmailChangeService,
};
use super::model::UserId;

pub const CHANGE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const RESEND_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::EmailTest,
    Transport::ControlPlane,
);

pub const CANCEL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(CHANGE_ROUTE, routes!(start_email_change))
        .route(RESEND_ROUTE, routes!(resend_email_change))
        .route(CANCEL_ROUTE, routes!(cancel_email_change))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{id}/email",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    request_body(
        content = ChangeEmailRequest,
        content_type = "application/json",
        description = "The new address is normalized with the canonical identity rules. It becomes the user's pending address; the current address stays canonical and keeps working for sign-in until the new one is verified. A previous pending change of the same user is replaced and its verification link stops working. The link is built from the configured public base URL only."
    ),
    responses(
        (status = 202, description = "Accepted. The pending address, a live verification and its durable verification message are recorded in one transaction together with `USER_EMAIL_CHANGE_REQUESTED`; the message is delivered later by the outbox worker. The verification token is never part of the response."),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`USER_EMAIL_TAKEN` when the address is canonical or pending on another user, or `FEATURE_UNAVAILABLE_SMTP` when outbound e-mail is not configured; nothing is recorded in either case.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for a malformed address or one equal to the user's current canonical address.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn start_email_change(
    Extension(service): Extension<EmailChangeService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return email_change_error(&EmailChangeError::NotFound, request_id.as_ref());
    };
    let input = match json::read::<ChangeEmailRequest>(request.into_body()).await {
        Ok(body) => match ChangeEmailInput::parse(body) {
            Ok(input) => input,
            Err(error) => return email_change_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.start(&admin, id, input, &client).await {
        Ok(()) => status_only(StatusCode::ACCEPTED),
        Err(error) => email_change_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{id}/email/resend",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 202, description = "A fresh one-time token replaces the previous one, which stops working, and a new verification message is queued; the undelivered message of the previous token is cancelled. The expiry of the pending change is unchanged and the canonical address is untouched."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`EMAIL_VERIFICATION_NOT_PENDING` when the user has no live pending change, including one whose verification has expired, or `FEATURE_UNAVAILABLE_SMTP`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn resend_email_change(
    Extension(service): Extension<EmailChangeService>,
    Admin(_admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return email_change_error(&EmailChangeError::NotFound, request_id.as_ref());
    };
    match service.resend(id).await {
        Ok(()) => status_only(StatusCode::ACCEPTED),
        Err(error) => email_change_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/users/{id}/email",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 204, description = "The pending address is cleared, its verification link stops working and an undelivered verification message is cancelled. The canonical address and every session are untouched. Cancelling when nothing is pending succeeds without any side effect."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn cancel_email_change(
    Extension(service): Extension<EmailChangeService>,
    Admin(_admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return email_change_error(&EmailChangeError::NotFound, request_id.as_ref());
    };
    match service.cancel(id).await {
        Ok(()) => status_only(StatusCode::NO_CONTENT),
        Err(error) => email_change_error(&error, request_id.as_ref()),
    }
}

pub fn email_change_error(error: &EmailChangeError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "e-mail change request failed");
    }
    tag_error(api_error, request_id).into_response()
}

pub fn status_only(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    response
}
