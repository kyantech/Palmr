use std::fmt::Write;
use std::io;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, MethodRouter};
use http::header::{
    ACCEPT, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use sha2::{Digest, Sha256};

use super::encoding::response_compression;
use super::error::ApiError;
use super::headers::CspNonce;
use super::request_id::{tag_error, RequestId};
use super::shell::{ShellInitError, ShellMetadata, ShellRenderer};
use crate::config::PublicBaseUrl;
use crate::domain::error_code::ErrorCode;

const INDEX_HTML: &str = "index.html";
const HASHED_ASSET_DIR: &str = "assets/";
const NON_SPA_NAMESPACES: [&str; 5] = ["/api", "/openapi.json", "/docs", "/health", "/e"];

const IMMUTABLE: HeaderValue = HeaderValue::from_static("public, max-age=31536000, immutable");
const REVALIDATE: HeaderValue = HeaderValue::from_static("no-cache");
const GET_HEAD: HeaderValue = HeaderValue::from_static("GET, HEAD");
const OCTET_STREAM: &str = "application/octet-stream";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetKey(String);

impl AssetKey {
    fn index() -> Self {
        Self(INDEX_HTML.to_owned())
    }

    fn from_request_path(path: &str) -> Option<Self> {
        let relative = path.strip_prefix('/')?;
        relative
            .split('/')
            .all(is_servable_segment)
            .then(|| Self(relative.to_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn is_index(&self) -> bool {
        self.0 == INDEX_HTML
    }

    fn is_hashed(&self) -> bool {
        self.0.starts_with(HASHED_ASSET_DIR)
    }

    fn cache_control(&self) -> HeaderValue {
        if self.is_hashed() {
            IMMUTABLE
        } else {
            REVALIDATE
        }
    }

    fn content_type(&self) -> HeaderValue {
        let essence = mime_guess::from_path(&self.0)
            .first_raw()
            .unwrap_or(OCTET_STREAM);
        if is_textual(essence) {
            HeaderValue::try_from(format!("{essence}; charset=utf-8"))
                .unwrap_or(HeaderValue::from_static(essence))
        } else {
            HeaderValue::from_static(essence)
        }
    }
}

// Segments starting with '.' cover `..`, `.` and dotfiles; rejecting '%' and
// '\' means no encoded or Windows-style separator can reach a filesystem join.
fn is_servable_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('.')
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_textual(essence: &str) -> bool {
    essence.starts_with("text/")
        || matches!(
            essence,
            "application/javascript" | "application/json" | "application/manifest+json"
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    bytes: Bytes,
    etag: HeaderValue,
}

impl Asset {
    // Weak because response compression re-encodes the body without touching
    // this header, so one validator names both the identity and the encoded
    // representation; weak comparison is what If-None-Match uses anyway.
    fn new(bytes: Bytes, sha256: [u8; 32]) -> Self {
        let mut tag = String::with_capacity(68);
        tag.push_str("W/\"");
        for byte in sha256 {
            let _ = write!(tag, "{byte:02x}");
        }
        tag.push('"');
        let etag = HeaderValue::try_from(tag).unwrap_or(HeaderValue::from_static("W/\"\""));
        Self { bytes, etag }
    }
}

pub trait AssetSource: Send + Sync + 'static {
    fn load(&self, key: &AssetKey) -> io::Result<Option<Asset>>;
}

#[cfg(not(feature = "dev-assets"))]
pub use embedded::EmbeddedDist as BuiltDist;

#[cfg(feature = "dev-assets")]
pub use dist_directory::DistDirectory as BuiltDist;

#[cfg(not(feature = "dev-assets"))]
mod embedded {
    use std::borrow::Cow;
    use std::io;

    use axum::body::Bytes;
    use rust_embed::RustEmbed;

    use super::{Asset, AssetKey, AssetSource};

    // Unoptimized builds tolerate a missing dist so the Rust lint and test
    // gates need no frontend build; an optimized build fails without it, so a
    // release binary can never ship without the SPA.
    #[derive(RustEmbed)]
    #[folder = "../web/dist"]
    #[cfg_attr(debug_assertions, allow_missing = true)]
    struct Dist;

    #[derive(Debug, Clone, Copy, Default)]
    pub struct EmbeddedDist;

    impl EmbeddedDist {
        pub fn built() -> Self {
            Self
        }

        #[cfg(test)]
        pub fn names() -> impl Iterator<Item = Cow<'static, str>> {
            Dist::iter()
        }
    }

    impl AssetSource for EmbeddedDist {
        fn load(&self, key: &AssetKey) -> io::Result<Option<Asset>> {
            Ok(Dist::get(key.as_str()).map(|file| {
                let bytes = match file.data {
                    Cow::Borrowed(bytes) => Bytes::from_static(bytes),
                    Cow::Owned(bytes) => Bytes::from(bytes),
                };
                Asset::new(bytes, file.metadata.sha256_hash())
            }))
        }
    }
}

#[cfg(any(feature = "dev-assets", test))]
pub(crate) mod dist_directory {
    use std::fs::File;
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};

    use axum::body::Bytes;
    use sha2::{Digest, Sha256};

    use super::{Asset, AssetKey, AssetSource};

    const MAX_DEV_ASSET_BYTES: u64 = 64 * 1024 * 1024;

    #[derive(Debug, Clone)]
    pub struct DistDirectory {
        root: PathBuf,
    }

    impl DistDirectory {
        #[cfg(feature = "dev-assets")]
        pub fn built() -> Self {
            Self {
                root: Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist"),
            }
        }

        #[cfg(test)]
        pub fn at(root: &Path) -> Self {
            Self {
                root: root.to_owned(),
            }
        }

        fn resolve(&self, key: &AssetKey) -> io::Result<Option<PathBuf>> {
            let root = match self.root.canonicalize() {
                Ok(root) => root,
                Err(error) if is_absent(&error) => return Ok(None),
                Err(error) => return Err(error),
            };
            match root.join(key.as_str()).canonicalize() {
                Ok(path) if path.starts_with(&root) => Ok(Some(path)),
                Ok(_) => Ok(None),
                Err(error) if is_absent(&error) => Ok(None),
                Err(error) => Err(error),
            }
        }
    }

    fn is_absent(error: &io::Error) -> bool {
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
        )
    }

    impl AssetSource for DistDirectory {
        fn load(&self, key: &AssetKey) -> io::Result<Option<Asset>> {
            let Some(path) = self.resolve(key)? else {
                return Ok(None);
            };
            let file = File::open(path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Ok(None);
            }
            if metadata.len() > MAX_DEV_ASSET_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "built asset exceeds the development asset size cap",
                ));
            }
            let mut bytes = Vec::new();
            file.take(MAX_DEV_ASSET_BYTES).read_to_end(&mut bytes)?;
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            Ok(Some(Asset::new(Bytes::from(bytes), digest)))
        }
    }
}

