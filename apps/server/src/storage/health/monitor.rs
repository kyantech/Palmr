use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::Mutex;
#[cfg(test)]
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::consistency::{self, ConsistencyOutcome, ConsistencyReport};
use super::machine::{HealthMachine, Transition};
use super::report::{CheckStatus, Diagnosis, ProbeDepth, SelfTestReport, SelfTestResult};
use super::StorageHealth;
use crate::domain::clock::Clock;
use crate::infra::db::ReadPool;
use crate::storage::provider::StorageProvider;

pub const HEALTH_CHECK_PERIOD: Duration = Duration::from_secs(60);
pub const STARTUP_STORAGE_SELFTEST_FAILED: &str = "STARTUP_STORAGE_SELFTEST_FAILED";
pub const STORAGE_SELF_TEST_FAILED: &str = "STORAGE_SELF_TEST_FAILED";
pub const STORAGE_PROVIDER_MISMATCH: &str = "STORAGE_PROVIDER_MISMATCH";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthSignal {
    pub health: StorageHealth,
    pub unreachable: bool,
}

pub type HealthSink = Arc<dyn Fn(HealthSignal) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub health: StorageHealth,
    pub core: StorageHealth,
    pub cause: Option<Diagnosis>,
    pub degradations: Vec<Diagnosis>,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub last_light: Option<Arc<SelfTestReport>>,
    pub last_full: Option<Arc<SelfTestReport>>,
    pub consistency: Option<ConsistencyReport>,
}

impl StatusSnapshot {
    pub fn signal(&self) -> HealthSignal {
        HealthSignal {
            health: self.health,
            unreachable: self.health == StorageHealth::Down
                && self.cause == Some(Diagnosis::Unreachable),
        }
    }
}

#[derive(Clone)]
pub struct StorageStatus {
    current: Arc<ArcSwap<StatusSnapshot>>,
}

impl StorageStatus {
    fn new(snapshot: StatusSnapshot) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(snapshot)),
        }
    }

    pub fn snapshot(&self) -> Arc<StatusSnapshot> {
        self.current.load_full()
    }

    fn store(&self, snapshot: StatusSnapshot) {
        self.current.store(Arc::new(snapshot));
    }
}

impl std::fmt::Debug for StorageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageStatus")
            .field("health", &self.snapshot().health)
            .finish_non_exhaustive()
    }
}

pub enum Schedule {
    Every(Duration),
    #[cfg(test)]
    Manual(mpsc::Receiver<oneshot::Sender<()>>),
}

struct Supervised {
    machine: HealthMachine,
    capability: Vec<Diagnosis>,
    consistency: Option<ConsistencyReport>,
    pending_consistency: Option<ReadPool>,
    last_light: Option<Arc<SelfTestReport>>,
    last_full: Option<Arc<SelfTestReport>>,
    last_logged: Option<Diagnosis>,
    capabilities_known: bool,
}

impl Supervised {
    fn health(&self) -> StorageHealth {
        match self.machine.state() {
            StorageHealth::Ok if !self.degradations().is_empty() => StorageHealth::Degraded,
            state => state,
        }
    }

    fn degradations(&self) -> Vec<Diagnosis> {
        let mut degradations = self.capability.clone();
        degradations.extend(
            self.consistency
                .and_then(|report| report.outcome.degradation()),
        );
        degradations
    }

    fn snapshot(&self) -> StatusSnapshot {
        StatusSnapshot {
            health: self.health(),
            core: self.machine.state(),
            cause: self.machine.cause(),
            degradations: self.degradations(),
            consecutive_failures: self.machine.consecutive_failures(),
            consecutive_successes: self.machine.consecutive_successes(),
            last_light: self.last_light.clone(),
            last_full: self.last_full.clone(),
            consistency: self.consistency,
        }
    }

    fn adopt_full(&mut self, report: &Arc<SelfTestReport>) {
        if report.capabilities_verified() {
            self.capability = report.degradations().collect();
            self.capabilities_known = true;
        }
        self.last_full = Some(Arc::clone(report));
    }
}

pub struct StorageMonitor {
    provider: Arc<dyn StorageProvider>,
    clock: Arc<dyn Clock>,
    sink: HealthSink,
    status: StorageStatus,
    state: Mutex<Supervised>,
}

impl StorageMonitor {
    pub fn new(
        provider: Arc<dyn StorageProvider>,
        clock: Arc<dyn Clock>,
        sink: HealthSink,
    ) -> Self {
        let state = Supervised {
            machine: HealthMachine::starting(None),
            capability: Vec::new(),
            consistency: None,
            pending_consistency: None,
            last_light: None,
            last_full: None,
            last_logged: None,
            capabilities_known: false,
        };
        let status = StorageStatus::new(state.snapshot());
        Self {
            provider,
            clock,
            sink,
            status,
            state: Mutex::new(state),
        }
    }

