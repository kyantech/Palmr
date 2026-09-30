use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::path::Parameter;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::role::Role;
use crate::features::auth::sessions::{SessionItem, SessionService};
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Admin;
use crate::infra::http::pagination::{
    cursor_parameter, enum_parameter, limit_parameter, search_parameter, Page,
};
use crate::infra::http::request_id::{tag_error, RequestId};

use super::admin_model::{AdminUserDetail, AdminUserItem, UserStatus};
use super::admin_service::{AdminUserError, AdminUserService, ROLE_PARAM, STATUS_PARAM, USER_SORT};
use super::model::UserId;

pub const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_users), &user_list_parameters()),
        )
        .route(READ_ROUTE, routes!(get_user))
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
