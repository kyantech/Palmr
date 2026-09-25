use std::convert::Infallible;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
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
use super::model::{
    AssetResolution, BrandingAsset, BundledAsset, FaviconState, ManifestSettings,
    FRESH_INSTALL_PRIMARY_COLOR,
};
use super::routes::{MANIFEST_ROUTE, PUBLIC_BRANDING_ROUTE};
use super::service::{public_url, render_manifest, resolve, BrandingService};
use crate::app::auth_class::AuthClass;
use crate::app::health::Health;
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{
    application_routes, serve_unmatched, with_middleware, HttpEdge, RateLimitClass, Transport,
};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};
use crate::domain::clock::TestClock;
use crate::features::settings::model::{AppSettings, AssetMode, EmailLogoMode};
use crate::features::settings::SettingsHandle;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::csrf::{AnonymousCsrf, CsrfGuard};
use crate::infra::http::etag::weak_etag;
use crate::infra::http::headers::{SecurityHeaders, SecurityPolicy};
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::http::shell::FRESH_INSTALL_APP_NAME;
use crate::infra::http::static_assets::dist_directory::DistDirectory;
use crate::infra::http::static_assets::StaticAssets;
use crate::infra::http::trace::RequestLog;
use crate::storage::key::{BrandingKind, KeyNamespace, ObjectKey};
use crate::storage::provider::PutHint;

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
    app_with_settings(dist, vars, AppSettings::defaults(), None)
}

fn app_with_settings(
    dist: &Dist,
    vars: &[(&str, &str)],
    settings: AppSettings,
    branding: Option<(BrandingService, StorageRuntime)>,
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
    let (service, storage) = match branding {
        Some((service, storage)) => (Some(service), storage),
        None => (None, StorageRuntime::for_test()),
    };
    let router = serve_unmatched(assembled.router, assets).with_state(AppState::new(
        clock.clone(),
        Health::new(readiness),
        docs,
        SettingsHandle::new(settings),
        storage,
    ));
    let router = match service {
        Some(service) => router.layer(axum::Extension(service)),
        None => router,
    };
    let edge = HttpEdge::new(
        clock,
        TrustedProxies::new(&config.trust_proxy),
        SecurityHeaders::new(&config),
        CsrfGuard::new(&config.base_url),
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
    mut request: Request,
) -> Fetched {
    request.extensions_mut().insert(ConnectInfo(
        "198.51.100.20:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
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

const LOGO_PNG: &[u8] = include_bytes!("../../../../web/public/branding/logo.png");
const FAVICON_PNG: &[u8] = include_bytes!("../../../../web/public/branding/favicon.png");
const LOGIN_BACKGROUND_PNG: &[u8] =
    include_bytes!("../../../../web/public/branding/login-background.png");
const OG_IMAGE_PNG: &[u8] = include_bytes!("../../../../web/public/branding/og-image.png");
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const SHARED_CACHE: &str = "public, max-age=300";

fn branding_get(path: &str, headers: &[(HeaderName, &str)]) -> Request {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    for (name, value) in headers {
        builder = builder.header(name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.starts_with(PNG_SIGNATURE));
    assert_eq!(&bytes[12..16], b"IHDR");
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    (width, height)
}

fn assert_error_code(fetched: &Fetched, status: StatusCode, code: &str) {
    assert_eq!(fetched.status, status);
    let envelope: Value = serde_json::from_slice(&fetched.body).unwrap();
    assert_eq!(envelope["error"]["code"], json!(code));
}

fn all_disabled() -> AppSettings {
    let mut settings = AppSettings::defaults();
    settings.branding.logo_mode = AssetMode::Disabled;
    settings.branding.favicon_mode = AssetMode::Disabled;
    settings.branding.login_background_mode = AssetMode::Disabled;
    settings.branding.og_image_mode = AssetMode::Disabled;
    settings.branding.email_logo_mode = EmailLogoMode::None;
    settings
}

#[test]
fn unit_branding_fresh_install_resolution() {
    let settings = AppSettings::defaults();
    let branding = &settings.branding;
    assert_eq!(
        resolve(branding, BrandingAsset::Logo),
        AssetResolution::Bundled(BundledAsset::Logo)
    );
    assert_eq!(
        resolve(branding, BrandingAsset::Favicon),
        AssetResolution::Bundled(BundledAsset::Favicon)
    );
    assert_eq!(
        resolve(branding, BrandingAsset::LoginBackground),
        AssetResolution::Bundled(BundledAsset::LoginBackground)
    );
    assert_eq!(
        resolve(branding, BrandingAsset::OgImage),
        AssetResolution::Bundled(BundledAsset::OgImage)
    );
    assert_eq!(branding.email_logo_mode, EmailLogoMode::Inherit);
    assert_eq!(
        resolve(branding, BrandingAsset::EmailLogo),
        AssetResolution::Bundled(BundledAsset::Logo)
    );
}

#[rstest]
#[case::inherit_default(
    EmailLogoMode::Inherit,
    AssetMode::Default,
    AssetResolution::Bundled(BundledAsset::Logo)
)]
#[case::inherit_custom(
    EmailLogoMode::Inherit,
    AssetMode::Custom,
    AssetResolution::Custom(BrandingAsset::Logo)
)]
#[case::inherit_disabled(EmailLogoMode::Inherit, AssetMode::Disabled, AssetResolution::Disabled)]
#[case::custom(
    EmailLogoMode::Custom,
    AssetMode::Disabled,
    AssetResolution::Custom(BrandingAsset::EmailLogo)
)]
#[case::none_default(EmailLogoMode::None, AssetMode::Default, AssetResolution::Disabled)]
#[case::none_custom(EmailLogoMode::None, AssetMode::Custom, AssetResolution::Disabled)]
fn unit_branding_email_logo_resolution(
    #[case] email: EmailLogoMode,
    #[case] logo: AssetMode,
    #[case] expected: AssetResolution,
) {
    let mut settings = AppSettings::defaults();
    settings.branding.email_logo_mode = email;
    settings.branding.logo_mode = logo;
    assert_eq!(
        resolve(&settings.branding, BrandingAsset::EmailLogo),
        expected
    );
}

