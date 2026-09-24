use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use time::macros::datetime;
use tower::{Service, ServiceExt};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use super::{
    routes, DatabaseState, DependencyHealth, Health, HealthReason, MigrationState, StorageState,
    HEALTH_ROUTE, VERSION,
};
use crate::app::auth_class::AuthClass;
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{
    application_routes, with_middleware, HttpEdge, RateLimitClass, Transport,
};
use crate::app::state::AppState;
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig, TrustProxy};
use crate::domain::clock::TestClock;
use crate::infra::http::headers::{SecurityHeaders, SecurityPolicy};
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::http::trace::RequestLog;
use crate::infra::telemetry::build_dispatch;

const PATHS: [&str; 3] = ["/health/live", "/health/ready", "/health"];
const BODY_CAP: usize = 4096;

const INFRASTRUCTURE_SENTINELS: [&str; 10] = [
    "s3://private-bucket",
    "private-bucket",
    "https://internal-minio:9000",
    "internal-minio",
    "/data/palmr.db",
    "database password=secret",
    "permission denied at /srv/private",
    "/srv/private",
    "AKIA-health-access-sentinel",
    "health-secret-sentinel-5c1a",
];

fn operator_config(vars: &[(&str, &str)]) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
}

fn sentinel_config() -> OperatorConfig {
    operator_config(&[
        ("PALMR_DATA_DIR", "/srv/private"),
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", "https://internal-minio:9000"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", "private-bucket"),
        ("PALMR_S3_ACCESS_KEY", "AKIA-health-access-sentinel"),
        ("PALMR_S3_SECRET_KEY", "health-secret-sentinel-5c1a"),
    ])
}

fn ready_health() -> (Readiness, Health) {
    let readiness = Readiness::new();
    readiness.set_for_test(true);
    let health = Health::new(readiness.clone());
    (readiness, health)
}

fn app_with(
    health: &Health,
    config: &OperatorConfig,
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let assembled = routes().build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &config.base_url).unwrap();
    let router = assembled
        .router
        .with_state(AppState::new(clock.clone(), health.clone(), docs));
    let edge = HttpEdge::new(
        clock,
        TrustedProxies::new(&TrustProxy::Off),
        SecurityHeaders::new(config),
    );
    with_middleware(router, &edge)
}

async fn fetch(health: &Health, path: &str) -> (StatusCode, HeaderMap, Value) {
    fetch_with(health, &operator_config(&[]), path).await
}

