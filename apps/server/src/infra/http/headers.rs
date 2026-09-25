use std::fmt;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::header::{
    HeaderMap, HeaderName, HeaderValue, CONTENT_SECURITY_POLICY, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use url::Url;

use super::error::ApiError;
use super::request_id::{tag_error, RequestId};
use crate::config::OperatorConfig;
use crate::storage::s3::config::effective_public_endpoint;

pub const CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");
pub const CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");
pub const PERMISSIONS_POLICY: HeaderName = HeaderName::from_static("permissions-policy");

const NOSNIFF: HeaderValue = HeaderValue::from_static("nosniff");
const STRICT_ORIGIN_WHEN_CROSS_ORIGIN: HeaderValue =
    HeaderValue::from_static("strict-origin-when-cross-origin");
const SAME_ORIGIN: HeaderValue = HeaderValue::from_static("same-origin");
const CROSS_ORIGIN: HeaderValue = HeaderValue::from_static("cross-origin");
const DENIED_FEATURES: HeaderValue =
    HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()");

// Used only when a nonce or a header value cannot be produced: the canonical
// policy with every nonce and external origin removed, and never framable.
const NONCE_LESS_CSP: HeaderValue = HeaderValue::from_static(
    "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; \
     media-src 'self' blob:; font-src 'self'; connect-src 'self'; object-src 'self'; \
     frame-src 'self'; form-action 'self'; base-uri 'self'; frame-ancestors 'none'",
);

const NONCE_BYTES: usize = 16;
const HEX: &[u8; 16] = b"0123456789abcdef";

const EMBED_PREFIXES: [&str; 2] = ["/e/", "/api/v1/public/embeds/"];

#[derive(Clone)]
pub struct CspNonce(String);

impl CspNonce {
    fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut bytes)?;
        Ok(Self(
            bytes
                .iter()
                .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
                .map(char::from)
                .collect(),
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for CspNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CspNonce(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityPolicy {
    #[default]
    Default,
    Embed,
}

impl SecurityPolicy {
    pub fn permits_path(self, path: &str) -> bool {
        match self {
            Self::Default => true,
            Self::Embed => EMBED_PREFIXES.iter().any(|prefix| path.starts_with(prefix)),
        }
    }

    pub(crate) fn apply<S>(self, handler: MethodRouter<S>) -> MethodRouter<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        match self {
            Self::Default => handler,
            Self::Embed => handler.route_layer(from_fn(mark_embed_response)),
        }
    }
}

// Only this module can construct the marker, so a handler cannot opt its own
// response into the embed relaxation; only a route declared with
// `SecurityPolicy::Embed` gets it.
#[derive(Clone, Copy)]
struct EmbedResponse(());

async fn mark_embed_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.extensions_mut().insert(EmbedResponse(()));
    response
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StorageOrigin(String);

impl StorageOrigin {
    fn of(url: &Url) -> Option<Self> {
        crate::storage::s3::config::public_origin(url).map(Self)
    }

    fn is_http(&self) -> bool {
        self.0.starts_with("http://")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityHeaders {
    storage_origin: Option<StorageOrigin>,
}

impl SecurityHeaders {
    pub fn new(config: &OperatorConfig) -> Self {
        let storage_origin = effective_public_endpoint(&config.storage).and_then(StorageOrigin::of);
        if let Some(origin) = &storage_origin {
            if config.base_url.url().scheme() == "https" && origin.is_http() {
                tracing::warn!(
                    s3_public_origin = origin.0.as_str(),
                    "the S3 public endpoint uses http under an https base URL; browsers will block direct uploads and previews as mixed content"
                );
            }
        }
        Self { storage_origin }
    }

    fn content_security_policy(&self, nonce: &CspNonce, policy: SecurityPolicy) -> HeaderValue {
        let nonce = format!("'nonce-{}'", nonce.as_str());
        let storage = self
            .storage_origin
            .as_ref()
            .map(|origin| format!(" {}", origin.0))
            .unwrap_or_default();
        let frame_ancestors = match policy {
            SecurityPolicy::Default => "'none'",
            SecurityPolicy::Embed => "*",
        };
        let rendered = format!(
            "default-src 'self'; \
             script-src 'self' {nonce}; \
             style-src 'self' {nonce}; \
             img-src 'self' data: blob:{storage}; \
             media-src 'self' blob:{storage}; \
             font-src 'self'; \
             connect-src 'self'{storage}; \
             object-src 'self'; \
             frame-src 'self'; \
             form-action 'self'; \
             base-uri 'self'; \
             frame-ancestors {frame_ancestors}"
        );
        HeaderValue::try_from(rendered).unwrap_or(NONCE_LESS_CSP)
    }

    fn apply(&self, headers: &mut HeaderMap, nonce: Option<&CspNonce>, policy: SecurityPolicy) {
        let (csp, resource_policy) = match nonce {
            Some(nonce) => (
                self.content_security_policy(nonce, policy),
                match policy {
                    SecurityPolicy::Default => SAME_ORIGIN,
                    SecurityPolicy::Embed => CROSS_ORIGIN,
                },
            ),
            None => (NONCE_LESS_CSP, SAME_ORIGIN),
        };
        headers.insert(CONTENT_SECURITY_POLICY, csp);
        headers.insert(X_CONTENT_TYPE_OPTIONS, NOSNIFF);
        headers.insert(REFERRER_POLICY, STRICT_ORIGIN_WHEN_CROSS_ORIGIN);
        headers.insert(CROSS_ORIGIN_RESOURCE_POLICY, resource_policy);
        headers.insert(CROSS_ORIGIN_OPENER_POLICY, SAME_ORIGIN);
        headers.insert(PERMISSIONS_POLICY, DENIED_FEATURES);
    }
}

pub async fn apply_security_headers(
    State(security): State<Arc<SecurityHeaders>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(nonce) = CspNonce::generate() else {
        tracing::error!("the operating system random source failed; the request was refused");
        let mut response =
            tag_error(ApiError::internal(), RequestId::of(&request).as_ref()).into_response();
        security.apply(response.headers_mut(), None, SecurityPolicy::Default);
        return response;
    };
    request.extensions_mut().insert(nonce.clone());

    let mut response = next.run(request).await;
    let policy = match response.extensions_mut().remove::<EmbedResponse>() {
        Some(_) => SecurityPolicy::Embed,
        None => SecurityPolicy::Default,
    };
    security.apply(response.headers_mut(), Some(&nonce), policy);
    response
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use http::header::{HeaderMap, CONTENT_SECURITY_POLICY};
    use rstest::rstest;
    use url::Url;

    use super::{CspNonce, SecurityHeaders, SecurityPolicy, StorageOrigin, NONCE_LESS_CSP};
    use crate::config::{EnvironmentSource, OperatorConfig};

    const S3: &[(&str, &str)] = &[
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", "http://minio:9000"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", "palmr"),
        ("PALMR_S3_ACCESS_KEY", "AKIAEXAMPLEACCESS"),
        ("PALMR_S3_SECRET_KEY", "example-secret-key-value"),
    ];

    const DIRECTIVES: [&str; 12] = [
        "default-src",
        "script-src",
        "style-src",
        "img-src",
        "media-src",
        "font-src",
        "connect-src",
        "object-src",
        "frame-src",
        "form-action",
        "base-uri",
        "frame-ancestors",
    ];

    fn config(vars: &[(&str, &str)]) -> OperatorConfig {
        OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
            .unwrap()
            .config
    }

    fn with_s3(extra: &[(&'static str, &'static str)]) -> OperatorConfig {
        let mut vars = S3.to_vec();
        vars.extend_from_slice(extra);
        config(&vars)
    }

    fn nonce() -> CspNonce {
        CspNonce::generate().unwrap()
    }

    fn csp(security: &SecurityHeaders, nonce: &CspNonce, policy: SecurityPolicy) -> String {
        security
            .content_security_policy(nonce, policy)
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn directive<'a>(policy: &'a str, name: &str) -> &'a str {
        policy
            .split("; ")
            .find(|directive| directive.split(' ').next() == Some(name))
            .unwrap_or_else(|| panic!("{name} missing from {policy}"))
    }

    #[test]
    fn unit_default_csp_is_canonical() {
        let security = SecurityHeaders::new(&config(&[]));
        let nonce = CspNonce("0123456789abcdef0123456789abcdef".to_owned());
        assert_eq!(
            csp(&security, &nonce, SecurityPolicy::Default),
            "default-src 'self'; \
             script-src 'self' 'nonce-0123456789abcdef0123456789abcdef'; \
             style-src 'self' 'nonce-0123456789abcdef0123456789abcdef'; \
             img-src 'self' data: blob:; \
             media-src 'self' blob:; \
             font-src 'self'; \
             connect-src 'self'; \
             object-src 'self'; \
             frame-src 'self'; \
             form-action 'self'; \
             base-uri 'self'; \
             frame-ancestors 'none'"
        );
    }

    #[test]
    fn unit_csp_directive_order_is_stable() {
        let security = SecurityHeaders::new(&with_s3(&[]));
        for policy in [SecurityPolicy::Default, SecurityPolicy::Embed] {
            let rendered = csp(&security, &nonce(), policy);
            let names: Vec<&str> = rendered
                .split("; ")
                .map(|directive| directive.split(' ').next().unwrap())
                .collect();
            assert_eq!(names, DIRECTIVES);
        }
    }

    #[test]
    fn unit_csp_never_allows_unsafe_sources() {
        for security in [
            SecurityHeaders::new(&config(&[])),
            SecurityHeaders::new(&with_s3(&[])),
        ] {
            for policy in [SecurityPolicy::Default, SecurityPolicy::Embed] {
                let rendered = csp(&security, &nonce(), policy);
                assert!(!rendered.contains("unsafe-"), "{rendered}");
                assert_eq!(directive(&rendered, "object-src"), "object-src 'self'");
                assert_eq!(directive(&rendered, "frame-src"), "frame-src 'self'");
            }
        }
        let fallback = NONCE_LESS_CSP;
        let fallback = fallback.to_str().unwrap();
        assert!(!fallback.contains("unsafe-"));
        assert!(!fallback.contains("nonce-"));
        assert_eq!(
            directive(fallback, "frame-ancestors"),
            "frame-ancestors 'none'"
        );
    }

    #[test]
    fn unit_script_and_style_share_the_nonce() {
        let security = SecurityHeaders::new(&config(&[]));
        let nonce = nonce();
        let rendered = csp(&security, &nonce, SecurityPolicy::Default);
        let source = format!("'nonce-{}'", nonce.as_str());
        assert_eq!(
            directive(&rendered, "script-src"),
            format!("script-src 'self' {source}")
        );
        assert_eq!(
            directive(&rendered, "style-src"),
            format!("style-src 'self' {source}")
        );
        assert_eq!(rendered.matches("nonce-").count(), 2);
    }

    #[test]
    fn unit_nonce_is_fresh_and_header_safe() {
        let nonces: BTreeSet<String> = (0..256).map(|_| nonce().as_str().to_owned()).collect();
        assert_eq!(nonces.len(), 256);
        for nonce in &nonces {
            assert_eq!(nonce.len(), 32);
            assert!(nonce
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
    }

    #[test]
    fn unit_nonce_debug_is_redacted() {
        let nonce = nonce();
        let debug = format!("{nonce:?}");
        assert_eq!(debug, "CspNonce(..)");
        assert!(!debug.contains(nonce.as_str()));
    }

    #[test]
    fn unit_local_storage_adds_no_external_origin() {
        let security = SecurityHeaders::new(&config(&[]));
        assert_eq!(security.storage_origin, None);
        let rendered = csp(&security, &nonce(), SecurityPolicy::Default);
        assert_eq!(directive(&rendered, "connect-src"), "connect-src 'self'");
        assert_eq!(
            directive(&rendered, "img-src"),
            "img-src 'self' data: blob:"
        );
        assert_eq!(directive(&rendered, "media-src"), "media-src 'self' blob:");
        assert!(!rendered.contains("http"));
    }

    #[test]
    fn unit_local_provider_ignores_s3_public_endpoint() {
        let security = SecurityHeaders::new(&config(&[(
            "PALMR_S3_PUBLIC_ENDPOINT",
            "https://files.example.com",
        )]));
        assert_eq!(security.storage_origin, None);
    }

    #[rstest]
    #[case::public_endpoint(
        &[("PALMR_S3_PUBLIC_ENDPOINT", "https://files.example.com")],
        "https://files.example.com"
    )]
    #[case::public_endpoint_port(
        &[("PALMR_S3_PUBLIC_ENDPOINT", "https://files.example.com:9443/")],
        "https://files.example.com:9443"
    )]
    #[case::default_port_elided(
        &[("PALMR_S3_PUBLIC_ENDPOINT", "https://files.example.com:443")],
        "https://files.example.com"
    )]
    #[case::falls_back_to_endpoint(&[], "http://minio:9000")]
    fn unit_s3_origin_is_placed_in_data_plane_directives(
        #[case] extra: &[(&'static str, &'static str)],
        #[case] origin: &str,
    ) {
        let security = SecurityHeaders::new(&with_s3(extra));
        let rendered = csp(&security, &nonce(), SecurityPolicy::Default);
        assert_eq!(
            directive(&rendered, "connect-src"),
            format!("connect-src 'self' {origin}")
        );
        assert_eq!(
            directive(&rendered, "img-src"),
            format!("img-src 'self' data: blob: {origin}")
        );
        assert_eq!(
            directive(&rendered, "media-src"),
            format!("media-src 'self' blob: {origin}")
        );
        assert_eq!(rendered.matches(origin).count(), 3, "{rendered}");
        for unrelated in [
            "default-src",
            "script-src",
            "style-src",
            "font-src",
            "object-src",
            "frame-src",
            "form-action",
            "base-uri",
            "frame-ancestors",
        ] {
            assert!(
                !directive(&rendered, unrelated).contains(origin),
                "{unrelated}"
            );
        }
    }

    #[rstest]
    #[case::path("https://files.example.com/some/prefix", "https://files.example.com")]
    #[case::query(
        "https://files.example.com/?X-Amz-Signature=abc",
        "https://files.example.com"
    )]
    #[case::fragment("https://files.example.com/#part", "https://files.example.com")]
    #[case::credentials("https://key:secret@files.example.com/", "https://files.example.com")]
    #[case::port(
        "https://files.example.com:9443/bucket",
        "https://files.example.com:9443"
    )]
    #[case::uppercase_host("https://FILES.Example.COM", "https://files.example.com")]
    #[case::idn("https://bücher.example", "https://xn--bcher-kva.example")]
    #[case::ipv6("http://[::1]:9000/", "http://[::1]:9000")]
    fn unit_storage_origin_is_scheme_host_port_only(#[case] url: &str, #[case] expected: &str) {
        let origin = StorageOrigin::of(&Url::parse(url).unwrap()).unwrap();
        assert_eq!(origin.0, expected);
    }

    #[test]
    fn unit_storage_origin_rejects_opaque_origins() {
        assert_eq!(
            StorageOrigin::of(&Url::parse("data:text/plain,hi").unwrap()),
            None
        );
    }

    #[test]
    fn unit_embed_policy_is_distinct_and_only_relaxes_framing() {
        let security = SecurityHeaders::new(&with_s3(&[]));
        let nonce = nonce();
        let strict = csp(&security, &nonce, SecurityPolicy::Default);
        let embed = csp(&security, &nonce, SecurityPolicy::Embed);
        assert_ne!(SecurityPolicy::Default, SecurityPolicy::Embed);
        assert_eq!(SecurityPolicy::default(), SecurityPolicy::Default);
        assert_eq!(
            directive(&strict, "frame-ancestors"),
            "frame-ancestors 'none'"
        );
        assert_eq!(directive(&embed, "frame-ancestors"), "frame-ancestors *");
        assert_eq!(
            strict.replace("frame-ancestors 'none'", "frame-ancestors *"),
            embed
        );
    }

    #[test]
    fn unit_resource_policy_follows_security_policy() {
        let security = SecurityHeaders::new(&config(&[]));
        let nonce = nonce();
        let mut strict = HeaderMap::new();
        security.apply(&mut strict, Some(&nonce), SecurityPolicy::Default);
        let mut embed = HeaderMap::new();
        security.apply(&mut embed, Some(&nonce), SecurityPolicy::Embed);
        assert_eq!(strict["cross-origin-resource-policy"], "same-origin");
        assert_eq!(embed["cross-origin-resource-policy"], "cross-origin");
        assert_eq!(strict["cross-origin-opener-policy"], "same-origin");
        assert_eq!(embed["cross-origin-opener-policy"], "same-origin");
    }

    #[test]
    fn unit_missing_nonce_falls_back_to_strict_policy() {
        let security = SecurityHeaders::new(&with_s3(&[]));
        let mut headers = HeaderMap::new();
        security.apply(&mut headers, None, SecurityPolicy::Embed);
        assert_eq!(headers[CONTENT_SECURITY_POLICY], NONCE_LESS_CSP);
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
    }

    #[test]
    fn unit_apply_replaces_existing_values() {
        let security = SecurityHeaders::new(&config(&[]));
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_SECURITY_POLICY, "default-src *".parse().unwrap());
        headers.append(CONTENT_SECURITY_POLICY, "script-src *".parse().unwrap());
        headers.insert("referrer-policy", "unsafe-url".parse().unwrap());
        security.apply(&mut headers, Some(&nonce()), SecurityPolicy::Default);
        security.apply(&mut headers, Some(&nonce()), SecurityPolicy::Default);
        for name in [
            "content-security-policy",
            "x-content-type-options",
            "referrer-policy",
            "cross-origin-resource-policy",
            "cross-origin-opener-policy",
            "permissions-policy",
        ] {
            assert_eq!(headers.get_all(name).iter().count(), 1, "{name}");
        }
        assert_eq!(
            headers["referrer-policy"],
            "strict-origin-when-cross-origin"
        );
    }

    #[rstest]
    #[case::viewer("/e/{token}", true)]
    #[case::metadata("/api/v1/public/embeds/{token}", true)]
    #[case::bare_prefix("/e", false)]
    #[case::lookalike("/embed/{token}", false)]
    #[case::share("/api/v1/public/shares/{alias}", false)]
    #[case::nested_word("/api/v1/files/{id}/embeds", false)]
    #[case::sibling("/api/v1/public/embedsx/{token}", false)]
    fn unit_embed_policy_is_confined_to_embed_prefixes(#[case] path: &str, #[case] embed: bool) {
        assert_eq!(SecurityPolicy::Embed.permits_path(path), embed);
        assert!(SecurityPolicy::Default.permits_path(path));
    }
}
