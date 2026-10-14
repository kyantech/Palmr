use axum::extract::{Extension, Path, Request};
use axum::response::{IntoResponse, Response};
use http::header::CACHE_CONTROL;
use http::StatusCode;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::infra::http::error::ApiErrorBody;
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::super::cancel::TransferCaller;
use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferSessionId};
use super::super::routes::{json_response, transfer_error, NO_STORE};
use super::model::{
    CompleteResponse, CreateMultipartResponse, PartsResponse, PresignRequest, PresignResponse,
    ReportPartsRequest,
};
use super::service::S3Service;

const CONTROL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferControl,
    Transport::ControlPlane,
);

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(CONTROL_ROUTE, routes!(create_multipart))
        .route(CONTROL_ROUTE, routes!(presign_parts))
        .route(CONTROL_ROUTE, routes!(report_parts, list_parts))
        .route(CONTROL_ROUTE, routes!(complete_multipart))
        .route(CONTROL_ROUTE, routes!(abort_multipart))
}

fn ids(id: &str, item_id: &str) -> Option<(TransferSessionId, SessionItemId)> {
    Some((id.parse().ok()?, item_id.parse().ok()?))
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 201, description = "The multipart upload now exists at the storage provider. The part plan was computed from the declared size before any byte moved; `multipartId` is Palmr's handle, never the provider's upload id. No bucket, object key or provider upload id is ever returned. `checksumHeadersRequired` is `true` only when the storage profile demands checksum headers a browser cannot produce: parts then use the server-proxied path and no part URL is issued.", body = CreateMultipartResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, the session is restricted, or the account is deactivated (`AUTH_ACCOUNT_INACTIVE`).", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND` for an unknown session or item, including another user's; `FOLDER_NOT_FOUND` when the target folder is gone.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_ALREADY_CREATED` when the item already has a multipart upload (read it with `GET …/parts`), or `TRANSFER_SESSION_STATE_INVALID` when the item does not use S3 multipart, is not startable or its session is terminal; `FOLDER_DELETING` when the target is being deleted.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`: the session passed its expiry.", body = ApiErrorBody),
        (status = 413, description = "`FILE_TOO_LARGE`: the declared size no longer fits the effective maximum file size or the storage provider.", body = ApiErrorBody),
        (status = 422, description = "`MULTIPART_PART_INVALID` with `details.reason`: the size is unknown, which needs the server-mediated upload path.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_UPSTREAM_ERROR`: the storage provider rejected the request.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`: the storage backend is unreachable.", body = ApiErrorBody),
    )
)]
async fn create_multipart(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::SessionNotFound, request_id.as_ref());
    };
    match service.create(principal.user_id, id, item_id).await {
        Ok(view) => json_response(StatusCode::CREATED, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/presign",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    request_body(content = PresignRequest, content_type = "application/json", description = "One to 16 distinct part numbers in `1..=partCount`. Ask again at any time for any part that is not uploaded yet: signing is local, unmetered and never cached on the server."),
    responses(
        (status = 200, description = "A presigned `PUT` URL for each requested part, valid for 15 minutes and signed against the public storage endpoint. The URL signs only `host`: send the part bytes and no other header. Read the part's `ETag` from the response and report it. Never store a URL.", body = PresignResponse),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`MULTIPART_NOT_FOUND`: unknown, aborted or another user's upload.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_ALREADY_COMPLETED`, or `TRANSFER_SESSION_STATE_INVALID` while the upload is being completed or its session is terminal.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`PRESIGN_BATCH_TOO_LARGE` for more than 16 part numbers; `MULTIPART_PART_INVALID` for a number outside `1..=partCount`, a repeated or malformed number, or when the storage profile needs the server-proxied part path; `VALIDATION_ERROR` for an empty or malformed body.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_UPSTREAM_ERROR`.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`.", body = ApiErrorBody),
    )
)]
async fn presign_parts(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::MultipartNotFound, request_id.as_ref());
    };
    let body = match json::read::<PresignRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.presign(principal.user_id, id, item_id, body).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/parts",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    request_body(content = ReportPartsRequest, content_type = "application/json", description = "Parts the storage provider acknowledged, with each `ETag` verbatim. These are hints that let progress survive a reload: the provider's `ListParts` decides which parts exist, and a reported part the provider does not hold is discarded when the upload is reconciled."),
    responses(
        (status = 204, description = "The hints are recorded. Repeating a report is safe."),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`MULTIPART_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_ALREADY_COMPLETED`, or `TRANSFER_SESSION_STATE_INVALID`.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`MULTIPART_PART_INVALID` for a part outside the plan, with an unusable `ETag` or of the wrong size; `VALIDATION_ERROR` for an empty or malformed body.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn report_parts(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::MultipartNotFound, request_id.as_ref());
    };
    let body = match json::read::<ReportPartsRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.report(principal.user_id, id, item_id, body).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/parts",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 200, description = "The storage provider's authoritative view of the upload, read from `ListParts` across every page and written back to Palmr's cache. `uploadedBytes` is the sum of the provider-reported part sizes. Only `missingPartNumbers` need to be signed and uploaded again; a held part of the wrong size is listed in `completedParts` with its real size.", body = PartsResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`MULTIPART_NOT_FOUND`, including another user's upload and an upload the provider no longer holds.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_ALREADY_COMPLETED`, or `TRANSFER_SESSION_STATE_INVALID`.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_UPSTREAM_ERROR`.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`.", body = ApiErrorBody),
    )
)]
async fn list_parts(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::MultipartNotFound, request_id.as_ref());
    };
    match service.list(principal.user_id, id, item_id).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/complete",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 200, description = "The item is committed as a file. The body carries no manifest: Palmr rebuilds it from the provider's `ListParts`, completes the upload in strictly ascending part order, measures the object with `HeadObject` and commits exactly once. A zero-byte item is written by the server with an empty `PutObject` and completed here too. Completing a completed item returns the original file again.", body = CompleteResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, the session is restricted, or the account is deactivated.", body = ApiErrorBody),
        (status = 404, description = "`MULTIPART_NOT_FOUND`, including another user's upload; `FOLDER_NOT_FOUND` when the target folder is gone.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_INCOMPLETE` with `details.missingPartNumbers`; `TRANSFER_SESSION_STATE_INVALID` for a canceled item or session; `FOLDER_DELETING`.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 413, description = "`FILE_TOO_LARGE`: the object exceeds the effective maximum file size. The object is tombstoned.", body = ApiErrorBody),
        (status = 422, description = "`MULTIPART_PART_INVALID`: a part has the wrong size or the provider refused one; `UPLOAD_LENGTH_MISMATCH`: the measured object size differs from the declared size and the object is tombstoned.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_UPSTREAM_ERROR`. Completion is durable and resumes: ask again or let the server re-drive it.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`, or `TRANSFER_BACKPRESSURE` while another request is still completing this item. Retry.", body = ApiErrorBody),
        (status = 507, description = "`QUOTA_EXCEEDED`: the measured size passes the quota. The object is tombstoned and nothing is charged.", body = ApiErrorBody),
    )
)]
async fn complete_multipart(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::MultipartNotFound, request_id.as_ref());
    };
    let tag = request_id.as_ref().map(|id| id.as_str().to_owned());
    match service.complete(principal.user_id, id, item_id, tag).await {
        Ok(view) => json_response(StatusCode::OK, &view, request_id.as_ref()),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/abort",
    tag = "transfers",
    params(
        ("sid" = String, Path, description = "Transfer session UUIDv7"),
        ("itemId" = String, Path, description = "Transfer item UUIDv7")
    ),
    responses(
        (status = 204, description = "The item is canceled, its share of the quota reservation is released and the provider upload is aborted. A provider that no longer knows the upload counts as aborted. Aborting twice is also `204`. A committed file is never touched."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`MULTIPART_NOT_FOUND`, including another user's upload.", body = ApiErrorBody),
        (status = 409, description = "`MULTIPART_ALREADY_COMPLETED`: the item was committed first and stays a file; `TRANSFER_SESSION_STATE_INVALID`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_UPSTREAM_ERROR`: the cancellation is recorded and the abort is retried by the abandoned-upload cleanup; call again to retry at once.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`: as `502`.", body = ApiErrorBody),
    )
)]
async fn abort_multipart(
    Extension(service): Extension<S3Service>,
    Authenticated(principal): Authenticated,
    Path((id, item_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Some((id, item_id)) = ids(&id, &item_id) else {
        return transfer_error(&TransferError::MultipartNotFound, request_id.as_ref());
    };
    let caller = TransferCaller {
        owner: principal.user_id,
        username: principal.username.clone(),
        client: client_metadata(&request),
    };
    match service.abort(&caller, id, item_id).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => transfer_error(&error, request_id.as_ref()),
    }
}
