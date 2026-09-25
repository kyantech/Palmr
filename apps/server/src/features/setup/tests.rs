use std::convert::Infallible;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use time::macros::datetime;
use tower::{Service, ServiceExt};

use super::model::Bootstrap;
use super::routes::BOOTSTRAP_ROUTE;
use crate::app::auth_class::AuthClass;
use crate::app::health::{Health, VERSION};
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{application_routes, with_middleware, HttpEdge, RateLimitClass};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::features::settings::model::{AppSettings, AssetMode};
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::token::Token;
use crate::infra::http::csrf::{AnonymousCsrf, CsrfGuard};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const BOOTSTRAP_PATH: &str = "/api/v1/bootstrap";
const BODY_CAP: usize = 64 * 1024;
const BOOTSTRAP_FIELDS: [&str; 12] = [
    "appDescription",
    "appName",
    "defaultLocale",
    "faviconUrl",
    "logoUrl",
    "passwordLoginEnabled",
    "poweredByVisible",
    "primaryColor",
    "providers",
    "setupCompleted",
    "supportedLocales",
    "version",
];
const SUPPORTED_LOCALES: [&str; 23] = [
    "ar-SA", "de-DE", "el-GR", "en-US", "es-ES", "fa-IR", "fr-FR", "he-IL", "hi-IN", "id-ID",
    "it-IT", "ja-JP", "ko-KR", "nl-NL", "pl-PL", "pt-BR", "ru-RU", "sv-SE", "th-TH", "tr-TR",
    "uk-UA", "vi-VN", "zh-CN",
];

fn app(
    settings: AppSettings,
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
        "PALMR_BASE_URL",
        "https://files.example.test",
    )]))
    .unwrap()
    .config;
    let clock = Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC)));
    let assembled = application_routes().build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &config.base_url).unwrap();
    let router = assembled.router.with_state(AppState::new(
        clock.clone(),
        Health::new(Readiness::new()),
        docs,
        SettingsHandle::new(settings),
        StorageRuntime::for_test(),
    ));
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
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }
}

async fn fetch(settings: AppSettings, path: &str, cookie: Option<&str>) -> Fetched {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, cookie);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "198.51.100.20:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let response = app(settings).oneshot(request).await.unwrap();
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

async fn bootstrap(settings: AppSettings) -> Fetched {
    fetch(settings, BOOTSTRAP_PATH, None).await
}

#[tokio::test]
async fn it_bootstrap_shape_matches_contract() {
    let fetched = bootstrap(AppSettings::defaults()).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        fetched.headers.get(CONTENT_TYPE).unwrap(),
        "application/json; charset=utf-8"
    );
    assert_eq!(fetched.headers.get(CACHE_CONTROL).unwrap(), "no-store");

    let body = fetched.json();
    let mut keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, BOOTSTRAP_FIELDS);
    assert_eq!(
        body,
        json!({
            "setupCompleted": false,
            "appName": "Palmr",
            "appDescription": "Self-hosted file transfer",
            "logoUrl": "/api/v1/public/branding/logo",
            "faviconUrl": "/api/v1/public/branding/favicon",
            "primaryColor": "#1668dc",
            "defaultLocale": "en-US",
            "supportedLocales": SUPPORTED_LOCALES,
            "passwordLoginEnabled": true,
            "providers": [],
            "poweredByVisible": true,
            "version": VERSION
        })
    );
    assert_eq!(
        LocaleCode::ALL.len(),
        body["supportedLocales"].as_array().unwrap().len()
    );
}

#[tokio::test]
async fn it_bootstrap_version_null_when_hidden() {
    let mut settings = AppSettings::defaults();
    settings.general.show_version = false;
    let body = bootstrap(settings).await.json();
    let object = body.as_object().unwrap();
    assert!(object.contains_key("version"));
    assert_eq!(object["version"], Value::Null);
    assert_eq!(object.len(), BOOTSTRAP_FIELDS.len());
    assert_eq!(object["poweredByVisible"], json!(true));

    let shown = bootstrap(AppSettings::defaults()).await.json();
    assert_eq!(shown["version"], json!(VERSION));
    assert!(!VERSION.is_empty());
}

#[tokio::test]
async fn it_bootstrap_reflects_persisted_instance_settings() {
    let mut settings = AppSettings::defaults();
    settings.general.setup_completed = true;
    settings.general.app_name = "Acme Files".to_owned();
    settings.general.app_description = "Acme transfer desk".to_owned();
    settings.general.default_locale = LocaleCode::PtBr;
    settings.general.powered_by_visible = false;
    settings.branding.primary_color = "#237804".to_owned();
    settings.branding.logo_mode = AssetMode::Disabled;
    settings.branding.favicon_mode = AssetMode::Custom;
    let body = bootstrap(settings).await.json();
    assert_eq!(body["setupCompleted"], json!(true));
    assert_eq!(body["appName"], json!("Acme Files"));
    assert_eq!(body["appDescription"], json!("Acme transfer desk"));
    assert_eq!(body["defaultLocale"], json!("pt-BR"));
    assert_eq!(body["poweredByVisible"], json!(false));
    assert_eq!(body["primaryColor"], json!("#237804"));
    assert_eq!(body["logoUrl"], Value::Null);
    assert_eq!(body["faviconUrl"], json!("/api/v1/public/branding/favicon"));
    assert_eq!(body["providers"], json!([]));
    assert_eq!(body["supportedLocales"], json!(SUPPORTED_LOCALES));
}

