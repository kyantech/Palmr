mod data_dir;
mod database;

use std::fmt;
use std::future::IntoFuture;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::migrate::Migrator;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, Signal, SignalKind};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use self::data_dir::{
    apply_process_umask, CrossDevice, DataDir, DataDirError, STARTUP_UPLOADS_STORAGE_CROSS_DEVICE,
};
use self::database::{log_database_closed, Database};
use super::health::Health;
use super::openapi::{ApiDocs, ApiDocsError};
use super::router::{
    application_routes, serve_unmatched, with_middleware, HttpEdge, RouteBuildError,
};
use super::state::AppState;
use crate::config::{
    ConfigError, ConfigWarning, EnvironmentSource, LoadedConfig, OperatorConfig, StorageConfig,
    STARTUP_BASE_URL_DEFAULTED,
};
use crate::domain::clock::{Clock, SystemClock};
use crate::infra::crypto::instance_key::{InstanceKey, InstanceKeyError, KeyOrigin};
use crate::infra::db::{DbOpenError, MigrationError, MIGRATOR};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::http::shell::ShellInitError;
use crate::infra::http::static_assets::StaticAssets;
use crate::infra::telemetry::{self, write_startup_failure, TelemetryInitError};

pub const STARTUP_BIND_FAILED: &str = "STARTUP_BIND_FAILED";

const EX_CONFIG: u8 = 78;
const EX_FAILURE: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutureStartupStep {
    Settings,
    Storage,
    Reconcile,
    Workers,
}

impl FutureStartupStep {
    pub const IN_ORDER: [Self; 4] = [
        Self::Settings,
        Self::Storage,
        Self::Reconcile,
        Self::Workers,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Storage => "storage",
            Self::Reconcile => "reconcile",
            Self::Workers => "workers",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutureShutdownStep {
    StopWorkers,
}

impl FutureShutdownStep {
    pub const IN_ORDER: [Self; 1] = [Self::StopWorkers];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StopWorkers => "stop_workers",
        }
    }
}

#[derive(Debug)]
pub enum StartupError {
    Config(ConfigError),
    Tracing(TelemetryInitError),
    DataDir(DataDirError),
    InstanceKey(InstanceKeyError),
    Database(DbOpenError),
    Migration(MigrationError),
    Bind(BindError),
    Router(RouteBuildError),
    Shell(ShellInitError),
    ApiDocs(ApiDocsError),
}

impl StartupError {
    pub const fn code(&self) -> Option<&'static str> {
        match self {
            Self::Config(error) => Some(error.code()),
            Self::Tracing(error) => Some(error.code()),
            Self::DataDir(error) => Some(error.code()),
            Self::InstanceKey(error) => Some(error.code()),
            Self::Database(error) => Some(error.code()),
            Self::Migration(error) => Some(error.code()),
            Self::Bind(error) => Some(error.code()),
            Self::Router(_) | Self::Shell(_) | Self::ApiDocs(_) => None,
        }
    }

    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::DataDir(_) | Self::InstanceKey(_) | Self::Database(_) | Self::Migration(_) => {
                EX_CONFIG
            }
            Self::Config(_)
            | Self::Tracing(_)
            | Self::Bind(_)
            | Self::Router(_)
            | Self::Shell(_)
            | Self::ApiDocs(_) => EX_FAILURE,
        }
    }

    fn log(&self) {
        match self {
            Self::DataDir(error) => tracing::error!(
                startup_error = error.code(),
                path = %error.path.display(),
                operation = error.operation.as_str(),
                uid = error.identity.uid,
                gid = error.identity.gid,
                observed_parent = error.observed.map(|observed| observed.parent),
                owner_uid = error.observed.map(|observed| observed.uid),
                owner_gid = error.observed.map(|observed| observed.gid),
                mode = error.observed.map(|observed| format!("{:04o}", observed.mode)),
                "{self}"
            ),
            Self::InstanceKey(error) => tracing::error!(
                startup_error = error.code(),
                path = %error.path().display(),
                "{self}"
            ),
            Self::Database(error) => tracing::error!(
                startup_error = error.code(),
                path = %error.path.display(),
                pool = error.pool().map(|pool| pool.as_str()),
                "{self}"
            ),
            Self::Migration(error) => tracing::error!(
                startup_error = error.code(),
                path = %error.path.display(),
                migration_version = error.version(),
                "{self}"
            ),
            Self::Bind(error) => tracing::error!(
                startup_error = error.code(),
                address = %error.address,
                "{self}"
            ),
            Self::Config(_)
            | Self::Tracing(_)
            | Self::Router(_)
            | Self::Shell(_)
            | Self::ApiDocs(_) => {
                tracing::error!(startup_error = self.code(), "{self}");
            }
        }
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::Tracing(error) => error.fmt(f),
            Self::DataDir(error) => error.fmt(f),
            Self::InstanceKey(error) => error.fmt(f),
            Self::Database(error) => error.fmt(f),
            Self::Migration(error) => error.fmt(f),
            Self::Bind(error) => error.fmt(f),
            Self::Router(error) => write!(f, "internal startup failure: {error}"),
            Self::Shell(error) => write!(f, "internal startup failure: {error}"),
            Self::ApiDocs(error) => write!(f, "internal startup failure: {error}"),
        }
    }
}

