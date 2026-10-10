use axum::extract::{Extension, Path, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::{CONTENT_LENGTH, LOCATION, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{
    BytePath, Deadline, RateLimitClass, RequestBody, ResponseEncoding, RoutePolicy, Routes,
    Transport,
};
use crate::app::state::AppState;
use crate::infra::http::csrf::RequestContent;
use crate::infra::http::error::ApiErrorBody;
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::TusError;
use super::headers::{
    self, imf_fixdate, method_override, no_store, number_header, protocol_header, text_header,
    MethodOverride, CHECKSUM_ALGORITHM, EXTENSIONS, PROTOCOL_VERSION, TUS_CHECKSUM_ALGORITHM,
    TUS_EXTENSION, TUS_MAX_SIZE, TUS_VERSION, UPLOAD_DEFER_LENGTH, UPLOAD_EXPIRES, UPLOAD_LENGTH,
    UPLOAD_OFFSET,
};
use super::metadata::UploadMetadata;
use super::repo::TusUploadId;
use super::service::{CreateInput, TusService};

const CONTROL_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferControl,
    Transport::ControlPlane,
)
.with_request_content(RequestContent::OffsetOctetStream)
.with_tus_protocol();

const WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
)
.with_request_content(RequestContent::OffsetOctetStream)
.with_tus_protocol();

const CREATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferControl,
    Transport::BytePath(BytePath::new(
        RequestBody::Streamed,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
)
.with_request_content(RequestContent::OffsetOctetStream)
.with_tus_protocol();

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(CONTROL_ROUTE, routes!(options_capabilities))
        .route(CREATE_ROUTE, routes!(create_upload))
        .route(CONTROL_ROUTE, routes!(head_upload))
        .route(WRITE_ROUTE, routes!(terminate_upload))
        .route(CONTROL_ROUTE, routes!(override_upload))
}

pub async fn protocol_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let (name, value) = protocol_header();
    response.headers_mut().entry(name).or_insert(value);
    let (name, value) = no_store();
    response.headers_mut().entry(name).or_insert(value);
    response
}

fn failure(error: &TusError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "upload request failed");
    }
    let mut response = tag_error(api_error, request_id).into_response();
    if matches!(error, TusError::VersionUnsupported) {
        response
            .headers_mut()
            .insert(TUS_VERSION, HeaderValue::from_static(PROTOCOL_VERSION));
    }
    response
}

fn upload_id(raw: &str) -> Result<TusUploadId, TusError> {
    raw.parse().map_err(|_| TusError::NotFound)
}

fn carries_body(headers: &HeaderMap) -> bool {
    let sized = headers
        .get(CONTENT_LENGTH)
        .is_some_and(|value| value.as_bytes() != b"0");
    sized || headers.contains_key(TRANSFER_ENCODING)
}