#[rstest]
#[case::logo(BrandingAsset::Logo)]
#[case::favicon(BrandingAsset::Favicon)]
#[case::login_background(BrandingAsset::LoginBackground)]
#[case::og_image(BrandingAsset::OgImage)]
fn unit_branding_main_asset_mode_resolution(#[case] asset: BrandingAsset) {
    for (mode, expected) in [
        (AssetMode::Custom, AssetResolution::Custom(asset)),
        (AssetMode::Disabled, AssetResolution::Disabled),
    ] {
        let mut settings = AppSettings::defaults();
        let branding = &mut settings.branding;
        match asset {
            BrandingAsset::Logo => branding.logo_mode = mode,
            BrandingAsset::Favicon => branding.favicon_mode = mode,
            BrandingAsset::LoginBackground => branding.login_background_mode = mode,
            BrandingAsset::OgImage => branding.og_image_mode = mode,
            BrandingAsset::EmailLogo => unreachable!(),
        }
        assert_eq!(resolve(&settings.branding, asset), expected, "{mode:?}");
        assert_eq!(
            public_url(&settings.branding, asset).is_some(),
            mode == AssetMode::Custom
        );
    }
}

#[test]
fn unit_branding_asset_segments_are_the_public_contract() {
    let segments: Vec<&str> = BrandingAsset::ALL.iter().map(|a| a.segment()).collect();
    assert_eq!(
        segments,
        [
            "logo",
            "favicon",
            "login-background",
            "og-image",
            "email-logo"
        ]
    );
    for asset in BrandingAsset::ALL {
        assert_eq!(BrandingAsset::from_segment(asset.segment()), Some(asset));
        assert_eq!(
            asset.public_path(),
            format!("/api/v1/public/branding/{}", asset.segment())
        );
    }
    for rejected in [
        "",
        "LOGO",
        "Logo",
        "logo.png",
        "og_image",
        "login_background",
        "email_logo",
        "avatar",
        "hero",
        "../logo",
        "logo/",
    ] {
        assert_eq!(BrandingAsset::from_segment(rejected), None, "{rejected:?}");
    }
    let kinds: Vec<&str> = BrandingAsset::ALL.iter().map(|a| a.kind()).collect();
    assert_eq!(
        kinds,
        [
            "logo",
            "favicon",
            "login_background",
            "og_default_image",
            "email_logo"
        ]
    );
}

