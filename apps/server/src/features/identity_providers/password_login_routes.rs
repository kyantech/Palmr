use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::routes;

use super::link_routes::json_response;
use super::password_login::{PasswordLoginError, PasswordLoginService, PasswordLoginState};
use super::routes::{READ_ROUTE, SENSITIVE_WRITE_ROUTE};
use crate::app::router::Routes;
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::{ApiError, ApiErrorBody};
use crate::infra::http::extractors::Admin;
use crate::infra::http::json::{self, JsonField, JsonKind, JsonRequest};
use crate::infra::http::request_id::{tag_error, RequestId};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordLoginRequest {
    pub enabled: bool,
    pub confirm: bool,
}

impl JsonRequest for PasswordLoginRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("enabled", JsonKind::Boolean),
        JsonField::required("confirm", JsonKind::Boolean),
    ];
}

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(READ_ROUTE, routes!(get_password_login))
        .route(SENSITIVE_WRITE_ROUTE, routes!(put_password_login))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/auth/password-login",
    tag = "admin-auth",
    responses(
        (status = 200, description = "Advisory pre-flight for the instance-wide local password login switch. `canDisable` is true only while password login is enabled and every disable precondition holds for the calling Administrator: external providers are globally enabled, an enabled provider has a successful test no older than 24 hours, an active Administrator is linked to it, and the caller is linked to such a provider. `blockers` explains each missing precondition with the accepted `NO_VALIDATED_PROVIDER` or `PASSWORD_LOGIN_DISABLE_UNSAFE` code. `safeAdminLoginPaths` lists active Administrators with an active link to an enabled provider (the structural standing path); `providerValidated` is true when that provider's last test succeeded within 24 hours, which is what disable admission requires. The list is empty while providers are globally disabled. The result is never a guarantee: `PUT` recomputes everything inside its own write transaction. No secret, token, external subject or provider response is returned.", body = PasswordLoginState),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_password_login(
    Extension(service): Extension<PasswordLoginService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.state(admin.user_id).await {
        Ok(state) => json_response(StatusCode::OK, &state, request_id.as_ref()),
        Err(error) => failure(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/auth/password-login",
    tag = "admin-auth",
    request_body(
        content = PasswordLoginRequest,
        content_type = "application/json",
        description = "`confirm` must be `true`. Disabling runs in one `BEGIN IMMEDIATE` transaction that recomputes every precondition: external providers are globally enabled, an enabled provider has a successful test no older than 24 hours, an active Administrator is linked to it and the acting Administrator is linked to such a provider. Enabling needs no precondition and is the recovery direction. A request for the state already in force changes nothing and writes no audit row."
    ),
    responses(
        (status = 200, description = "The state after the request, in the shape of `GET`. A change is audited as `PASSWORD_LOGIN_DISABLED` or `PASSWORD_LOGIN_ENABLED` in the same transaction and visible to the next request. Existing sessions, trusted devices and local password hashes are untouched.", body = PasswordLoginState),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 409, description = "`PASSWORD_LOGIN_DISABLE_UNSAFE` with `details.blockers[]`; nothing changed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an unknown field, a non-boolean member or `confirm` that is not `true`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn put_password_login(
    Extension(service): Extension<PasswordLoginService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let body = match json::read::<PasswordLoginRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    if !body.confirm {
        return tag_error(ApiError::validation(["confirm"]), request_id.as_ref()).into_response();
    }
    match service.set(&admin, body.enabled, &client).await {
        Ok(state) => json_response(StatusCode::OK, &state, request_id.as_ref()),
        Err(error) => failure(&error, request_id.as_ref()),
    }
}

fn failure(error: &PasswordLoginError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "password login request failed");
    }
    tag_error(api_error, request_id).into_response()
}
