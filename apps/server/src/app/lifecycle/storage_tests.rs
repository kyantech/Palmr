use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;
use time::macros::datetime;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use super::{Application, Drain, StartupError};
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::telemetry::build_dispatch;
use crate::storage::build_provider;
use crate::storage::health::{ProbeDepth, Schedule, SelfTestResult};
use crate::storage::s3::fake_server::{FakeS3, PresignFault, BUCKET};

const ACCESS_KEY: &str = "AKIA-startup-access-sentinel";
const SECRET_KEY: &str = "startup-secret-sentinel-81c4";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn reserve_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn load(vars: &[(&str, &str)]) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
}

fn s3_config(data: &Path, address: SocketAddr, endpoint: &str) -> OperatorConfig {
    s3_config_with(data, address, endpoint, ("minio", None))
}

fn s3_config_with(
    data: &Path,
    address: SocketAddr,
    endpoint: &str,
    (profile, forced): (&str, Option<&str>),
) -> OperatorConfig {
    let port = address.port().to_string();
    let base_url = format!("http://{address}");
    load(
        &[
            ("PALMR_HOST", "127.0.0.1"),
            ("PALMR_PORT", &port),
            ("PALMR_BASE_URL", &base_url),
            ("PALMR_DATA_DIR", data.to_str().unwrap()),
            ("PALMR_STORAGE_PROVIDER", "s3"),
            ("PALMR_S3_ENDPOINT", endpoint),
            ("PALMR_S3_REGION", "us-east-1"),
            ("PALMR_S3_BUCKET", BUCKET),
            ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
            ("PALMR_S3_SECRET_KEY", SECRET_KEY),
            ("PALMR_S3_PROFILE", profile),
        ]
        .into_iter()
        .chain(forced.map(|value| ("PALMR_S3_REQUIRE_PART_CHECKSUMS", value)))
        .collect::<Vec<_>>(),
    )
}

async fn tick(ticks: &mpsc::Sender<oneshot::Sender<()>>) {
    let (done, finished) = oneshot::channel();
    ticks.send(done).await.unwrap();
    finished.await.unwrap();
}

async fn fetch(address: SocketAddr, path: &str) -> (u16, String) {
    let response = reqwest::get(format!("http://{address}{path}"))
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap())
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[tokio::test(flavor = "multi_thread")]
async fn it_startup_s3_down_degraded_not_fatal() {
    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("palmr_server=debug"),
        LogFormat::Json,
        output.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let data = TempDir::new().unwrap();
    let s3_port = reserve_port();
    let endpoint = format!("http://127.0.0.1:{s3_port}");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = s3_config(data.path(), address, &endpoint);
    let (ticks, schedule) = mpsc::channel(1);

    let application = Application::start_with_schedule(
        listener,
        &config,
        Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        Schedule::Manual(schedule),
    )
    .await
    .unwrap_or_else(|error| panic!("an unreachable S3 backend must not abort startup: {error}"));

    let (status, live) = fetch(address, "/health/live").await;
    assert_eq!(status, 200);
    assert_eq!(json(&live)["status"], "ok");

    let (status, ready) = fetch(address, "/health/ready").await;
    assert_eq!(status, 503);
    let ready_body = json(&ready);
    assert_eq!(keys(&ready_body), ["reason", "status", "version"]);
    assert_eq!(ready_body["status"], "not_ready");
    assert_eq!(ready_body["reason"], "storage_unreachable");

    let (status, summary) = fetch(address, "/health").await;
    assert_eq!(status, 503);
    let summary_body = json(&summary);
    assert_eq!(summary_body["status"], "not_ready");
    assert_eq!(summary_body["storage"], "down");
    assert_eq!(summary_body["reason"], "storage_unreachable");
    for body in [&live, &ready, &summary] {
        for leaked in [
            s3_port.to_string().as_str(),
            ACCESS_KEY,
            SECRET_KEY,
            BUCKET,
            "_palmr",
            "Dispatch",
            "refused",
            "HeadBucket",
        ] {
            assert!(!body.contains(leaked), "{leaked} in {body}");
        }
    }

    let (status, _) = fetch(address, "/no/such/route").await;
    assert_ne!(status, 503);

    let logs = String::from_utf8(
        output
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    )
    .unwrap();
    assert!(logs.contains("STARTUP_STORAGE_SELFTEST_FAILED"), "{logs}");
    assert!(logs.contains("\"diagnosis\":\"unreachable\""), "{logs}");
    for leaked in [ACCESS_KEY, SECRET_KEY, "X-Amz-Signature", "Authorization"] {
        assert!(!logs.contains(leaked), "{leaked} leaked");
    }

    tick(&ticks).await;
    assert_eq!(fetch(address, "/health/ready").await.0, 503);

    let fake = FakeS3::start_on(s3_port).await;
    tick(&ticks).await;
    let (status, body) = fetch(address, "/health/ready").await;
    assert_eq!(
        status, 503,
        "down recovers only after two consecutive successes"
    );
    assert_eq!(json(&body)["reason"], "storage_unreachable");

    tick(&ticks).await;
    let (status, body) = fetch(address, "/health/ready").await;
    assert_eq!(status, 200, "{body}");
    let body = json(&body);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["storage"], "ok");

    let seen = fake.seen();
    assert!(seen
        .iter()
        .any(|request| request.method == "HEAD" && request.path == format!("/{BUCKET}/")));
    assert!(seen.iter().any(|request| request.method == "OPTIONS"));
    assert!(seen.iter().any(|request| request.presigned()));
    assert!(fake.keys().is_empty());

    assert_eq!(
        application.shutdown(Duration::from_secs(5)).await,
        Drain::Completed
    );
    let (done, _) = oneshot::channel();
    assert!(
        ticks.send(done).await.is_err(),
        "the health task stops with the application"
    );
}

