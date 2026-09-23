use axum::body::Bytes;
use http::HeaderValue;

use super::manifest::WebAppManifest;
use super::model::ManifestSettings;
use crate::infra::http::etag::weak_etag;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedManifest {
    body: Bytes,
    etag: HeaderValue,
}

impl RenderedManifest {
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    pub const fn etag(&self) -> &HeaderValue {
        &self.etag
    }
}

pub fn render_manifest(settings: &ManifestSettings<'_>) -> serde_json::Result<RenderedManifest> {
    let body = serde_json::to_vec(&WebAppManifest::new(settings))?;
    let etag = weak_etag(&body);
    Ok(RenderedManifest {
        body: Bytes::from(body),
        etag,
    })
}
