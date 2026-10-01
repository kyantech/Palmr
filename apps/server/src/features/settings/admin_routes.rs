use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use serde::Serialize;
use utoipa_axum::routes;

use super::admin::{AdminSettings, AdminSettingsError, AdminSettingsService};
use super::groups::general::{GeneralPatch, GeneralSettings};
use super::groups::public_links::{PublicLinkPatch, PublicLinkSettings};
use super::groups::quotas::{QuotaPatch, QuotaSettings};
use super::groups::security::{SecurityPatch, SecuritySettings};
use super::groups::SettingsGroup;
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::AuthenticatedPrincipal;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Admin;
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

pub const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const SECURITY_WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(READ_ROUTE, routes!(get_all_settings))
        .route(READ_ROUTE, routes!(get_general))
        .route(WRITE_ROUTE, routes!(patch_general))
        .route(READ_ROUTE, routes!(get_security))
        .route(SECURITY_WRITE_ROUTE, routes!(patch_security))
        .route(READ_ROUTE, routes!(get_quotas))
        .route(WRITE_ROUTE, routes!(patch_quotas))
        .route(READ_ROUTE, routes!(get_public_links))
        .route(WRITE_ROUTE, routes!(patch_public_links))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings",
    tag = "admin-settings",
    responses(
        (status = 200, description = "Every settings group available at this release, keyed by group name. Secrets are never returned.", body = AdminSettings),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_all_settings(
    Extension(service): Extension<AdminSettingsService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    json_ok(&service.all(), RequestId::of(&request).as_ref())
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings/general",
    tag = "admin-settings",
    responses(
        (status = 200, description = "The current `general` group. `hideVersion` is the inverse of the stored version-visibility flag.", body = GeneralSettings),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_general(
    Extension(service): Extension<AdminSettingsService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    json_ok(&service.general(), RequestId::of(&request).as_ref())
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/settings/general",
    tag = "admin-settings",
    request_body(
        content = GeneralPatch,
        content_type = "application/json",
        description = "Any subset of the `general` members. An absent member is left unchanged and no member of this group is nullable, so an explicit `null` is rejected. Every member is validated before anything is written; the changed members are then written and audited in one transaction and take effect on the next request. A member equal to its current value is not written and not audited."
    ),
    responses(
        (status = 200, description = "The group after the change.", body = GeneralSettings),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`SETTING_UNKNOWN` for a member the group does not define, `SETTING_VALUE_INVALID` (`details.key`) for a wrong type, `null` or an invalid value, or `VALIDATION_ERROR` when the body is not an object. Nothing is written.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn patch_general(
    Extension(service): Extension<AdminSettingsService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    patch_group(
        &service,
        SettingsGroup::General,
        &admin,
        request,
        |service| json_value(&service.general()),
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings/security",
    tag = "admin-settings",
    responses(
        (status = 200, description = "The current `security` group.", body = SecuritySettings),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_security(
    Extension(service): Extension<AdminSettingsService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    json_ok(&service.security(), RequestId::of(&request).as_ref())
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/settings/security",
    tag = "admin-settings",
    request_body(
        content = SecurityPatch,
        content_type = "application/json",
        description = "Any subset of the `security` members. An absent member is left unchanged and no member of this group is nullable, so an explicit `null` is rejected. Every member is validated before anything is written; the changed members are then written in one transaction, each audited as `SECURITY_POLICY_CHANGED` (`twoFactorRequired` as `MANDATORY_2FA_POLICY_CHANGED`), and take effect on the next request. A member equal to its current value is not written and not audited."
    ),
    responses(
        (status = 200, description = "The group after the change.", body = SecuritySettings),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication (`AUTH_RECENT_AUTH_REQUIRED`) required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`SETTING_UNKNOWN` for a member the group does not define, `SETTING_BELOW_FLOOR` (`details.key`, `details.floor`) below the platform floor, `SETTING_VALUE_INVALID` (`details.key`, and `details.max` above a documented range or the 32-bit integer limit) for a wrong type, `null` or an out-of-range value, or `VALIDATION_ERROR` when the body is not an object. Nothing is written.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn patch_security(
    Extension(service): Extension<AdminSettingsService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    patch_group(
        &service,
        SettingsGroup::Security,
        &admin,
        request,
        |service| json_value(&service.security()),
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings/quotas",
    tag = "admin-settings",
    responses(
        (status = 200, description = "The current `quotas` group; `null` is Unlimited.", body = QuotaSettings),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_quotas(
    Extension(service): Extension<AdminSettingsService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    json_ok(&service.quotas(), RequestId::of(&request).as_ref())
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/settings/quotas",
    tag = "admin-settings",
    request_body(
        content = QuotaPatch,
        content_type = "application/json",
        description = "Any subset of the `quotas` members. An absent member is left unchanged; an explicit `null` stores Unlimited. Administrators have no quota bypass. Every member is validated before anything is written; the changed members are then written and audited in one transaction and take effect on the next request. A member equal to its current value is not written and not audited."
    ),
    responses(
        (status = 200, description = "The group after the change.", body = QuotaSettings),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`SETTING_UNKNOWN` for a member the group does not define, `SETTING_BELOW_FLOOR` (`details.key`, `details.floor`) for a negative count, `SETTING_VALUE_INVALID` (`details.key`, and `details.max` above the safe JSON integer limit) for a wrong type or value, or `VALIDATION_ERROR` when the body is not an object. Nothing is written.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn patch_quotas(
    Extension(service): Extension<AdminSettingsService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    patch_group(
        &service,
        SettingsGroup::Quotas,
        &admin,
        request,
        |service| json_value(&service.quotas()),
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings/public-links",
    tag = "admin-settings",
    responses(
        (status = 200, description = "The current `public-links` group; `null` means no maximum.", body = PublicLinkSettings),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_public_links(
    Extension(service): Extension<AdminSettingsService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    json_ok(&service.public_links(), RequestId::of(&request).as_ref())
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/settings/public-links",
    tag = "admin-settings",
    request_body(
        content = PublicLinkPatch,
        content_type = "application/json",
        description = "The `public-links` members. An absent member is left unchanged; an explicit `null` removes the maximum. Validated before anything is written; a changed member is written and audited in one transaction and takes effect on the next request. A member equal to its current value is not written and not audited."
    ),
    responses(
        (status = 200, description = "The group after the change.", body = PublicLinkSettings),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`SETTING_UNKNOWN` for a member the group does not define, `SETTING_BELOW_FLOOR` (`details.key`, `details.floor`) below one day, `SETTING_VALUE_INVALID` (`details.key`, and `details.max` above the 32-bit integer limit) for a wrong type or value, or `VALIDATION_ERROR` when the body is not an object. Nothing is written.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn patch_public_links(
    Extension(service): Extension<AdminSettingsService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    patch_group(
        &service,
        SettingsGroup::PublicLinks,
        &admin,
        request,
        |service| json_value(&service.public_links()),
    )
    .await
}

async fn patch_group(
    service: &AdminSettingsService,
    group: SettingsGroup,
    admin: &AuthenticatedPrincipal,
    request: Request,
    current: impl FnOnce(&AdminSettingsService) -> Option<serde_json::Value>,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let body = match json::read_value(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.patch(group, &body, admin, &client).await {
        Ok(()) => match current(service) {
            Some(view) => json_ok(&view, request_id.as_ref()),
            None => tag_error(ApiError::internal(), request_id.as_ref()).into_response(),
        },
        Err(error) => settings_error(&error, request_id.as_ref()),
    }
}

fn json_value<T: Serialize>(view: &T) -> Option<serde_json::Value> {
    serde_json::to_value(view).ok()
}

fn settings_error(error: &AdminSettingsError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "admin settings request failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn json_ok<T: Serialize>(body: &T, request_id: Option<&RequestId>) -> Response {
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
