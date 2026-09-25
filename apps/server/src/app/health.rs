#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::routes;

use super::auth_class::AuthClass;
use super::lifecycle::Readiness;
use super::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use super::state::AppState;
use crate::infra::http::error::JSON_CONTENT_TYPE;
use crate::infra::http::trace::RequestLog;
use crate::storage::health::{HealthSignal, StorageHealth};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HEALTH_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
)
.with_request_log(RequestLog::Polled);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthReason {
    StorageDown,
    StorageUnreachable,
    DatabaseNotWritable,
    DatabaseUnavailable,
    MigrationsPending,
}

impl HealthReason {
    pub const ALL: [Self; 5] = [
        Self::StorageDown,
        Self::StorageUnreachable,
        Self::DatabaseNotWritable,
        Self::DatabaseUnavailable,
        Self::MigrationsPending,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseState {
    Writable,
    NotWritable,
    Unavailable,
}

impl DatabaseState {
    const fn failure(self) -> Option<HealthReason> {
        match self {
            Self::Writable => None,
            Self::NotWritable => Some(HealthReason::DatabaseNotWritable),
            Self::Unavailable => Some(HealthReason::DatabaseUnavailable),
        }
    }

    const fn status(self) -> Option<DatabaseHealthStatus> {
        match self {
            Self::Writable => Some(DatabaseHealthStatus::Ok),
            Self::NotWritable | Self::Unavailable => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    Current,
    Pending,
}

impl MigrationState {
    const fn failure(self) -> Option<HealthReason> {
        match self {
            Self::Current => None,
            Self::Pending => Some(HealthReason::MigrationsPending),
        }
    }

    const fn status(self) -> Option<MigrationHealthStatus> {
        match self {
            Self::Current => Some(MigrationHealthStatus::Current),
            Self::Pending => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageState {
    Ok,
    Degraded,
    Down,
    Unreachable,
}

impl StorageState {
    pub const fn from_signal(signal: HealthSignal) -> Self {
        match (signal.health, signal.unreachable) {
            (StorageHealth::Ok, _) => Self::Ok,
            (StorageHealth::Degraded, _) => Self::Degraded,
            (StorageHealth::Down, true) => Self::Unreachable,
            (StorageHealth::Down, false) => Self::Down,
        }
    }

    const fn failure(self) -> Option<HealthReason> {
        match self {
            Self::Ok | Self::Degraded => None,
            Self::Down => Some(HealthReason::StorageDown),
            Self::Unreachable => Some(HealthReason::StorageUnreachable),
        }
    }

    const fn status(self) -> StorageHealthStatus {
        match self {
            Self::Ok => StorageHealthStatus::Ok,
            Self::Degraded => StorageHealthStatus::Degraded,
            Self::Down | Self::Unreachable => StorageHealthStatus::Down,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DependencyHealth {
    pub database: Option<DatabaseState>,
    pub migrations: Option<MigrationState>,
    pub storage: Option<StorageState>,
}

impl DependencyHealth {
    fn failure(self) -> Option<HealthReason> {
        self.database
            .and_then(DatabaseState::failure)
            .or_else(|| self.migrations.and_then(MigrationState::failure))
            .or_else(|| self.storage.and_then(StorageState::failure))
    }
}

// The owning infrastructure publishes its last observed state here from its
// own background work; health handlers only read it, so polling can never
// trigger database or storage I/O.
#[derive(Debug, Default)]
pub struct DependencyChecks {
    cached: RwLock<DependencyHealth>,
    #[cfg(test)]
    snapshots: AtomicUsize,
}

impl DependencyChecks {
    pub fn set_database(&self, state: DatabaseState) {
        self.update(|health| health.database = Some(state));
    }

    pub fn set_migrations(&self, state: MigrationState) {
        self.update(|health| health.migrations = Some(state));
    }

    pub fn set_storage(&self, state: StorageState) {
        self.update(|health| health.storage = Some(state));
    }

    pub fn snapshot(&self) -> DependencyHealth {
        #[cfg(test)]
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        *self.cached.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, apply: impl FnOnce(&mut DependencyHealth)) {
        apply(&mut self.cached.write().unwrap_or_else(PoisonError::into_inner));
    }
}

#[derive(Debug, Clone)]
pub struct Health {
    readiness: Readiness,
    checks: Arc<DependencyChecks>,
}

impl Health {
    pub fn new(readiness: Readiness) -> Self {
        Self {
            readiness,
            checks: Arc::default(),
        }
    }

    pub fn checks(&self) -> &DependencyChecks {
        &self.checks
    }

    fn assess(&self) -> Assessment {
        let dependencies = self.checks.snapshot();
        let reason = dependencies.failure();
        Assessment {
            ready: self.readiness.is_ready() && reason.is_none(),
            reason,
            dependencies,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Assessment {
    ready: bool,
    reason: Option<HealthReason>,
    dependencies: DependencyHealth,
}

impl Assessment {
    const fn status_code(&self) -> StatusCode {
        if self.ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthLiveStatus {
    Ok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthReadyStatus {
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthNotReadyStatus {
    NotReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthSummaryStatus {
    Ok,
    Degraded,
    NotReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseHealthStatus {
    Ok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MigrationHealthStatus {
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct HealthLive {
    status: HealthLiveStatus,
    version: &'static str,
}

impl HealthLive {
    const CURRENT: Self = Self {
        status: HealthLiveStatus::Ok,
        version: VERSION,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct HealthReady {
    status: HealthReadyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage: Option<StorageHealthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<DatabaseHealthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    migrations: Option<MigrationHealthStatus>,
    version: &'static str,
}

impl HealthReady {
    fn new(dependencies: DependencyHealth) -> Self {
        Self {
            status: HealthReadyStatus::Ready,
            storage: dependencies.storage.map(StorageState::status),
            database: dependencies.database.and_then(DatabaseState::status),
            migrations: dependencies.migrations.and_then(MigrationState::status),
            version: VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct HealthNotReady {
    status: HealthNotReadyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<HealthReason>,
    version: &'static str,
}

impl HealthNotReady {
    const fn new(reason: Option<HealthReason>) -> Self {
        Self {
            status: HealthNotReadyStatus::NotReady,
            reason,
            version: VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct HealthSummary {
    status: HealthSummaryStatus,
    live: bool,
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage: Option<StorageHealthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<DatabaseHealthStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<HealthReason>,
    version: &'static str,
}

impl HealthSummary {
    fn new(assessment: Assessment) -> Self {
        let storage = assessment.dependencies.storage.map(StorageState::status);
        let status = match (assessment.ready, storage) {
            (false, _) => HealthSummaryStatus::NotReady,
            (true, Some(StorageHealthStatus::Degraded)) => HealthSummaryStatus::Degraded,
            (true, _) => HealthSummaryStatus::Ok,
        };
        Self {
            status,
            live: true,
            ready: assessment.ready,
            storage,
            database: assessment
                .dependencies
                .database
                .and_then(DatabaseState::status),
            reason: assessment.reason,
            version: VERSION,
        }
    }
}

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(HEALTH_ROUTE, routes!(live))
        .route(HEALTH_ROUTE, routes!(ready))
        .route(HEALTH_ROUTE, routes!(summary))
}

#[utoipa::path(
    get,
    path = "/health/live",
    tag = "health",
    responses((status = 200, description = "The process is responsive.", body = HealthLive))
)]
async fn live() -> Response {
    json_response(StatusCode::OK, &HealthLive::CURRENT)
}

#[utoipa::path(
    get,
    path = "/health/ready",
    tag = "health",
    responses(
        (status = 200, description = "Traffic may be routed to this instance.", body = HealthReady),
        (status = 503, description = "Traffic must not be routed to this instance.", body = HealthNotReady),
    )
)]
async fn ready(State(state): State<AppState>) -> Response {
    let assessment = state.health().assess();
    if assessment.ready {
        json_response(StatusCode::OK, &HealthReady::new(assessment.dependencies))
    } else {
        json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &HealthNotReady::new(assessment.reason),
        )
    }
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "The instance is ready.", body = HealthSummary),
        (status = 503, description = "The instance is not ready.", body = HealthSummary),
    )
)]
async fn summary(State(state): State<AppState>) -> Response {
    let assessment = state.health().assess();
    json_response(assessment.status_code(), &HealthSummary::new(assessment))
}

fn json_response(status: StatusCode, body: &impl Serialize) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => (
            status,
            [(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests;
