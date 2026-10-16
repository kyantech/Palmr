use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    ETAG, LAST_MODIFIED, LOCATION, RETRY_AFTER, X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn};
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::Required;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{
    BytePath, Deadline, RateLimitClass, RequestBody, ResponseEncoding, RoutePolicy, Routes,
    Transport,
};
use crate::app::state::AppState;
use crate::domain::http_range::format_http_date;
use crate::features::files::FileId;
use crate::infra::http::error::{ApiError, ApiErrorBody};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::DownloadError;
use super::model::{parse_disposition, DISPOSITION_PARAM};
use super::service::{Content, Delivery, DownloadRequest, DownloadService, Representation};

#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
#[allow(dead_code, reason = "the type only names the binary response schema")]
struct FileBytes(Vec<u8>);

const CONTENT_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferData,
    Transport::BytePath(BytePath::new(
        RequestBody::GlobalLimit,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
);

const BYTES: HeaderValue = HeaderValue::from_static("bytes");
const PRIVATE_NO_CACHE: HeaderValue = HeaderValue::from_static("private, no-cache");
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");
const NOSNIFF: HeaderValue = HeaderValue::from_static("nosniff");

fn disposition_parameters() -> Vec<Parameter> {
    vec![ParameterBuilder::new()
        .name(DISPOSITION_PARAM)
        .parameter_in(ParameterIn::Query)
        .required(Required::False)
        .description(Some(
            "`attachment` (default) or `inline`. A type that cannot be rendered safely on the application origin is always served as an attachment, whatever is requested.",
        ))
        .schema(Some(
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(["attachment", "inline"])),
        ))
        .build()]
}

