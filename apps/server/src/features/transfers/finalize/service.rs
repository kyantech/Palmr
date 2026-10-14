use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::service::AuditService;
use crate::features::settings::SettingsHandle;
use crate::infra::db::{DbError, DbPools};
use crate::storage::error::StorageError;

use super::super::admission::TransferStorage;
use super::super::error::TransferError;
use super::super::model::SessionItemId;
use super::commit::Committer;
use super::hooks::{FinalizeHooks, NoHooks};
use super::repo;

const REDRIVE_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizeLimits {
    pub lease_ttl: Duration,
    pub heartbeat: Duration,
    pub concurrency: usize,
    pub busy_wait: Duration,
    pub busy_poll: Duration,
    pub attempts: u32,
    pub backoff: Duration,
    pub sweep_interval: Duration,
}

impl FinalizeLimits {
    pub const fn production() -> Self {
        Self {
            lease_ttl: Duration::from_secs(60),
            heartbeat: Duration::from_secs(20),
            concurrency: 4,
            busy_wait: Duration::from_secs(30),
            busy_poll: Duration::from_millis(25),
            attempts: 5,
            backoff: Duration::from_millis(500),
            sweep_interval: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizeOutcome {
    Committed { file_id: String, created: bool },
    Failed { code: String, retryable: bool },
    Superseded,
    NotReady,
    Busy,
}

#[derive(Debug)]
pub enum FinalizeError {
    Transfer(TransferError),
    Storage(StorageError),
    NotAvailable,
    Interrupted,
    ShuttingDown,
}

impl FinalizeError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Transfer(error) => error.kind(),
            Self::Storage(error) => error.kind(),
            Self::NotAvailable => "finalize_not_available",
            Self::Interrupted => "finalize_interrupted",
            Self::ShuttingDown => "finalize_shutting_down",
        }
    }
}

impl fmt::Display for FinalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transfer(error) => write!(f, "upload finalization failed: {error}"),
            Self::Storage(error) => write!(f, "upload finalization storage failure: {error}"),
            Self::NotAvailable => f.write_str("the storage provider has no local staging area"),
            Self::Interrupted => f.write_str("upload finalization was interrupted"),
            Self::ShuttingDown => f.write_str("upload finalization stopped for shutdown"),
        }
    }
}

impl std::error::Error for FinalizeError {}

impl From<TransferError> for FinalizeError {
    fn from(error: TransferError) -> Self {
        Self::Transfer(error)
    }
}

impl From<StorageError> for FinalizeError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<DbError> for FinalizeError {
    fn from(error: DbError) -> Self {
        Self::Transfer(error.into())
    }
}

impl From<sqlx::Error> for FinalizeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Transfer(error.into())
    }
}

impl From<crate::domain::time::InvalidTimestamp> for FinalizeError {
    fn from(error: crate::domain::time::InvalidTimestamp) -> Self {
        Self::Transfer(error.into())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedriveReport {
    pub visited: u64,
    pub committed: u64,
    pub failed: u64,
    pub superseded: u64,
    pub skipped: u64,
    pub errors: u64,
}

pub struct FinalizerParts {
    pub pools: DbPools,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsHandle,
    pub storage: TransferStorage,
    pub audit: AuditService,
    pub holder: String,
    pub stop: CancellationToken,
    pub limits: FinalizeLimits,
    pub hooks: Option<Arc<dyn FinalizeHooks>>,
}

pub(super) struct Inner {
    pub(super) pools: DbPools,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) storage: TransferStorage,
    pub(super) settings: SettingsHandle,
    pub(super) committer: Committer,
    pub(super) holder: Arc<str>,
    pub(super) limits: FinalizeLimits,
    pub(super) hooks: Arc<dyn FinalizeHooks>,
    pub(super) stop: CancellationToken,
    active: Mutex<HashSet<SessionItemId>>,
    tasks: Mutex<JoinSet<()>>,
    permits: Semaphore,
    sweeping: AtomicBool,
}

#[derive(Clone)]
pub struct Finalizer {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct ActiveSlot {
    inner: Arc<Inner>,
    item: SessionItemId,
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        if let Ok(mut active) = self.inner.active.lock() {
            active.remove(&self.item);
        }
    }
}

impl Finalizer {
    pub fn new(parts: FinalizerParts) -> Self {
        let committer = Committer::new(
            parts.pools.clone(),
            Arc::clone(&parts.clock),
            parts.settings.clone(),
            parts.audit,
            parts.storage.clone(),
        );
        let hooks: Arc<dyn FinalizeHooks> = parts.hooks.unwrap_or_else(|| Arc::new(NoHooks));
        Self {
            inner: Arc::new(Inner {
                pools: parts.pools,
                clock: parts.clock,
                storage: parts.storage,
                settings: parts.settings,
                committer,
                holder: Arc::from(format!("{}:finalize", parts.holder)),
                limits: parts.limits,
                hooks,
                stop: parts.stop,
                active: Mutex::new(HashSet::new()),
                tasks: Mutex::new(JoinSet::new()),
                permits: Semaphore::new(parts.limits.concurrency.max(1)),
                sweeping: AtomicBool::new(false),
            }),
        }
    }

    pub(super) fn now(&self) -> Result<Timestamp, FinalizeError> {
        Ok(Timestamp::try_from(self.inner.clock.now())?)
    }

