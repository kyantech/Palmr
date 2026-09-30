use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::path::Parameter;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{IdempotencyMode, RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::role::Role;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::{SessionItem, SessionService};
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Admin;
use crate::infra::http::idempotency::{Admission, Claim, IdempotencyRequest};
use crate::infra::http::json;
use crate::infra::http::pagination::{
    cursor_parameter, enum_parameter, limit_parameter, search_parameter, Page,
};
use crate::infra::http::request_id::{tag_error, RequestId};

use super::admin_input::{
    ChangeRoleRequest, CreateInput, CreateUserRequest, UpdateInput, UpdateUserRequest,
};
use super::admin_model::{AdminUserDetail, AdminUserItem, UserStatus};
use super::admin_service::{AdminUserError, AdminUserService, ROLE_PARAM, STATUS_PARAM, USER_SORT};
use super::model::UserId;

pub const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const CREATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
)
.with_idempotency(IdempotencyMode::Plaintext);

pub const UPDATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const SENSITIVE_WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_users), &user_list_parameters()),
        )
        .route(CREATE_ROUTE, routes!(create_user))
        .route(READ_ROUTE, routes!(get_user))
        .route(UPDATE_ROUTE, routes!(update_user))
        .route(SENSITIVE_WRITE_ROUTE, routes!(change_user_role))
        .route(UPDATE_ROUTE, routes!(activate_user))
        .route(SENSITIVE_WRITE_ROUTE, routes!(deactivate_user))
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_user_sessions), &session_list_parameters()),
        )
}

fn user_list_parameters() -> Vec<Parameter> {
    let roles = Role::ALL.map(Role::as_str);
    let statuses = UserStatus::ALL.map(UserStatus::as_str);
    vec![
        search_parameter(),
        enum_parameter(ROLE_PARAM, &roles),
        enum_parameter(STATUS_PARAM, &statuses),
        USER_SORT.parameter(),
        cursor_parameter(),
        limit_parameter(),
    ]
}

