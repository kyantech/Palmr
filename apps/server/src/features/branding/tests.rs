use std::convert::Infallible;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::Response;
use http::header::{
    ACCEPT, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE,
    ETAG, IF_NONE_MATCH, X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use proptest::prelude::*;
use rstest::rstest;
use serde_json::{json, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tower::{Service, ServiceExt};
use unicode_segmentation::UnicodeSegmentation;

use super::manifest::{
    short_name, WebAppManifest, MANIFEST_BACKGROUND_COLOR, SHORT_NAME_MAX_GRAPHEMES,
};
use super::model::{FaviconState, ManifestSettings, FRESH_INSTALL_PRIMARY_COLOR};
use super::routes::MANIFEST_ROUTE;
use super::service::render_manifest;
use crate::app::auth_class::AuthClass;
use crate::app::health::Health;
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{
    application_routes, serve_unmatched, with_middleware, HttpEdge, RateLimitClass, Transport,
};
use crate::app::state::AppState;
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::http::etag::weak_etag;
use crate::infra::http::headers::{SecurityHeaders, SecurityPolicy};
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::http::shell::FRESH_INSTALL_APP_NAME;
use crate::infra::http::static_assets::dist_directory::DistDirectory;
use crate::infra::http::static_assets::StaticAssets;
use crate::infra::http::trace::RequestLog;

const MANIFEST_PATH: &str = "/manifest.webmanifest";
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const BROWSER_NAVIGATION: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8";
const INDEX: &str = "<!doctype html><html><head><meta charset=\"UTF-8\" />\
    <base href=\"/\" /><title>Palmr</title><!--palmr:head--></head>\
    <body><div id=\"root\"></div></body></html>";
const STALE_MANIFEST: &str = "{\"name\":\"palmr-stale-static-manifest-sentinel\"}";
const BODY_CAP: usize = 64 * 1024;

fn fresh_install_manifest() -> Value {
    json!({
        "name": "Palmr",
        "short_name": "Palmr",
        "theme_color": "#1668dc",
        "background_color": "#ffffff",
        "icons": [
            { "src": "api/v1/public/branding/favicon", "sizes": "512x512", "type": "image/png" }
        ]
    })
}

fn rendered_json(settings: &ManifestSettings<'_>) -> Value {
    serde_json::from_slice(render_manifest(settings).unwrap().body()).unwrap()
}

fn settings(app_name: &str) -> ManifestSettings<'_> {
    ManifestSettings {
        app_name,
        ..ManifestSettings::FRESH_INSTALL
    }
}

#[test]
fn unit_manifest_fresh_install_defaults() {
    assert_eq!(FRESH_INSTALL_APP_NAME, "Palmr");
    assert_eq!(FRESH_INSTALL_PRIMARY_COLOR, "#1668dc");
    assert_eq!(MANIFEST_BACKGROUND_COLOR, "#ffffff");
    assert_eq!(
        ManifestSettings::FRESH_INSTALL.favicon,
        FaviconState::Default
    );
    assert_eq!(
        rendered_json(&ManifestSettings::FRESH_INSTALL),
        fresh_install_manifest()
    );
}

#[test]
fn unit_manifest_wire_fields_are_exactly_the_contract() {
    let manifest = rendered_json(&ManifestSettings::FRESH_INSTALL);
    let keys: Vec<&str> = manifest
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "background_color",
            "icons",
            "name",
            "short_name",
            "theme_color"
        ]
    );
    let body = render_manifest(&ManifestSettings::FRESH_INSTALL).unwrap();
    let text = std::str::from_utf8(body.body()).unwrap();
    assert!(text.starts_with("{\"name\":\"Palmr\",\"short_name\":\"Palmr\","));
}