async fn fetch_with(
    health: &Health,
    config: &OperatorConfig,
    path: &str,
) -> (StatusCode, HeaderMap, Value) {
    let (status, headers, bytes) = fetch_raw(health, config, path).await;
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

async fn fetch_raw(
    health: &Health,
    config: &OperatorConfig,
    path: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = app_with(health, config).oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = Limited::new(body, BODY_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (parts.status, parts.headers, bytes)
}

fn snapshot_reads(health: &Health) -> usize {
    health.checks.snapshots.load(Ordering::SeqCst)
}

#[tokio::test]
async fn it_health_summary_alias() {
    let (readiness, health) = ready_health();
    health.checks().set_database(DatabaseState::Writable);
    health.checks().set_migrations(MigrationState::Current);
    health.checks().set_storage(StorageState::Degraded);

    let (live_status, _, live) = fetch(&health, "/health/live").await;
    let (ready_status, _, ready) = fetch(&health, "/health/ready").await;
    let (summary_status, _, summary) = fetch(&health, "/health").await;

    assert_eq!(live_status, StatusCode::OK);
    assert_eq!(live, json!({ "status": "ok", "version": VERSION }));
    assert_eq!(ready_status, StatusCode::OK);
    assert_eq!(
        ready,
        json!({
            "status": "ready",
            "storage": "degraded",
            "database": "ok",
            "migrations": "current",
            "version": VERSION,
        })
    );
    assert_eq!(summary_status, StatusCode::OK);
    assert_eq!(
        summary,
        json!({
            "status": "degraded",
            "live": true,
            "ready": true,
            "storage": "degraded",
            "database": "ok",
            "version": VERSION,
        })
    );
    assert_ne!(summary, ready);
    assert_ne!(summary, live);
    assert_ne!(ready, live);

    readiness.set_for_test(false);

    let (live_status, _, live) = fetch(&health, "/health/live").await;
    let (ready_status, _, ready) = fetch(&health, "/health/ready").await;
    let (summary_status, _, summary) = fetch(&health, "/health").await;

    assert_eq!(live_status, StatusCode::OK);
    assert_eq!(live, json!({ "status": "ok", "version": VERSION }));
    assert_eq!(ready_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(ready, json!({ "status": "not_ready", "version": VERSION }));
    assert_eq!(summary_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        summary,
        json!({
            "status": "not_ready",
            "live": true,
            "ready": false,
            "storage": "degraded",
            "database": "ok",
            "version": VERSION,
        })
    );
}

#[tokio::test]
async fn it_health_live_never_touches_dependencies() {
    let health = Health::new(Readiness::new());
    health.checks().set_database(DatabaseState::Unavailable);
    health.checks().set_migrations(MigrationState::Pending);
    health.checks().set_storage(StorageState::Down);

    for _ in 0..3 {
        let (status, _, body) = fetch(&health, "/health/live").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "status": "ok", "version": VERSION }));
    }
    assert_eq!(snapshot_reads(&health), 0);

    let (status, _, _) = fetch(&health, "/health/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(snapshot_reads(&health), 1);

    let (status, _, _) = fetch(&health, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(snapshot_reads(&health), 2);
}

fn every_dependency_state() -> Vec<(bool, DependencyHealth)> {
    let lifecycle = [true, false];
    let database = [
        None,
        Some(DatabaseState::Writable),
        Some(DatabaseState::NotWritable),
        Some(DatabaseState::Unavailable),
    ];
    let migrations = [
        None,
        Some(MigrationState::Current),
        Some(MigrationState::Pending),
    ];
    let storage = [
        None,
        Some(StorageState::Ok),
        Some(StorageState::Degraded),
        Some(StorageState::Down),
        Some(StorageState::Unreachable),
    ];
    let mut states = Vec::new();
    for ready in lifecycle {
        for database in database {
            for migrations in migrations {
                for storage in storage {
                    states.push((
                        ready,
                        DependencyHealth {
                            database,
                            migrations,
                            storage,
                        },
                    ));
                }
            }
        }
    }
    states
}

fn collect_strings(value: &Value, keys: &mut BTreeSet<String>, strings: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            strings.insert(text.clone());
        }
        Value::Object(map) => {
            for (key, value) in map {
                keys.insert(key.clone());
                collect_strings(value, keys, strings);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_strings(item, keys, strings);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[tokio::test]
async fn it_health_bodies_disclose_no_internals() {
    let config = sentinel_config();
    let allowed_keys: BTreeSet<&str> = [
        "status",
        "live",
        "ready",
        "storage",
        "database",
        "migrations",
        "reason",
        "version",
    ]
    .into();
    let allowed_strings: BTreeSet<&str> = [
        "ok",
        "ready",
        "not_ready",
        "degraded",
        "down",
        "current",
        "storage_down",
        "storage_unreachable",
        "database_not_writable",
        "database_unavailable",
        "migrations_pending",
        VERSION,
    ]
    .into();

    let mut keys = BTreeSet::new();
    let mut strings = BTreeSet::new();
    for (lifecycle_ready, dependencies) in every_dependency_state() {
        let readiness = Readiness::new();
        if lifecycle_ready {
            readiness.set_for_test(true);
        }
        let health = Health::new(readiness);
        if let Some(state) = dependencies.database {
            health.checks().set_database(state);
        }
        if let Some(state) = dependencies.migrations {
            health.checks().set_migrations(state);
        }
        if let Some(state) = dependencies.storage {
            health.checks().set_storage(state);
        }
        assert_eq!(health.checks().snapshot(), dependencies);

        for path in PATHS {
            let (status, headers, bytes) = fetch_raw(&health, &config, path).await;
            assert!(
                status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
                "{path}: {status}"
            );
            assert!(bytes.len() < 256, "{path}: {} bytes", bytes.len());
            let text = String::from_utf8(bytes).unwrap();
            for sentinel in INFRASTRUCTURE_SENTINELS {
                assert!(!text.contains(sentinel), "{path} leaked {sentinel}: {text}");
            }
            for (name, value) in &headers {
                let value = value.to_str().unwrap();
                for sentinel in [
                    "private-bucket",
                    "/srv/private",
                    "AKIA-health",
                    "health-secret",
                ] {
                    assert!(!value.contains(sentinel), "{path} {name} leaked {sentinel}");
                }
            }
            collect_strings(
                &serde_json::from_str(&text).unwrap(),
                &mut keys,
                &mut strings,
            );
        }
    }

    for key in &keys {
        assert!(allowed_keys.contains(key.as_str()), "unexpected key {key}");
    }
    for value in &strings {
        assert!(
            allowed_strings.contains(value.as_str()),
            "unexpected value {value}"
        );
    }
    for reason in HealthReason::ALL {
        let wire = serde_json::to_value(reason).unwrap();
        assert!(
            strings.contains(wire.as_str().unwrap()),
            "{wire} never emitted"
        );
    }
}

#[test]
fn unit_health_reason_wire_values_are_the_closed_set() {
    let wire: Vec<Value> = HealthReason::ALL
        .iter()
        .map(|reason| serde_json::to_value(reason).unwrap())
        .collect();
    assert_eq!(
        wire,
        [
            "storage_down",
            "storage_unreachable",
            "database_not_writable",
            "database_unavailable",
            "migrations_pending",
        ]
    );
}

#[tokio::test]
async fn it_health_ready_reads_cached_dependency_state() {
    let (_, health) = ready_health();
    let (status, _, body) = fetch(&health, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ready", "version": VERSION }));

    let cases = [
        (
            StorageState::Ok,
            StatusCode::OK,
            json!({ "status": "ready", "storage": "ok", "version": VERSION }),
        ),
        (
            StorageState::Degraded,
            StatusCode::OK,
            json!({ "status": "ready", "storage": "degraded", "version": VERSION }),
        ),
        (
            StorageState::Down,
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "not_ready", "reason": "storage_down", "version": VERSION }),
        ),
        (
            StorageState::Unreachable,
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "not_ready", "reason": "storage_unreachable", "version": VERSION }),
        ),
    ];
    for (storage, expected_status, expected_body) in cases {
        health.checks().set_storage(storage);
        let (status, _, body) = fetch(&health, "/health/ready").await;
        assert_eq!(status, expected_status, "{storage:?}");
        assert_eq!(body, expected_body, "{storage:?}");
    }
}

#[tokio::test]
async fn it_health_ready_reports_database_before_migrations_before_storage() {
    let (_, health) = ready_health();
    health.checks().set_storage(StorageState::Down);
    health.checks().set_migrations(MigrationState::Pending);

    let expectations = [
        (DatabaseState::NotWritable, "database_not_writable"),
        (DatabaseState::Unavailable, "database_unavailable"),
        (DatabaseState::Writable, "migrations_pending"),
    ];
    for (database, reason) in expectations {
        health.checks().set_database(database);
        let (status, _, body) = fetch(&health, "/health/ready").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason"], reason);
    }

    health.checks().set_migrations(MigrationState::Current);
    let (_, _, body) = fetch(&health, "/health/ready").await;
    assert_eq!(body["reason"], "storage_down");

    health.checks().set_storage(StorageState::Ok);
    let (status, _, body) = fetch(&health, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "status": "ready",
            "storage": "ok",
            "database": "ok",
            "migrations": "current",
            "version": VERSION,
        })
    );
}