#[utoipa::path(
    options,
    path = "/api/v1/uploads/tus",
    tag = "uploads",
    responses(
        (status = 204, description = "The TUS 1.0 capabilities of this instance. `Tus-Max-Size` is the effective maximum file size and is omitted when no limit applies. `concatenation` is never advertised. The request does not need `Tus-Resumable`.",
            headers(
                ("Tus-Resumable" = String, description = "`1.0.0`."),
                ("Tus-Version" = String, description = "`1.0.0`."),
                ("Tus-Extension" = String, description = "`creation,creation-with-upload,expiration,termination,checksum`."),
                ("Tus-Checksum-Algorithm" = String, description = "`sha256`."),
                ("Tus-Max-Size" = Option<i64>, description = "Effective maximum file size in bytes; absent when unlimited."),
            )),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`NOT_FOUND`: this instance does not store uploads locally.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn options_capabilities(
    Extension(service): Extension<TusService>,
    Authenticated(_principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.capabilities() {
        Err(error) => failure(&error, request_id.as_ref()),
        Ok(capabilities) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            let headers = response.headers_mut();
            headers.insert(TUS_VERSION, HeaderValue::from_static(PROTOCOL_VERSION));
            headers.insert(TUS_EXTENSION, HeaderValue::from_static(EXTENSIONS));
            headers.insert(
                TUS_CHECKSUM_ALGORITHM,
                HeaderValue::from_static(CHECKSUM_ALGORITHM),
            );
            if let Some(max) = capabilities.max_size {
                headers.insert(TUS_MAX_SIZE, number_header(max.get()));
            }
            response
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/uploads/tus",
    tag = "uploads",
    params(
        ("Tus-Resumable" = String, Header, description = "Must be `1.0.0`."),
        ("Upload-Length" = Option<i64>, Header, description = "The total size in bytes. Exactly one of `Upload-Length` and `Upload-Defer-Length` is required."),
        ("Upload-Defer-Length" = Option<i64>, Header, description = "`1` when the size is not known yet. Allowed only for a file planned without a size."),
        ("Upload-Metadata" = String, Header, description = "TUS metadata, at most 8 KiB: `filename`, `transferSessionId` and `itemId` are required; `filetype` is advisory; `relativePath` is required for a file inside a directory; `folderId`, when present, must be the session's target folder. Unknown keys are ignored. `filename` and `relativePath` must match the planned item and are never used as paths."),
    ),
    request_body(
        content_type = "application/offset+octet-stream",
        description = "Optional initial bytes (`creation-with-upload`), streamed through a bounded buffer and never collected."
    ),
    responses(
        (status = 201, description = "The upload resource exists for the planned item. A repeated creation for the same item returns the existing resource. `Upload-Offset` is the number of bytes durably persisted.",
            headers(
                ("Tus-Resumable" = String, description = "`1.0.0`."),
                ("Location" = String, description = "Absolute URL of the upload, built from the configured public base URL."),
                ("Upload-Expires" = String, description = "RFC 7231 IMF-fixdate."),
                ("Upload-Offset" = i64, description = "Bytes durably persisted."),
                ("Cache-Control" = String, description = "`no-store`."),
            )),
        (status = 400, description = "`UPLOAD_METADATA_INVALID`: a header is missing or malformed; `details.key` names it.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`TRANSFER_SESSION_NOT_FOUND`: the session or item is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 408, description = "`TRANSFER_IDLE_TIMEOUT`: no body frame arrived within the per-frame idle timeout (never a function of file size); the persisted offset is kept and the upload can be resumed.", body = ApiErrorBody),
        (status = 409, description = "`TRANSFER_SESSION_STATE_INVALID` or `FOLDER_DELETING`.", body = ApiErrorBody),
        (status = 410, description = "`TRANSFER_SESSION_EXPIRED` or `UPLOAD_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 412, description = "`TUS_VERSION_UNSUPPORTED`; the response carries `Tus-Version`.", body = ApiErrorBody),
        (status = 413, description = "`FILE_TOO_LARGE`: the length exceeds the effective maximum, or the body exceeded the declared length.", body = ApiErrorBody),
        (status = 415, description = "`UNSUPPORTED_MEDIA_TYPE`: the body is not `application/offset+octet-stream`.", body = ApiErrorBody),
        (status = 422, description = "`UPLOAD_LENGTH_MISMATCH`: the declared length differs from the planned file.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 501, description = "`TUS_EXTENSION_UNSUPPORTED`: `Upload-Concat` was supplied.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`.", body = ApiErrorBody),
        (status = 507, description = "`QUOTA_EXCEEDED` or `STORAGE_WRITE_FAILED`.", body = ApiErrorBody),
    )
)]
async fn create_upload(
    Extension(service): Extension<TusService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let (parts, body) = request.into_parts();
    let outcome = async {
        headers::require_version(&parts.headers)?;
        headers::reject_concatenation(&parts.headers)?;
        headers::reject_method_override(&parts.headers)?;
        let length = headers::declared_length(&parts.headers)?;
        let metadata = UploadMetadata::parse(&parts.headers)?;
        let body = carries_body(&parts.headers).then_some(body);
        service
            .create(
                principal.user_id,
                CreateInput {
                    metadata,
                    length,
                    body,
                    request_id: request_id.as_ref().map(|id| id.as_str().to_owned()),
                },
            )
            .await
    }
    .await;
    match outcome {
        Err(error) => failure(&error, request_id.as_ref()),
        Ok(created) => {
            let mut response = StatusCode::CREATED.into_response();
            let headers = response.headers_mut();
            headers.insert(LOCATION, text_header(&created.location));
            headers.insert(
                UPLOAD_EXPIRES,
                text_header(&imf_fixdate(created.expires_at)),
            );
            headers.insert(UPLOAD_OFFSET, number_header(created.offset));
            response
        }
    }
}

async fn head(
    service: &TusService,
    principal: &Authenticated,
    raw_id: &str,
    request_headers: &HeaderMap,
) -> Result<Response, TusError> {
    headers::require_version(request_headers)?;
    let head = service
        .head(principal.0.user_id, upload_id(raw_id)?)
        .await?;
    let mut response = StatusCode::OK.into_response();
    let headers = response.headers_mut();
    headers.insert(UPLOAD_OFFSET, number_header(head.offset));
    match head.length {
        Some(length) => {
            headers.insert(UPLOAD_LENGTH, number_header(length.get()));
        }
        None => {
            headers.insert(UPLOAD_DEFER_LENGTH, HeaderValue::from_static("1"));
        }
    }
    headers.insert(UPLOAD_EXPIRES, text_header(&imf_fixdate(head.expires_at)));
    Ok(response)
}

async fn terminate(
    service: &TusService,
    principal: &Authenticated,
    raw_id: &str,
    request_headers: &HeaderMap,
) -> Result<Response, TusError> {
    headers::require_version(request_headers)?;
    service
        .terminate(principal.0.user_id, upload_id(raw_id)?)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(
    head,
    path = "/api/v1/uploads/tus/{id}",
    tag = "uploads",
    params(
        ("id" = String, Path, description = "Opaque upload id. It is not a storage key."),
        ("Tus-Resumable" = String, Header, description = "Must be `1.0.0`."),
    ),
    responses(
        (status = 200, description = "The authoritative offset: the lower of the recorded offset and the bytes on disk. A known length is `Upload-Length`; a deferred one is `Upload-Defer-Length: 1`.",
            headers(
                ("Tus-Resumable" = String, description = "`1.0.0`."),
                ("Upload-Offset" = i64, description = "Authoritative offset in bytes."),
                ("Upload-Length" = Option<i64>, description = "Present when the length is known."),
                ("Upload-Defer-Length" = Option<i64>, description = "`1` while the length is not known."),
                ("Upload-Expires" = String, description = "RFC 7231 IMF-fixdate."),
                ("Cache-Control" = String, description = "`no-store`."),
            )),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`NOT_FOUND`: the upload is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 410, description = "`UPLOAD_SESSION_EXPIRED`: the upload expired or was terminated.", body = ApiErrorBody),
        (status = 412, description = "`TUS_VERSION_UNSUPPORTED`; the response carries `Tus-Version`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn head_upload(
    Extension(service): Extension<TusService>,
    principal: Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match head(&service, &principal, &id, request.headers()).await {
        Ok(response) => response,
        Err(error) => failure(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/uploads/tus/{id}",
    tag = "uploads",
    params(
        ("id" = String, Path, description = "Opaque upload id. It is not a storage key."),
        ("Tus-Resumable" = String, Header, description = "Must be `1.0.0`."),
    ),
    responses(
        (status = 204, description = "The upload is terminated: its item is canceled, its share of the quota reservation is released and the staging bytes are marked for removal. Terminating a terminated upload is also `204`. A finalized file is never removed here.",
            headers(("Tus-Resumable" = String, description = "`1.0.0`."))),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`NOT_FOUND`: the upload is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 410, description = "`UPLOAD_SESSION_EXPIRED`: the upload already expired.", body = ApiErrorBody),
        (status = 412, description = "`TUS_VERSION_UNSUPPORTED`; the response carries `Tus-Version`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn terminate_upload(
    Extension(service): Extension<TusService>,
    principal: Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match terminate(&service, &principal, &id, request.headers()).await {
        Ok(response) => response,
        Err(error) => failure(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/uploads/tus/{id}",
    tag = "uploads",
    params(
        ("id" = String, Path, description = "Opaque upload id. It is not a storage key."),
        ("Tus-Resumable" = String, Header, description = "Must be `1.0.0`."),
        ("X-HTTP-Method-Override" = String, Header, description = "`HEAD` or `DELETE`, for clients behind proxies that strip those methods. `PATCH` is recognised but not available yet and answers `METHOD_NOT_ALLOWED`. Any other value is `UPLOAD_METADATA_INVALID`. Authorization and CSRF are those of this `POST`."),
    ),
    responses(
        (status = 200, description = "As `HEAD` when overridden with `HEAD`."),
        (status = 204, description = "As `DELETE` when overridden with `DELETE`."),
        (status = 400, description = "`UPLOAD_METADATA_INVALID`: the override value is not permitted.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`NOT_FOUND`: the upload is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 405, description = "`METHOD_NOT_ALLOWED`: no override was supplied, or it names `PATCH`.", body = ApiErrorBody),
        (status = 410, description = "`UPLOAD_SESSION_EXPIRED`.", body = ApiErrorBody),
        (status = 412, description = "`TUS_VERSION_UNSUPPORTED`; the response carries `Tus-Version`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn override_upload(
    Extension(service): Extension<TusService>,
    principal: Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let (parts, _body) = request.into_parts();
    let outcome = async {
        headers::require_version(&parts.headers)?;
        match method_override(&parts.headers)? {
            Some(MethodOverride::Head) => head(&service, &principal, &id, &parts.headers).await,
            Some(MethodOverride::Delete) => {
                terminate(&service, &principal, &id, &parts.headers).await
            }
            Some(MethodOverride::Patch) | None => Err(TusError::MethodNotAllowed),
        }
    }
    .await;
    match outcome {
        Ok(response) => response,
        Err(error) => failure(&error, request_id.as_ref()),
    }
}
