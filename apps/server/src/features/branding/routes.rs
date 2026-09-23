use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use super::manifest::{WebAppManifest, MANIFEST_CONTENT_TYPE};
use super::model::ManifestSettings;
use super::service::render_manifest;
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::infra::http::error::ApiError;
use crate::infra::http::etag::matches_validator;
use crate::infra::http::request_id::{tag_error, RequestId};

pub const MANIFEST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

const REVALIDATE: HeaderValue = HeaderValue::from_static("no-cache");

pub fn routes() -> Routes<AppState> {
    Routes::new().route(MANIFEST_ROUTE, routes!(manifest))
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
async fn manifest(request: Request) -> Response {
    let rendered = match render_manifest(&ManifestSettings::FRESH_INSTALL) {
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