#[test]
fn unit_bundled_defaults_are_local_rasters_within_contract_geometry() {
    for (asset, file, max_edge, exact) in [
        (BundledAsset::Logo, LOGO_PNG, 512, None),
        (BundledAsset::Favicon, FAVICON_PNG, 512, Some((512, 512))),
        (
            BundledAsset::LoginBackground,
            LOGIN_BACKGROUND_PNG,
            2560,
            None,
        ),
        (BundledAsset::OgImage, OG_IMAGE_PNG, 1200, Some((1200, 630))),
    ] {
        let loaded = asset.load().unwrap();
        assert_eq!(loaded.bytes.as_ref(), file, "{asset:?}");
        assert_eq!(loaded.content_type, "image/png", "{asset:?}");
        assert_eq!(loaded.etag, weak_etag(file), "{asset:?}");
        let (width, height) = png_dimensions(file);
        assert!(width.max(height) <= max_edge, "{asset:?}");
        if let Some(expected) = exact {
            assert_eq!((width, height), expected, "{asset:?}");
        }
        assert!(file.len() < 256 * 1024, "{asset:?}");
        let text = String::from_utf8_lossy(file);
        for leaked in ["http:", "https:", "<svg", "kyantech"] {
            assert!(
                !text.to_ascii_lowercase().contains(leaked),
                "{asset:?} {leaked}"
            );
        }
    }
}