#[rstest]
#[case::default(FaviconState::Default, 1)]
#[case::custom(FaviconState::Custom, 1)]
#[case::disabled(FaviconState::Disabled, 0)]
fn unit_manifest_icons_follow_favicon_state(#[case] favicon: FaviconState, #[case] icons: usize) {
    let manifest = rendered_json(&ManifestSettings {
        favicon,
        ..ManifestSettings::FRESH_INSTALL
    });
    let entries = manifest["icons"].as_array().unwrap();
    assert_eq!(entries.len(), icons);
    for entry in entries {
        assert_eq!(entry, &fresh_install_manifest()["icons"][0]);
    }
}

#[test]
fn unit_manifest_icon_sources_are_base_relative_and_local() {
    let manifest = rendered_json(&ManifestSettings::FRESH_INSTALL);
    for entry in manifest["icons"].as_array().unwrap() {
        let src = entry["src"].as_str().unwrap();
        assert!(!src.starts_with('/'), "{src}");
        assert!(!src.contains(':'), "{src}");
        assert!(!src.contains('?'), "{src}");
    }
}

#[rstest]
#[case::ascii("Palmr", "Palmr")]
#[case::cjk_suffix("Palmr 企業", "Palmr 企業")]
#[case::emoji_suffix("Arquivo 🚀", "Arquivo 🚀")]
#[case::exactly_the_limit("Palmr Transf", "Palmr Transf")]
#[case::long_ascii("Palmr Transfers", "Palmr Transf")]
#[case::cut_at_space("Palmr Files and more", "Palmr Files")]
#[case::surrounding_space("  Palmr  ", "Palmr")]
#[case::empty("", "")]
#[case::rtl("مشاركة الملفات الآمنة", "مشاركة الملف")]
#[case::long_multibyte("企業ファイル共有サービスプラットフォーム", "企業ファイル共有サービス")]
fn unit_manifest_short_name_truncation(#[case] app_name: &str, #[case] expected: &str) {
    assert_eq!(short_name(app_name), expected);
    assert_eq!(
        rendered_json(&settings(app_name))["short_name"],
        json!(expected)
    );
}

#[rstest]
#[case::zwj_family("👨\u{200d}👩\u{200d}👧\u{200d}👦")]
#[case::regional_indicator_flag("🇧🇷")]
#[case::skin_tone("👍🏽")]
#[case::combining_acute("e\u{301}")]
#[case::stacked_combining_marks("a\u{300}\u{316}\u{35c}")]
#[case::hebrew_with_points("שָׁ")]
fn unit_manifest_short_name_keeps_clusters_whole(#[case] cluster: &str) {
    assert_eq!(cluster.graphemes(true).count(), 1);
    let name = cluster.repeat(SHORT_NAME_MAX_GRAPHEMES + 1);
    assert_eq!(short_name(&name), cluster.repeat(SHORT_NAME_MAX_GRAPHEMES));
}

fn tricky_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("👨\u{200d}👩\u{200d}👧".to_owned()),
        Just("🇧🇷".to_owned()),
        Just("e\u{301}".to_owned()),
        Just("\u{301}".to_owned()),
        Just("שָׁ".to_owned()),
        Just("مشاركة".to_owned()),
        Just("\u{202e}".to_owned()),
        Just(" ".to_owned()),
        Just("\"</script>\\".to_owned()),
        any::<char>().prop_map(String::from),
    ];
    prop::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
}

proptest! {
    #[test]
    fn prop_manifest_short_name_is_a_bounded_grapheme_prefix(app_name in tricky_text()) {
        let name = app_name.trim();
        let derived = short_name(&app_name);
        let boundaries: Vec<usize> = name
            .grapheme_indices(true)
            .map(|(start, _)| start)
            .chain([name.len()])
            .collect();

        prop_assert!(name.starts_with(derived));
        prop_assert!(boundaries.contains(&derived.len()));
        prop_assert!(derived.graphemes(true).count() <= SHORT_NAME_MAX_GRAPHEMES);
        prop_assert_eq!(derived, short_name(&app_name));
        if name.graphemes(true).count() <= SHORT_NAME_MAX_GRAPHEMES {
            prop_assert_eq!(derived, name);
        }
    }

    #[test]
    fn prop_manifest_serializer_round_trips_any_name(app_name in tricky_text()) {
        let manifest = rendered_json(&settings(&app_name));
        prop_assert_eq!(manifest["name"].as_str(), Some(app_name.as_str()));
        prop_assert_eq!(manifest["short_name"].as_str(), Some(short_name(&app_name)));
    }
}

