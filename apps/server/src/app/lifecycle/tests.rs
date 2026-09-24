use std::future::{poll_fn, Future};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Extension;
use serde_json::{Map, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Notify, Semaphore};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use utoipa_axum::routes;

use super::data_dir::{
    CrossDevice, DataDirCause, DataDirError, DataDirOperation, OWNED_DIRECTORIES,
    STARTUP_DATA_DIR_NOT_WRITABLE, STARTUP_UPLOADS_STORAGE_CROSS_DEVICE,
};
use super::{
    bind, composed_router, edge_router, log_config_warnings, log_startup_completed, prepare_data,
    stop_accepting, warn_cross_device, BindError, Drain, FutureStartupStep, Readiness, Server,
    ShutdownSignal, ShutdownSignals, StartupError, EX_FAILURE, STARTUP_BIND_FAILED,
};
use crate::app::auth_class::AuthClass;
use crate::app::health::{Health, VERSION};
use crate::app::openapi::ApiDocs;
use crate::app::router::{application_routes, RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::crypto::instance_key::{
    InstanceKeyError, INSTANCE_KEY_FILE, STARTUP_INSTANCE_KEY_INVALID,
};
use crate::infra::http::shell::tests::{base_hrefs, meta, scan, VITE_INDEX};
use crate::infra::http::shell::ShellInitError;
use crate::infra::http::static_assets::dist_directory::DistDirectory;
use crate::infra::http::static_assets::StaticAssets;
use crate::infra::telemetry::{build_dispatch, write_startup_failure};

const ACCESS_KEY: &str = "AKIA-lifecycle-access-sentinel";
const SECRET_KEY: &str = "lifecycle-secret-sentinel-77e1";

const PROBE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

#[derive(Clone)]
struct Probe {
    readiness: Readiness,
    entered: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release: Arc<Notify>,
}

fn readiness_text(readiness: &Readiness) -> &'static str {
    if readiness.is_ready() {
        "ready"
    } else {
        "not-ready"
    }
}

#[utoipa::path(get, path = "/probe/ready", responses((status = 200)))]
async fn probe_ready(Extension(probe): Extension<Probe>) -> &'static str {
    readiness_text(&probe.readiness)
}

#[utoipa::path(get, path = "/probe/hold", responses((status = 200)))]
async fn probe_hold(Extension(probe): Extension<Probe>) -> &'static str {
    let entered = probe
        .entered
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(entered) = entered {
        let _ = entered.send(());
    }
    probe.release.notified().await;
    readiness_text(&probe.readiness)
}

fn config(vars: &[(&str, &str)]) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
}

fn s3_config() -> OperatorConfig {
    config(&[
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", "http://minio:9000"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", "palmr"),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
    ])
}

fn probe_router(probe: Probe) -> axum::Router {
    let routes = Routes::<()>::new()
        .route(PROBE, routes!(probe_ready))
        .route(PROBE, routes!(probe_hold))
        .build()
        .unwrap()
        .router
        .layer(Extension(probe));
    edge_router(
        routes,
        &config(&[]),
        Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC))),
    )
}

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

async fn get(address: SocketAddr, path: &str) -> (String, String) {
    get_with(address, path, "").await
}

async fn get_with(address: SocketAddr, path: &str, extra_headers: &str) -> (String, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\n{extra_headers}Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        assert!(response.len() <= 64 * 1024);
    }
    let response = String::from_utf8(response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.to_ascii_lowercase(), body.to_owned())
}

struct Harness {
    readiness: Readiness,
    server: Server,
    entered: oneshot::Receiver<()>,
    release: Arc<Notify>,
}

async fn start_probe_server() -> Harness {
    let readiness = Readiness::new();
    let (entered_tx, entered) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let router = probe_router(Probe {
        readiness: readiness.clone(),
        entered: Arc::new(Mutex::new(Some(entered_tx))),
        release: Arc::clone(&release),
    });
    let listener = bind(loopback()).await.unwrap();
    assert!(!readiness.is_ready());
    let server = Server::start(listener, router, &readiness).unwrap();
    Harness {
        readiness,
        server,
        entered,
        release,
    }
}

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

fn captured(emit: impl FnOnce()) -> (String, Vec<Map<String, Value>>) {
    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("debug"),
        LogFormat::Json,
        output.clone(),
        (),
        false,
    );
    tracing::dispatcher::with_default(&dispatch, emit);
    let text = String::from_utf8(
        output
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    )
    .unwrap();
    let lines = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (text, lines)
}

fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

#[test]
fn unit_readiness_starts_false() {
    let readiness = Readiness::new();
    assert!(!readiness.is_ready());

    let shared = readiness.clone();
    readiness.mark_ready();
    assert!(shared.is_ready());
    shared.mark_not_ready();
    assert!(!readiness.is_ready());
}

#[tokio::test]
async fn it_shutdown_flips_readiness_first() {
    let readiness = Readiness::new();
    readiness.mark_ready();
    let (stop, stopped) = oneshot::channel();
    let mut stop_accepting = pin!(stop_accepting(readiness.clone(), stopped));

    let before = poll_fn(|cx| Poll::Ready(stop_accepting.as_mut().poll(cx))).await;
    assert!(before.is_pending());
    assert!(readiness.is_ready());

    stop.send(()).unwrap();
    stop_accepting.await;
    assert!(!readiness.is_ready());

    let Harness {
        readiness,
        server,
        entered,
        release,
    } = start_probe_server().await;
    assert!(readiness.is_ready());
    let address = server.address();
    let (head, body) = get(address, "/probe/ready").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-security-policy:"), "{head}");
    assert_eq!(body, "ready");

    let in_flight = tokio::spawn(get(address, "/probe/hold"));
    entered.await.unwrap();

    let draining = tokio::spawn(server.shutdown(Duration::from_secs(10)));
    while readiness.is_ready() {
        tokio::task::yield_now().await;
    }
    release.notify_one();

    let (head, body) = in_flight.await.unwrap();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert_eq!(body, "not-ready");
    assert_eq!(draining.await.unwrap(), Drain::Completed);
    assert!(!readiness.is_ready());
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn unit_shutdown_grace_bounds_in_flight_requests() {
    let Harness {
        readiness,
        server,
        entered,
        release: _release,
    } = start_probe_server().await;
    let address = server.address();
    let in_flight = tokio::spawn(get(address, "/probe/hold"));
    entered.await.unwrap();

    let started = Instant::now();
    let drain = server.shutdown(Duration::from_millis(150)).await;

    assert_eq!(drain, Drain::GraceElapsed);
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!readiness.is_ready());
    in_flight.abort();
}

#[tokio::test]
async fn unit_shutdown_with_no_requests_completes_immediately() {
    let Harness {
        readiness, server, ..
    } = start_probe_server().await;
    let address = server.address();

    assert_eq!(
        server.shutdown(Duration::from_secs(10)).await,
        Drain::Completed
    );
    assert!(!readiness.is_ready());
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn unit_bind_occupied_address_fails_with_code() {
    let occupied = std::net::TcpListener::bind(loopback()).unwrap();
    let address = occupied.local_addr().unwrap();
    let readiness = Readiness::new();

    let error = bind(address).await.unwrap_err();

    assert_eq!(error.code(), STARTUP_BIND_FAILED);
    assert_eq!(error.address, address);
    assert_eq!(error.source.kind(), io::ErrorKind::AddrInUse);
    let text = error.to_string();
    assert!(text.starts_with(&format!("STARTUP_BIND_FAILED: cannot listen on {address}")));
    assert!(text.contains("already listening"));
    assert!(!readiness.is_ready());

    let startup = StartupError::from(error);
    assert_eq!(startup.code(), Some(STARTUP_BIND_FAILED));
    assert_eq!(startup.exit_code(), 1);
}

#[test]
fn unit_bind_permission_denied_has_hint() {
    let error = BindError {
        address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 80)),
        source: io::Error::from(io::ErrorKind::PermissionDenied),
    };

    let text = error.to_string();
    assert!(text.starts_with("STARTUP_BIND_FAILED: cannot listen on 0.0.0.0:80"));
    assert!(text.contains("PALMR_PORT of 1024 or above"));
}

