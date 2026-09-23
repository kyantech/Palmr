use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::Response;
use http::header::{
    ACCEPT, ACCEPT_ENCODING, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH,
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use rstest::rstest;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use time::macros::datetime;
use tower::{Service, ServiceExt};

use super::dist_directory::DistDirectory;
use super::{accepts_html, is_non_spa_path, Asset, AssetKey, AssetSource, StaticAssets};
use crate::app::health::Health;
use crate::app::lifecycle::Readiness;
use crate::app::router::{application_routes, serve_unmatched, with_middleware, HttpEdge};
use crate::app::state::AppState;
use crate::config::{ConfigWarning, EnvironmentSource, OperatorConfig, TrustProxy};
use crate::domain::clock::TestClock;
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const BROWSER_NAVIGATION: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8";
const BODY_CAP: usize = 1024 * 1024;

const INDEX: &str = "<!doctype html><html><head><title>Palmr</title>\
    <script type=\"module\" crossorigin src=\"./assets/index-C0iiOcF1.js\"></script>\
    <link rel=\"stylesheet\" crossorigin href=\"./assets/index-B5BXDqMa.css\"></head>\
    <body><div id=\"root\"></div></body></html>";
const SCRIPT_PATH: &str = "/assets/index-C0iiOcF1.js";
const STYLE_PATH: &str = "/assets/index-B5BXDqMa.css";
const OUTSIDE_SECRET: &str = "palmr-static-outside-root-sentinel";

struct Dist {
    _temp: TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn script() -> String {
    format!("export const palmr = {:?};\n", "a".repeat(2048))
}

fn style() -> String {
    format!("body {{ margin: 0; }}\n/* {} */\n", "b".repeat(2048))
}

fn write(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn built_dist() -> Dist {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dist");
    let outside = temp.path().join("secret.txt");
    write(&outside, OUTSIDE_SECRET.as_bytes());
    write(&root.join("index.html"), INDEX.as_bytes());
    write(
        &root.join(SCRIPT_PATH.trim_start_matches('/')),
        script().as_bytes(),
    );
    write(
        &root.join(STYLE_PATH.trim_start_matches('/')),
        style().as_bytes(),
    );
    write(&root.join("favicon.ico"), &[0, 0, 1, 0, 1, 0]);
    write(
        &root.join("assets/logo-D4x9.svg"),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    );
    write(&root.join(".gitkeep"), b"");
    std::os::unix::fs::symlink(&outside, root.join("assets/escape-Ab12.js")).unwrap();
    Dist {
        _temp: temp,
        root,
        outside,
    }
}

fn empty_dist() -> Dist {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dist");
    fs::create_dir_all(&root).unwrap();
    let outside = temp.path().join("secret.txt");
    Dist {
        _temp: temp,
        root,
        outside,
    }
}

fn app(
    dist: &Dist,
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let readiness = Readiness::new();
    readiness.set_for_test(true);
    let routes = application_routes().build().unwrap().router;
    let router = serve_unmatched(
        routes,
        StaticAssets::from_source(DistDirectory::at(&dist.root)),
    )
    .with_state(AppState::new(clock.clone(), Health::new(readiness)));
    let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
        &str,
        &str,
    )>()))
    .unwrap()
    .config;
    let edge = HttpEdge::new(
        clock,
        TrustedProxies::new(&TrustProxy::Off),
        SecurityHeaders::new(&config),
    );
    with_middleware(router, &edge)
}

struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Fetched {
    fn header(&self, name: HeaderName) -> &str {
        self.headers
            .get(&name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .to_str()
            .unwrap()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

async fn send(dist: &Dist, request: Request) -> Fetched {
    let response = app(dist).oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let body = Limited::new(body, BODY_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    Fetched {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

fn request(method: Method, path: &str, accept: Option<&str>) -> Request {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(accept) = accept {
        builder = builder.header(ACCEPT, accept);
    }
    builder.body(Body::empty()).unwrap()
}

async fn navigate(dist: &Dist, method: Method, path: &str) -> Fetched {
    send(dist, request(method, path, Some(BROWSER_NAVIGATION))).await
}

fn weak_sha256(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("W/\"{hex}\"")
}

fn assert_not_html(fetched: &Fetched, context: &str) {
    let content_type = fetched
        .headers
        .get(CONTENT_TYPE)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    assert!(!content_type.contains("html"), "{context}: {content_type}");
    assert!(!fetched.body.starts_with(b"<"), "{context}: HTML body");
}

fn assert_json_error(fetched: &Fetched, status: StatusCode, code: &str, context: &str) {
    assert_eq!(fetched.status, status, "{context}");
    assert_eq!(
        fetched.header(CONTENT_TYPE),
        "application/json; charset=utf-8",
        "{context}"
    );
    assert_not_html(fetched, context);
    let body = fetched.json();
    assert_eq!(body["error"]["code"], code, "{context}");
    assert_eq!(
        body["error"]["requestId"],
        fetched.header(X_REQUEST_ID),
        "{context}"
    );
}

fn assert_shell(fetched: &Fetched, context: &str) {
    assert_eq!(fetched.status, StatusCode::OK, "{context}");
    assert_eq!(
        fetched.header(CONTENT_TYPE),
        "text/html; charset=utf-8",
        "{context}"
    );
    assert_eq!(fetched.header(CACHE_CONTROL), "no-cache", "{context}");
    assert_eq!(
        fetched.header(ETAG),
        weak_sha256(INDEX.as_bytes()),
        "{context}"
    );
}

fn assert_global_headers(fetched: &Fetched, context: &str) {
    assert!(!fetched.header(X_REQUEST_ID).is_empty(), "{context}");
    let csp = fetched.header(CONTENT_SECURITY_POLICY);
    assert!(csp.contains("frame-ancestors 'none'"), "{context}: {csp}");
    assert!(!csp.contains("unsafe-inline"), "{context}: {csp}");
    assert_eq!(
        fetched.header(HeaderName::from_static("x-content-type-options")),
        "nosniff"
    );
    assert_eq!(
        fetched.header(HeaderName::from_static("cross-origin-resource-policy")),
        "same-origin",
        "{context}"
    );
}

#[tokio::test]
async fn it_spa_fallback_only_for_html_get() {
    let dist = built_dist();

    for path in [
        "/",
        "/workspaces",
        "/files/folder/nested",
        "/s/holiday",
        "/r/inbox",
    ] {
        let fetched = navigate(&dist, Method::GET, path).await;
        assert_shell(&fetched, path);
        assert_eq!(fetched.body, INDEX.as_bytes(), "{path}");
        assert_eq!(fetched.header(CONTENT_LENGTH), INDEX.len().to_string());
    }

    let head = navigate(&dist, Method::HEAD, "/workspaces").await;
    assert_shell(&head, "HEAD /workspaces");
    assert!(head.body.is_empty());
    assert_eq!(head.header(CONTENT_LENGTH), INDEX.len().to_string());

    for method in [
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        let context = format!("{method} /workspaces");
        let fetched = navigate(&dist, method, "/workspaces").await;
        assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", &context);
    }

    for accept in [
        None,
        Some("*/*"),
        Some("application/json"),
        Some("text/*"),
        Some("text/html;q=0"),
        Some("application/json, text/html; q=0.000"),
    ] {
        let context = format!("GET /workspaces Accept {accept:?}");
        let fetched = send(&dist, request(Method::GET, "/workspaces", accept)).await;
        assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", &context);
    }

    let missing_chunk = send(
        &dist,
        request(Method::GET, "/assets/index-Stale000.js", Some("*/*")),
    )
    .await;
    assert_json_error(
        &missing_chunk,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "stale chunk",
    );
}

#[tokio::test]
async fn it_api_unknown_path_json_404() {
    let dist = built_dist();
    for path in [
        "/api",
        "/api/",
        "/api/v1",
        "/api/v1/does-not-exist",
        "/api/v1/files/0192f3a7/content",
        "/api/v2/anything",
        "/API/v1/does-not-exist",
        "/api/../index.html",
    ] {
        for method in [Method::GET, Method::HEAD, Method::POST, Method::DELETE] {
            let context = format!("{method} {path}");
            let fetched = navigate(&dist, method.clone(), path).await;
            if method == Method::HEAD {
                assert_eq!(fetched.status, StatusCode::NOT_FOUND, "{context}");
                assert_not_html(&fetched, &context);
                assert!(fetched.body.is_empty(), "{context}");
            } else {
                assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", &context);
            }
        }
    }
}

#[tokio::test]
async fn it_reserved_namespaces_never_fall_back() {
    let dist = built_dist();
    for path in [
        "/openapi.json",
        "/docs",
        "/docs/scalar.js",
        "/health/unknown",
        "/health/live/extra",
        "/e",
        "/e/not-real",
        "/E/not-real",
    ] {
        let fetched = navigate(&dist, Method::GET, path).await;
        assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", path);
    }
}

#[tokio::test]
async fn it_health_routes_win_over_static_fallback() {
    let dist = built_dist();
    for (path, status) in [
        ("/health", "ok"),
        ("/health/live", "ok"),
        ("/health/ready", "ready"),
    ] {
        let fetched = navigate(&dist, Method::GET, path).await;
        assert_eq!(fetched.status, StatusCode::OK, "{path}");
        assert_eq!(
            fetched.header(CONTENT_TYPE),
            "application/json; charset=utf-8"
        );
        assert_eq!(fetched.json()["status"], status, "{path}");
    }

    let wrong_method = navigate(&dist, Method::POST, "/health/live").await;
    assert_json_error(
        &wrong_method,
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "POST /health/live",
    );
    assert!(wrong_method.header(ALLOW).contains("GET"));
    assert_global_headers(&wrong_method, "POST /health/live");
}

#[tokio::test]
async fn it_assets_immutable_cache_headers() {
    let dist = built_dist();
    let cases = [
        (SCRIPT_PATH, script(), "text/javascript; charset=utf-8"),
        (STYLE_PATH, style(), "text/css; charset=utf-8"),
        (
            "/assets/logo-D4x9.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_owned(),
            "image/svg+xml",
        ),
    ];
    for (path, contents, content_type) in cases {
        let fetched = send(&dist, request(Method::GET, path, Some("*/*"))).await;
        assert_eq!(fetched.status, StatusCode::OK, "{path}");
        assert_eq!(fetched.body, contents.as_bytes(), "{path}");
        assert_eq!(fetched.header(CONTENT_TYPE), content_type, "{path}");
        assert_eq!(
            fetched.header(CONTENT_LENGTH),
            contents.len().to_string(),
            "{path}"
        );
        assert_eq!(
            fetched.header(CACHE_CONTROL),
            "public, max-age=31536000, immutable",
            "{path}"
        );
        let etag = fetched.header(ETAG).to_owned();
        assert_eq!(etag, weak_sha256(contents.as_bytes()), "{path}");
        assert_global_headers(&fetched, path);

        let again = send(&dist, request(Method::GET, path, None)).await;
        assert_eq!(again.header(ETAG), etag, "{path}");

        let head = send(&dist, request(Method::HEAD, path, None)).await;
        assert_eq!(head.status, StatusCode::OK, "{path}");
        assert!(head.body.is_empty(), "{path}");
        for name in [CONTENT_TYPE, CONTENT_LENGTH, CACHE_CONTROL, ETAG] {
            assert_eq!(head.headers[&name], fetched.headers[&name], "{path} {name}");
        }

        let mut conditional = request(Method::GET, path, None);
        conditional
            .headers_mut()
            .insert(IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        let not_modified = send(&dist, conditional).await;
        assert_eq!(not_modified.status, StatusCode::NOT_MODIFIED, "{path}");
        assert!(not_modified.body.is_empty(), "{path}");
        assert_eq!(not_modified.header(ETAG), etag, "{path}");
        assert_eq!(
            not_modified.header(CACHE_CONTROL),
            "public, max-age=31536000, immutable"
        );

        let posted = send(&dist, request(Method::POST, path, None)).await;
        assert_json_error(
            &posted,
            StatusCode::METHOD_NOT_ALLOWED,
            "METHOD_NOT_ALLOWED",
            path,
        );
        assert_eq!(posted.header(ALLOW), "GET, HEAD");
    }
}

#[tokio::test]
async fn it_index_html_no_cache() {
    let dist = built_dist();
    for path in ["/", "/index.html", "/workspaces/deep/link"] {
        let fetched = navigate(&dist, Method::GET, path).await;
        assert_shell(&fetched, path);
        assert!(!fetched.header(CACHE_CONTROL).contains("immutable"));
        assert!(!fetched.header(CACHE_CONTROL).contains("max-age"));
        assert_global_headers(&fetched, path);
    }

    let direct = send(&dist, request(Method::GET, "/index.html", Some("*/*"))).await;
    assert_shell(&direct, "GET /index.html Accept */*");

    let mut conditional = request(Method::GET, "/overview", Some(BROWSER_NAVIGATION));
    conditional.headers_mut().insert(
        IF_NONE_MATCH,
        HeaderValue::from_str(&format!("\"unrelated\", {}", weak_sha256(INDEX.as_bytes())))
            .unwrap(),
    );
    let not_modified = send(&dist, conditional).await;
    assert_eq!(not_modified.status, StatusCode::NOT_MODIFIED);
    assert!(not_modified.body.is_empty());
    assert_eq!(not_modified.header(CACHE_CONTROL), "no-cache");

    let mut stale = request(Method::GET, "/overview", Some(BROWSER_NAVIGATION));
    stale
        .headers_mut()
        .insert(IF_NONE_MATCH, HeaderValue::from_static("W/\"stale\""));
    assert_shell(&send(&dist, stale).await, "stale validator");

    let favicon = send(&dist, request(Method::GET, "/favicon.ico", None)).await;
    assert_eq!(favicon.status, StatusCode::OK);
    assert_eq!(favicon.header(CONTENT_TYPE), "image/x-icon");
    assert_eq!(favicon.header(CACHE_CONTROL), "no-cache");
}

#[tokio::test]
async fn it_static_responses_use_shared_compression() {
    let dist = built_dist();
    let mut compressed = request(Method::GET, SCRIPT_PATH, None);
    compressed
        .headers_mut()
        .insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let fetched = send(&dist, compressed).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.header(CONTENT_ENCODING), "gzip");
    assert_eq!(fetched.header(VARY), "accept-encoding");
    assert!(fetched.headers.get(CONTENT_LENGTH).is_none());
    assert_eq!(fetched.header(ETAG), weak_sha256(script().as_bytes()));
}

#[tokio::test]
async fn it_static_paths_cannot_escape_the_build_root() {
    let dist = built_dist();
    for path in [
        "/.gitkeep",
        "/assets/escape-Ab12.js",
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/assets/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/assets/..%5c..%5csecret.txt",
        "/%2Fetc%2Fpasswd",
    ] {
        let fetched = send(&dist, request(Method::GET, path, Some("*/*"))).await;
        assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", path);
        let html = navigate(&dist, Method::GET, path).await;
        assert!(
            html.status == StatusCode::NOT_FOUND || html.body == INDEX.as_bytes(),
            "{path}"
        );
        for body in [&fetched.body, &html.body] {
            assert!(
                !String::from_utf8_lossy(body).contains(OUTSIDE_SECRET),
                "{path}"
            );
        }
    }
    assert!(dist.outside.exists());
}

#[tokio::test]
async fn it_missing_shell_is_json_not_found() {
    let dist = empty_dist();
    let fetched = navigate(&dist, Method::GET, "/workspaces").await;
    assert_json_error(&fetched, StatusCode::NOT_FOUND, "NOT_FOUND", "no shell");
}

#[rstest]
#[case::index("/index.html", Some("index.html"))]
#[case::hashed("/assets/index-C0iiOcF1.js", Some("assets/index-C0iiOcF1.js"))]
#[case::root_file("/favicon.ico", Some("favicon.ico"))]
#[case::slash("/", None)]
#[case::parent("/../secret", None)]
#[case::current("/./index.html", None)]
#[case::nested_parent("/assets/../index.html", None)]
#[case::dotfile("/.env", None)]
#[case::encoded_dots("/%2e%2e/secret", None)]
#[case::encoded_slash("/assets%2Findex.js", None)]
#[case::backslash("/..\\secret", None)]
#[case::drive("/C:/secret", None)]
#[case::empty_segment("/assets//index.js", None)]
#[case::relative("index.html", None)]
#[case::nul("/index\0.html", None)]
fn unit_asset_key_accepts_only_normalized_build_paths(
    #[case] path: &str,
    #[case] expected: Option<&str>,
) {
    assert_eq!(
        AssetKey::from_request_path(path)
            .as_ref()
            .map(AssetKey::as_str),
        expected
    );
}

#[rstest]
#[case::html("text/html", true)]
#[case::navigation(BROWSER_NAVIGATION, true)]
#[case::case_insensitive("TEXT/HTML", true)]
#[case::weighted("application/json, text/html;q=0.5", true)]
#[case::spaced(" text/html ; charset=utf-8", true)]
#[case::wildcard("*/*", false)]
#[case::text_wildcard("text/*", false)]
#[case::json("application/json", false)]
#[case::refused("text/html;q=0", false)]
#[case::refused_precise("text/html; q=0.000", false)]
#[case::xhtml_only("application/xhtml+xml", false)]
#[case::empty("", false)]
fn unit_accepts_html(#[case] accept: &str, #[case] expected: bool) {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_str(accept).unwrap());
    assert_eq!(accepts_html(&headers), expected);
}

#[test]
fn unit_accepts_html_across_repeated_headers() {
    let mut headers = HeaderMap::new();
    assert!(!accepts_html(&headers));
    headers.append(ACCEPT, HeaderValue::from_static("application/json"));
    headers.append(ACCEPT, HeaderValue::from_static("text/html"));
    assert!(accepts_html(&headers));
}

#[rstest]
#[case("/api", true)]
#[case("/api/v1/files", true)]
#[case("/Api/V1", true)]
#[case("/openapi.json", true)]
#[case("/docs", true)]
#[case("/docs/index.html", true)]
#[case("/health", true)]
#[case("/health/ready", true)]
#[case("/e", true)]
#[case("/e/token", true)]
#[case("/apis", false)]
#[case("/openapi.jsonx", false)]
#[case("/docsearch", false)]
#[case("/healthy", false)]
#[case("/events", false)]
#[case("/s/alias", false)]
#[case("/r/alias", false)]
#[case("/manifest.webmanifest", false)]
#[case("/assets/index.js", false)]
#[case("/", false)]
fn unit_non_spa_namespaces(#[case] path: &str, #[case] excluded: bool) {
    assert_eq!(is_non_spa_path(path), excluded);
}

#[rstest]
#[case("index.html", "text/html; charset=utf-8")]
#[case("assets/a.js", "text/javascript; charset=utf-8")]
#[case("assets/a.mjs", "application/javascript; charset=utf-8")]
#[case("assets/a.css", "text/css; charset=utf-8")]
#[case("assets/en-US.json", "application/json; charset=utf-8")]
#[case("manifest.webmanifest", "application/manifest+json; charset=utf-8")]
#[case("assets/a.svg", "image/svg+xml")]
#[case("assets/a.png", "image/png")]
#[case("assets/a.jpg", "image/jpeg")]
#[case("assets/a.jpeg", "image/jpeg")]
#[case("assets/a.webp", "image/webp")]
#[case("favicon.ico", "image/x-icon")]
#[case("assets/a.woff", "application/font-woff")]
#[case("assets/a.woff2", "font/woff2")]
#[case("assets/a.wasm", "application/wasm")]
#[case("assets/a.unknownext", "application/octet-stream")]
fn unit_content_type_by_extension(#[case] key: &str, #[case] expected: &str) {
    assert_eq!(AssetKey(key.to_owned()).content_type(), expected);
}

#[test]
fn unit_etag_is_a_deterministic_content_hash() {
    let first = Asset::new(
        Bytes::from_static(b"palmr"),
        Sha256::digest(b"palmr").into(),
    );
    let same = Asset::new(
        Bytes::from_static(b"palmr"),
        Sha256::digest(b"palmr").into(),
    );
    let different = Asset::new(
        Bytes::from_static(b"palmr!"),
        Sha256::digest(b"palmr!").into(),
    );
    assert_eq!(first.etag, same.etag);
    assert_ne!(first.etag, different.etag);
    assert_eq!(first.etag.to_str().unwrap(), weak_sha256(b"palmr"));
    assert_eq!(first.etag.len(), 68);
}

#[test]
fn unit_dist_directory_rejects_unvalidated_escapes() {
    let dist = built_dist();
    let source = DistDirectory::at(&dist.root);
    for key in [
        "../secret.txt",
        "assets/../../secret.txt",
        "assets/escape-Ab12.js",
        "assets",
        "missing.js",
        "index.html/child",
    ] {
        assert_eq!(
            source.load(&AssetKey(key.to_owned())).unwrap(),
            None,
            "{key}"
        );
    }
    let absolute = AssetKey(dist.outside.to_str().unwrap().to_owned());
    assert_eq!(source.load(&absolute).unwrap(), None);

    let index = source.load(&AssetKey::index()).unwrap().unwrap();
    assert_eq!(index.bytes, INDEX.as_bytes());
}

#[test]
fn unit_dist_directory_missing_root_serves_nothing() {
    let temp = TempDir::new().unwrap();
    let source = DistDirectory::at(&temp.path().join("absent"));
    assert_eq!(source.load(&AssetKey::index()).unwrap(), None);
}

#[cfg(not(feature = "dev-assets"))]
#[test]
fn unit_embedded_and_filesystem_sources_agree() {
    use super::embedded::EmbeddedDist;

    let embedded = EmbeddedDist::built();
    let filesystem = DistDirectory::at(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist"));
    for name in EmbeddedDist::names() {
        let Some(key) = AssetKey::from_request_path(&format!("/{name}")) else {
            assert!(
                name.split('/').any(|segment| segment.starts_with('.')),
                "built asset {name} is unreachable"
            );
            continue;
        };
        let from_binary = embedded.load(&key).unwrap().unwrap();
        let from_disk = filesystem.load(&key).unwrap().unwrap();
        assert_eq!(from_binary.bytes, from_disk.bytes, "{name}");
        assert_eq!(from_binary.etag, from_disk.etag, "{name}");
        assert_eq!(
            from_binary.etag.to_str().unwrap(),
            weak_sha256(&from_binary.bytes),
            "{name}"
        );
    }
}

#[test]
fn unit_asset_location_is_not_runtime_configuration() {
    let names = [
        "PALMR_ASSET_DIR",
        "PALMR_ASSETS_DIR",
        "PALMR_STATIC_DIR",
        "PALMR_WEB_DIST",
        "PALMR_DIST_DIR",
    ];
    let loaded = OperatorConfig::load(&EnvironmentSource::from_vars(
        names.map(|name| (name, "/tmp/x")),
    ))
    .unwrap();
    let unknown: Vec<&str> = loaded
        .warnings
        .iter()
        .filter_map(|warning| match warning {
            ConfigWarning::UnknownVariable { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut expected = names.to_vec();
    expected.sort_unstable();
    assert_eq!(unknown, expected);
}