#[test]
fn unit_manifest_serializer_escapes_hostile_names() {
    let hostile = "\"},\"start_url\":\"https://evil.example/\",\"x\":{\"</script>\u{202e}\\";
    let rendered = render_manifest(&settings(hostile)).unwrap();
    let manifest: Value = serde_json::from_slice(rendered.body()).unwrap();
    assert_eq!(manifest["name"], json!(hostile));
    assert_eq!(manifest.as_object().unwrap().len(), 5);
    assert!(manifest.get("start_url").is_none());
}

#[test]
fn unit_manifest_render_is_deterministic() {
    let first = render_manifest(&ManifestSettings::FRESH_INSTALL).unwrap();
    let second = render_manifest(&ManifestSettings::FRESH_INSTALL).unwrap();
    assert_eq!(first.body(), second.body());
    assert_eq!(first.etag(), second.etag());
    assert_eq!(first.etag(), &weak_etag(first.body()));
}

#[test]
fn unit_manifest_changed_input_changes_body_and_etag() {
    let fresh = render_manifest(&ManifestSettings::FRESH_INSTALL).unwrap();
    let variants = [
        settings("Acme Files"),
        ManifestSettings {
            primary_color: "#237804",
            ..ManifestSettings::FRESH_INSTALL
        },
        ManifestSettings {
            favicon: FaviconState::Disabled,
            ..ManifestSettings::FRESH_INSTALL
        },
    ];
    let mut etags = vec![fresh.etag().clone()];
    for variant in variants {
        let rendered = render_manifest(&variant).unwrap();
        assert_ne!(rendered.body(), fresh.body(), "{variant:?}");
        assert!(!etags.contains(rendered.etag()), "{variant:?}");
        etags.push(rendered.etag().clone());
    }
    assert_eq!(
        rendered_json(&ManifestSettings {
            primary_color: "#237804",
            ..ManifestSettings::FRESH_INSTALL
        })["theme_color"],
        json!("#237804")
    );
}

#[test]
fn unit_manifest_wire_model_borrows_its_input() {
    let manifest = WebAppManifest::new(&ManifestSettings::FRESH_INSTALL);
    assert_eq!(
        serde_json::to_value(manifest).unwrap(),
        fresh_install_manifest()
    );
}

struct Dist {
    _temp: TempDir,
    root: std::path::PathBuf,
}

fn built_dist() -> Dist {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dist");
    write(&root.join("index.html"), INDEX.as_bytes());
    write(
        &root.join("manifest.webmanifest"),
        STALE_MANIFEST.as_bytes(),
    );
    Dist { _temp: temp, root }
}

