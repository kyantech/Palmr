use std::borrow::Cow;
use std::sync::Arc;

use axum::body::Bytes;
use http::{HeaderMap, HeaderValue};
use rust_embed::RustEmbed;

use super::manifest::WebAppManifest;
use super::model::{AssetResolution, BrandingAsset, BundledAsset, ManifestSettings};
use super::repo::{self, CurrentAsset};
use crate::features::settings::model::{AssetMode, BrandingSettings, EmailLogoMode};
use crate::infra::db::{DbError, ReadPool};
use crate::infra::http::etag::{matches_validator, weak_etag, weak_etag_from_sha256};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{ObjectBody, StorageProvider};

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

pub const fn resolve(branding: &BrandingSettings, asset: BrandingAsset) -> AssetResolution {
    match asset {
        BrandingAsset::Logo => main(branding.logo_mode, asset, BundledAsset::Logo),
        BrandingAsset::Favicon => main(branding.favicon_mode, asset, BundledAsset::Favicon),
        BrandingAsset::LoginBackground => main(
            branding.login_background_mode,
            asset,
            BundledAsset::LoginBackground,
        ),
        BrandingAsset::OgImage => main(branding.og_image_mode, asset, BundledAsset::OgImage),
        BrandingAsset::EmailLogo => match branding.email_logo_mode {
            EmailLogoMode::Inherit => resolve(branding, BrandingAsset::Logo),
            EmailLogoMode::Custom => AssetResolution::Custom(BrandingAsset::EmailLogo),
            EmailLogoMode::None => AssetResolution::Disabled,
        },
    }
}

const fn main(mode: AssetMode, asset: BrandingAsset, bundled: BundledAsset) -> AssetResolution {
    match mode {
        AssetMode::Default => AssetResolution::Bundled(bundled),
        AssetMode::Custom => AssetResolution::Custom(asset),
        AssetMode::Disabled => AssetResolution::Disabled,
    }
}

pub fn public_url(branding: &BrandingSettings, asset: BrandingAsset) -> Option<&'static str> {
    resolve(branding, asset)
        .is_served()
        .then_some(asset.public_path())
}

#[derive(RustEmbed)]
#[folder = "../web/public/branding"]
struct Defaults;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledBytes {
    pub bytes: Bytes,
    pub etag: HeaderValue,
    pub content_type: HeaderValue,
}

impl BundledAsset {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Logo => "logo.png",
            Self::Favicon => "favicon.png",
            Self::LoginBackground => "login-background.png",
            Self::OgImage => "og-image.png",
        }
    }

    pub fn load(self) -> Option<BundledBytes> {
        Defaults::get(self.file_name()).map(|file| BundledBytes {
            bytes: match file.data {
                Cow::Borrowed(bytes) => Bytes::from_static(bytes),
                Cow::Owned(bytes) => Bytes::from(bytes),
            },
            etag: weak_etag_from_sha256(file.metadata.sha256_hash()),
            content_type: HeaderValue::from_static("image/png"),
        })
    }
}

#[derive(Debug)]
pub enum CustomAssetError {
    Missing,
    Db(DbError),
    Storage(StorageError),
}

pub enum CustomBody {
    NotModified,
    Stream { size_bytes: u64, body: ObjectBody },
}

pub struct CustomBytes {
    pub etag: HeaderValue,
    pub content_type: HeaderValue,
    pub body: CustomBody,
}

#[derive(Clone)]
pub struct BrandingService {
    reader: ReadPool,
    storage: Arc<dyn StorageProvider>,
}

impl BrandingService {
    pub fn new(reader: ReadPool, storage: Arc<dyn StorageProvider>) -> Self {
        Self { reader, storage }
    }

    pub async fn open_custom(
        &self,
        asset: BrandingAsset,
        request_headers: &HeaderMap,
    ) -> Result<CustomBytes, CustomAssetError> {
        let CurrentAsset {
            mime_type,
            storage_object_id,
            object_key,
        } = repo::current(&self.reader, asset)
            .await
            .map_err(CustomAssetError::Db)?
            .ok_or(CustomAssetError::Missing)?;
        let etag = weak_etag(storage_object_id.as_bytes());
        let content_type =
            HeaderValue::try_from(mime_type).map_err(|_| CustomAssetError::Missing)?;
        if matches_validator(request_headers, &etag) {
            return Ok(CustomBytes {
                etag,
                content_type,
                body: CustomBody::NotModified,
            });
        }
        let key = ObjectKey::parse(&object_key)
            .map_err(|_| CustomAssetError::Storage(StorageError::InvalidKey))?;
        let (stat, body) = self
            .storage
            .open_read(&key)
            .await
            .map_err(CustomAssetError::Storage)?;
        Ok(CustomBytes {
            etag,
            content_type,
            body: CustomBody::Stream {
                size_bytes: stat.size,
                body,
            },
        })
    }
}