#[tokio::test]
async fn it_public_branding_defaults_served() {
    let dist = built_dist();
    for (segment, file) in [
        ("logo", LOGO_PNG),
        ("favicon", FAVICON_PNG),
        ("login-background", LOGIN_BACKGROUND_PNG),
        ("og-image", OG_IMAGE_PNG),
        ("email-logo", LOGO_PNG),
    ] {
        let path = format!("/api/v1/public/branding/{segment}");
        let fetched = send(app(&dist, &[]), branding_get(&path, &[])).await;
        assert_eq!(fetched.status, StatusCode::OK, "{segment}");
        assert_eq!(fetched.body.as_ref(), file, "{segment}");
        assert!(fetched.body.starts_with(PNG_SIGNATURE), "{segment}");
        assert_eq!(fetched.header(CONTENT_TYPE), "image/png", "{segment}");
        assert_eq!(
            fetched.header(CONTENT_LENGTH),
            file.len().to_string(),
            "{segment}"
        );
        assert_eq!(fetched.header(CACHE_CONTROL), SHARED_CACHE, "{segment}");
        assert_eq!(fetched.header(ETAG), weak_etag(file), "{segment}");
        assert_eq!(
            fetched.header(X_CONTENT_TYPE_OPTIONS),
            "nosniff",
            "{segment}"
        );
        assert!(fetched.headers.get("set-cookie").is_none(), "{segment}");

        let revalidated = send(
            app(&dist, &[]),
            branding_get(&path, &[(IF_NONE_MATCH, fetched.header(ETAG))]),
        )
        .await;
        assert_eq!(revalidated.status, StatusCode::NOT_MODIFIED, "{segment}");
        assert!(revalidated.body.is_empty(), "{segment}");
        assert_eq!(revalidated.header(ETAG), fetched.header(ETAG), "{segment}");
        assert_eq!(revalidated.header(CACHE_CONTROL), SHARED_CACHE, "{segment}");
        assert!(revalidated.headers.get("set-cookie").is_none(), "{segment}");

        let head = send(
            app(&dist, &[]),
            Request::builder()
                .method(Method::HEAD)
                .uri(&path)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(head.status, StatusCode::OK, "{segment}");
        assert!(head.body.is_empty(), "{segment}");
        assert_eq!(head.header(ETAG), fetched.header(ETAG), "{segment}");
    }
}

#[tokio::test]
async fn it_public_branding_is_identical_for_every_visitor() {
    let dist = built_dist();
    let anonymous = send(
        app(&dist, &[]),
        branding_get("/api/v1/public/branding/logo", &[]),
    )
    .await;
    let with_cookies = send(
        app(&dist, &[]),
        branding_get(
            "/api/v1/public/branding/logo",
            &[(COOKIE, "palmr_session=forged; palmr_csrf=forged")],
        ),
    )
    .await;
    assert_eq!(with_cookies.status, StatusCode::OK);
    assert_eq!(with_cookies.body, anonymous.body);
    assert_eq!(with_cookies.header(ETAG), anonymous.header(ETAG));
    assert!(with_cookies.headers.get("set-cookie").is_none());
    assert!(with_cookies
        .headers
        .get(http::header::VARY)
        .is_none_or(|vary| {
            !vary
                .to_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("cookie")
        }));
}

#[tokio::test]
async fn it_public_branding_disabled_404() {
    let dist = built_dist();
    for segment in [
        "logo",
        "favicon",
        "login-background",
        "og-image",
        "email-logo",
    ] {
        let path = format!("/api/v1/public/branding/{segment}");
        let fetched = send(
            app_with_settings(&dist, &[], all_disabled(), None),
            branding_get(&path, &[]),
        )
        .await;
        assert_error_code(&fetched, StatusCode::NOT_FOUND, "BRANDING_ASSET_UNKNOWN");
        assert!(fetched.headers.get("set-cookie").is_none(), "{segment}");
        assert!(fetched.headers.get(ETAG).is_none(), "{segment}");
        assert_ne!(
            fetched
                .headers
                .get(CACHE_CONTROL)
                .map(|v| v.to_str().unwrap()),
            Some(SHARED_CACHE),
            "{segment}"
        );
    }

    let mut inherit_disabled = AppSettings::defaults();
    inherit_disabled.branding.logo_mode = AssetMode::Disabled;
    let fetched = send(
        app_with_settings(&dist, &[], inherit_disabled, None),
        branding_get("/api/v1/public/branding/email-logo", &[]),
    )
    .await;
    assert_error_code(&fetched, StatusCode::NOT_FOUND, "BRANDING_ASSET_UNKNOWN");

    let mut none_with_logo = AppSettings::defaults();
    none_with_logo.branding.email_logo_mode = EmailLogoMode::None;
    let dist_app = app_with_settings(&dist, &[], none_with_logo, None);
    let email = send(
        dist_app.clone(),
        branding_get("/api/v1/public/branding/email-logo", &[]),
    )
    .await;
    assert_error_code(&email, StatusCode::NOT_FOUND, "BRANDING_ASSET_UNKNOWN");
    let logo = send(dist_app, branding_get("/api/v1/public/branding/logo", &[])).await;
    assert_eq!(logo.status, StatusCode::OK);
}

#[tokio::test]
async fn it_public_branding_unknown_asset_404() {
    let dist = built_dist();
    for segment in [
        "unknown",
        "LOGO",
        "logo.png",
        "og_image",
        "avatar",
        "hero",
        "branding%2Flogo%2F0192",
        "..%2F..%2Fdata%2Finstance.key",
    ] {
        let path = format!("/api/v1/public/branding/{segment}");
        let fetched = send(app(&dist, &[]), branding_get(&path, &[])).await;
        assert_error_code(&fetched, StatusCode::NOT_FOUND, "BRANDING_ASSET_UNKNOWN");
        assert!(fetched.headers.get("set-cookie").is_none(), "{segment}");
        let text = std::str::from_utf8(&fetched.body).unwrap();
        for leaked in ["/data", "instance.key", "branding/", "storage"] {
            assert!(!text.contains(leaked), "{segment} {leaked}");
        }
    }
}

struct CustomHarness {
    _root: TempDir,
    pools: DbPools,
    storage: StorageRuntime,
    provider: Arc<dyn crate::storage::provider::StorageProvider>,
}

impl CustomHarness {
    async fn open() -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 2, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let (provider, status) = crate::storage::test_runtime();
        Self {
            _root: root,
            pools,
            storage: StorageRuntime::new(Arc::clone(&provider), status),
            provider,
        }
    }

    fn service(&self) -> (BrandingService, StorageRuntime) {
        (
            BrandingService::new(self.pools.reader().clone(), Arc::clone(&self.provider)),
            self.storage.clone(),
        )
    }

    async fn upload(&self, kind: BrandingKind, mime: &str, bytes: &'static [u8]) -> String {
        let key = ObjectKey::allocate(KeyNamespace::Branding(kind));
        self.provider
            .put_stream(
                &key,
                Box::pin(std::io::Cursor::new(bytes)),
                PutHint::default(),
            )
            .await
            .unwrap();
        let object_id = uuid::Uuid::now_v7().to_string();
        let now = "2026-09-25T12:00:00.000Z";
        let clock = TestClock::new(datetime!(2026-09-25 12:00 UTC));
        let size = i64::try_from(bytes.len()).unwrap();
        let key_text = key.as_str().to_owned();
        let asset_kind = kind.as_str().to_owned();
        let mime = mime.to_owned();
        let id = object_id.clone();
        self.pools
            .write_tx(&clock, "branding.test_custom", async move |tx| {
                sqlx::query(
                    "INSERT INTO storage_objects \
                     (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at) \
                     VALUES (?1, ?2, 'local', ?3, 'active', 1, ?4, ?4, ?4)",
                )
                .bind(&id)
                .bind(&key_text)
                .bind(size)
                .bind(now)
                .execute(tx.executor())
                .await?;
                sqlx::query("UPDATE branding_assets SET is_current = 0 WHERE kind = ?1")
                    .bind(&asset_kind)
                    .execute(tx.executor())
                    .await?;
                sqlx::query(
                    "INSERT INTO branding_assets \
                     (id, kind, storage_object_id, mime_type, size_bytes, is_current, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                )
                .bind(uuid::Uuid::now_v7().to_string())
                .bind(&asset_kind)
                .bind(&id)
                .bind(&mime)
                .bind(size)
                .bind(now)
                .execute(tx.executor())
                .await?;
                Ok::<_, crate::infra::db::DbError>(())
            })
            .await
            .unwrap();
        object_id
    }
}

#[tokio::test]
async fn it_public_branding_custom_asset_streamed_from_storage() {
    const CUSTOM_LOGO: &[u8] = b"RIFF\x1a\x00\x00\x00WEBPVP8L\x0d\x00\x00\x00palmr-custom";
    const REPLACEMENT: &[u8] = b"RIFF\x1a\x00\x00\x00WEBPVP8L\x0d\x00\x00\x00palmr-second";
    let dist = built_dist();
    let harness = CustomHarness::open().await;
    let missing = send(
        app_with_settings(&dist, &[], AppSettings::defaults(), Some(harness.service())),
        branding_get("/api/v1/public/branding/logo", &[]),
    )
    .await;
    assert_eq!(missing.body.as_ref(), LOGO_PNG);

    let orphaned = send(
        app_with_settings(
            &dist,
            &[],
            custom_logo_and_favicon(),
            Some(harness.service()),
        ),
        branding_get("/api/v1/public/branding/favicon", &[]),
    )
    .await;
    assert_error_code(&orphaned, StatusCode::NOT_FOUND, "BRANDING_ASSET_UNKNOWN");

    let first = harness
        .upload(BrandingKind::Logo, "image/webp", CUSTOM_LOGO)
        .await;
    let fetched = send(
        app_with_settings(
            &dist,
            &[],
            custom_logo_and_favicon(),
            Some(harness.service()),
        ),
        branding_get("/api/v1/public/branding/logo", &[]),
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.body.as_ref(), CUSTOM_LOGO);
    assert_eq!(fetched.header(CONTENT_TYPE), "image/webp");
    assert_eq!(
        fetched.header(CONTENT_LENGTH),
        CUSTOM_LOGO.len().to_string()
    );
    assert_eq!(fetched.header(CACHE_CONTROL), SHARED_CACHE);
    assert_eq!(fetched.header(ETAG), weak_etag(first.as_bytes()));
    assert!(fetched.headers.get("set-cookie").is_none());
    assert!(!std::str::from_utf8(fetched.header(ETAG).as_bytes())
        .unwrap()
        .contains("branding/"));

    let inherited = send(
        app_with_settings(
            &dist,
            &[],
            custom_logo_and_favicon(),
            Some(harness.service()),
        ),
        branding_get("/api/v1/public/branding/email-logo", &[]),
    )
    .await;
    assert_eq!(inherited.body.as_ref(), CUSTOM_LOGO);

    let revalidated = send(
        app_with_settings(
            &dist,
            &[],
            custom_logo_and_favicon(),
            Some(harness.service()),
        ),
        branding_get(
            "/api/v1/public/branding/logo",
            &[(IF_NONE_MATCH, fetched.header(ETAG))],
        ),
    )
    .await;
    assert_eq!(revalidated.status, StatusCode::NOT_MODIFIED);
    assert!(revalidated.body.is_empty());

    let second = harness
        .upload(BrandingKind::Logo, "image/webp", REPLACEMENT)
        .await;
    let replaced = send(
        app_with_settings(
            &dist,
            &[],
            custom_logo_and_favicon(),
            Some(harness.service()),
        ),
        branding_get(
            "/api/v1/public/branding/logo",
            &[(IF_NONE_MATCH, fetched.header(ETAG))],
        ),
    )
    .await;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_eq!(replaced.body.as_ref(), REPLACEMENT);
    assert_eq!(replaced.header(ETAG), weak_etag(second.as_bytes()));
    assert_ne!(replaced.header(ETAG), fetched.header(ETAG));
}

fn custom_logo_and_favicon() -> AppSettings {
    let mut settings = AppSettings::defaults();
    settings.branding.logo_mode = AssetMode::Custom;
    settings.branding.favicon_mode = AssetMode::Custom;
    settings
}

#[test]
fn svc_public_branding_route_is_public_rl_public_read_without_cookies() {
    let assembled = application_routes().build().unwrap();
    let path = "/api/v1/public/branding/{asset}";
    let entries: Vec<_> = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path() == path)
        .collect();
    assert_eq!(entries.len(), 1);
    let policy = entries[0].policy();
    assert_eq!(entries[0].method(), Method::GET);
    assert_eq!(policy, PUBLIC_BRANDING_ROUTE);
    assert_eq!(policy.auth(), AuthClass::Public);
    assert_eq!(policy.rate_limit(), RateLimitClass::PublicRead);
    assert_eq!(policy.anonymous_csrf(), AnonymousCsrf::None);
    assert!(assembled.openapi.paths.paths.contains_key(path));
}

