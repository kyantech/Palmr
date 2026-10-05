use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use utoipa::openapi::path::Parameter;
use utoipa_axum::routes;

use super::authorize::{AuthorizeResponse, AUTH_REQUEST_TTL_SECONDS};
use super::callback::ExternalLoginService;
use super::error::ExternalLoginError;
use super::link::{IdentityLinkItem, UnlinkCommand, UnlinkScope, LINK_SORT};
use super::model::IdentityLinkId;
use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::users::model::UserId;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, AuthenticatedRecentAuth};
use crate::infra::http::pagination::{cursor_parameter, limit_parameter, Page};
use crate::infra::http::request_id::{tag_error, RequestId};

pub const LINK_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

pub const LIST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const UNLINK_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LINK_ROUTE, routes!(link_provider))
        .route(
            LIST_ROUTE,
            with_query_parameters(routes!(list_identity_links), &list_parameters()),
        )
        .route(UNLINK_ROUTE, routes!(unlink_identity))
}

pub fn list_parameters() -> Vec<Parameter> {
    vec![LINK_SORT.parameter(), cursor_parameter(), limit_parameter()]
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/providers/{slug}/link",
    tag = "auth-providers",
    params(("slug" = String, Path, description = "Provider slug")),
    responses(
        (status = 200, description = "An authorization URL for linking the provider to the signed-in account, plus the `palmr_oauth` browser-binding cookie. The request is bound to the caller and returns to `/settings/security`. No body is accepted and nothing is linked until the callback validates the provider identity.", body = AuthorizeResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "`AUTH_RECENT_AUTH_REQUIRED`, `PROVIDER_DISABLED` (the provider is disabled or the global provider toggle is off), the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`PROVIDER_IDENTITY_ALREADY_LINKED`: the account already has an identity from this provider. Nothing is persisted and no cookie is set.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an unusable provider configuration.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn link_provider(
    Extension(service): Extension<ExternalLoginService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.start_link(&principal, &slug).await {
        Ok(authorized) => {
            let mut response = json_response(
                StatusCode::OK,
                &AuthorizeResponse {
                    authorization_url: authorized.authorization_url,
                },
                request_id.as_ref(),
            );
            if service
                .cookie_policy()
                .append_oauth_binding(
                    response.headers_mut(),
                    &authorized.binding,
                    AUTH_REQUEST_TTL_SECONDS,
                )
                .is_err()
            {
                return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
            }
            response
        }
        Err(error) => link_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/identity-links",
    tag = "identity-links",
    responses(
        (status = 200, description = "The caller's own external identity links, oldest first with a stable tie-break. No provider configuration, client id, secret, token or state is ever included. `totalCount` is exact.", body = Page<IdentityLinkItem>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_identity_links(
    Extension(service): Extension<ExternalLoginService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let page = match service.link_query(raw_query.as_deref()) {
        Ok(page) => page,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.list_links(principal.user_id, page).await {
        Ok(page) => json_response(StatusCode::OK, &page, request_id.as_ref()),
        Err(error) => link_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/identity-links/{id}",
    tag = "identity-links",
    params(("id" = String, Path, description = "Identity-link UUIDv7")),
    responses(
        (status = 204, description = "The identity link is removed and every session and trusted device of the caller is revoked in the same transaction, including the current session. `palmr_session`, `palmr_csrf` and `palmr_device` are expired."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "`AUTH_RECENT_AUTH_REQUIRED`, the session is restricted, or the CSRF proof or origin is not allowed.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_LINK_NOT_FOUND`: the id is unknown, malformed or belongs to another account; the cases are indistinguishable.", body = ApiErrorBody),
        (status = 409, description = "`IDENTITY_LINK_LAST_LOGIN_PATH`: the account has no local password and this is its only identity link. Nothing changes.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn unlink_identity(
    Extension(service): Extension<ExternalLoginService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(link) = id.parse::<IdentityLinkId>() else {
        return link_error(
            &ExternalLoginError::refused(
                crate::domain::error_code::ErrorCode::ProviderLinkNotFound,
            ),
            request_id.as_ref(),
        );
    };
    unlink_response(
        &service,
        UnlinkCommand {
            target: principal.user_id,
            actor: &principal,
            link,
            scope: UnlinkScope::SelfService,
            client: &client_metadata(&request),
        },
        request_id.as_ref(),
    )
    .await
}

pub async fn unlink_response(
    service: &ExternalLoginService,
    command: UnlinkCommand<'_>,
    request_id: Option<&RequestId>,
) -> Response {
    match service.unlink(command).await {
        Ok(unlinked) => no_content(service, unlinked.actor_credentials_revoked, request_id),
        Err(error) => link_error(&error, request_id),
    }
}

pub fn parse_user_id(id: &str) -> Option<UserId> {
    id.parse().ok()
}

pub fn no_content(
    service: &ExternalLoginService,
    clear_credentials: bool,
    request_id: Option<&RequestId>,
) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    if clear_credentials && expire_credentials(service, response.headers_mut()).is_err() {
        return tag_error(ApiError::internal(), request_id).into_response();
    }
    response
}

fn expire_credentials(
    service: &ExternalLoginService,
    headers: &mut HeaderMap,
) -> Result<(), crate::infra::http::cookies::CookieError> {
    let policy = service.cookie_policy();
    policy.expire_session_pair(headers)?;
    policy.expire_device(headers)
}

pub fn link_error(error: &ExternalLoginError, request_id: Option<&RequestId>) -> Response {
    if error.is_server_fault() {
        tracing::error!(kind = error.kind(), "identity link request failed");
    }
    tag_error(ApiError::new(error.code()), request_id).into_response()
}

pub fn json_response<T: serde::Serialize>(
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