#[tokio::test]
async fn it_startup_listener_serves_application_stack() {
    let dist = TempDir::new().unwrap();
    std::fs::write(dist.path().join("index.html"), VITE_INDEX).unwrap();
    let config = config(&[("PALMR_BASE_URL", "https://files.example.test/palmr")]);
    let assets =
        StaticAssets::from_source(DistDirectory::at(dist.path()), &config.base_url).unwrap();
    let readiness = Readiness::new();
    let router = composed_router(
        &config,
        Health::new(readiness.clone()),
        assets,
        Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC))),
    )
    .unwrap();
    let listener = bind(loopback()).await.unwrap();
    let server = Server::start(listener, router, &readiness).unwrap();
    assert!(readiness.is_ready());

    let (head, _) = get(server.address(), "/no/such/route").await;
    assert!(head.starts_with("http/1.1 404"), "{head}");
    assert!(head.contains("content-security-policy:"), "{head}");
    assert!(head.contains("x-content-type-options: nosniff"), "{head}");
    assert!(head.contains("x-request-id:"), "{head}");

    for path in ["/health", "/health/live", "/health/ready"] {
        let (head, body) = get(server.address(), path).await;
        assert!(head.starts_with("http/1.1 200"), "{path}: {head}");
        assert!(head.contains("content-security-policy:"), "{path}: {head}");
        assert!(head.contains("x-request-id:"), "{path}: {head}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["version"], VERSION, "{path}");
    }

    let (head, body) = get_with(
        server.address(),
        "/workspaces",
        "Accept: text/html\r\nX-Forwarded-Host: attacker.test\r\n",
    )
    .await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    let csp = head
        .lines()
        .find_map(|line| line.strip_prefix("content-security-policy: "))
        .unwrap();
    let nonce = csp
        .split_once("script-src 'self' 'nonce-")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(nonce, _)| nonce)
        .unwrap();
    assert!(
        csp.contains(&format!("style-src 'self' 'nonce-{nonce}'")),
        "{csp}"
    );
    let nodes = scan(&body);
    assert_eq!(meta(&nodes, "name", "csp-nonce"), [nonce]);
    assert_eq!(base_hrefs(&nodes), ["/palmr/"]);

    assert_eq!(
        server.shutdown(Duration::from_secs(10)).await,
        Drain::Completed
    );
}

#[test]
fn it_startup_invalid_shell_fails_before_listening() {
    let empty = TempDir::new().unwrap();
    let config = config(&[]);
    let error = StaticAssets::from_source(DistDirectory::at(empty.path()), &config.base_url)
        .map(|_| ())
        .map_err(StartupError::from)
        .unwrap_err();
    assert!(matches!(
        error,
        StartupError::Shell(ShellInitError::MissingIndex)
    ));
    assert_eq!(error.code(), None);
    assert_eq!(error.exit_code(), EX_FAILURE);
    assert_eq!(
        error.to_string(),
        "internal startup failure: the built SPA has no index.html"
    );
}

const HOLD_HEADER: &str = "x-test-hold";

#[derive(Clone)]
struct Gate {
    entered: mpsc::UnboundedSender<()>,
    release: Arc<Semaphore>,
}

async fn hold_marked_requests(State(gate): State<Gate>, request: Request, next: Next) -> Response {
    if request.headers().contains_key(HOLD_HEADER) {
        let _ = gate.entered.send(());
        gate.release.acquire().await.unwrap().forget();
    }
    next.run(request).await
}

