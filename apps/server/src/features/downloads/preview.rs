use axum::extract::{Extension, Path, Request};
use axum::response::Response;
use http::HeaderMap;
use mime_guess::mime::{self, Mime};
use utoipa_axum::routes;

use crate::domain::content_disposition::DispositionType;
use crate::features::files::FileId;
use crate::features::users::model::UserId;
use crate::infra::http::error::ApiErrorBody;
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::request_id::RequestId;

use super::error::DownloadError;
use super::routes::{respond, FileBytes};
use super::service::{Delivery, DownloadService, Plan};

pub const TEXT_PREVIEW_PREFIX_BYTES: u64 = 512 * 1024;
pub const TEXT_PREVIEW_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const TEXT_PREVIEW_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

const UNSAFE_CONTENT: &str = "unsafe_content";
const UNSUPPORTED_TYPE: &str = "unsupported_type";

const IMAGES: &[&str] = &[
    "image/png",
    "image/apng",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/heic",
    "image/heif",
    "image/tiff",
    "image/jxl",
];

const VIDEO: &[&str] = &[
    "video/mp4",
    "video/webm",
    "video/ogg",
    "video/quicktime",
    "video/x-matroska",
    "video/x-m4v",
    "video/mpeg",
    "video/x-msvideo",
    "video/3gpp",
    "video/3gpp2",
    "video/mp2t",
];

const AUDIO: &[&str] = &[
    "audio/mpeg",
    "audio/mp3",
    "audio/mp4",
    "audio/aac",
    "audio/x-m4a",
    "audio/ogg",
    "audio/opus",
    "audio/wav",
    "audio/x-wav",
    "audio/wave",
    "audio/vnd.wave",
    "audio/webm",
    "audio/flac",
    "audio/x-flac",
];

const TEXT: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/x-markdown",
    "text/csv",
    "text/tab-separated-values",
    "text/css",
    "text/yaml",
    "text/x-yaml",
    "text/x-toml",
    "text/x-log",
    "text/x-diff",
    "text/x-patch",
    "text/x-sh",
    "text/x-shellscript",
    "text/x-python",
    "text/x-rust",
    "text/x-c",
    "text/x-csrc",
    "text/x-chdr",
    "text/x-c++src",
    "text/x-c++hdr",
    "text/x-java",
    "text/x-java-source",
    "text/x-go",
    "text/x-ruby",
    "text/x-php",
    "text/x-sql",
    "text/x-lua",
    "text/x-kotlin",
    "text/x-swift",
    "text/x-scala",
    "text/x-perl",
    "text/x-typescript",
    "text/javascript",
    "application/javascript",
    "application/x-javascript",
    "application/typescript",
    "application/json",
    "application/x-ndjson",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/sql",
    "application/x-sh",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewVerdict {
    Inline {
        kind: PreviewKind,
        content_type: String,
    },
    Unsafe,
    Unsupported,
}

pub fn classify(stored_mime: &str) -> PreviewVerdict {
    let Ok(parsed) = stored_mime.parse::<Mime>() else {
        return PreviewVerdict::Unsupported;
    };
    if is_active_markup(&parsed) {
        return PreviewVerdict::Unsafe;
    }
    let essence = parsed.essence_str().to_ascii_lowercase();
    let essence = essence.as_str();
    let inline = |kind: PreviewKind, content_type: &str| PreviewVerdict::Inline {
        kind,
        content_type: content_type.to_owned(),
    };
    if IMAGES.contains(&essence) {
        inline(PreviewKind::Image, essence)
    } else if VIDEO.contains(&essence) {
        inline(PreviewKind::Video, essence)
    } else if AUDIO.contains(&essence) {
        inline(PreviewKind::Audio, essence)
    } else if essence == "application/pdf" {
        inline(PreviewKind::Pdf, essence)
    } else if TEXT.contains(&essence) || parsed.suffix() == Some(mime::JSON) {
        inline(PreviewKind::Text, TEXT_PREVIEW_CONTENT_TYPE)
    } else {
        PreviewVerdict::Unsupported
    }
}

fn is_active_markup(parsed: &Mime) -> bool {
    let subtype = parsed.subtype().as_str().to_ascii_lowercase();
    parsed.suffix() == Some(mime::XML)
        || ["xml", "html", "svg"]
            .iter()
            .any(|marker| subtype.contains(marker))
}

fn plan(stored_mime: &str) -> Result<Plan, DownloadError> {
    match classify(stored_mime) {
        PreviewVerdict::Inline { kind, content_type } => Ok(Plan {
            content_type,
            disposition: DispositionType::Inline,
            redirect: kind != PreviewKind::Text,
            max_length: (kind == PreviewKind::Text).then_some(TEXT_PREVIEW_MAX_BYTES),
        }),
        PreviewVerdict::Unsafe => Err(DownloadError::PreviewUnsupported {
            reason: UNSAFE_CONTENT,
        }),
        PreviewVerdict::Unsupported => Err(DownloadError::PreviewUnsupported {
            reason: UNSUPPORTED_TYPE,
        }),
    }
}

impl DownloadService {
    pub async fn preview(
        &self,
        owner: UserId,
        file: FileId,
        headers: &HeaderMap,
    ) -> Result<Delivery, DownloadError> {
        self.serve(owner, file, headers, |stored| plan(&stored.mime_type))
            .await
    }
}