fn session_list_parameters() -> Vec<Parameter> {
    vec![
        SessionService::sort_parameter(),
        cursor_parameter(),
        limit_parameter(),
    ]
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    tag = "admin-users",
    responses(
        (status = 200, description = "Users with accounted storage, effective quota and resource counts. `usedBytes` is My Files plus Received; `quotaBytes` and `effectiveQuotaBytes` are `null` when Unlimited. `totalCount` is the exact filtered user count.", body = Page<AdminUserItem>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_users(
    Extension(service): Extension<AdminUserService>,
    Admin(_admin): Admin,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let query = match service.query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.list(query).await {
        Ok(page) => json_ok(&page, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/users",
    tag = "admin-users",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "16–128 characters. A replay within 24 hours returns the original `201` without creating another account.")
    ),
    request_body(
        content = CreateUserRequest,
        content_type = "application/json",
        description = "With `password` the account has a local credential and `requirePasswordChange` defaults to `true`. Without it the account is SSO-only and cannot sign in locally. The password is write-only and never appears in any response."
    ),
    responses(
        (status = 201, description = "The created user, in the shape of a `GET /api/v1/admin/users` row. Credential material is never part of this response.", body = AdminUserItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 409, description = "`USER_EMAIL_TAKEN` or `USER_USERNAME_TAKEN` when the case-insensitive identity is already in use; `IDEMPOTENCY_KEY_CONFLICT` or `IDEMPOTENCY_REQUEST_IN_PROGRESS` for a reused key.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid field, including `requirePasswordChange: true` without a password; `PASSWORD_POLICY_VIOLATION` when the password is shorter than the effective minimum.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn create_user(
    Extension(service): Extension<AdminUserService>,
    Admin(admin): Admin,
    idempotency: IdempotencyRequest,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let body = match json::read_value(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let claim = match service.idempotency().claim(idempotency, &body).await {
        Ok(Admission::Execute(claim)) => claim,
        Ok(Admission::Replay(mut response)) => {
            response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
            return response;
        }
        Err(rejection) => return rejection.into_response(),
    };
    let parsed = match json::parse::<CreateUserRequest>(body) {
        Ok(parsed) => parsed,
        Err(error) => {
            release(&service, claim).await;
            return tag_error(error, request_id.as_ref()).into_response();
        }
    };
    let created = match CreateInput::parse(parsed) {
        Ok(input) => service.create(&admin, input, &claim, &client).await,
        Err(error) => Err(error),
    };
    match created {
        Ok(created) => json_response(StatusCode::CREATED, &created, request_id.as_ref()),
        Err(error) => {
            release(&service, claim).await;
            admin_user_error(&error, request_id.as_ref())
        }
    }
}

async fn release(service: &AdminUserService, claim: Claim) {
    if let Err(error) = service.idempotency().release(claim).await {
        tracing::error!(
            kind = error.kind(),
            "admin user idempotency claim could not be released"
        );
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 200, description = "The list row plus `overQuota`, `sessionCount`, `trustedDeviceCount`, `lockout` and `identityLinks`.", body = AdminUserDetail),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_user(
    Extension(service): Extension<AdminUserService>,
    Admin(_admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    match service.detail(id).await {
        Ok(detail) => json_ok(&detail, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    request_body(
        content = UpdateUserRequest,
        content_type = "application/json",
        description = "Only `firstName`, `lastName` and `username` are editable; any other member is rejected, and at least one editable member is required. E-mail, role, activation, quota and password have dedicated endpoints."
    ),
    responses(
        (status = 200, description = "The updated user, in the shape of a `GET /api/v1/admin/users` row.", body = AdminUserItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`USER_USERNAME_TAKEN` when the case-insensitive username belongs to another account.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid or non-editable member, or an empty body.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn update_user(
    Extension(service): Extension<AdminUserService>,
    Admin(_admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    let input = match json::read::<UpdateUserRequest>(request.into_body()).await {
        Ok(body) => match UpdateInput::parse(body) {
            Ok(input) => input,
            Err(error) => return admin_user_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update(id, input).await {
        Ok(item) => json_response(StatusCode::OK, &item, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/users/{id}/role",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    request_body(
        content = ChangeRoleRequest,
        content_type = "application/json",
        description = "The new role, `admin` or `user`. A change revokes every session of the target user in the same transaction; trusted devices are kept. Setting the current role succeeds without any side effect."
    ),
    responses(
        (status = 200, description = "The user in the shape of a `GET /api/v1/admin/users` row.", body = AdminUserItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`LAST_ADMIN_PROTECTED` when the target is the only active Admin.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for a missing or unknown role.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn change_user_role(
    Extension(service): Extension<AdminUserService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    let role = match json::read::<ChangeRoleRequest>(request.into_body()).await {
        Ok(body) => match body.parse() {
            Ok(role) => role,
            Err(error) => return admin_user_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.change_role(&admin, id, role, &client).await {
        Ok(item) => json_ok(&item, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{id}/activate",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 200, description = "The user in the shape of a `GET /api/v1/admin/users` row. Identity links suspended by the deactivation are restored; revoked sessions and trusted devices are not. Activating an active user succeeds without any side effect.", body = AdminUserItem),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn activate_user(
    Extension(service): Extension<AdminUserService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    match service.activate(&admin, id, &client).await {
        Ok(item) => json_ok(&item, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{id}/deactivate",
    tag = "admin-users",
    params(("id" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 200, description = "The user in the shape of a `GET /api/v1/admin/users` row. In one transaction every session and trusted device of the user is revoked and every active identity link is suspended; no content is changed. Deactivating an inactive user succeeds without any side effect.", body = AdminUserItem),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`LAST_ADMIN_PROTECTED` when the target is the only active Admin.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn deactivate_user(
    Extension(service): Extension<AdminUserService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    match service.deactivate(&admin, id, &client).await {
        Ok(item) => json_ok(&item, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{userId}/sessions",
    tag = "admin-users",
    params(("userId" = String, Path, description = "User UUIDv7")),
    responses(
        (status = 200, description = "The target user's active sessions, in the shape of `GET /api/v1/sessions`. Token material is never part of this response.", body = Page<SessionItem>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 404, description = "`USER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_user_sessions(
    Extension(service): Extension<AdminUserService>,
    Admin(admin): Admin,
    Path(user_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(user_id) = user_id.parse::<UserId>() else {
        return admin_user_error(&AdminUserError::NotFound, request_id.as_ref());
    };
    let page = match service.session_page_request(raw_query.as_deref()) {
        Ok(page) => page,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.sessions(&admin, user_id, page).await {
        Ok(page) => json_ok(&page, request_id.as_ref()),
        Err(error) => admin_user_error(&error, request_id.as_ref()),
    }
}

fn admin_user_error(error: &AdminUserError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "admin user request failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn json_ok<T: serde::Serialize>(body: &T, request_id: Option<&RequestId>) -> Response {
    json_response(StatusCode::OK, body, request_id)
}

fn json_response<T: serde::Serialize>(
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