#[derive(Clone)]
pub struct StaticAssets {
    source: Arc<dyn AssetSource>,
    shell: Arc<ShellRenderer>,
    metadata: Arc<ShellMetadata>,
}

impl StaticAssets {
    pub fn built(base_url: &PublicBaseUrl) -> Result<Self, ShellInitError> {
        Self::from_source(BuiltDist::built(), base_url)
    }

    pub fn from_source(
        source: impl AssetSource,
        base_url: &PublicBaseUrl,
    ) -> Result<Self, ShellInitError> {
        let index = source
            .load(&AssetKey::index())
            .map_err(ShellInitError::UnreadableIndex)?
            .ok_or(ShellInitError::MissingIndex)?;
        let shell = ShellRenderer::new(&index.bytes, base_url)?;
        Ok(Self {
            source: Arc::new(source),
            shell: Arc::new(shell),
            metadata: Arc::new(ShellMetadata::fresh_install()),
        })
    }

    pub fn fallback(self) -> MethodRouter {
        any(move |request: Request| {
            let assets = self.clone();
            async move { assets.serve(&request) }
        })
        .layer(response_compression())
    }

    fn serve(&self, request: &Request) -> Response {
        let request_id = RequestId::of(request);
        match self.resolve(request) {
            Ok(Resolution::Asset { key, asset }) => {
                respond_with_asset(request.method(), request.headers(), &key, asset)
            }
            Ok(Resolution::Shell) => self.respond_with_shell(request),
            Ok(Resolution::NotFound) => {
                tag_error(ApiError::new(ErrorCode::NotFound), request_id.as_ref()).into_response()
            }
            Ok(Resolution::MethodNotAllowed) => {
                let mut response = tag_error(
                    ApiError::new(ErrorCode::MethodNotAllowed),
                    request_id.as_ref(),
                )
                .into_response();
                response.headers_mut().insert(ALLOW, GET_HEAD);
                response
            }
            Err(error) => {
                tracing::error!(error = %error, "a built frontend asset could not be read");
                tag_error(ApiError::internal(), request_id.as_ref()).into_response()
            }
        }
    }