#[test]
fn unit_bootstrap_setup_state_comes_only_from_settings() {
    let mut settings = AppSettings::defaults();
    assert!(!Bootstrap::from_settings(&settings).setup_completed);
    settings.general.setup_completed = true;
    assert!(Bootstrap::from_settings(&settings).setup_completed);
}

#[tokio::test]
async fn it_bootstrap_contains_no_user_session_or_secret_data() {
    let mut settings = AppSettings::defaults();
    settings.smtp.enabled = true;
    settings.smtp.host = Some("smtp.secret-host.example".to_owned());
    settings.smtp.username = Some("smtp-user-sentinel".to_owned());
    settings.smtp.password = Some(Secret::new("smtp-password-sentinel".to_owned()));
    settings.quotas.default_user_quota_bytes = Some(1_234_567_u64.try_into().unwrap());
    let session = Token::mint().unwrap().encode();
    let cookie = format!(
        "palmr_session={}; palmr_csrf={}",
        session.expose_secret(),
        session.expose_secret()
    );
    let anonymous = bootstrap(AppSettings::defaults()).await;
    let with_cookie = fetch(settings, BOOTSTRAP_PATH, Some(&cookie)).await;
    assert_eq!(with_cookie.status, StatusCode::OK);
    assert_eq!(with_cookie.body, anonymous.body);

    let text = std::str::from_utf8(&with_cookie.body)
        .unwrap()
        .to_ascii_lowercase();
    for leaked in [
        "smtp",
        "sentinel",
        "secret",
        "password\"",
        "1234567",
        "userid",
        "username",
        "email",
        "role",
        "session",
        "restriction",
        "quota",
        "trusted",
        "csrf",
        "token",
        "s3",
        "bucket",
        "clientsecret",
        "/data",
    ] {
        assert!(!text.contains(leaked), "{leaked}");
    }
    assert!(!text.contains(&session.expose_secret().to_ascii_lowercase()));
}

#[tokio::test]
async fn it_bootstrap_identical_for_anonymous_callers() {
    let first = bootstrap(AppSettings::defaults()).await;
    let second = bootstrap(AppSettings::defaults()).await;
    assert_eq!(first.body, second.body);
}

#[tokio::test]
async fn it_bootstrap_issues_anonymous_csrf_once() {
    let fresh = bootstrap(AppSettings::defaults()).await;
    let cookies = fresh.set_cookies();
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert!(cookies[0].starts_with("palmr_csrf="), "{cookies:?}");
    assert!(cookies[0].contains("; Path=/; SameSite=Lax"));
    assert!(cookies[0].contains("; Secure"));
    assert!(!cookies[0].contains("HttpOnly"));
    assert_eq!(fresh.headers.get(CACHE_CONTROL).unwrap(), "no-store");

    let token = Token::mint().unwrap().encode();
    let existing = format!("palmr_csrf={}", token.expose_secret());
    let kept = fetch(AppSettings::defaults(), BOOTSTRAP_PATH, Some(&existing)).await;
    assert_eq!(kept.status, StatusCode::OK);
    assert!(kept.set_cookies().is_empty());
    assert_eq!(kept.body, fresh.body);

    let malformed = fetch(
        AppSettings::defaults(),
        BOOTSTRAP_PATH,
        Some("palmr_csrf=not-a-token"),
    )
    .await;
    let reissued = malformed.set_cookies();
    assert_eq!(reissued.len(), 1);
    assert!(reissued[0].starts_with("palmr_csrf="));
}

#[test]
fn svc_bootstrap_route_is_public_rl_public_read_with_anonymous_csrf() {
    let assembled = application_routes().build().unwrap();
    let entry = assembled
        .inventory
        .get(&Method::GET, BOOTSTRAP_PATH)
        .unwrap();
    let policy = entry.policy();
    assert_eq!(policy, BOOTSTRAP_ROUTE);
    assert_eq!(policy.auth(), AuthClass::Public);
    assert_eq!(policy.rate_limit(), RateLimitClass::PublicRead);
    assert_eq!(policy.anonymous_csrf(), AnonymousCsrf::Issue);
    assert!(assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path() == BOOTSTRAP_PATH)
        .all(|entry| entry.method() == Method::GET));
    assert!(assembled.openapi.paths.paths.contains_key(BOOTSTRAP_PATH));
}

#[tokio::test]
async fn svc_legacy_app_bootstrap_route_does_not_exist() {
    let assembled = application_routes().build().unwrap();
    for entry in assembled.inventory.entries() {
        assert!(!entry.path().contains("/app/bootstrap"), "{}", entry.path());
        assert!(
            !entry.path().ends_with("bootstrap") || entry.path() == BOOTSTRAP_PATH,
            "{}",
            entry.path()
        );
    }
    assert!(!assembled
        .openapi
        .paths
        .paths
        .keys()
        .any(|path| path.contains("/app/bootstrap")));

    let fetched = fetch(AppSettings::defaults(), "/api/v1/app/bootstrap", None).await;
    assert_eq!(fetched.status, StatusCode::NOT_FOUND);
    assert!(fetched.set_cookies().is_empty());

    let golden = include_str!("../../../../../tests/snapshots/public_routes.txt");
    assert!(golden
        .lines()
        .any(|line| line == "GET /api/v1/bootstrap public"));
    assert!(!golden.contains("/app/bootstrap"));
}