    pub fn status(&self) -> StorageStatus {
        self.status.clone()
    }

    pub async fn startup(&self, catalog: Option<ReadPool>) -> Arc<SelfTestReport> {
        let started = self.clock.monotonic();
        let report = Arc::new(self.provider.self_test(ProbeDepth::Full).await);
        let mut state = self.state.lock().await;
        state.machine = HealthMachine::starting(report.core_failure());
        state.adopt_full(&report);
        state.pending_consistency = catalog;
        state.last_logged = report.core_failure();
        log_report(&report);
        log_startup(&report, &state);
        self.consistency_if_reachable(&mut state).await;
        self.publish(&state);
        tracing::debug!(
            duration_ms = millis(self.clock.monotonic().saturating_duration_since(started)),
            "storage startup self-test finished"
        );
        report
    }

    pub async fn check(&self) {
        let mut state = self.state.lock().await;
        let light = Arc::new(self.provider.self_test(ProbeDepth::Light).await);
        log_report(&light);
        state.last_light = Some(Arc::clone(&light));
        let transition = match light.core_failure() {
            None => state.machine.record_success(),
            Some(diagnosis) if state.machine.failure_would_classify_down() => {
                let full = Arc::new(self.provider.self_test(ProbeDepth::Full).await);
                log_report(&full);
                state.adopt_full(&full);
                match full.core_failure() {
                    Some(classified) => state.machine.record_failure(classified),
                    None => {
                        tracing::info!(
                            light_diagnosis = diagnosis.as_str(),
                            "a full storage self-test passed after repeated light failures"
                        );
                        state.machine.record_success()
                    }
                }
            }
            Some(diagnosis) => state.machine.record_failure(diagnosis),
        };
        self.observe(&mut state, ProbeDepth::Light, transition);
        if state.machine.state() == StorageHealth::Ok && !state.capabilities_known {
            let full = Arc::new(self.provider.self_test(ProbeDepth::Full).await);
            log_report(&full);
            state.adopt_full(&full);
        }
        self.consistency_if_reachable(&mut state).await;
        self.publish(&state);
    }

    pub async fn verify(&self) -> Arc<SelfTestReport> {
        let mut state = self.state.lock().await;
        let full = Arc::new(self.provider.self_test(ProbeDepth::Full).await);
        log_report(&full);
        state.adopt_full(&full);
        let transition = match full.core_failure() {
            Some(diagnosis) => state.machine.record_failure(diagnosis),
            None => state.machine.record_success(),
        };
        self.observe(&mut state, ProbeDepth::Full, transition);
        self.consistency_if_reachable(&mut state).await;
        self.publish(&state);
        full
    }

