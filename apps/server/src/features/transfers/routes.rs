use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{IdempotencyMode, RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::idempotency::{Admission, Claim, IdempotencyRequest};
use crate::infra::http::json;
use crate::infra::http::pagination::Page;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::TransferError;
use super::model::{
    CreateTransferSessionRequest, SessionItemId, TransferFileView, TransferSessionId,
    TransferSessionSummary, TransferSessionView, ValidatedSession,
};
use super::service::TransferService;

const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const CONTROL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferControl,
    Transport::ControlPlane,
);

const CREATE_ROUTE: RoutePolicy = CONTROL_ROUTE.with_idempotency(IdempotencyMode::Plaintext);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(CREATE_ROUTE, routes!(create_session))
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_sessions), &TransferService::list_parameters()),
        )
        .route(READ_ROUTE, routes!(get_session))
        .route(WRITE_ROUTE, routes!(cancel_session))
        .route(CONTROL_ROUTE, routes!(complete_session))
        .route(CONTROL_ROUTE, routes!(retry_item))
        .route(WRITE_ROUTE, routes!(cancel_item))
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions",
    tag = "transfers",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "16–128 characters. A replay within 24 hours returns the original `201` and body with `Idempotency-Replayed: true` without creating a second session or reserving quota again.")
    ),
    request_body(
        content = CreateTransferSessionRequest,
        content_type = "application/json",
        description = "Declares the files of one upload. Every pre-flight check runs in one transaction before any byte moves: the account is active, the target folder is yours and not being deleted, each declared size fits the effective maximum file size and the storage provider, the S3 part plan is computable, any directories named by `relativePath` are resolved or created, and the quota covering the declared total is reserved. The server allocates every storage identity and never returns it. The target folder is fixed for the life of the session."
    ),
    responses(
        (status = 201, description = "The session, in state `created`, with one item per declared file and the protocol to upload it with. No upload resource exists yet: S3 part sizes and counts are plan metadata only.", body = TransferSessionView),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, the session is restricted, or the account is deactivated (`AUTH_ACCOUNT_INACTIVE`).", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the target folder is unknown, malformed or belongs to another user.", body = ApiErrorBody),
        (status = 409, description = "`FOLDER_DELETING` for a target or directory that is being deleted, or `IDEMPOTENCY_KEY_CONFLICT` / `IDEMPOTENCY_REQUEST_IN_PROGRESS` for a reused key.", body = ApiErrorBody),
        (status = 413, description = "`FILE_TOO_LARGE`: a declared size exceeds the effective maximum file size or what the storage provider can store. `details` carries `itemClientId`, `declaredBytes`, `reason` and the applicable `maxBytes` and `providerMaxObjectBytes`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`BATCH_TOO_LARGE` for more than 2 000 files, `NAME_INVALID` for an invalid file name, `FOLDER_DEPTH_EXCEEDED` when a directory chain would pass depth 64, or `VALIDATION_ERROR` for any other invalid field, including a duplicate `clientId`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`: the storage backend is down.", body = ApiErrorBody),
        (status = 507, description = "`QUOTA_EXCEEDED`: used bytes plus held reservations plus this request would pass the effective quota. Not retryable.", body = ApiErrorBody),
    )
)]
async fn create_session(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    idempotency: IdempotencyRequest,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
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
    let parsed = match json::parse::<CreateTransferSessionRequest>(body) {
        Ok(parsed) => parsed,
        Err(error) => {
            release(&service, claim).await;
            return tag_error(error, request_id.as_ref()).into_response();
        }
    };
    let created = match ValidatedSession::parse(parsed) {
        Ok(validated) => service.create(principal.user_id, validated, &claim).await,
        Err(error) => Err(error),
    };
    match created {
        Ok(envelope) => {
            let mut response = envelope.into_response();
            response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
            response
        }
        Err(error) => {
            release(&service, claim).await;
            transfer_error(&error, request_id.as_ref())
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/transfers/sessions",
    tag = "transfers",
    responses(
        (status = 200, description = "The caller's transfer sessions, newest first, as summaries without their files. Server state is authoritative after a reload: fetch a session for its files. `state` may repeat and accepts only the durable server states.", body = Page<TransferSessionSummary>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for a client-only or unknown `state`, or an invalid `limit`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_sessions(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let query = match service.list_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.list(principal.user_id, query).await {
        Ok(page) => json_response(StatusCode::OK, &page, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/transfers/sessions/{id}",
    tag = "transfers",
    params(("id" = String, Path, description = "Transfer session UUIDv7")),
    responses(
        (status = 200, description = "The session and its files with persisted, authoritative progress. Progress is `0` until a protocol resource records bytes; nothing here is read from storage. `fileId` is non-null only for a `completed` item whose file exists.", body = TransferSessionView),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND`, including another user's session.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_session(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<TransferSessionId>() else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.detail(principal.user_id, id).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/transfers/sessions/{id}",
    tag = "transfers",
    params(("id" = String, Path, description = "Transfer session UUIDv7")),
    responses(
        (status = 204, description = "The session is canceled. Every unfinished item is canceled and the quota still held for them is released in the same transaction; items that already completed stay as ordinary content. Canceling an already canceled session is also `204`. Physical upload resources are marked for the upload adapters to clean up; nothing is deleted from storage here."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND`, including another user's session.", body = ApiErrorBody),
        (status = 409, description = "`TRANSFER_SESSION_STATE_INVALID`: the session is `completed` or `expired`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn cancel_session(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<TransferSessionId>() else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.cancel(principal.user_id, id).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{id}/complete",
    tag = "transfers",
    params(("id" = String, Path, description = "Transfer session UUIDv7")),
    responses(
        (status = 200, description = "The closed session. Completing is a close, not a finalization: it succeeds only when no item is still in flight and the persisted completed items agree with the session's totals, then settles the quota reservation. Completing an already completed session returns it again.", body = TransferSessionView),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND`, including another user's session.", body = ApiErrorBody),
        (status = 409, description = "`TRANSFER_SESSION_STATE_INVALID`: an item is still in flight, or the session is `canceled` or `expired`.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`: the session passed its expiry while not completed.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn complete_session(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<TransferSessionId>() else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.complete(principal.user_id, id).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{id}/files/{itemId}/retry",
    tag = "transfers",
    params(
        ("id" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 200, description = "The item, moved from `failed` back to `uploading` with its attempt count incremented. The declared set is unchanged, so quota is not admitted again. Only an item whose upload resource still exists can be retried.", body = TransferFileView),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND` for an unknown session or item, including another user's.", body = ApiErrorBody),
        (status = 409, description = "`TRANSFER_SESSION_STATE_INVALID`: the session is terminal, the item is not `failed`, or the item has no upload resource to resume.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`: the session passed its expiry.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn retry_item(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let (Ok(id), Ok(item_id)) = (
        id.parse::<TransferSessionId>(),
        item_id.parse::<SessionItemId>(),
    ) else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.retry_item(principal.user_id, id, item_id).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/transfers/sessions/{id}/files/{itemId}",
    tag = "transfers",
    params(
        ("id" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 204, description = "The item is canceled and its share of the reservation is released. Canceling an already canceled item is also `204`. When no item is left the session is canceled as well. A completed item is content and is never removed here."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND` for an unknown session or item, including another user's.", body = ApiErrorBody),
        (status = 409, description = "`TRANSFER_SESSION_STATE_INVALID`: the item is `completed` or `expired`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn cancel_item(
    Extension(service): Extension<TransferService>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let (Ok(id), Ok(item_id)) = (
        id.parse::<TransferSessionId>(),
        item_id.parse::<SessionItemId>(),
    ) else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.cancel_item(principal.user_id, id, item_id).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

async fn release(service: &TransferService, claim: Claim) {
    if let Err(error) = service.idempotency().release(claim).await {
        tracing::error!(
            kind = error.kind(),
            "transfer idempotency claim could not be released"
        );
    }
}

fn transfer_error(error: &TransferError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "transfer request failed");
    }
    tag_error(api_error, request_id).into_response()
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