    pub(super) fn enter(&self, item: SessionItemId) -> Option<ActiveSlot> {
        let mut active = self.inner.active.lock().ok()?;
        active.insert(item).then(|| ActiveSlot {
            inner: Arc::clone(&self.inner),
            item,
        })
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the S3 and Received finalizers await a result through this entry point"
        )
    )]
    pub async fn finalize(
        &self,
        item: SessionItemId,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        self.run_local(item, request_id, true).await
    }

    pub fn trigger(&self, item: SessionItemId, request_id: Option<String>) {
        let this = self.clone();
        self.spawn(async move { this.drive(item, request_id).await });
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let Ok(mut tasks) = self.inner.tasks.lock() else {
            return;
        };
        while tasks.try_join_next().is_some() {}
        tasks.spawn(task);
    }

    async fn drive(&self, item: SessionItemId, request_id: Option<String>) {
        let Ok(_permit) = self.inner.permits.acquire().await else {
            return;
        };
        let attempts = self.inner.limits.attempts.max(1);
        for attempt in 0..attempts {
            match self.run_local(item, request_id.as_deref(), false).await {
                Ok(outcome) => {
                    tracing::debug!(outcome = ?outcome, "an upload finalization finished");
                    return;
                }
                Err(FinalizeError::Interrupted | FinalizeError::ShuttingDown) => return,
                Err(error) => {
                    tracing::warn!(
                        kind = error.kind(),
                        attempt,
                        "an upload finalization failed and is retried"
                    );
                    let delay = self
                        .inner
                        .limits
                        .backoff
                        .saturating_mul(1_u32.checked_shl(attempt).unwrap_or(u32::MAX));
                    tokio::select! {
                        () = self.inner.stop.cancelled() => return,
                        () = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
        tracing::error!(
            "an upload finalization kept failing; the startup re-drive or a retry resumes it"
        );
    }

    pub async fn redrive(&self) -> Result<RedriveReport, TransferError> {
        let mut report = RedriveReport::default();
        let mut after: Option<String> = None;
        loop {
            let page = {
                let mut connection = self
                    .inner
                    .pools
                    .reader()
                    .executor()
                    .acquire()
                    .await
                    .map_err(DbError::from)?;
                repo::redrive_page(
                    &mut connection,
                    after.as_deref(),
                    REDRIVE_PAGE_SIZE,
                    Timestamp::try_from(self.inner.clock.now())?,
                )
                .await?
            };
            let Some(last) = page.last().copied() else {
                return Ok(report);
            };
            for item in &page {
                if self.inner.stop.is_cancelled() {
                    return Ok(report);
                }
                report.visited += 1;
                match self.run_local(*item, None, false).await {
                    Ok(FinalizeOutcome::Committed { .. }) => report.committed += 1,
                    Ok(FinalizeOutcome::Failed { .. }) => report.failed += 1,
                    Ok(FinalizeOutcome::Superseded) => report.superseded += 1,
                    Ok(FinalizeOutcome::NotReady | FinalizeOutcome::Busy) => report.skipped += 1,
                    Err(error) => {
                        report.errors += 1;
                        tracing::warn!(
                            kind = error.kind(),
                            "an upload finalization could not be re-driven"
                        );
                    }
                }
            }
            after = Some(last.to_string());
            if page.len() < usize::try_from(REDRIVE_PAGE_SIZE).unwrap_or(usize::MAX) {
                return Ok(report);
            }
        }
    }

    pub async fn pending_redrive(&self) -> Result<u64, TransferError> {
        let mut connection = self
            .inner
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        repo::redrive_count(
            &mut connection,
            Timestamp::try_from(self.inner.clock.now())?,
        )
        .await
    }

    pub async fn start_redrive(&self) -> Result<u64, TransferError> {
        let pending = self.pending_redrive().await?;
        if pending > 0 {
            let this = self.clone();
            self.spawn(async move {
                match this.redrive().await {
                    Ok(report) => tracing::info!(
                        visited = report.visited,
                        committed = report.committed,
                        failed = report.failed,
                        superseded = report.superseded,
                        skipped = report.skipped,
                        errors = report.errors,
                        "upload finalization re-drive finished"
                    ),
                    Err(error) => tracing::warn!(
                        kind = error.kind(),
                        "upload finalization re-drive could not page its candidates"
                    ),
                }
            });
        }
        Ok(pending)
    }

    pub fn spawn_sweeper(&self, interval: Duration) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = this.inner.stop.cancelled() => return,
                    () = tokio::time::sleep(interval) => {}
                }
                this.sweep();
            }
        });
    }

    pub(super) fn sweep(&self) {
        if self.inner.sweeping.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        self.spawn(async move {
            match this.pending_redrive().await {
                Ok(0) => {}
                Ok(_) => {
                    if let Err(error) = this.redrive().await {
                        tracing::warn!(
                            kind = error.kind(),
                            "the periodic upload finalization sweep could not page its candidates"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    kind = error.kind(),
                    "the periodic upload finalization sweep could not count its candidates"
                ),
            }
            this.inner.sweeping.store(false, Ordering::SeqCst);
        });
    }

    pub async fn idle(&self) {
        let mut set = self
            .inner
            .tasks
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        while set.join_next().await.is_some() {}
    }

    pub async fn drain(&self, grace: Duration) -> bool {
        tokio::time::timeout(grace, self.idle()).await.is_ok()
    }
}