    pub fn spawn(
        self: &Arc<Self>,
        schedule: Schedule,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            tracing::debug!("storage health monitor started");
            match schedule {
                Schedule::Every(period) => monitor.every(period, &cancel).await,
                #[cfg(test)]
                Schedule::Manual(ticks) => monitor.manual(ticks, &cancel).await,
            }
            tracing::debug!("storage health monitor stopped");
        })
    }

    async fn every(&self, period: Duration, cancel: &CancellationToken) {
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                _ = interval.tick() => {}
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = self.check() => {}
            }
        }
    }

    #[cfg(test)]
    async fn manual(
        &self,
        mut ticks: mpsc::Receiver<oneshot::Sender<()>>,
        cancel: &CancellationToken,
    ) {
        loop {
            let done = tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                tick = ticks.recv() => match tick {
                    Some(done) => done,
                    None => return,
                },
            };
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = self.check() => {}
            }
            let _ = done.send(());
        }
    }

    fn observe(&self, state: &mut Supervised, depth: ProbeDepth, transition: Option<Transition>) {
        if let Some(transition) = transition {
            log_transition(transition);
        }
        let cause = match state.machine.state() {
            StorageHealth::Ok => None,
            StorageHealth::Degraded | StorageHealth::Down => state.machine.cause(),
        };
        if let Some(cause) = cause {
            if transition.is_some() || state.last_logged != Some(cause) {
                tracing::warn!(
                    notice = STORAGE_SELF_TEST_FAILED,
                    depth = depth.as_str(),
                    provider = self.provider.describe().provider.as_str(),
                    diagnosis = cause.as_str(),
                    consecutive_failures = state.machine.consecutive_failures(),
                    remediation = cause.remediation(),
                    "the periodic storage self-test failed"
                );
            }
        }
        state.last_logged = cause;
    }

    async fn consistency_if_reachable(&self, state: &mut Supervised) {
        if state.machine.state() != StorageHealth::Ok {
            return;
        }
        let Some(catalog) = state.pending_consistency.take() else {
            return;
        };
        let rows = match consistency::sample(&catalog).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    db_error = error.kind().as_str(),
                    "the startup storage consistency probe could not read the object catalog"
                );
                return;
            }
        };
        let report = consistency::probe(self.provider.as_ref(), &rows).await;
        log_consistency(&report);
        if matches!(report.outcome, ConsistencyOutcome::Inconclusive(_)) {
            state.pending_consistency = Some(catalog);
        } else {
            state.consistency = Some(report);
        }
    }

    fn publish(&self, state: &Supervised) {
        let snapshot = state.snapshot();
        let signal = snapshot.signal();
        self.status.store(snapshot);
        (self.sink)(signal);
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn log_report(report: &SelfTestReport) {
    for check in &report.checks {
        tracing::debug!(
            depth = report.depth.as_str(),
            provider = report.provider.as_str(),
            check = check.name.as_str(),
            status = check.status.as_str(),
            duration_ms = millis(check.duration),
            diagnosis = check.diagnosis.map(Diagnosis::as_str),
            "storage self-test check"
        );
    }
    tracing::debug!(
        depth = report.depth.as_str(),
        provider = report.provider.as_str(),
        result = report.result().as_str(),
        duration_ms = millis(report.duration),
        "storage self-test finished"
    );
}

fn log_startup(report: &SelfTestReport, state: &Supervised) {
    let provider = report.provider.as_str();
    let duration_ms = millis(report.duration);
    match report.result() {
        SelfTestResult::Passed => tracing::info!(
            provider,
            duration_ms,
            health = state.health().as_str(),
            "storage self-test passed"
        ),
        SelfTestResult::Degraded => {
            for diagnosis in report.degradations() {
                tracing::warn!(
                    startup_notice = STARTUP_STORAGE_SELFTEST_FAILED,
                    provider,
                    diagnosis = diagnosis.as_str(),
                    remediation = diagnosis.remediation(),
                    "the startup storage self-test found a degraded capability; Palmr serves traffic with storage degraded"
                );
            }
        }
        SelfTestResult::Failed => {
            let failed = report
                .checks
                .iter()
                .find(|check| check.status == CheckStatus::Failed);
            let diagnosis = report.core_failure().unwrap_or(Diagnosis::ProviderError);
            tracing::error!(
                startup_notice = STARTUP_STORAGE_SELFTEST_FAILED,
                provider,
                check = failed.map(|check| check.name.as_str()),
                diagnosis = diagnosis.as_str(),
                remediation = diagnosis.remediation(),
                "the startup storage self-test failed; Palmr keeps running, reports not ready and re-checks storage every minute"
            );
        }
    }
    for check in report
        .checks
        .iter()
        .filter(|check| matches!(check.status, CheckStatus::Warning | CheckStatus::Info))
    {
        if let Some(diagnosis) = check.diagnosis {
            tracing::warn!(
                provider,
                check = check.name.as_str(),
                diagnosis = diagnosis.as_str(),
                remediation = diagnosis.remediation(),
                "storage self-test note"
            );
        }
    }
}

fn log_transition(transition: Transition) {
    let cause = transition.cause.map(Diagnosis::as_str);
    if transition.to == StorageHealth::Ok {
        tracing::info!(
            from = transition.from.as_str(),
            to = transition.to.as_str(),
            "storage.health.transition"
        );
    } else {
        tracing::warn!(
            from = transition.from.as_str(),
            to = transition.to.as_str(),
            diagnosis = cause,
            "storage.health.transition"
        );
    }
}

fn log_consistency(report: &ConsistencyReport) {
    match report.outcome {
        ConsistencyOutcome::StorageReplaced => tracing::error!(
            severity = "fatal",
            diagnosis = Diagnosis::StorageReplaced.as_str(),
            sampled = report.sampled,
            missing = report.missing,
            present = report.present,
            remediation = Diagnosis::StorageReplaced.remediation(),
            "storage appears to have been replaced: {} of {} sampled objects are missing; nothing is repaired or deleted",
            report.missing,
            report.present + report.missing
        ),
        ConsistencyOutcome::ProviderMismatch => tracing::error!(
            severity = "fatal",
            error_code = STORAGE_PROVIDER_MISMATCH,
            sampled = report.sampled,
            mismatched = report.mismatched,
            remediation = Diagnosis::ProviderMismatch.remediation(),
            "stored objects belong to a different storage provider than the configured one; nothing is migrated or deleted"
        ),
        ConsistencyOutcome::Inconclusive(diagnosis) => tracing::warn!(
            diagnosis = diagnosis.as_str(),
            sampled = report.sampled,
            "the storage consistency probe was interrupted by a storage failure; it runs again once storage is healthy"
        ),
        ConsistencyOutcome::Empty | ConsistencyOutcome::Consistent => tracing::debug!(
            outcome = report.outcome.as_str(),
            sampled = report.sampled,
            missing = report.missing,
            "storage consistency probe finished"
        ),
    }
}