    fn resolve(&self, request: &Request) -> io::Result<Resolution> {
        let path = request.uri().path();
        if is_non_spa_path(path) {
            return Ok(Resolution::NotFound);
        }
        let readable = matches!(*request.method(), Method::GET | Method::HEAD);
        if let Some(key) = AssetKey::from_request_path(path) {
            if key.is_index() {
                return Ok(if readable {
                    Resolution::Shell
                } else {
                    Resolution::MethodNotAllowed
                });
            }
            if let Some(asset) = self.source.load(&key)? {
                return Ok(if readable {
                    Resolution::Asset { key, asset }
                } else {
                    Resolution::MethodNotAllowed
                });
            }
        }
        Ok(if readable && accepts_html(request.headers()) {
            Resolution::Shell
        } else {
            Resolution::NotFound
        })
    }

    // The body embeds this response's CSP nonce, so If-None-Match is ignored:
    // a 304 would revive a cached shell whose nonce no longer matches the
    // policy header sent with it.
    fn respond_with_shell(&self, request: &Request) -> Response {
        let request_id = RequestId::of(request);
        let Some(nonce) = request.extensions().get::<CspNonce>() else {
            tracing::error!("the SPA shell was requested without a CSP nonce");
            return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
        };
        let html = match self.shell.render_default(&self.metadata, nonce) {
            Ok(html) => Bytes::from(html),
            Err(error) => {
                tracing::error!(error = %error, "the SPA shell could not be rendered");
                return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
            }
        };
        let digest: [u8; 32] = Sha256::digest(&html).into();
        let shell = Asset::new(html, digest);
        let length = HeaderValue::from(shell.bytes.len());
        let body = if request.method() == Method::HEAD {
            Body::empty()
        } else {
            Body::from(shell.bytes)
        };
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, AssetKey::index().content_type()),
                (CONTENT_LENGTH, length),
                (CACHE_CONTROL, REVALIDATE),
                (ETAG, shell.etag),
            ],
            body,
        )
            .into_response()
    }
}

enum Resolution {
    Asset { key: AssetKey, asset: Asset },
    Shell,
    NotFound,
    MethodNotAllowed,
}

fn is_non_spa_path(path: &str) -> bool {
    NON_SPA_NAMESPACES.iter().any(|namespace| {
        path.get(..namespace.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(namespace))
            && matches!(path.as_bytes().get(namespace.len()), None | Some(b'/'))
    })
}

fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|range| {
            let mut parts = range.split(';');
            let media = parts.next().unwrap_or_default().trim();
            media.eq_ignore_ascii_case("text/html") && !parts.any(is_zero_quality)
        })
}

fn is_zero_quality(parameter: &str) -> bool {
    let Some((name, value)) = parameter.split_once('=') else {
        return false;
    };
    let value = value.trim();
    name.trim().eq_ignore_ascii_case("q")
        && value.starts_with('0')
        && value.bytes().all(|byte| matches!(byte, b'0' | b'.'))
}

fn matches_validator(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Ok(etag) = etag.to_str() else {
        return false;
    };
    let current = opaque_tag(etag);
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || opaque_tag(candidate) == current)
}

fn opaque_tag(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

fn respond_with_asset(
    method: &Method,
    headers: &HeaderMap,
    key: &AssetKey,
    asset: Asset,
) -> Response {
    let cache_control = key.cache_control();
    if matches_validator(headers, &asset.etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(CACHE_CONTROL, cache_control), (ETAG, asset.etag)],
        )
            .into_response();
    }
    let length = HeaderValue::from(asset.bytes.len());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(asset.bytes)
    };
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, key.content_type()),
            (CONTENT_LENGTH, length),
            (CACHE_CONTROL, cache_control),
            (ETAG, asset.etag),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests;