#[tokio::test]
async fn it_health_ready_false_during_shutdown() {
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let readiness = Readiness::new();
    let (entered_tx, mut entered) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let assembled = application_routes().build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &config(&[]).base_url).unwrap();
    let routes = assembled
        .router
        .with_state(AppState::new(
            clock.clone(),
            Health::new(readiness.clone()),
            docs,
        ))
        .layer(from_fn_with_state(
            Gate {
                entered: entered_tx,
                release: Arc::clone(&release),
            },
            hold_marked_requests,
        ));
    let router = edge_router(routes, &config(&[]), clock);
    let listener = bind(loopback()).await.unwrap();
    let server = Server::start(listener, router, &readiness).unwrap();
    let address = server.address();

    let (head, body) = get(address, "/health/ready").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        serde_json::json!({ "status": "ready", "version": VERSION })
    );

    let held_paths = ["/health/ready", "/health", "/health/live"];
    let held: Vec<_> = held_paths
        .iter()
        .map(|path| tokio::spawn(get_with(address, path, "x-test-hold: 1\r\n")))
        .collect();
    for _ in held_paths {
        entered.recv().await.unwrap();
    }
    assert!(readiness.is_ready());

    let draining = tokio::spawn(server.shutdown(Duration::from_secs(10)));
    while readiness.is_ready() {
        tokio::task::yield_now().await;
    }
    assert!(!draining.is_finished());
    release.add_permits(held_paths.len());

    let mut responses = Vec::new();
    for request in held {
        let (head, body) = request.await.unwrap();
        responses.push((head, serde_json::from_str::<Value>(&body).unwrap()));
    }
    let [(ready_head, ready), (summary_head, summary), (live_head, live)] =
        <[_; 3]>::try_from(responses).unwrap();
    assert!(ready_head.starts_with("http/1.1 503"), "{ready_head}");
    assert_eq!(
        ready,
        serde_json::json!({ "status": "not_ready", "version": VERSION })
    );
    assert!(summary_head.starts_with("http/1.1 503"), "{summary_head}");
    assert_eq!(summary["status"], "not_ready");
    assert_eq!(summary["live"], true);
    assert_eq!(summary["ready"], false);
    assert!(live_head.starts_with("http/1.1 200"), "{live_head}");
    assert_eq!(
        live,
        serde_json::json!({ "status": "ok", "version": VERSION })
    );

    assert_eq!(draining.await.unwrap(), Drain::Completed);
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn unit_shutdown_signals_share_one_path() {
    use rustix::process::{getpid, kill_process, Signal};

    let mut signals = ShutdownSignals::install().unwrap();

    kill_process(getpid(), Signal::TERM).unwrap();
    assert_eq!(signals.recv().await, ShutdownSignal::Terminate);

    kill_process(getpid(), Signal::INT).unwrap();
    assert_eq!(signals.recv().await, ShutdownSignal::Interrupt);
}

#[test]
fn unit_prepare_data_orders_data_dir_before_instance_key() {
    let root = TempDir::new().unwrap();

    let (data_dir, key) = prepare_data(root.path()).unwrap();

    assert_eq!(data_dir.root(), root.path());
    for relative in OWNED_DIRECTORIES {
        assert!(root.path().join(relative).is_dir(), "{relative}");
    }
    assert_eq!(key.expose_secret().len(), 32);
    assert!(root.path().join(INSTANCE_KEY_FILE).is_file());
    assert!(!root.path().join("palmr.db").exists());
}

#[test]
fn unit_prepare_data_stops_before_key_when_data_dir_fails() {
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("data");

    let error = prepare_data(&missing).unwrap_err();

    assert_eq!(error.code(), Some(STARTUP_DATA_DIR_NOT_WRITABLE));
    assert_eq!(error.exit_code(), 78);
    assert!(!missing.exists());

    if is_root() {
        return;
    }
    let readonly = TempDir::new().unwrap();
    std::fs::set_permissions(
        readonly.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o500),
    )
    .unwrap();
    let error = prepare_data(readonly.path()).unwrap_err();
    std::fs::set_permissions(
        readonly.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    assert_eq!(error.code(), Some(STARTUP_DATA_DIR_NOT_WRITABLE));
    assert!(!readonly.path().join(INSTANCE_KEY_FILE).exists());
}

#[test]
fn unit_prepare_data_rejects_invalid_key_after_validating_tree() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join(INSTANCE_KEY_FILE), [7_u8; 31]).unwrap();
    std::fs::set_permissions(
        root.path().join(INSTANCE_KEY_FILE),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();

    let error = prepare_data(root.path()).unwrap_err();

    assert_eq!(error.code(), Some(STARTUP_INSTANCE_KEY_INVALID));
    assert_eq!(error.exit_code(), 78);
    assert!(matches!(
        error,
        StartupError::InstanceKey(InstanceKeyError::Invalid { .. })
    ));
    assert!(root.path().join("storage/objects").is_dir());
    assert_eq!(
        std::fs::metadata(root.path().join(INSTANCE_KEY_FILE))
            .unwrap()
            .len(),
        31
    );
}

#[test]
fn unit_startup_failure_is_one_line_with_code() {
    let error = StartupError::DataDir(DataDirError {
        path: "/data/storage/objects".into(),
        operation: DataDirOperation::Create,
        cause: DataDirCause::Io(io::Error::from(io::ErrorKind::ReadOnlyFilesystem)),
        identity: super::data_dir::ProcessIdentity {
            uid: 10001,
            gid: 10001,
        },
        observed: None,
        data_root: false,
    });

    let mut out = Vec::new();
    write_startup_failure(&mut out, &error).unwrap();
    let line = String::from_utf8(out).unwrap();

    assert!(line.starts_with("FATAL: STARTUP_DATA_DIR_NOT_WRITABLE: Palmr cannot use /data/storage/objects (operation: create;"));
    assert!(line.contains("running as uid=10001 gid=10001"));
    assert!(line.contains("remove `:ro`"));
    assert_eq!(line.matches('\n').count(), 1);
    assert_eq!(error.exit_code(), 78);
}

