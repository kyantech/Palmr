use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG};
use http::{HeaderValue, StatusCode};
use tokio_util::io::ReaderStream;
use utoipa_axum::routes;

use super::manifest::{WebAppManifest, MANIFEST_CONTENT_TYPE};
use super::model::{AssetResolution, BrandingAsset, ManifestSettings};
use super::service::{
    render_manifest, resolve, BrandingService, CustomAssetError, CustomBody, CustomBytes,
};
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::error_code::ErrorCode;
use crate::infra::http::error::{ApiError, ApiErrorBody};
use crate::infra::http::etag::matches_validator;
use crate::infra::http::request_id::{tag_error, RequestId};
use crate::storage::error::StorageError;

pub const MANIFEST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

pub const PUBLIC_BRANDING_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::PublicRead,
    Transport::ControlPlane,
);

const REVALIDATE: HeaderValue = HeaderValue::from_static("no-cache");
const SHARED_CACHE: HeaderValue = HeaderValue::from_static("public, max-age=300");
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(MANIFEST_ROUTE, routes!(manifest))
        .route(PUBLIC_BRANDING_ROUTE, routes!(public_branding))
}

#[utoipa::path(
    get,
    path = "/manifest.webmanifest",
    tag = "branding",
    responses(
        (
            status = 200,
            description = "The instance-branded web app manifest.",
            body = WebAppManifest,
            content_type = "application/manifest+json"
        ),
        (status = 304, description = "The cached manifest is still current."),
    )
)]
async fn manifest(State(state): State<AppState>, request: Request) -> Response {
    let settings = state.settings().load();
    let rendered = match render_manifest(&ManifestSettings::from_settings(&settings)) {
        Ok(rendered) => rendered,
        Err(error) => {
            tracing::error!(error = %error, "the web app manifest could not be serialized");
            return tag_error(ApiError::internal(), RequestId::of(&request).as_ref())
                .into_response();
        }
    };
    let etag = rendered.etag().clone();
    if matches_validator(request.headers(), &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(CACHE_CONTROL, REVALIDATE), (ETAG, etag)],
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static(MANIFEST_CONTENT_TYPE),
            ),
            (CACHE_CONTROL, REVALIDATE),
            (ETAG, etag),
        ],
        rendered.body().clone(),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/public/branding/{asset}",
    tag = "branding",
    params(
        (
            "asset" = String,
            Path,
            description = "logo | favicon | login-background | og-image | email-logo"
        )
    ),
    responses(
        (
            status = 200,
            description = "The effective branding asset bytes.",
            content(
                (Vec<u8> = "image/png"),
                (Vec<u8> = "image/webp")
            )
        ),
        (status = 304, description = "The cached asset is still current."),
        (status = 404, description = "Unknown or disabled asset.", body = ApiErrorBody),
    )
)]
async fn public_branding(
    State(state): State<AppState>,
    Path(segment): Path<String>,
    request: Request,
) -> Response {
    let Some(asset) = BrandingAsset::from_segment(&segment) else {
        return unknown(&request);
    };
    let resolution = resolve(&state.settings().load().branding, asset);
    match resolution {
        AssetResolution::Disabled => unknown(&request),
        AssetResolution::Bundled(bundled) => {
            let Some(bytes) = bundled.load() else {
                tracing::error!(
                    asset = asset.segment(),
                    "a bundled default branding asset is missing from the binary"
                );
                return internal(&request);
            };
            if matches_validator(request.headers(), &bytes.etag) {
                return not_modified(bytes.etag);
            }
            (
                StatusCode::OK,
                [
                    (CONTENT_TYPE, bytes.content_type),
                    (CONTENT_LENGTH, HeaderValue::from(bytes.bytes.len())),
                    (CACHE_CONTROL, SHARED_CACHE),
                    (ETAG, bytes.etag),
                ],
                bytes.bytes,
            )
                .into_response()
        }
        AssetResolution::Custom(custom) => serve_custom(custom, request).await,
    }
}

async fn serve_custom(asset: BrandingAsset, request: Request) -> Response {
    let request_id = RequestId::of(&request);
    let Some(service) = request.extensions().get::<BrandingService>().cloned() else {
        tracing::error!("custom branding was requested without a branding service");
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    };
    let (parts, _) = request.into_parts();
    match service.open_custom(asset, &parts.headers).await {
        Ok(CustomBytes {
            etag,
            body: CustomBody::NotModified,
            ..
        }) => not_modified(etag),
        Ok(CustomBytes {
            etag,
            content_type,
            body: CustomBody::Stream { size_bytes, body },
        }) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, content_type),
                (CONTENT_LENGTH, HeaderValue::from(size_bytes)),
                (CACHE_CONTROL, SHARED_CACHE),
                (ETAG, etag),
            ],
            Body::from_stream(ReaderStream::with_capacity(body, STREAM_CHUNK_BYTES)),
        )
            .into_response(),
        Err(CustomAssetError::Missing | CustomAssetError::Storage(StorageError::NotFound)) => {
            tracing::warn!(
                asset = asset.segment(),
                "branding is in custom mode but its current asset is unavailable"
            );
            tag_error(
                ApiError::new(ErrorCode::BrandingAssetUnknown),
                request_id.as_ref(),
            )
            .into_response()
        }
        Err(CustomAssetError::Db(error)) => {
            tag_error(ApiError::from(error), request_id.as_ref()).into_response()
        }
        Err(CustomAssetError::Storage(error)) => {
            tag_error(ApiError::from(error), request_id.as_ref()).into_response()
        }
    }
}

fn not_modified(etag: HeaderValue) -> Response {
    (
        StatusCode::NOT_MODIFIED,
        [(CACHE_CONTROL, SHARED_CACHE), (ETAG, etag)],
    )
        .into_response()
}

fn unknown(request: &Request) -> Response {
    tag_error(
        ApiError::new(ErrorCode::BrandingAssetUnknown),
        RequestId::of(request).as_ref(),
    )
    .into_response()
}

fn internal(request: &Request) -> Response {
    tag_error(ApiError::internal(), RequestId::of(request).as_ref()).into_response()
}