impl std::error::Error for StartupError {}

impl From<ConfigError> for StartupError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<TelemetryInitError> for StartupError {
    fn from(error: TelemetryInitError) -> Self {
        Self::Tracing(error)
    }
}

impl From<DataDirError> for StartupError {
    fn from(error: DataDirError) -> Self {
        Self::DataDir(error)
    }
}

impl From<InstanceKeyError> for StartupError {
    fn from(error: InstanceKeyError) -> Self {
        Self::InstanceKey(error)
    }
}

impl From<DbOpenError> for StartupError {
    fn from(error: DbOpenError) -> Self {
        Self::Database(error)
    }
}

impl From<MigrationError> for StartupError {
    fn from(error: MigrationError) -> Self {
        Self::Migration(error)
    }
}

impl From<ShellInitError> for StartupError {
    fn from(error: ShellInitError) -> Self {
        Self::Shell(error)
    }
}

impl From<BindError> for StartupError {
    fn from(error: BindError) -> Self {
        Self::Bind(error)
    }
}

#[derive(Debug)]
pub struct BindError {
    pub address: SocketAddr,
    pub source: io::Error,
}

impl BindError {
    pub const fn code(&self) -> &'static str {
        STARTUP_BIND_FAILED
    }

    fn hint(&self) -> &'static str {
        match self.source.kind() {
            io::ErrorKind::AddrInUse => {
                "another process is already listening on this address; stop it or choose a different PALMR_PORT"
            }
            io::ErrorKind::PermissionDenied => {
                "the process may not bind this port; ports below 1024 need extra privileges, so use a PALMR_PORT of 1024 or above and publish it through Docker"
            }
            io::ErrorKind::AddrNotAvailable => {
                "PALMR_HOST is not an address of this machine"
            }
            _ => "check PALMR_HOST and PALMR_PORT",
        }
    }
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{STARTUP_BIND_FAILED}: cannot listen on {}: {}. {}",
            self.address,
            self.source,
            self.hint()
        )
    }
}

impl std::error::Error for BindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub async fn bind(address: SocketAddr) -> Result<TcpListener, BindError> {
    TcpListener::bind(address)
        .await
        .map_err(|source| BindError { address, source })
}

#[derive(Debug, Clone, Default)]
pub struct Readiness(Arc<AtomicBool>);

