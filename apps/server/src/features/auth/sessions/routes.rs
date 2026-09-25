use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CONTENT_TYPE, USER_AGENT};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::audit::model::ClientMetadata;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, AuthenticatedRecentAuth};
use crate::infra::http::pagination::{Page, QueryParams};
use crate::infra::http::proxy::ResolvedClient;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::model::{RevokedReason, SessionId, SessionItem};
use super::{SessionError, SessionService};

const LIST: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const REVOKE_ONE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const REVOKE_ALL: RoutePolicy = RoutePolicy::new(
    AuthClass::AuthenticatedRecentAuth,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LIST, routes!(list_sessions))
        .route(REVOKE_ONE, routes!(revoke_session))
        .route(REVOKE_ALL, routes!(revoke_sessions))
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    tag = "sessions",
    params(
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor."),
        ("limit" = Option<u16>, Query, minimum = 1, maximum = 200, description = "Page size."),
        ("sort" = Option<String>, Query, description = "Sort: lastSeenAt:asc|desc")
    ),
    responses(
        (status = 200, description = "The caller's active sessions.", body = Page<SessionItem>),
        (status = 400, description = "Invalid cursor or query.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
    )
)]
async fn list_sessions(
    Extension(service): Extension<SessionService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let page = match service.page_request(raw_query.as_deref()) {
        Ok(page) => page,
        Err(error) => {
            return tag_error(error, RequestId::of(&request).as_ref()).into_response();
        }
    };
    match service.list(&principal, page).await {
        Ok(page) => json_response(StatusCode::OK, &page),
        Err(error) => session_error(error, &request),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{id}",
    tag = "sessions",
    params(("id" = String, Path, description = "Session UUIDv7")),
    responses(
        (status = 204, description = "Session revoked."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 404, description = "Unknown or foreign session.", body = ApiErrorBody),
    )
)]
async fn revoke_session(
    Extension(service): Extension<SessionService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Ok(id) = id.parse::<SessionId>() else {
        return session_error(SessionError::NotFound, &request);
    };
    match service
        .revoke_one(
            &principal,
            id,
            RevokedReason::UserRequest,
            &client_metadata(&request),
        )
        .await
    {
        Ok(current) => no_content(&service, current),
        Err(error) => session_error(error, &request),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions",
    tag = "sessions",
    params(("includeCurrent" = Option<bool>, Query, description = "Also revoke the current session.")),
    responses(
        (status = 204, description = "Sessions revoked."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Recent authentication required.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
    )
)]
async fn revoke_sessions(
    Extension(service): Extension<SessionService>,
    AuthenticatedRecentAuth(principal): AuthenticatedRecentAuth,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let include_current = match include_current(raw_query.as_deref()) {
        Ok(value) => value,
        Err(error) => {
            return tag_error(error, RequestId::of(&request).as_ref()).into_response();
        }
    };
    let client = client_metadata(&request);
    let result = if include_current {
        service
            .revoke_all(&principal, RevokedReason::UserRequest, &client)
            .await
    } else {
        service
            .revoke_all_others(&principal, RevokedReason::UserRequest, &client)
            .await
    };
    match result {
        Ok(_) => no_content(&service, include_current),
        Err(error) => session_error(error, &request),
    }
}

fn include_current(raw_query: Option<&str>) -> Result<bool, ApiError> {
    let params = QueryParams::parse(raw_query);
    let Some(value) = params.single("includeCurrent")? else {
        return Ok(false);
    };
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(crate::infra::http::pagination::invalid_param(
            "includeCurrent",
        )),
    }
}

fn client_metadata(request: &Request) -> ClientMetadata {
    let user_agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok());
    request
        .extensions()
        .get::<ResolvedClient>()
        .map_or_else(ClientMetadata::none, |client| {
            ClientMetadata::from_request(client, RequestId::of(request).as_ref(), user_agent)
        })
}

fn no_content(service: &SessionService, clear_current: bool) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    if clear_current
        && service
            .cookie_policy()
            .expire_session_pair(response.headers_mut())
            .is_err()
    {
        return ApiError::internal().into_response();
    }
    response
}

fn json_response<T: serde::Serialize>(status: StatusCode, body: &T) -> Response {
    match serde_json::to_vec(body) {
        Ok(body) => (
            status,
            [(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))],
            body,
        )
            .into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}

fn session_error(error: SessionError, request: &Request) -> Response {
    if matches!(
        error,
        SessionError::RepositoryInvariant { .. }
            | SessionError::Audit(_)
            | SessionError::Crypto(_)
            | SessionError::Cookie(_)
            | SessionError::Db(_)
            | SessionError::Time(_)
    ) {
        tracing::error!(kind = error.kind(), "session request failed");
    }
    tag_error(error.api_error(), RequestId::of(request).as_ref()).into_response()
}
