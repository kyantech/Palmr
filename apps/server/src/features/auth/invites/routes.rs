use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{IdempotencyMode, RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::model::LoginResponse;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::SessionService;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Admin;
use crate::infra::http::idempotency::{Admission, IdempotencyRequest};
use crate::infra::http::json;
use crate::infra::http::pagination::Page;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::InviteError;
use super::model::{
    AcceptInput, AcceptInviteRequest, CreateInput, CreateInviteRequest, CreateInviteResponse,
    InviteId, InviteItem, InviteLookupResponse, PresentedInvite,
};
use super::service::{AcceptContext, InviteService};

pub const LIST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const CREATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
)
.with_idempotency(IdempotencyMode::Sealed);

pub const RESEND_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::EmailTest,
    Transport::ControlPlane,
);

pub const REVOKE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const LOOKUP_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthToken,
    Transport::ControlPlane,
);

pub const ACCEPT_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthToken,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LIST_ROUTE, routes!(list_invites))
        .route(CREATE_ROUTE, routes!(create_invite))
        .route(RESEND_ROUTE, routes!(resend_invite))
        .route(REVOKE_ROUTE, routes!(revoke_invite))
        .route(LOOKUP_ROUTE, routes!(lookup_invite))
        .route(ACCEPT_ROUTE, routes!(accept_invite))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/invites",
    tag = "invites",
    params(
        ("status" = Option<String>, Query, description = "Filter: pending|accepted|revoked|expired. A pending invite past its expiry is reported as expired."),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor."),
        ("limit" = Option<u16>, Query, minimum = 1, maximum = 200, description = "Page size."),
        ("sort" = Option<String>, Query, description = "Sort: createdAt:asc|desc")
    ),
    responses(
        (status = 200, description = "Invites, newest first. The token, its digest and its sealed copy are never part of this response.", body = Page<InviteItem>),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
    )
)]
async fn list_invites(
    Extension(service): Extension<InviteService>,
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
        Ok(page) => json_response(StatusCode::OK, &page, request_id.as_ref()),
        Err(error) => invite_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/invites",
    tag = "invites",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "16–128 characters. A replay within 24 hours returns the original response from a sealed record without creating another invite.")
    ),
    request_body(
        content = CreateInviteRequest,
        content_type = "application/json",
        description = "The body carries no URL of any kind: the invite link is always the configured public base URL plus `/invite/{token}`."
    ),
    responses(
        (status = 201, description = "The invite is created. `inviteUrl` is returned only here (and on a keyed replay) so it can be delivered by hand.", body = CreateInviteResponse),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 409, description = "`USER_EMAIL_TAKEN` when an account or a pending invite already holds the address; `FEATURE_UNAVAILABLE_SMTP` when `sendEmail` is true and SMTP is not configured; `IDEMPOTENCY_KEY_CONFLICT` or `IDEMPOTENCY_REQUEST_IN_PROGRESS` for a reused key.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn create_invite(
    Extension(service): Extension<InviteService>,
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
    let created = match json::parse::<CreateInviteRequest>(body) {
        Ok(parsed) => match CreateInput::parse(parsed) {
            Ok(input) => service.create(&admin, input, &claim, &client).await,
            Err(error) => Err(error),
        },
        Err(error) => {
            release(&service, claim).await;
            return tag_error(error, request_id.as_ref()).into_response();
        }
    };
    match created {
        Ok(created) => json_response(StatusCode::CREATED, &created, request_id.as_ref()),
        Err(error) => {
            release(&service, claim).await;
            invite_error(&error, request_id.as_ref())
        }
    }
}

