use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{interval_at, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use super::backoff::Jitter;
use super::claim::{
    claim, has_runnable, renew, settle_failure, settle_success, sweep_expired_leases, Settled,
};
use super::kinds::JobKind;
use super::{Claimant, ClaimedJob, JobId, JobsError};
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, InstanceId};

pub type HandlerResult = anyhow::Result<()>;
type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;
type HandlerFn = Arc<dyn Fn(ClaimedJob) -> HandlerFuture + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Idempotency(&'static str);

impl Idempotency {
    pub const fn key(key: &'static str) -> Self {
        Self(key)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone)]
struct Handler {
    idempotency: Idempotency,
    run: HandlerFn,
}

#[derive(Clone, Default)]
pub struct Registry {
    handlers: BTreeMap<JobKind, Handler>,
}

impl Registry {
    pub fn production() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn register<F, Fut>(mut self, kind: JobKind, idempotency: Idempotency, handler: F) -> Self
    where
        F: Fn(ClaimedJob) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        let run: HandlerFn = Arc::new(move |job| Box::pin(handler(job)));
        let previous = self.handlers.insert(kind, Handler { idempotency, run });
        assert!(
            previous.is_none(),
            "job kind {kind} has more than one handler"
        );
        self
    }

    pub fn kinds(&self) -> Vec<JobKind> {
        self.handlers.keys().copied().collect()
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(
                self.handlers
                    .iter()
                    .map(|(kind, handler)| (kind.as_str(), handler.idempotency.as_str())),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    HandlerFailed,
    HandlerPanicked,
    NoHandler,
}

impl FailureClass {
    pub const fn code(self) -> &'static str {
        match self {
            Self::HandlerFailed => "JOB_HANDLER_FAILED",
            Self::HandlerPanicked => "JOB_HANDLER_PANICKED",
            Self::NoHandler => "JOB_NO_HANDLER",
        }
    }

    fn last_error(self, job: &ClaimedJob) -> String {
        format!(
            "{} job_id={} attempt={}",
            self.code(),
            job.id(),
            job.attempts().saturating_add(1)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAuditEvent {
    DeadLettered {
        job_id: JobId,
        kind: JobKind,
        attempts: u32,
        failure: FailureClass,
    },
}

impl JobAuditEvent {
    pub const fn action(&self) -> &'static str {
        match self {
            Self::DeadLettered { .. } => "JOB_DEAD_LETTERED",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct JobAudit(Option<mpsc::Sender<JobAuditEvent>>);

impl JobAudit {
    pub const fn detached() -> Self {
        Self(None)
    }

    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<JobAuditEvent>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self(Some(sender)), receiver)
    }

    fn emit(&self, event: JobAuditEvent) {
        let Some(sender) = &self.0 else {
            return;
        };
        match sender.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => tracing::warn!(
                action = event.action(),
                "job audit event dropped because the audit channel is full"
            ),
            Err(TrySendError::Closed(event)) => tracing::warn!(
                action = event.action(),
                "job audit event dropped because the audit channel is closed"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Retrying { run_at: Timestamp },
    DeadLettered { attempts: u32 },
    LeaseLost,
    Interrupted,
}

impl From<Settled> for Outcome {
    fn from(settled: Settled) -> Self {
        match settled {
            Settled::Succeeded => Self::Succeeded,
            Settled::Retrying { run_at } => Self::Retrying { run_at },
            Settled::Dead { attempts } => Self::DeadLettered { attempts },
            Settled::LeaseLost => Self::LeaseLost,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTiming {
    pub poll: Duration,
    pub lease_sweep: Duration,
    pub lease_renewal: Duration,
}

impl RuntimeTiming {
    pub const DEFAULT: Self = Self {
        poll: Duration::from_secs(1),
        lease_sweep: Duration::from_secs(30),
        lease_renewal: Duration::from_secs(60),
    };
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Inner {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    registry: Registry,
    kinds: Vec<JobKind>,
    jitter: Jitter,
    audit: JobAudit,
    lease_renewal: Duration,
}

#[derive(Clone)]
pub struct Dispatcher(Arc<Inner>);

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("registry", &self.0.registry)
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        registry: Registry,
        jitter: Jitter,
        audit: JobAudit,
        lease_renewal: Duration,
    ) -> Self {
        let kinds = registry.kinds();
        Self(Arc::new(Inner {
            pools,
            clock,
            registry,
            kinds,
            jitter,
            audit,
            lease_renewal,
        }))
    }

    pub async fn run_next(
        &self,
        claimant: &Claimant,
        proceed: impl Fn() -> bool,
    ) -> Result<Option<Outcome>, JobsError> {
        let inner = &self.0;
        self.run_next_in(claimant, &inner.kinds, proceed).await
    }

    pub async fn run_next_kind(
        &self,
        claimant: &Claimant,
        kind: JobKind,
        proceed: impl Fn() -> bool,
    ) -> Result<Option<Outcome>, JobsError> {
        self.run_next_in(claimant, std::slice::from_ref(&kind), proceed)
            .await
    }

    async fn run_next_in(
        &self,
        claimant: &Claimant,
        kinds: &[JobKind],
        proceed: impl Fn() -> bool,
    ) -> Result<Option<Outcome>, JobsError> {
        let inner = &self.0;
        if !has_runnable(&inner.pools, inner.clock.as_ref(), kinds).await? {
            return Ok(None);
        }
        let claimed = claim(
            &inner.pools,
            inner.clock.as_ref(),
            claimant,
            kinds,
            1,
            proceed,
        )
        .await?;
        match claimed.into_iter().next() {
            Some(job) => Ok(Some(self.execute(job).await?)),
            None => Ok(None),
        }
    }

    pub async fn execute(&self, job: ClaimedJob) -> Result<Outcome, JobsError> {
        let failure = match self.0.registry.handlers.get(&job.kind()) {
            Some(handler) => match self.supervise(handler, &job).await {
                Ok(Ok(())) => None,
                Ok(Err(_)) => Some(FailureClass::HandlerFailed),
                Err(error) if error.is_panic() => Some(FailureClass::HandlerPanicked),
                Err(_) => return Ok(Outcome::Interrupted),
            },
            None => Some(FailureClass::NoHandler),
        };
        let outcome = match failure {
            None => settle_success(&self.0.pools, self.0.clock.as_ref(), &job)
                .await?
                .into(),
            Some(failure) => self.settle_failed(&job, failure).await?,
        };
        if outcome == Outcome::LeaseLost {
            tracing::warn!(
                job_id = %job.id(),
                kind = job.kind().as_str(),
                "job lease was lost before its result could be recorded; the job runs again"
            );
        }
        Ok(outcome)
    }

    async fn supervise(
        &self,
        handler: &Handler,
        job: &ClaimedJob,
    ) -> Result<HandlerResult, JoinError> {
        let mut task = AbortOnDrop(tokio::spawn((handler.run)(job.clone())));
        let period = self.0.lease_renewal;
        let mut renewals = interval_at(Instant::now() + period, period);
        renewals.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                joined = &mut task.0 => return joined,
                _ = renewals.tick() => self.renew_lease(job).await,
            }
        }
    }

    async fn renew_lease(&self, job: &ClaimedJob) {
        match renew(&self.0.pools, self.0.clock.as_ref(), job).await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                job_id = %job.id(),
                kind = job.kind().as_str(),
                "job lease could not be renewed because it is no longer held"
            ),
            Err(error) => tracing::warn!(
                job_id = %job.id(),
                kind = job.kind().as_str(),
                error_kind = error.kind(),
                "job lease renewal failed"
            ),
        }
    }

    async fn settle_failed(
        &self,
        job: &ClaimedJob,
        failure: FailureClass,
    ) -> Result<Outcome, JobsError> {
        let settled = settle_failure(
            &self.0.pools,
            self.0.clock.as_ref(),
            job,
            &failure.last_error(job),
            self.0.jitter.sample(),
        )
        .await?;
        match settled {
            Settled::Retrying { run_at } => tracing::warn!(
                job_id = %job.id(),
                kind = job.kind().as_str(),
                attempts = job.attempts().saturating_add(1),
                max_attempts = job.max_attempts(),
                error_class = failure.code(),
                run_at = %run_at,
                "job.retry_scheduled"
            ),
            Settled::Dead { attempts } => {
                tracing::error!(
                    job_id = %job.id(),
                    kind = job.kind().as_str(),
                    attempts,
                    max_attempts = job.max_attempts(),
                    error_class = failure.code(),
                    "job.dead_lettered"
                );
                self.0.audit.emit(JobAuditEvent::DeadLettered {
                    job_id: job.id(),
                    kind: job.kind(),
                    attempts,
                    failure,
                });
            }
            Settled::Succeeded | Settled::LeaseLost => {}
        }
        Ok(settled.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobsDrain {
    Completed,
    GraceElapsed { interrupted: usize },
}

impl JobsDrain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::GraceElapsed { .. } => "grace_elapsed",
        }
    }
}

pub struct JobRuntime {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

impl JobRuntime {
    pub fn start(
        dispatcher: &Dispatcher,
        instance: InstanceId,
        workers: u16,
        timing: RuntimeTiming,
    ) -> Self {
        let cancel = CancellationToken::new();
        let mut tasks: Vec<JoinHandle<()>> = (0..workers)
            .map(|index| {
                tokio::spawn(work(
                    dispatcher.clone(),
                    Claimant::worker(instance, index),
                    cancel.clone(),
                    timing.poll,
                ))
            })
            .collect();
        tasks.push(tokio::spawn(sweep(
            dispatcher.clone(),
            cancel.clone(),
            timing.lease_sweep,
        )));
        tracing::info!(
            workers,
            handlers = dispatcher.0.kinds.len(),
            "job workers started"
        );
        Self { cancel, tasks }
    }

    pub fn stop_claiming(&self) {
        self.cancel.cancel();
    }

    pub async fn shutdown(mut self, grace: Duration) -> JobsDrain {
        self.cancel.cancel();
        let joined = tokio::time::timeout(grace, async {
            for task in &mut self.tasks {
                let _ = task.await;
            }
        })
        .await;
        if joined.is_ok() {
            return JobsDrain::Completed;
        }
        let interrupted = self.tasks.iter().filter(|task| !task.is_finished()).count();
        for task in &self.tasks {
            task.abort();
        }
        JobsDrain::GraceElapsed { interrupted }
    }
}

impl Drop for JobRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn idle(cancel: &CancellationToken, period: Duration) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = tokio::time::sleep(period) => {}
    }
}

async fn work(
    dispatcher: Dispatcher,
    claimant: Claimant,
    cancel: CancellationToken,
    poll: Duration,
) {
    while !cancel.is_cancelled() {
        match dispatcher
            .run_next(&claimant, || !cancel.is_cancelled())
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => idle(&cancel, poll).await,
            Err(error) => {
                tracing::warn!(
                    worker = %claimant,
                    error_kind = error.kind(),
                    "job worker iteration failed"
                );
                idle(&cancel, poll).await;
            }
        }
    }
}

async fn sweep(dispatcher: Dispatcher, cancel: CancellationToken, period: Duration) {
    let mut ticks = interval_at(Instant::now() + period, period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticks.tick() => {}
        }
        match sweep_expired_leases(&dispatcher.0.pools, dispatcher.0.clock.as_ref()).await {
            Ok(0) => {}
            Ok(reclaimed) => tracing::info!(reclaimed, "expired job leases returned to pending"),
            Err(error) => tracing::warn!(error_kind = error.kind(), "job lease sweep failed"),
        }
    }
}