impl Readiness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn mark_ready(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn mark_not_ready(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn set_for_test(&self, ready: bool) {
        self.0.store(ready, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drain {
    Completed,
    GraceElapsed,
}

impl Drain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::GraceElapsed => "grace_elapsed",
        }
    }
}

pub struct Server {
    address: SocketAddr,
    task: JoinHandle<io::Result<()>>,
    stop: oneshot::Sender<()>,
}

pub struct Application {
    server: Server,
    readiness: Readiness,
    database: Database,
    _data_dir: DataDir,
    _instance_key: InstanceKey,
}

impl Application {
    pub async fn bind(
        config: &OperatorConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartupError> {
        Self::bind_with(config, clock, &MIGRATOR).await
    }

    async fn bind_with(
        config: &OperatorConfig,
        clock: Arc<dyn Clock>,
        migrator: &Migrator,
    ) -> Result<Self, StartupError> {
        let initialized = initialize(config, clock, migrator).await?;
        let address = SocketAddr::new(config.host, config.port);
        match bind(address).await {
            Ok(listener) => Self::from_listener(listener, initialized).await,
            Err(error) => Err(initialized.abandon(error.into()).await),
        }
    }

    pub async fn start(
        listener: TcpListener,
        config: &OperatorConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartupError> {
        let initialized = initialize(config, clock, &MIGRATOR).await?;
        Self::from_listener(listener, initialized).await
    }

    async fn from_listener(
        listener: TcpListener,
        initialized: InitializedApplication,
    ) -> Result<Self, StartupError> {
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(source) => {
                let error = StartupError::Bind(BindError {
                    address: SocketAddr::from(([127, 0, 0, 1], 0)),
                    source,
                });
                return Err(initialized.abandon(error).await);
            }
        };
        let server =
            match Server::start(listener, initialized.router.clone(), &initialized.readiness) {
                Ok(server) => server,
                Err(source) => {
                    let error = StartupError::Bind(BindError { address, source });
                    return Err(initialized.abandon(error).await);
                }
            };
        Ok(Self {
            server,
            readiness: initialized.readiness,
            database: initialized.database,
            _data_dir: initialized.data_dir,
            _instance_key: initialized.instance_key,
        })
    }

    pub const fn address(&self) -> SocketAddr {
        self.server.address()
    }

    pub async fn shutdown(self, grace: Duration) -> Drain {
        let Self {
            server,
            readiness: _,
            database,
            _data_dir: data_dir,
            _instance_key: instance_key,
        } = self;
        let drain = server.shutdown(grace).await;
        for step in FutureShutdownStep::IN_ORDER {
            tracing::debug!(
                step = step.as_str(),
                "shutdown step reserved for a later release"
            );
        }
        log_database_closed(&database.close().await);
        drop((data_dir, instance_key));
        drain
    }
}

struct InitializedApplication {
    router: axum::Router,
    readiness: Readiness,
    database: Database,
    data_dir: DataDir,
    instance_key: InstanceKey,
}

impl InitializedApplication {
    async fn abandon(self, error: StartupError) -> StartupError {
        log_database_closed(&self.database.close().await);
        error
    }
}

impl Server {
    pub fn start(
        listener: TcpListener,
        router: axum::Router,
        readiness: &Readiness,
    ) -> io::Result<Self> {
        let address = listener.local_addr()?;
        let (stop, stopped) = oneshot::channel();
        let service = router.into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(
            axum::serve(listener, service)
                .with_graceful_shutdown(stop_accepting(readiness.clone(), stopped))
                .into_future(),
        );
        readiness.mark_ready();
        Ok(Self {
            address,
            task,
            stop,
        })
    }

    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub async fn shutdown(mut self, grace: Duration) -> Drain {
        let _ = self.stop.send(());
        if tokio::time::timeout(grace, &mut self.task).await.is_ok() {
            Drain::Completed
        } else {
            self.task.abort();
            Drain::GraceElapsed
        }
    }
}

// The HTTP server stops accepting connections only once this future resolves,
// so readiness is always withdrawn before the listener closes. Proxies polling
// readiness then stop routing here while in-flight requests keep being served.
async fn stop_accepting(readiness: Readiness, stopped: oneshot::Receiver<()>) {
    let _ = stopped.await;
    readiness.mark_not_ready();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    Terminate,
    Interrupt,
}

impl ShutdownSignal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
        }
    }
}

