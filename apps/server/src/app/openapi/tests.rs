use std::convert::Infallible;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::Response;
use http::header::{
    ACCEPT, ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    ETAG, HOST, IF_NONE_MATCH, ORIGIN, X_CONTENT_TYPE_OPTIONS,
};
use http::{HeaderMap, HeaderName, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use time::macros::datetime;
use tower::{Service, ServiceExt};
use utoipa::openapi::OpenApi;
use utoipa_axum::routes;

use super::{
    export_document, ApiDocs, Credential, DOCS_PATH, OPENAPI_PATH, SCALAR_RUNTIME,
    SCALAR_RUNTIME_PATH, SCALAR_RUNTIME_SHA256,
};
use crate::app::auth_class::AuthClass;
use crate::app::health::{Health, VERSION};
use crate::app::lifecycle::Readiness;
use crate::app::router::{
    application_routes, serve_unmatched, with_middleware, HttpEdge, RateLimitClass, RoutePolicy,
    Routes, Transport,
};
use crate::app::state::AppState;
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::http::etag::{weak_etag, weak_etag_from_sha256};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::http::static_assets::dist_directory::DistDirectory;
use crate::infra::http::static_assets::StaticAssets;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
const SPA_MARKER: &str = "<div id=\"root\"></div>";
const INDEX: &str = "<!doctype html><html><head><meta charset=\"UTF-8\" />\
    <base href=\"/\" /><title>Palmr</title><!--palmr:head--></head>\
    <body><div id=\"root\"></div></body></html>";
const BODY_CAP: usize = 8 * 1024 * 1024;
const BROWSER_NAVIGATION: &str = "text/html,application/xhtml+xml,*/*;q=0.8";

struct Dist {
    _temp: TempDir,
    root: PathBuf,
}

fn built_dist() -> Dist {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dist");
    write(&root.join("index.html"), INDEX.as_bytes());
    Dist { _temp: temp, root }
}

fn write(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn config(vars: &[(&str, &str)]) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
}

fn app(
    dist: &Dist,
    vars: &[(&str, &str)],
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    let config = config(vars);
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

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }

    fn csp_nonce(&self) -> String {
        let policy = self.header(CONTENT_SECURITY_POLICY);
        let start = policy.find("'nonce-").unwrap() + "'nonce-".len();
        policy[start..start + 32].to_owned()
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

fn get(path: &str, headers: &[(HeaderName, &str)]) -> Request {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    for (name, value) in headers {
        builder = builder.header(name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn application_document() -> Value {
    let docs = ApiDocs::new(
        application_routes().build().unwrap().openapi,
        &config(&[]).base_url,
    )
    .unwrap();
    serde_json::from_slice(docs.document()).unwrap()
}

fn assert_security_headers(fetched: &Fetched) {
    assert!(!fetched.header(X_REQUEST_ID).is_empty());
    assert_eq!(fetched.header(X_CONTENT_TYPE_OPTIONS), "nosniff");
    let policy = fetched.header(CONTENT_SECURITY_POLICY);
    assert!(policy.contains("script-src 'self' 'nonce-"), "{policy}");
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    assert!(!policy.contains("unsafe-inline"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
    assert!(!policy.contains("https:"), "{policy}");
    assert!(fetched.headers.get(ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
}

#[tokio::test]
async fn it_openapi_json_served_with_etag_and_revalidation() {
    let dist = built_dist();
    let fetched = send(app(&dist, &[]), get(OPENAPI_PATH, &[])).await;

    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        fetched.header(CONTENT_TYPE),
        "application/json; charset=utf-8"
    );
    assert_eq!(fetched.header(CACHE_CONTROL), "no-cache, max-age=60");
    assert!(!fetched.header(CACHE_CONTROL).contains("immutable"));
    assert_eq!(fetched.header(ETAG), weak_etag(&fetched.body));
    assert_security_headers(&fetched);

    let document: Value = serde_json::from_slice(&fetched.body).unwrap();
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(document["info"]["title"], "Palmr");
    assert_eq!(document["info"]["version"], VERSION);
    let parsed: OpenApi = serde_json::from_slice(&fetched.body).unwrap();
    assert!(parsed.paths.paths.contains_key(OPENAPI_PATH));

    let again = send(app(&dist, &[]), get(OPENAPI_PATH, &[])).await;
    assert_eq!(again.body, fetched.body);
    assert_eq!(again.header(ETAG), fetched.header(ETAG));
}

#[tokio::test]
async fn it_openapi_if_none_match_returns_304() {
    let dist = built_dist();
    let etag = send(app(&dist, &[]), get(OPENAPI_PATH, &[]))
        .await
        .header(ETAG)
        .to_owned();
    let strong = etag.trim_start_matches("W/").to_owned();

    for validator in [etag.as_str(), strong.as_str(), "\"stale\", *"] {
        let fetched = send(
            app(&dist, &[]),
            get(OPENAPI_PATH, &[(IF_NONE_MATCH, validator)]),
        )
        .await;
        assert_eq!(fetched.status, StatusCode::NOT_MODIFIED, "{validator}");
        assert!(fetched.body.is_empty(), "{validator}");
        assert_eq!(fetched.header(ETAG), etag, "{validator}");
        assert_eq!(fetched.header(CACHE_CONTROL), "no-cache, max-age=60");
        assert_security_headers(&fetched);
    }

    let stale = send(
        app(&dist, &[]),
        get(OPENAPI_PATH, &[(IF_NONE_MATCH, "W/\"stale\"")]),
    )
    .await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_eq!(stale.header(ETAG), etag);
}

#[tokio::test]
async fn it_openapi_json_is_never_the_spa_shell() {
    let dist = built_dist();
    let fetched = send(
        app(&dist, &[]),
        get(OPENAPI_PATH, &[(ACCEPT, BROWSER_NAVIGATION)]),
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert!(fetched.header(CONTENT_TYPE).starts_with("application/json"));
    assert!(!fetched.text().contains(SPA_MARKER));
    serde_json::from_slice::<Value>(&fetched.body).unwrap();
}

#[tokio::test]
async fn it_docs_serves_scalar_against_the_served_document() {
    let dist = built_dist();
    let fetched = send(
        app(&dist, &[]),
        get(
            DOCS_PATH,
            &[
                (ACCEPT, BROWSER_NAVIGATION),
                (ORIGIN, "https://attacker.example"),
            ],
        ),
    )
    .await;

    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.header(CONTENT_TYPE), "text/html; charset=utf-8");
    assert_eq!(fetched.header(CACHE_CONTROL), "no-cache");
    assert!(fetched.headers.get(ETAG).is_none());
    assert_security_headers(&fetched);

    let html = fetched.text();
    let nonce = fetched.csp_nonce();
    assert!(!html.contains(SPA_MARKER));
    assert!(html.contains(&format!(
        "<meta property=\"csp-nonce\" content=\"{nonce}\">"
    )));
    assert!(html.contains("<script src=\"/docs/scalar.js\"></script>"));
    assert!(html.contains(&format!(
        "<script nonce=\"{nonce}\">Scalar.createApiReference(\"#palmr-api-reference\", "
    )));
    assert!(html.contains("<div id=\"palmr-api-reference\"></div>"));
    assert!(!html.contains("id=\"api-reference\""));
    assert_eq!(html.matches("<script").count(), 2);
    assert!(!html.contains("http://"));
    assert!(!html.contains("https://"));
    assert!(!html.contains("\"openapi\""));

    let config = scalar_config(html);
    assert_eq!(config["url"], "/openapi.json");
    assert_eq!(config["telemetry"], false);
    assert_eq!(config["withDefaultFonts"], false);
    assert_eq!(config["showDeveloperTools"], "never");
    assert_eq!(config["agent"], json!({ "disabled": true }));
    assert_eq!(config["mcp"], json!({ "disabled": true }));
    assert!(config.get("servers").is_none());
    assert!(config.get("proxyUrl").is_none());
    assert!(config.get("content").is_none());
}

fn scalar_config(html: &str) -> Value {
    let marker = "\"#palmr-api-reference\", ";
    let start = html.find(marker).unwrap() + marker.len();
    let end = html.rfind(");</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}

#[tokio::test]
async fn it_docs_nonce_is_fresh_per_response() {
    let dist = built_dist();
    let first = send(app(&dist, &[]), get(DOCS_PATH, &[])).await;
    let second = send(app(&dist, &[]), get(DOCS_PATH, &[])).await;
    assert_ne!(first.csp_nonce(), second.csp_nonce());
    assert!(second.text().contains(&second.csp_nonce()));
    assert!(!second.text().contains(&first.csp_nonce()));
}

#[tokio::test]
async fn it_docs_under_sub_path_use_the_configured_base_path() {
    let dist = built_dist();
    let vars = [("PALMR_BASE_URL", "https://files.example.com/palmr/")];
    let fetched = send(
        app(&dist, &vars),
        get(
            DOCS_PATH,
            &[
                (HOST, "attacker.example"),
                (X_FORWARDED_HOST, "forwarded.example"),
            ],
        ),
    )
    .await;

    assert_eq!(fetched.status, StatusCode::OK);
    let html = fetched.text();
    assert!(html.contains("<script src=\"/palmr/docs/scalar.js\"></script>"));
    let config = scalar_config(html);
    assert_eq!(config["url"], "/palmr/openapi.json");
    assert_eq!(config["servers"], json!([{ "url": "/palmr" }]));
    for leaked in ["files.example.com", "attacker.example", "forwarded.example"] {
        assert!(!html.contains(leaked), "{leaked}");
    }

    let document = send(
        app(&dist, &vars),
        get(OPENAPI_PATH, &[(HOST, "attacker.example")]),
    )
    .await;
    assert!(!document.text().contains("attacker.example"));
    assert!(!document.text().contains("files.example.com"));
}

#[test]
fn unit_docs_page_escapes_the_base_path() {
    let config = config(&[("PALMR_BASE_URL", "https://example.com/a'b&c(d)/")]);
    let docs = ApiDocs::new(OpenApi::default(), &config.base_url).unwrap();
    let html = docs.render_page(&crate::infra::http::headers::CspNonce::for_test(
        "0123456789abcdef0123456789abcdef",
    ));

    assert!(html.contains("<script src=\"/a&#39;b&amp;c(d)/docs/scalar.js\"></script>"));
    assert_eq!(scalar_config(&html)["url"], "/a'b&c(d)/openapi.json");
    assert_eq!(html.matches("</script>").count(), 2);
    assert_eq!(html.matches("<script").count(), 2);
}

#[test]
fn unit_docs_page_is_not_a_template_for_the_base_path() {
    let config = config(&[("PALMR_BASE_URL", "https://example.com/$spec/")]);
    let docs = ApiDocs::new(OpenApi::default(), &config.base_url).unwrap();
    let html = docs.render_page(&crate::infra::http::headers::CspNonce::for_test(
        "0123456789abcdef0123456789abcdef",
    ));
    assert!(html.contains("<script src=\"/$spec/docs/scalar.js\"></script>"));
    assert_eq!(scalar_config(&html)["url"], "/$spec/openapi.json");
}

#[tokio::test]
async fn svc_docs_without_nonce_returns_the_error_envelope() {
    let config = config(&[]);
    let assembled = application_routes().build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &config.base_url).unwrap();
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let router =
        assembled
            .router
            .with_state(AppState::new(clock, Health::new(Readiness::new()), docs));

    let fetched = send(router, get(DOCS_PATH, &[])).await;
    assert_eq!(fetched.status, StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = serde_json::from_slice(&fetched.body).unwrap();
    assert_eq!(body["error"]["code"], "INTERNAL_ERROR");
}

#[tokio::test]
async fn it_scalar_runtime_is_served_from_self_with_revalidation() {
    let dist = built_dist();
    let fetched = send(app(&dist, &[]), get(SCALAR_RUNTIME_PATH, &[])).await;

    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        fetched.header(CONTENT_TYPE),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(fetched.header(CACHE_CONTROL), "no-cache");
    assert_eq!(
        fetched.header(ETAG),
        weak_etag_from_sha256(SCALAR_RUNTIME_SHA256)
    );
    assert_eq!(fetched.body.as_ref(), SCALAR_RUNTIME);
    assert_security_headers(&fetched);

    let revalidated = send(
        app(&dist, &[]),
        get(
            SCALAR_RUNTIME_PATH,
            &[(IF_NONE_MATCH, fetched.header(ETAG))],
        ),
    )
    .await;
    assert_eq!(revalidated.status, StatusCode::NOT_MODIFIED);
    assert!(revalidated.body.is_empty());
    assert_eq!(revalidated.header(ETAG), fetched.header(ETAG));
}

#[test]
fn unit_scalar_runtime_matches_its_pinned_digest() {
    let digest: [u8; 32] = Sha256::digest(SCALAR_RUNTIME).into();
    assert_eq!(digest, SCALAR_RUNTIME_SHA256);
}

#[test]
fn unit_openapi_document_is_deterministic() {
    let base_url = config(&[]).base_url;
    let first = ApiDocs::new(application_routes().build().unwrap().openapi, &base_url).unwrap();
    let second = ApiDocs::new(application_routes().build().unwrap().openapi, &base_url).unwrap();
    assert_eq!(first.document(), second.document());
    assert_eq!(first.etag(), second.etag());
    assert_eq!(first.etag(), weak_etag(first.document()));
}

#[tokio::test]
async fn it_openapi_export_is_the_served_document() {
    let dist = built_dist();
    let served = send(app(&dist, &[]), get(OPENAPI_PATH, &[])).await;
    assert_eq!(served.status, StatusCode::OK);

    let exported = export_document().unwrap();
    assert_eq!(served.body, exported);
    assert_eq!(export_document().unwrap(), exported);

    let under_sub_path = send(
        app(
            &dist,
            &[("PALMR_BASE_URL", "https://files.example.com/palmr")],
        ),
        get(OPENAPI_PATH, &[]),
    )
    .await;
    assert_eq!(under_sub_path.body, exported);
}

#[utoipa::path(get, path = "/test/extra", responses((status = 200)))]
async fn extra() -> StatusCode {
    StatusCode::OK
}

#[test]
fn unit_openapi_etag_changes_with_the_document() {
    let base_url = config(&[]).base_url;
    let current = ApiDocs::new(application_routes().build().unwrap().openapi, &base_url).unwrap();
    let widened = application_routes().merge(Routes::new().route(
        RoutePolicy::new(
            AuthClass::Public,
            RateLimitClass::None,
            Transport::ControlPlane,
        ),
        routes!(extra),
    ));
    let widened = ApiDocs::new(widened.build().unwrap().openapi, &base_url).unwrap();
    assert_ne!(current.document(), widened.document());
    assert_ne!(current.etag(), widened.etag());
}

#[test]
fn unit_openapi_document_describes_the_error_envelope() {
    let document = application_document();
    let schemas = &document["components"]["schemas"];

    let body = &schemas["ApiErrorBody"];
    assert_eq!(body["required"], json!(["error"]));
    assert_eq!(
        body["properties"]["error"]["$ref"],
        "#/components/schemas/ApiErrorPayload"
    );
    let payload = &schemas["ApiErrorPayload"];
    let mut required: Vec<&str> = payload["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["code", "details", "message", "requestId"]);
    assert_eq!(
        payload["properties"]["code"]["$ref"],
        "#/components/schemas/ErrorCode"
    );
    assert!(schemas["ErrorCode"]["enum"]
        .as_array()
        .unwrap()
        .contains(&json!("INTERNAL_ERROR")));

    let docs_error = &document["paths"][DOCS_PATH]["get"]["responses"]["500"];
    assert_eq!(
        docs_error["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ApiErrorBody"
    );
}

#[test]
fn unit_openapi_document_keeps_typed_wire_schemas() {
    let document = application_document();
    let schemas = &document["components"]["schemas"];
    for name in [
        "HealthLive",
        "HealthReady",
        "HealthNotReady",
        "HealthSummary",
        "WebAppManifest",
    ] {
        assert_eq!(schemas[name]["type"], "object", "{name}");
        assert!(schemas[name]["properties"].is_object(), "{name}");
    }
    let live = &document["paths"]["/health/live"]["get"]["responses"]["200"];
    assert_eq!(
        live["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/HealthLive"
    );
    let manifest = &document["paths"]["/manifest.webmanifest"]["get"]["responses"]["200"];
    assert_eq!(
        manifest["content"]["application/manifest+json"]["schema"]["$ref"],
        "#/components/schemas/WebAppManifest"
    );
}

#[test]
fn unit_openapi_security_schemes_are_cookie_and_header_keys() {
    let document = application_document();
    let schemes = document["components"]["securitySchemes"]
        .as_object()
        .unwrap();
    let names: Vec<&str> = schemes.keys().map(String::as_str).collect();
    let mut expected: Vec<&str> = Credential::ALL
        .into_iter()
        .map(Credential::scheme_name)
        .collect();
    expected.sort_unstable();
    assert_eq!(names, expected);

    let expect = |scheme: &str, location: &str, name: &str| {
        assert_eq!(schemes[scheme]["type"], "apiKey", "{scheme}");
        assert_eq!(schemes[scheme]["in"], location, "{scheme}");
        assert_eq!(schemes[scheme]["name"], name, "{scheme}");
        assert!(schemes[scheme]["description"].is_string(), "{scheme}");
    };
    expect("palmrSession", "cookie", "palmr_session");
    expect("palmrCsrfCookie", "cookie", "palmr_csrf");
    expect("palmrCsrfHeader", "header", "X-Palmr-CSRF");
    expect("shareGrant", "cookie", "palmr_share_{sharePublicId}");
    expect(
        "reverseShareGrant",
        "cookie",
        "palmr_rs_{reverseSharePublicId}",
    );

    for (name, scheme) in schemes {
        assert_eq!(scheme["type"], "apiKey", "{name}");
        assert!(scheme.get("scheme").is_none(), "{name}");
        assert!(scheme.get("bearerFormat").is_none(), "{name}");
        assert!(
            !scheme["name"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case("authorization"),
            "{name}"
        );
    }
    let wire = document.to_string().to_ascii_lowercase();
    for forbidden in ["bearer", "jwt"] {
        assert!(!wire.contains(forbidden), "{forbidden}");
    }
    assert!(document.get("servers").is_none());
    assert!(document.get("security").is_none());
}

#[utoipa::path(get, path = "/test/account", responses((status = 200)))]
async fn read_account() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(post, path = "/test/account", responses((status = 204)))]
async fn change_account() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[test]
fn unit_session_classes_require_the_session_and_csrf_only_when_state_changing() {
    let mut expectations = Vec::new();
    for class in AuthClass::ALL {
        let assembled = Routes::<()>::new()
            .route(
                RoutePolicy::new(class, RateLimitClass::Read, Transport::ControlPlane),
                routes!(read_account),
            )
            .route(
                RoutePolicy::new(class, RateLimitClass::Write, Transport::ControlPlane),
                routes!(change_account),
            )
            .build()
            .unwrap();
        let document = serde_json::to_value(&assembled.openapi).unwrap();
        let item = &document["paths"]["/test/account"];
        expectations.push((
            class,
            item["get"]["security"].clone(),
            item["post"]["security"].clone(),
        ));
    }

    let session = json!([{ "palmrSession": [] }]);
    let session_and_csrf =
        json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }]);
    for (class, read, write) in expectations {
        match class {
            AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup => {
                assert_eq!(read, Value::Null, "{class}");
                assert_eq!(write, Value::Null, "{class}");
            }
            AuthClass::Authenticated
            | AuthClass::AuthenticatedRecentAuth
            | AuthClass::Admin
            | AuthClass::AdminRecentAuth => {
                assert_eq!(read, session, "{class}");
                assert_eq!(write, session_and_csrf, "{class}");
            }
        }
    }
}

#[test]
fn unit_openapi_document_carries_no_source_traceability() {
    let document = application_document().to_string();
    for leaked in [
        "SPEC",
        "ADR",
        "API_DESIGN",
        "ARCHITECTURE",
        "TEST_STRATEGY",
        "Decision ",
        "R-0",
        "§",
    ] {
        assert!(!document.contains(leaked), "{leaked}");
    }
}