fn write(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn app(
    dist: &Dist,
    vars: &[(&str, &str)],
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    let config = OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config;
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let readiness = Readiness::new();
    readiness.set_for_test(true);
    let assembled = application_routes().build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &config.base_url).unwrap();
    let assets =
        StaticAssets::from_source(DistDirectory::at(&dist.root), &config.base_url).unwrap();
    let router = serve_unmatched(assembled.router, assets).with_state(AppState::new(
        clock.clone(),
        Health::new(readiness),
        docs,
    ));
    let edge = HttpEdge::new(
        clock,
        TrustedProxies::new(&config.trust_proxy),
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
}

async fn send(
    app: impl Service<Request, Response = Response, Error = Infallible>,
    request: Request,
) -> Fetched {
    let response = app.oneshot(request).await.unwrap();
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

fn get(headers: &[(HeaderName, &str)]) -> Request {
    request(Method::GET, headers)
}

fn request(method: Method, headers: &[(HeaderName, &str)]) -> Request {
    let mut builder = Request::builder().method(method).uri(MANIFEST_PATH);
    for (name, value) in headers {
        builder = builder.header(name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn assert_manifest_response(fetched: &Fetched) {
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        fetched.header(CONTENT_TYPE),
        "application/manifest+json; charset=utf-8"
    );
    assert_eq!(fetched.header(CACHE_CONTROL), "no-cache");
    assert_eq!(fetched.header(ETAG), weak_etag(&fetched.body));
    let manifest: Value = serde_json::from_slice(&fetched.body).unwrap();
    assert_eq!(manifest, fresh_install_manifest());
}

#[tokio::test]
async fn it_manifest_served_no_cache_with_etag() {
    let dist = built_dist();
    let fetched = send(app(&dist, &[]), get(&[])).await;

    assert_manifest_response(&fetched);
    assert!(!fetched.header(CACHE_CONTROL).contains("immutable"));
    assert!(!fetched.header(CACHE_CONTROL).contains("max-age"));
    assert!(fetched.header(ETAG).starts_with("W/\""));
    assert!(!fetched.header(X_REQUEST_ID).is_empty());
    assert!(!fetched.header(CONTENT_SECURITY_POLICY).is_empty());
    assert_eq!(fetched.header(X_CONTENT_TYPE_OPTIONS), "nosniff");

    let again = send(app(&dist, &[]), get(&[])).await;
    assert_eq!(again.body, fetched.body);
    assert_eq!(again.header(ETAG), fetched.header(ETAG));
}

#[tokio::test]
async fn it_manifest_if_none_match_returns_304() {
    let dist = built_dist();
    let etag = send(app(&dist, &[]), get(&[]))
        .await
        .header(ETAG)
        .to_owned();
    let strong = etag.trim_start_matches("W/").to_owned();

    for validator in [etag.as_str(), strong.as_str(), "\"stale\", W/\"x\", *"] {
        let fetched = send(app(&dist, &[]), get(&[(IF_NONE_MATCH, validator)])).await;
        assert_eq!(fetched.status, StatusCode::NOT_MODIFIED, "{validator}");
        assert!(fetched.body.is_empty(), "{validator}");
        assert_eq!(fetched.header(ETAG), etag, "{validator}");
        assert_eq!(fetched.header(CACHE_CONTROL), "no-cache", "{validator}");
        assert!(!fetched.header(X_REQUEST_ID).is_empty(), "{validator}");
    }

    let stale = send(app(&dist, &[]), get(&[(IF_NONE_MATCH, "W/\"stale\"")])).await;
    assert_manifest_response(&stale);
}

#[tokio::test]
async fn it_manifest_wins_over_spa_fallback_and_built_file() {
    let dist = built_dist();
    for accept in [BROWSER_NAVIGATION, "text/html", "*/*"] {
        let fetched = send(app(&dist, &[]), get(&[(ACCEPT, accept)])).await;
        assert_manifest_response(&fetched);
        let text = std::str::from_utf8(&fetched.body).unwrap();
        assert!(!text.contains("<html"), "{accept}");
        assert!(
            !text.contains("palmr-stale-static-manifest-sentinel"),
            "{accept}"
        );
    }
}

#[tokio::test]
async fn it_manifest_head_carries_validators_without_body() {
    let dist = built_dist();
    let full = send(app(&dist, &[]), get(&[])).await;
    let head = send(app(&dist, &[]), request(Method::HEAD, &[])).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(head.header(ETAG), full.header(ETAG));
    assert_eq!(head.header(CACHE_CONTROL), "no-cache");
    assert_eq!(head.header(CONTENT_LENGTH), full.body.len().to_string());
}

#[tokio::test]
async fn it_manifest_other_methods_are_json_405() {
    let dist = built_dist();
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let fetched = send(app(&dist, &[]), request(method.clone(), &[])).await;
        assert_eq!(fetched.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        let envelope: Value = serde_json::from_slice(&fetched.body).unwrap();
        assert_eq!(envelope["error"]["code"], json!("METHOD_NOT_ALLOWED"));
        assert!(fetched.headers.get(ALLOW).is_some(), "{method}");
    }
}

#[tokio::test]
async fn it_manifest_is_anonymous_and_ignores_credentials() {
    let dist = built_dist();
    let with_cookie = send(
        app(&dist, &[]),
        get(&[(COOKIE, "palmr_session=forged; palmr_csrf=forged")]),
    )
    .await;
    assert_manifest_response(&with_cookie);
    assert!(with_cookie.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn it_manifest_is_identical_under_a_sub_path_base_url() {
    let dist = built_dist();
    let root = send(app(&dist, &[]), get(&[])).await;
    let sub_path = send(
        app(
            &dist,
            &[("PALMR_BASE_URL", "https://files.example.com/palmr/")],
        ),
        get(&[
            (HeaderName::from_static("host"), "attacker.example"),
            (
                HeaderName::from_static("x-forwarded-host"),
                "attacker.example",
            ),
            (HeaderName::from_static("x-forwarded-proto"), "http"),
        ]),
    )
    .await;

    assert_manifest_response(&sub_path);
    assert_eq!(sub_path.body, root.body);
    let text = std::str::from_utf8(&sub_path.body).unwrap();
    for leaked in ["example.com", "attacker", "localhost", "5487", "palmr/"] {
        assert!(!text.contains(leaked), "{leaked}");
    }

    let base = url::Url::parse("https://files.example.com/palmr/").unwrap();
    let manifest_url = base.join("manifest.webmanifest").unwrap();
    let icon = manifest_url
        .join(
            fresh_install_manifest()["icons"][0]["src"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        icon.as_str(),
        "https://files.example.com/palmr/api/v1/public/branding/favicon"
    );
}

#[tokio::test]
async fn it_manifest_discloses_no_internals() {
    let dist = built_dist();
    let fetched = send(app(&dist, &[]), get(&[])).await;
    let text = std::str::from_utf8(&fetched.body).unwrap();
    let root = dist.root.to_string_lossy().into_owned();
    for leaked in [
        root.as_str(),
        "/data",
        "PALMR_",
        "instance.key",
        "ibb.co",
        "imgur",
        "http:",
        "https:",
        "//",
    ] {
        assert!(!text.contains(leaked), "{leaked}");
    }
}

#[test]
fn svc_manifest_route_is_public_rl_none() {
    let assembled = application_routes().build().unwrap();
    let entry = assembled
        .inventory
        .get(&Method::GET, MANIFEST_PATH)
        .unwrap();
    let policy = entry.policy();

    assert_eq!(policy, MANIFEST_ROUTE);
    assert_eq!(policy.auth(), AuthClass::Public);
    assert_eq!(policy.rate_limit(), RateLimitClass::None);
    assert_eq!(policy.transport(), Transport::ControlPlane);
    assert_eq!(policy.security(), SecurityPolicy::Default);
    assert_eq!(policy.request_log(), RequestLog::Standard);
    assert!(entry.layers().compresses_response());
    assert!(assembled.openapi.paths.paths.contains_key(MANIFEST_PATH));
    assert!(assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path() == MANIFEST_PATH)
        .all(|entry| entry.method() == Method::GET));
}

#[test]
fn svc_public_route_golden_file_lists_the_manifest() {
    let golden = include_str!("../../../../../tests/snapshots/public_routes.txt");
    assert!(golden
        .lines()
        .any(|line| line == "GET /manifest.webmanifest public"));
}

#[test]
fn unit_manifest_content_type_is_the_web_app_manifest_type() {
    assert_eq!(
        HeaderValue::from_static(super::manifest::MANIFEST_CONTENT_TYPE),
        "application/manifest+json; charset=utf-8"
    );
}