async fn release(service: &InviteService, claim: crate::infra::http::idempotency::Claim) {
    if let Err(error) = service.idempotency().release(claim).await {
        tracing::error!(
            kind = error.kind(),
            "invite idempotency claim could not be released"
        );
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/invites/{id}/resend",
    tag = "invites",
    params(("id" = String, Path, description = "Invite UUIDv7")),
    responses(
        (status = 202, description = "The original token is queued for delivery again; the expiry is unchanged."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 404, description = "`INVITE_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`FEATURE_UNAVAILABLE_SMTP` when SMTP is not configured.", body = ApiErrorBody),
        (status = 410, description = "`INVITE_ALREADY_USED`, `INVITE_REVOKED` or `INVITE_EXPIRED`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn resend_invite(
    Extension(service): Extension<InviteService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<InviteId>() else {
        return invite_error(&InviteError::NotFound, request_id.as_ref());
    };
    match service.resend(&admin, id).await {
        Ok(()) => status_only(StatusCode::ACCEPTED),
        Err(error) => invite_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/invites/{id}",
    tag = "invites",
    params(("id" = String, Path, description = "Invite UUIDv7")),
    responses(
        (status = 204, description = "The invite is revoked and its sealed token copy wiped. An invite that is already accepted, revoked or expired is left unchanged."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 404, description = "`INVITE_NOT_FOUND`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn revoke_invite(
    Extension(service): Extension<InviteService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<InviteId>() else {
        return invite_error(&InviteError::NotFound, request_id.as_ref());
    };
    match service.revoke(&admin, id, &client).await {
        Ok(()) => status_only(StatusCode::NO_CONTENT),
        Err(error) => invite_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/public/invites/{token}",
    tag = "invites",
    params(("token" = String, Path, description = "The opaque token from the `/invite/{token}` link.")),
    responses(
        (status = 200, description = "The invite is live. Only the bound e-mail, the password policy and the expiry are disclosed.", body = InviteLookupResponse),
        (status = 404, description = "`INVITE_NOT_FOUND` for a malformed or unknown token.", body = ApiErrorBody),
        (status = 410, description = "`INVITE_EXPIRED`, `INVITE_ALREADY_USED` or `INVITE_REVOKED`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn lookup_invite(
    Extension(service): Extension<InviteService>,
    Path(token): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let result = match PresentedInvite::parse(&token) {
        Ok(token) => service.lookup(&token).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(found) => json_response(StatusCode::OK, &found, request_id.as_ref()),
        Err(error) => invite_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/public/invites/{token}/accept",
    tag = "invites",
    params(("token" = String, Path, description = "The opaque token from the `/invite/{token}` link.")),
    request_body(
        content = AcceptInviteRequest,
        content_type = "application/json",
        description = "The account's e-mail address and role come from the invite and cannot be supplied here."
    ),
    responses(
        (status = 201, description = "The account is created and signed in; `palmr_session` and `palmr_csrf` are set. The invite is consumed in the same transaction.", body = LoginResponse),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 404, description = "`INVITE_NOT_FOUND` for a malformed or unknown token.", body = ApiErrorBody),
        (status = 409, description = "`USER_USERNAME_TAKEN` or `USER_EMAIL_TAKEN`; the invite stays usable.", body = ApiErrorBody),
        (status = 410, description = "`INVITE_EXPIRED`, `INVITE_ALREADY_USED` or `INVITE_REVOKED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`PASSWORD_POLICY_VIOLATION` with `details.minLength`, or the request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn accept_invite(
    Extension(service): Extension<InviteService>,
    Path(token): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let context = AcceptContext {
        session: SessionService::client(&request),
        audit: client_metadata(&request),
        presented_session: SessionService::presented_token(request.headers()),
    };
    let token = match PresentedInvite::parse(&token) {
        Ok(token) => token,
        Err(error) => return invite_error(&error, request_id.as_ref()),
    };
    let input = match json::read::<AcceptInviteRequest>(request.into_body()).await {
        Ok(body) => match AcceptInput::parse(body) {
            Ok(input) => input,
            Err(error) => return invite_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let accepted = match service.accept(&token, input, &context).await {
        Ok(accepted) => accepted,
        Err(error) => return invite_error(&error, request_id.as_ref()),
    };
    let mut response = json_response(StatusCode::CREATED, &accepted.response, request_id.as_ref());
    if let Err(error) = service
        .auth()
        .sessions()
        .emit_cookies(response.headers_mut(), &accepted.session)
    {
        tracing::error!(
            kind = error.kind(),
            "invite session cookies could not be emitted"
        );
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

fn invite_error(error: &InviteError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "invite request failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn status_only(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    response
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