pub fn routes() -> utoipa_axum::router::UtoipaMethodRouter<crate::app::state::AppState> {
    routes!(preview_file)
}

#[utoipa::path(
    get,
    path = "/api/v1/files/{id}/preview",
    tag = "downloads",
    params(("id" = String, Path, description = "File UUIDv7")),
    responses(
        (status = 200, description = "The whole file, inline, streamed with an exact `Content-Length` and no content encoding. `Content-Type` is the server-detected type stored at upload for an image, video, audio or PDF; text and code are always `text/plain; charset=utf-8`. `X-Content-Type-Options: nosniff` is always present, the response is `Cache-Control: private, no-cache`, and it carries a strong `ETag`. A text or code response never carries more than 5 MiB.",
            content((FileBytes = "application/octet-stream")),
            headers(
                ("Content-Length" = i64, description = "The exact size of the response body in bytes."),
                ("Content-Disposition" = String, description = "`inline` with an RFC 6266 ASCII `filename` fallback and an RFC 5987 `filename*`."),
                ("Accept-Ranges" = String, description = "`bytes`."),
                ("ETag" = String, description = "Strong validator of the stored object."),
                ("Last-Modified" = String, description = "When the stored object was written."),
                ("Cache-Control" = String, description = "`private, no-cache`."),
            )),
        (status = 206, description = "The satisfiable first byte range of `Range`, honoured when `If-Range` is absent or matches; the same rules as `GET /api/v1/files/{id}/content`. A text or code range is answered only when its resolved length is at most 5 MiB; the SPA asks for `bytes=0-524287`.",
            content((FileBytes = "application/octet-stream")),
            headers(
                ("Content-Range" = String, description = "`bytes <first>-<last>/<size>`, both bounds inclusive."),
                ("Content-Length" = i64, description = "`last - first + 1`."),
                ("Content-Disposition" = String, description = "`inline`, as for `200`."),
                ("Accept-Ranges" = String, description = "`bytes`."),
                ("ETag" = String, description = "Strong validator of the stored object."),
            )),
        (status = 302, description = "S3 storage only, and only for an image, video, audio or PDF: the caller owns the file, its type is previewable, and `Location` is a presigned `GET` signed against the public S3 endpoint for 300 seconds with `response-content-disposition` (`inline`) and `response-content-type` pinned into the signature. The same conditions as `GET /api/v1/files/{id}/content` decide between this redirect and a Palmr-streamed answer. Text and code are never redirected: their bytes are always relayed by Palmr within the 5 MiB limit, because a presigned URL cannot enforce it.",
            headers(
                ("Location" = String, description = "The short-lived presigned URL; a capability, never logged or stored."),
                ("Cache-Control" = String, description = "`no-store`."),
            )),
        (status = 304, description = "`If-None-Match` matched the current `ETag`; evaluated only after ownership, previewability and the text size limit are established.",
            headers(
                ("ETag" = String, description = "Strong validator of the stored object."),
                ("Cache-Control" = String, description = "`private, no-cache`."),
            )),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND`: the id is malformed, unknown, deleted or another user's file.", body = ApiErrorBody),
        (status = 413, description = "`PREVIEW_TOO_LARGE`: a text or code response would carry more than 5 MiB, because the request has no `Range`, an open-ended or suffix `Range`, or a `Range` that resolves to more than 5 MiB. `details.prefixBytes` is the 524288-byte prefix the SPA requests with `Range: bytes=0-524287`. No file bytes are sent.", body = ApiErrorBody),
        (status = 415, description = "`PREVIEW_UNSUPPORTED`: the file's stored type has no safe inline rendering (active content such as HTML, XML and SVG, or an unknown or binary type). `details.reason` is `unsafe_content` or `unsupported_type`. No file bytes are sent and no presigned URL is issued; the client offers `GET /api/v1/files/{id}/content`.", body = ApiErrorBody),
        (status = 416, description = "`RANGE_NOT_SATISFIABLE`: the first range starts at or beyond the end of the file, or asks for a zero-length suffix, or the file is empty.", body = ApiErrorBody,
            headers(("Content-Range" = String, description = "`bytes */<size>` with the authoritative size."))),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 500, description = "`STORAGE_PERMISSION_DENIED`, `STORAGE_PROVIDER_MISMATCH`, `STORAGE_SIZE_MISMATCH` or `INTERNAL_ERROR`.", body = ApiErrorBody),
        (status = 502, description = "`STORAGE_OBJECT_MISSING` or `STORAGE_UPSTREAM_ERROR`.", body = ApiErrorBody),
        (status = 503, description = "`STORAGE_UNAVAILABLE`, or `TRANSFER_BACKPRESSURE` (with `Retry-After`) while every download slot is busy.", body = ApiErrorBody,
            headers(("Retry-After" = i64, description = "Seconds to wait; present on `TRANSFER_BACKPRESSURE`."))),
    )
)]
async fn preview_file(
    Extension(service): Extension<DownloadService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let delivery = match id.parse::<FileId>() {
        Ok(file) => {
            service
                .preview(principal.user_id, file, request.headers())
                .await
        }
        Err(_) => Err(DownloadError::NotFound),
    };
    respond(delivery, request_id.as_ref())
}