pub struct ShutdownSignals {
    terminate: Signal,
    interrupt: Signal,
}

impl ShutdownSignals {
    pub fn install() -> io::Result<Self> {
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    pub async fn recv(&mut self) -> ShutdownSignal {
        tokio::select! {
            _ = self.terminate.recv() => ShutdownSignal::Terminate,
            _ = self.interrupt.recv() => ShutdownSignal::Interrupt,
        }
    }
}

pub async fn run(source: &EnvironmentSource, mut signals: ShutdownSignals) -> ExitCode {
    let clock = SystemClock;
    let started = clock.monotonic();

    let LoadedConfig { config, warnings } = match OperatorConfig::load(source) {
        Ok(loaded) => loaded,
        Err(error) => return startup_failed(&error.into(), Diagnostics::StderrOnly),
    };
    if let Err(error) = telemetry::init(&config.log_level, config.log_format) {
        return startup_failed(&error.into(), Diagnostics::StderrOnly);
    }
    log_config_warnings(&warnings);

    let mut application = match Application::bind(&config, Arc::new(clock)).await {
        Ok(application) => application,
        Err(error) => return startup_failed(&error, Diagnostics::Logged),
    };
    log_startup_completed(
        application.address(),
        &config,
        clock.monotonic().saturating_duration_since(started),
    );

    let signal = tokio::select! {
        signal = signals.recv() => signal,
        ended = &mut application.server.task => {
            application.readiness.mark_not_ready();
            tracing::error!(outcome = ?ended.map(|result| result.map_err(|error| error.kind())), "the HTTP server stopped unexpectedly");
            log_database_closed(&application.database.close().await);
            flush_diagnostics();
            return ExitCode::from(EX_FAILURE);
        }
    };
    tracing::info!(
        signal = signal.as_str(),
        grace_secs = config.shutdown_grace.as_secs(),
        "shutdown.started"
    );

    let drain = application.shutdown(config.shutdown_grace).await;
    tracing::info!(drain = drain.as_str(), "shutdown.completed");
    flush_diagnostics();
    ExitCode::SUCCESS
}

fn prepare_data(root: &Path) -> Result<(DataDir, InstanceKey), StartupError> {
    let inherited = apply_process_umask();
    tracing::debug!(
        inherited_umask = format!("{inherited:04o}"),
        "process umask set"
    );

    let data_dir = DataDir::prepare(root)?;
    if let Some(cross) = data_dir.cross_device() {
        warn_cross_device(cross);
    }

    let (key, origin) = InstanceKey::load_or_create(data_dir.root())?;
    match origin {
        KeyOrigin::Created => tracing::info!("instance key created"),
        KeyOrigin::Loaded => tracing::debug!("instance key loaded"),
    }
    Ok((data_dir, key))
}

async fn initialize(
    config: &OperatorConfig,
    clock: Arc<dyn Clock>,
    migrator: &Migrator,
) -> Result<InitializedApplication, StartupError> {
    let (data_dir, instance_key) = tokio::task::block_in_place(|| prepare_data(&config.data_dir))?;
    let readiness = Readiness::new();
    let health = Health::new(readiness.clone());
    let database = Database::open(config, data_dir.root(), &health).await?;
    if let Err(error) = database.migrate(migrator, &health).await {
        log_database_closed(&database.close().await);
        return Err(error.into());
    }
    for step in FutureStartupStep::IN_ORDER {
        tracing::debug!(
            step = step.as_str(),
            "startup step reserved for a later release"
        );
    }
    let router = match StaticAssets::built(&config.base_url)
        .map_err(StartupError::from)
        .and_then(|assets| composed_router(config, health, assets, clock))
    {
        Ok(router) => router,
        Err(error) => {
            log_database_closed(&database.close().await);
            return Err(error);
        }
    };
    Ok(InitializedApplication {
        router,
        readiness,
        database,
        data_dir,
        instance_key,
    })
}

pub fn application_router(
    config: &OperatorConfig,
    readiness: &Readiness,
    clock: Arc<dyn Clock>,
) -> Result<axum::Router, StartupError> {
    let assets = StaticAssets::built(&config.base_url)?;
    composed_router(config, Health::new(readiness.clone()), assets, clock)
}

fn composed_router(
    config: &OperatorConfig,
    health: Health,
    assets: StaticAssets,
    clock: Arc<dyn Clock>,
) -> Result<axum::Router, StartupError> {
    let assembled = application_routes().build().map_err(StartupError::Router)?;
    let api_docs =
        ApiDocs::new(assembled.openapi, &config.base_url).map_err(StartupError::ApiDocs)?;
    let routes = serve_unmatched(assembled.router, assets).with_state(AppState::new(
        Arc::clone(&clock),
        health,
        api_docs,
    ));
    Ok(edge_router(routes, config, clock))
}

fn edge_router(
    routes: axum::Router,
    config: &OperatorConfig,
    clock: Arc<dyn Clock>,
) -> axum::Router {
    let edge = HttpEdge::new(
        clock,
        TrustedProxies::new(&config.trust_proxy),
        SecurityHeaders::new(config),
    );
    axum::Router::new().fallback_service(with_middleware(routes, &edge))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Diagnostics {
    StderrOnly,
    Logged,
}

fn startup_failed(error: &StartupError, diagnostics: Diagnostics) -> ExitCode {
    if diagnostics == Diagnostics::Logged {
        error.log();
    }
    flush_diagnostics();
    let _ = write_startup_failure(&mut io::stderr().lock(), error);
    ExitCode::from(error.exit_code())
}

fn flush_diagnostics() {
    let _ = io::stdout().lock().flush();
    let _ = io::stderr().lock().flush();
}

fn warn_cross_device(cross: CrossDevice) {
    tracing::warn!(
        startup_warning = STARTUP_UPLOADS_STORAGE_CROSS_DEVICE,
        uploads_device = format!("{:#06x}", cross.uploads_device),
        objects_device = format!("{:#06x}", cross.objects_device),
        "upload staging (uploads/) and object storage (storage/objects/) are on different filesystems; finalization will copy each completed upload through a bounded buffer instead of an atomic rename, writing its bytes twice and taking roughly twice as long. Mount a single volume at PALMR_DATA_DIR to restore the fast path"
    );
}

fn log_config_warnings(warnings: &[ConfigWarning]) {
    for warning in warnings {
        match warning {
            ConfigWarning::BaseUrlDefaulted { effective } => tracing::warn!(
                startup_warning = STARTUP_BASE_URL_DEFAULTED,
                base_url = %effective,
                "PALMR_BASE_URL is not set; generated links and OAuth callbacks point at the default base URL and browsers reaching Palmr through any other origin have state-changing requests refused"
            ),
            ConfigWarning::S3VariablesIgnored { variables } => {
                let names: Vec<&str> = variables.iter().map(|variable| variable.name()).collect();
                tracing::warn!(
                    variables = names.join(","),
                    "PALMR_S3_* variables are ignored because the storage provider is local"
                );
            }
            ConfigWarning::S3TlsVerificationDisabled { endpoint } => tracing::warn!(
                endpoint = %endpoint,
                "TLS certificate verification is disabled for the S3 client; prefer PALMR_S3_CA_FILE"
            ),
            ConfigWarning::UnknownVariable { name } => tracing::warn!(
                variable = name.as_str(),
                "unrecognized PALMR_* variable ignored; check it for typos"
            ),
        }
    }
}

const fn storage_provider(config: &OperatorConfig) -> &'static str {
    match config.storage {
        StorageConfig::Local => "local",
        StorageConfig::S3(_) => "s3",
    }
}

fn log_startup_completed(address: SocketAddr, config: &OperatorConfig, elapsed: Duration) {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bound_address = %address,
        storage_provider = storage_provider(config),
        duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        "startup.completed"
    );
}

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod tests;