#[tokio::test]
async fn it_health_summary_reflects_readiness() {
    let (_, health) = ready_health();
    let (status, _, body) = fetch(&health, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "status": "ok", "live": true, "ready": true, "version": VERSION })
    );

    health.checks().set_database(DatabaseState::Writable);
    health.checks().set_storage(StorageState::Unreachable);
    let (status, _, body) = fetch(&health, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        json!({
            "status": "not_ready",
            "live": true,
            "ready": false,
            "storage": "down",
            "database": "ok",
            "reason": "storage_unreachable",
            "version": VERSION,
        })
    );

    health.checks().set_storage(StorageState::Ok);
    health.checks().set_database(DatabaseState::Unavailable);
    let (status, _, body) = fetch(&health, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        json!({
            "status": "not_ready",
            "live": true,
            "ready": false,
            "storage": "ok",
            "reason": "database_unavailable",
            "version": VERSION,
        })
    );
}

#[tokio::test]
async fn it_health_responses_keep_global_headers() {
    let (_, health) = ready_health();
    for path in PATHS {
        let (_, headers, _) = fetch(&health, path).await;
        assert_eq!(
            headers[CONTENT_TYPE], "application/json; charset=utf-8",
            "{path}"
        );
        assert!(headers.contains_key("x-request-id"), "{path}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{path}");
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"), "{path}: {csp}");
        assert!(!headers.contains_key("set-cookie"), "{path}");
    }
}

