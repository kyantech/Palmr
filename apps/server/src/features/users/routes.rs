use axum::body::Body;
use axum::extract::{Extension, Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::model::MeUser;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, PasswordChangeCaller};
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::preferences::{Preferences, PreferencesRequest};
use super::profile::{
    PasswordChangeRequest, ProfileChange, ProfileError, ProfileRequest, ProfileService,
    UsageResponse,
};

pub const PROFILE_READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const PROFILE_WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

pub const PASSWORD_CHANGE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
)
.with_forced_password_change_waiver();

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(PROFILE_READ_ROUTE, routes!(get_profile))
        .route(PROFILE_WRITE_ROUTE, routes!(update_profile))
        .route(PROFILE_READ_ROUTE, routes!(get_preferences))
        .route(PROFILE_WRITE_ROUTE, routes!(update_preferences))
        .route(PASSWORD_CHANGE_ROUTE, routes!(change_password))
        .route(PROFILE_READ_ROUTE, routes!(get_usage))
}

#[utoipa::path(
    get,
    path = "/api/v1/profile",
    tag = "profile",
    responses(
        (status = 200, description = "The caller's profile; the `user` object of `GET /api/v1/auth/me`.", body = MeUser),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
    )
)]
async fn get_profile(
    Extension(service): Extension<ProfileService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.profile(&principal).await {
        Ok(profile) => json_ok(&profile, request_id.as_ref()),
        Err(error) => profile_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/profile",
    tag = "profile",
    request_body(
        content = ProfileRequest,
        content_type = "application/json",
        description = "Only `firstName` and `lastName` are editable; any other member is rejected."
    ),
    responses(
        (status = 200, description = "The updated profile.", body = MeUser),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
    )
)]
async fn update_profile(
    Extension(service): Extension<ProfileService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let change = match json::read::<ProfileRequest>(request.into_body()).await {
        Ok(body) => match ProfileChange::parse(body) {
            Ok(change) => change,
            Err(error) => return profile_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update_profile(&principal, change).await {
        Ok(profile) => json_ok(&profile, request_id.as_ref()),
        Err(error) => profile_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/profile/preferences",
    tag = "profile",
    responses(
        (status = 200, description = "The caller's presentation preferences.", body = Preferences),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
    )
)]
async fn get_preferences(
    Extension(service): Extension<ProfileService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.preferences(&principal).await {
        Ok(preferences) => json_ok(&preferences, request_id.as_ref()),
        Err(error) => profile_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/profile/preferences",
    tag = "profile",
    request_body(
        content = PreferencesRequest,
        content_type = "application/json",
        description = "`locale` is one of the 23 supported locales, `theme` is `light`, `dark` or `system`, and `accent` is a curated preset key: `default`, `blue`, `violet`, `emerald`, `amber`, `rose` or `slate`."
    ),
    responses(
        (status = 200, description = "The updated preferences.", body = Preferences),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "Unknown locale, theme or accent key, or an unknown member.", body = ApiErrorBody),
    )
)]
async fn update_preferences(
    Extension(service): Extension<ProfileService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let body = match json::read::<PreferencesRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update_preferences(&principal, body).await {
        Ok(preferences) => json_ok(&preferences, request_id.as_ref()),
        Err(error) => profile_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/profile/password",
    tag = "profile",
    request_body(content = PasswordChangeRequest, content_type = "application/json"),
    responses(
        (
            status = 204,
            description = "The password is changed; every other session and every trusted device is revoked, and the current session is rotated with fresh `palmr_session` and `palmr_csrf` cookies."
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The current password did not verify, recent authentication is required, password login is disabled, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation or the new password violates the password policy.", body = ApiErrorBody),
    )
)]
async fn change_password(
    Extension(service): Extension<ProfileService>,
    PasswordChangeCaller(principal): PasswordChangeCaller,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let body = match json::read::<PasswordChangeRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let session = match service.change_password(&principal, body, &client).await {
        Ok(session) => session,
        Err(error) => return profile_error(&error, request_id.as_ref()),
    };
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    if let Err(error) = service
        .auth()
        .sessions()
        .emit_cookies(response.headers_mut(), &session)
    {
        tracing::error!(
            kind = error.kind(),
            "rotated session cookies could not be emitted after a password change"
        );
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/profile/usage",
    tag = "profile",
    responses(
        (status = 200, description = "The caller's storage accounting: My Files plus Received, held reservations and effective limits.", body = UsageResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
    )
)]
async fn get_usage(
    Extension(service): Extension<ProfileService>,
    Authenticated(principal): Authenticated,
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.usage(&principal, state.storage().caps()).await {
        Ok(usage) => json_ok(&usage, request_id.as_ref()),
        Err(error) => profile_error(&error, request_id.as_ref()),
    }
}

fn profile_error(error: &ProfileError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "profile request failed");
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