async fn start_with_profile(
    profile: &str,
    fault: PresignFault,
    output: &Capture,
) -> (Application, SocketAddr, FakeS3, TempDir) {
    start_configured((profile, None), fault, output).await
}

async fn start_configured(
    selection: (&str, Option<&str>),
    fault: PresignFault,
    output: &Capture,
) -> (Application, SocketAddr, FakeS3, TempDir) {
    let dispatch = build_dispatch(
        EnvFilter::new("palmr_server=debug"),
        LogFormat::Json,
        output.clone(),
        (),
        false,
    );
    let guard = tracing::dispatcher::set_default(&dispatch);
    let data = TempDir::new().unwrap();
    let fake = FakeS3::start().await;
    fake.set_fault(fault);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = s3_config_with(data.path(), address, &fake.endpoint(), selection);
    let (_ticks, schedule) = mpsc::channel(1);
    let application = Application::start_with_schedule(
        listener,
        &config,
        Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        Schedule::Manual(schedule),
    )
    .await
    .unwrap();
    drop(guard);
    (application, address, fake, data)
}

fn log_lines(output: &Capture) -> Vec<Value> {
    String::from_utf8(
        output
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    )
    .unwrap()
    .lines()
    .filter_map(|line| serde_json::from_str(line).ok())
    .collect()
}