pub fn routes() -> Routes<AppState> {
    Routes::new().route(
        CONTENT_ROUTE,
        with_query_parameters(routes!(download_content), &disposition_parameters()),
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/files/{id}/content",
    tag = "downloads",
    params(("id" = String, Path, description = "File UUIDv7")),
    responses(
        (status = 200, description = "The whole file, streamed with an exact `Content-Length` and no content encoding. `Content-Type` is the server-detected type stored at upload, `X-Content-Type-Options: nosniff` is always present, and the response is `Cache-Control: private, no-cache` with a strong `ETag`.",
            content((FileBytes = "application/octet-stream")),
            headers(
                ("Content-Length" = i64, description = "The exact size of the file in bytes."),
                ("Content-Disposition" = String, description = "RFC 6266 with an ASCII `filename` fallback and an RFC 5987 `filename*`."),
                ("Accept-Ranges" = String, description = "`bytes`."),
                ("ETag" = String, description = "Strong validator of the stored object."),
                ("Last-Modified" = String, description = "When the stored object was written."),
            )),
        (status = 206, description = "The satisfiable first byte range of `Range`, honoured when `If-Range` is absent or matches. On S3, both Palmr's `ETag` and the provider's own strong `ETag` (what a client holds after a direct download) match; `Last-Modified` dates match exactly. A header with several ranges is answered with its first range only; later ranges are ignored and never merged.",
            content((FileBytes = "application/octet-stream")),
            headers(
                ("Content-Range" = String, description = "`bytes <first>-<last>/<size>`, both bounds inclusive."),
                ("Content-Length" = i64, description = "`last - first + 1`."),
                ("Accept-Ranges" = String, description = "`bytes`."),
                ("ETag" = String, description = "Strong validator of the stored object."),
            )),
        (status = 302, description = "S3 storage only: the caller owns the file, and `Location` is a presigned `GET` signed against the public S3 endpoint for 300 seconds, with `response-content-disposition` and `response-content-type` pinned into the signature. The redirect is never cached. It is issued only when Palmr has nothing to decide: a plain request, or a `Range` without `If-Range` that is satisfiable against the recorded size (the provider then answers the `Range`). A request with `If-None-Match`, or with both `Range` and `If-Range`, is decided by Palmr and, when it needs bytes, streamed by Palmr (`200` or `206`). A missing object is discovered by the provider after this redirect and is the provider's own error, not a Palmr envelope.",
            headers(
                ("Location" = String, description = "The short-lived presigned URL; a capability, never logged or stored."),
                ("Cache-Control" = String, description = "`no-store`."),
            )),
        (status = 304, description = "`If-None-Match` matched the current `ETag`; evaluated only after ownership is established.",
            headers(
                ("ETag" = String, description = "Strong validator of the stored object."),
                ("Cache-Control" = String, description = "`private, no-cache`."),
            )),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND`: the id is malformed, unknown, deleted or another user's file.", body = ApiErrorBody),
        (status = 416, description = "`RANGE_NOT_SATISFIABLE`: the first range starts at or beyond the end of the file, or asks for a zero-length suffix, or the file is empty.", body = ApiErrorBody,
            headers(("Content-Range" = String, description = "`bytes */<size>` with the authoritative size."))),
        (status = 422, description = "`VALIDATION_ERROR` for a `disposition` other than `attachment` or `inline`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 500, description = "`STORAGE_PERMISSION_DENIED`, `STORAGE_PROVIDER_MISMATCH`, `STORAGE_SIZE_MISMATCH` or `INTERNAL_ERROR`.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_OBJECT_MISSING` or `STORAGE_UPSTREAM_ERROR`.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`, or `TRANSFER_BACKPRESSURE` (with `Retry-After`) while every download slot is busy.", body = ApiErrorBody,
            headers(("Retry-After" = i64, description = "Seconds to wait; present on `TRANSFER_BACKPRESSURE`."))),
    )
)]
async fn download_content(
    Extension(service): Extension<DownloadService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(file) = id.parse::<FileId>() else {
        return failure(&DownloadError::NotFound, request_id.as_ref());
    };
    let disposition = match parse_disposition(raw_query.as_deref()) {
        Ok(disposition) => disposition,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let delivery = service
        .deliver(DownloadRequest {
            owner: principal.user_id,
            file,
            disposition,
            headers: request.headers(),
        })
        .await;
    match delivery {
        Ok(Delivery::Redirect(location)) => redirect(location),
        Ok(Delivery::NotModified(representation)) => not_modified(&representation),
        Ok(Delivery::Content(content)) => content_response(content, request_id.as_ref()),
        Err(error) => failure(&error, request_id.as_ref()),
    }
}

fn redirect(location: HeaderValue) -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    let headers = response.headers_mut();
    headers.insert(LOCATION, location);
    headers.insert(CACHE_CONTROL, NO_STORE);
    response
}

fn validators(headers: &mut http::HeaderMap, representation: &Representation) {
    if let Ok(etag) = HeaderValue::from_str(representation.etag.as_str()) {
        headers.insert(ETAG, etag);
    }
    if let Some(modified) = format_http_date(representation.modified_at)
        .and_then(|date| HeaderValue::from_str(&date).ok())
    {
        headers.insert(LAST_MODIFIED, modified);
    }
    headers.insert(CACHE_CONTROL, PRIVATE_NO_CACHE);
    headers.insert(X_CONTENT_TYPE_OPTIONS, NOSNIFF);
}

fn not_modified(representation: &Representation) -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    validators(response.headers_mut(), representation);
    response
}

fn content_response(content: Content, request_id: Option<&RequestId>) -> Response {
    let length = content.length();
    let range = content.range;
    let size = content.size;
    let mut response = Response::new(axum::body::Body::empty());
    {
        let representation = &content.representation;
        let headers = response.headers_mut();
        validators(headers, representation);
        if let Ok(content_type) = HeaderValue::from_str(&representation.content_type) {
            headers.insert(CONTENT_TYPE, content_type);
        }
        headers.insert(
            CONTENT_DISPOSITION,
            representation.disposition.to_header_value(),
        );
        headers.insert(ACCEPT_RANGES, BYTES);
        headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
        if let Some(range) = range {
            if let Ok(framing) = HeaderValue::from_str(&range.content_range()) {
                headers.insert(CONTENT_RANGE, framing);
            }
        }
    }
    *response.status_mut() = if range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    debug_assert!(range.is_none_or(|range| range.size() == size));
    *response.body_mut() = content.into_body(request_id.map(RequestId::as_str));
    response
}

fn failure(error: &DownloadError, request_id: Option<&RequestId>) -> Response {
    let api_error: ApiError = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "download failed");
    }
    let mut response = tag_error(api_error, request_id).into_response();
    match error {
        DownloadError::RangeNotSatisfiable { size } => {
            if let Ok(framing) = HeaderValue::from_str(&format!("bytes */{size}")) {
                response.headers_mut().insert(CONTENT_RANGE, framing);
            }
        }
        DownloadError::Backpressure { retry_after } => {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(*retry_after));
        }
        _ => {}
    }
    response
}