#[test]
fn unit_health_version_is_the_package_version() {
    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn unit_health_routes_are_public_unlimited_polled_control_plane() {
    let assembled = application_routes().build().unwrap();
    for path in PATHS {
        let entry = assembled.inventory.get(&Method::GET, path).unwrap();
        let policy = entry.policy();
        assert_eq!(policy, HEALTH_ROUTE, "{path}");
        assert_eq!(policy.auth(), AuthClass::Public, "{path}");
        assert_eq!(policy.rate_limit(), RateLimitClass::None, "{path}");
        assert_eq!(policy.transport(), Transport::ControlPlane, "{path}");
        assert_eq!(policy.security(), SecurityPolicy::Default, "{path}");
        assert_eq!(policy.request_log(), RequestLog::Polled, "{path}");
        assert!(entry.layers().deadline().is_some(), "{path}");
        assert!(entry.layers().body_limit().is_some(), "{path}");
    }
    let health_entries = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with("/health"))
        .count();
    assert_eq!(health_entries, PATHS.len());
}

#[test]
fn unit_health_openapi_declares_typed_responses() {
    let assembled = application_routes().build().unwrap();
    let doc = serde_json::to_value(&assembled.openapi).unwrap();

    let responses = |path: &str| -> BTreeSet<String> {
        doc["paths"][path]["get"]["responses"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(responses("/health/live"), ["200".to_owned()].into());
    assert_eq!(
        responses("/health/ready"),
        ["200".to_owned(), "503".to_owned()].into()
    );
    assert_eq!(
        responses("/health"),
        ["200".to_owned(), "503".to_owned()].into()
    );

    let schema_ref = |path: &str, status: &str| {
        doc["paths"][path]["get"]["responses"][status]["content"]["application/json"]["schema"]
            ["$ref"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        schema_ref("/health/live", "200"),
        "#/components/schemas/HealthLive"
    );
    assert_eq!(
        schema_ref("/health/ready", "200"),
        "#/components/schemas/HealthReady"
    );
    assert_eq!(
        schema_ref("/health/ready", "503"),
        "#/components/schemas/HealthNotReady"
    );
    assert_eq!(
        schema_ref("/health", "200"),
        "#/components/schemas/HealthSummary"
    );
    assert_eq!(
        schema_ref("/health", "503"),
        "#/components/schemas/HealthSummary"
    );

    let schemas = &doc["components"]["schemas"];
    assert_eq!(
        schemas["HealthReason"]["enum"],
        json!([
            "storage_down",
            "storage_unreachable",
            "database_not_writable",
            "database_unavailable",
            "migrations_pending",
        ])
    );
    assert_eq!(
        schemas["HealthSummaryStatus"]["enum"],
        json!(["ok", "degraded", "not_ready"])
    );
    assert_eq!(
        schemas["StorageHealthStatus"]["enum"],
        json!(["ok", "degraded", "down"])
    );
    let operations = doc["paths"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|path| path.starts_with("/health"))
        .count();
    assert_eq!(operations, PATHS.len());
    for path in PATHS {
        assert_eq!(
            doc["paths"][path]["get"]["tags"],
            json!(["health", "public"]),
            "{path}"
        );
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

async fn completion_levels(health: &Health, filter: &str, path: &str) -> Vec<String> {
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new(filter),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);
    fetch(health, path).await;
    let bytes = capture
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["message"] == "request completed")
        .map(|line| {
            assert_eq!(line["route"], path);
            line["level"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn it_health_polling_logs_at_debug() {
    let (readiness, health) = ready_health();
    for path in PATHS {
        assert!(
            completion_levels(&health, "info", path).await.is_empty(),
            "{path}"
        );
        assert_eq!(
            completion_levels(&health, "debug", path).await,
            ["DEBUG"],
            "{path}"
        );
    }

    readiness.set_for_test(false);
    assert_eq!(
        completion_levels(&health, "info", "/health/ready").await,
        ["INFO"]
    );
    assert_eq!(
        completion_levels(&health, "info", "/health").await,
        ["INFO"]
    );
    assert!(completion_levels(&health, "info", "/health/live")
        .await
        .is_empty());
}