#[test]
fn unit_startup_error_log_is_structured() {
    let error = StartupError::DataDir(DataDirError {
        path: "/data/uploads".into(),
        operation: DataDirOperation::Fsync,
        cause: DataDirCause::Io(io::Error::from(io::ErrorKind::StorageFull)),
        identity: super::data_dir::ProcessIdentity {
            uid: 1000,
            gid: 1000,
        },
        observed: Some(super::data_dir::ObservedPath {
            parent: false,
            uid: 0,
            gid: 0,
            mode: 0o755,
        }),
        data_root: false,
    });

    let (_, lines) = captured(|| error.log());

    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line["level"], "ERROR");
    assert_eq!(line["startup_error"], STARTUP_DATA_DIR_NOT_WRITABLE);
    assert_eq!(line["path"], "/data/uploads");
    assert_eq!(line["operation"], "fsync");
    assert_eq!(line["uid"], 1000);
    assert_eq!(line["observed_parent"], false);
    assert_eq!(line["owner_uid"], 0);
    assert_eq!(line["mode"], "0755");
}

#[test]
fn it_startup_exdev_warning_is_logged_as_warning() {
    let (_, lines) = captured(|| {
        warn_cross_device(CrossDevice {
            uploads_device: 0x0801,
            objects_device: 0x0900,
        });
    });

    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line["level"], "WARN");
    assert_eq!(
        line["startup_warning"],
        STARTUP_UPLOADS_STORAGE_CROSS_DEVICE
    );
    assert_eq!(line["uploads_device"], "0x0801");
    assert_eq!(line["objects_device"], "0x0900");
    assert!(line["message"]
        .as_str()
        .unwrap()
        .contains("instead of an atomic rename"));
}

#[test]
fn unit_startup_diagnostics_never_print_secrets() {
    let loaded = OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", "http://minio:9000"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", "palmr"),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
        ("PALMR_S3_REJECT_UNAUTHORIZED", "false"),
        ("PALMR_S3_SECRETKEY", SECRET_KEY),
    ]))
    .unwrap();
    let config = s3_config();

    let (text, lines) = captured(|| {
        log_config_warnings(&loaded.warnings);
        log_startup_completed(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 5487)),
            &config,
            Duration::from_millis(42),
        );
    });

    assert!(!text.contains(ACCESS_KEY), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
    let completed = lines
        .iter()
        .find(|line| line.get("message") == Some(&Value::from("startup.completed")))
        .unwrap();
    assert_eq!(completed["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(completed["bound_address"], "127.0.0.1:5487");
    assert_eq!(completed["storage_provider"], "s3");
    assert_eq!(completed["duration_ms"], 42);
    assert!(lines.iter().any(
        |line| line.get("startup_warning") == Some(&Value::from("STARTUP_BASE_URL_DEFAULTED"))
    ));
    assert!(lines
        .iter()
        .any(|line| line.get("variable") == Some(&Value::from("PALMR_S3_SECRETKEY"))));
}

#[test]
fn unit_startup_error_codes_and_exit_codes() {
    let config_error =
        OperatorConfig::load(&EnvironmentSource::from_vars([("PALMR_PORT", "0")])).unwrap_err();
    let config_error = StartupError::from(config_error);
    assert_eq!(config_error.code(), Some("STARTUP_CONFIG_INVALID"));
    assert_eq!(config_error.exit_code(), 1);

    let router_error = Routes::<()>::new()
        .route(PROBE, routes!(probe_ready))
        .route(PROBE, routes!(probe_ready))
        .build()
        .err()
        .unwrap();
    let router_error = StartupError::Router(router_error);
    assert_eq!(router_error.code(), None);
    assert_eq!(router_error.exit_code(), 1);
    assert!(router_error
        .to_string()
        .starts_with("internal startup failure: "));
}

#[test]
fn unit_future_lifecycle_steps_are_reserved_in_order() {
    let startup: Vec<&str> = FutureStartupStep::IN_ORDER
        .iter()
        .map(|step| step.as_str())
        .collect();
    assert_eq!(startup, ["settings", "storage", "reconcile"]);
}