#[test]
fn unit_manifest_settings_follow_the_settings_snapshot() {
    assert_eq!(
        ManifestSettings::from_settings(&AppSettings::defaults()),
        ManifestSettings::FRESH_INSTALL
    );
    let mut settings = AppSettings::defaults();
    settings.general.app_name = "Acme Files".to_owned();
    settings.branding.primary_color = "#237804".to_owned();
    for (mode, favicon) in [
        (AssetMode::Default, FaviconState::Default),
        (AssetMode::Custom, FaviconState::Custom),
        (AssetMode::Disabled, FaviconState::Disabled),
    ] {
        settings.branding.favicon_mode = mode;
        assert_eq!(
            ManifestSettings::from_settings(&settings),
            ManifestSettings {
                app_name: "Acme Files",
                primary_color: "#237804",
                favicon,
            }
        );
    }
}

#[tokio::test]
async fn it_manifest_uses_effective_branding_settings() {
    let dist = built_dist();
    let mut settings = AppSettings::defaults();
    settings.general.app_name = "Acme Files and Transfers".to_owned();
    settings.branding.primary_color = "#237804".to_owned();
    settings.branding.favicon_mode = AssetMode::Disabled;
    let fetched = send(app_with_settings(&dist, &[], settings, None), get(&[])).await;
    assert_eq!(fetched.status, StatusCode::OK);
    let manifest: Value = serde_json::from_slice(&fetched.body).unwrap();
    assert_eq!(
        manifest,
        json!({
            "name": "Acme Files and Transfers",
            "short_name": "Acme Files a",
            "theme_color": "#237804",
            "background_color": "#ffffff",
            "icons": []
        })
    );
    let fresh = send(app(&dist, &[]), get(&[])).await;
    assert_ne!(fetched.header(ETAG), fresh.header(ETAG));
}

#[test]
fn svc_public_route_golden_file_lists_public_branding() {
    let golden = include_str!("../../../../../tests/snapshots/public_routes.txt");
    assert!(golden
        .lines()
        .any(|line| line == "GET /api/v1/public/branding/{asset} public"));
}