fn proxied_warnings(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|line| line["data_plane"] == "proxied")
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_startup_warns_once_when_the_provider_requires_proxied_parts() {
    let output = Capture::default();
    let (application, address, _fake, _data) =
        start_with_profile("r2", PresignFault::SignatureMismatch, &output).await;

    let (_, ready) = fetch(address, "/health/ready").await;
    let ready = json(&ready);
    assert_eq!(ready["dataPlane"], "proxied", "{ready}");
    assert_eq!(ready["proxyCeilingBytes"], 83_047_219_200_u64);

    let lines = log_lines(&output);
    let warnings = proxied_warnings(&lines);
    assert_eq!(warnings.len(), 1, "one startup warning, never one per part");
    let warning = warnings[0];
    assert_eq!(warning["checksum_requirement"], "profile");
    assert_eq!(warning["level"], "WARN");
    assert_eq!(warning["part_size_bytes"], 8_388_608);
    assert_eq!(warning["max_concurrent_parts"], 8);
    assert_eq!(warning["max_object_bytes"], 83_047_219_200_u64);
    let text = warning["message"].as_str().unwrap();
    assert!(text.contains("slower"), "{text}");
    let logs = serde_json::to_string(&lines).unwrap();
    for leaked in [
        ACCESS_KEY,
        SECRET_KEY,
        "X-Amz-Signature",
        "Authorization",
        "objects/",
    ] {
        assert!(!logs.contains(leaked), "{leaked} leaked");
    }
    assert_eq!(
        application.shutdown(Duration::from_secs(5)).await,
        Drain::Completed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn it_startup_keeps_direct_parts_when_the_self_test_proves_them_for_a_checksum_profile() {
    let output = Capture::default();
    let (application, address, _fake, _data) =
        start_with_profile("r2", PresignFault::None, &output).await;

    let (_, ready) = fetch(address, "/health/ready").await;
    let ready = json(&ready);
    assert_eq!(ready["dataPlane"], "direct", "{ready}");
    assert!(ready.get("proxyCeilingBytes").is_none(), "{ready}");
    let lines = log_lines(&output);
    assert!(proxied_warnings(&lines).is_empty(), "no stale warning");
    assert!(lines.iter().any(|line| line["message"]
        .as_str()
        .is_some_and(|message| message.contains("parts go directly from the browser"))));

    let (application_minio, address, _fake, _data) =
        start_with_profile("minio", PresignFault::None, &Capture::default()).await;
    let (_, ready) = fetch(address, "/health/ready").await;
    assert_eq!(json(&ready)["dataPlane"], "direct");
    for application in [application, application_minio] {
        assert_eq!(
            application.shutdown(Duration::from_secs(5)).await,
            Drain::Completed
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_startup_honors_an_explicit_checksum_requirement_over_profile_and_self_test() {
    let output = Capture::default();
    let (forced_on, address, _fake, _data) =
        start_configured(("generic", Some("true")), PresignFault::None, &output).await;
    let (_, ready) = fetch(address, "/health/ready").await;
    let ready = json(&ready);
    assert_eq!(
        ready["dataPlane"], "proxied",
        "a tolerant provider does not undo the operator's setting: {ready}"
    );
    assert_eq!(ready["proxyCeilingBytes"], 83_047_219_200_u64);
    let lines = log_lines(&output);
    let warnings = proxied_warnings(&lines);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["checksum_requirement"], "operator");
    assert!(
        !lines.iter().any(|line| line["message"]
            .as_str()
            .is_some_and(|message| message.contains("parts go directly from the browser"))),
        "no relaxation is logged"
    );
    assert!(lines.iter().any(|line| line["message"]
        .as_str()
        .is_some_and(|message| message.contains("PALMR_S3_REQUIRE_PART_CHECKSUMS is set"))));

    let (forced_off, address, _fake, _data) = start_configured(
        ("r2", Some("false")),
        PresignFault::SignatureMismatch,
        &Capture::default(),
    )
    .await;
    let (_, ready) = fetch(address, "/health/ready").await;
    assert_eq!(
        json(&ready)["dataPlane"],
        "direct",
        "an explicit false beats the r2 default even when the probe fails"
    );
    for application in [forced_on, forced_off] {
        assert_eq!(
            application.shutdown(Duration::from_secs(5)).await,
            Drain::Completed
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_startup_installs_local_provider_and_probes_it() {
    let data = TempDir::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let port = address.port().to_string();
    let config = load(&[
        ("PALMR_HOST", "127.0.0.1"),
        ("PALMR_PORT", &port),
        ("PALMR_DATA_DIR", data.path().to_str().unwrap()),
    ]);
    let (_ticks, schedule) = mpsc::channel(1);
    let application = Application::start_with_schedule(
        listener,
        &config,
        Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        Schedule::Manual(schedule),
    )
    .await
    .unwrap();

    let (status, body) = fetch(address, "/health/ready").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(json(&body)["storage"], "ok");
    assert!(
        json(&body).get("dataPlane").is_none(),
        "local storage has no object-store data plane: {body}"
    );
    let probes = data.path().join("storage").join("_palmr").join("probe");
    assert!(probes.is_dir());
    assert_eq!(std::fs::read_dir(&probes).unwrap().count(), 0);
    assert_eq!(
        application.shutdown(Duration::from_secs(5)).await,
        Drain::Completed
    );
}

#[tokio::test]
async fn it_storage_factory_builds_one_real_provider_per_kind() {
    let data = TempDir::new().unwrap();
    for dir in ["storage/objects", "uploads", "branding"] {
        std::fs::create_dir_all(data.path().join(dir)).unwrap();
    }
    let clock = Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC)));
    let local = build_provider(
        &load(&[("PALMR_DATA_DIR", data.path().to_str().unwrap())]),
        clock.clone(),
    )
    .unwrap();
    assert_eq!(local.describe().provider.as_str(), "local");
    assert!(local.as_multipart().is_none() && local.as_presign().is_none());
    assert_eq!(
        local.self_test(ProbeDepth::Full).await.result(),
        SelfTestResult::Passed
    );

    let fake = FakeS3::start().await;
    let s3 = build_provider(
        &s3_config(
            data.path(),
            SocketAddr::from(([127, 0, 0, 1], 5487)),
            &fake.endpoint(),
        ),
        clock,
    )
    .unwrap();
    assert_eq!(s3.describe().provider.as_str(), "s3");
    assert!(s3.as_multipart().is_some() && s3.as_presign().is_some());
    assert_eq!(
        s3.self_test(ProbeDepth::Light).await.result(),
        SelfTestResult::Passed
    );

    let missing_ca = load(&[
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", &fake.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", BUCKET),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
        ("PALMR_S3_CA_FILE", "/nonexistent/palmr-ca.pem"),
    ]);
    let error = StartupError::from(
        build_provider(
            &missing_ca,
            Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        )
        .err()
        .unwrap(),
    );
    assert_eq!(error.code(), Some("STORAGE_CONFIG_INVALID"));
    assert_eq!(error.exit_code(), 78);
    assert!(!error.to_string().contains(SECRET_KEY));

    let empty = TempDir::new().unwrap();
    let error = StartupError::from(
        build_provider(
            &load(&[("PALMR_DATA_DIR", empty.path().to_str().unwrap())]),
            Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        )
        .err()
        .unwrap(),
    );
    assert_eq!(error.code(), Some("STARTUP_DATA_DIR_NOT_WRITABLE"));
}

#[test]
fn unit_app_state_holds_one_non_optional_provider() {
    let state = include_str!("../state.rs");
    let production = state.split("#[cfg(test)]\nmod tests").next().unwrap();
    assert!(production.contains("    storage: Arc<dyn StorageProvider>,\n"));
    for forbidden in [
        "Option<Arc<dyn StorageProvider>>",
        "LocalProvider",
        "S3Provider",
        "NullStorageProvider",
        "StubStorageProvider",
        "UnavailableStorageProvider",
        "ArcSwap<Arc<dyn StorageProvider",
    ] {
        assert!(!production.contains(forbidden), "{forbidden}");
    }

    let lifecycle = include_str!("../lifecycle.rs");
    let production = lifecycle.split("#[cfg(test)]\nmod ").next().unwrap();
    let initialize = production
        .split("async fn initialize(")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    let order = [
        "SettingsService::load",
        "storage::build_provider",
        "monitor\n        .startup(",
        "StorageHealthTask::start",
        "ReconcileRegistry::production()",
        "start_jobs(",
        "composed_router(",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|needle| {
            initialize
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from initialize"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{positions:?}"
    );
    assert_eq!(production.matches("StorageHealthTask::start(").count(), 1);
    assert_eq!(production.matches(".spawn(schedule").count(), 1);

    let router = include_str!("../router.rs");
    let health = include_str!("../health.rs");
    for source in [router, health] {
        assert!(!source.contains("StorageMonitor"));
        assert!(!source.contains("self_test("));
    }
}
