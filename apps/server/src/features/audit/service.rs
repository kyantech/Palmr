use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio_util::sync::CancellationToken;

use super::actions;
use super::error::AuditError;
use super::model::{
    Actor, AuditAction, AuditCode, AuditEvent, Outcome, Target, TargetType, WritePath,
};
use super::repo;
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::jobs::claim::MAX_ROWS_PER_TX;
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{
    BackgroundDrain, ClaimedJob, Idempotency, JobAuditEvent, JobAuditSink, JobKind, JobPayload,
    Registry,
};

pub const AUDIT_CHANNEL_CAPACITY: usize = 1024;
pub const AUDIT_BATCH_MAX: usize = 512;
pub const AUDIT_RETENTION_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone)]
pub struct AuditService {
    sender: mpsc::Sender<AuditEvent>,
    dropped: Arc<AtomicU64>,
    clock: Arc<dyn Clock>,
}

pub struct AuditDrain {
    receiver: mpsc::Receiver<AuditEvent>,
    pools: DbPools,
    clock: Arc<dyn Clock>,
}

pub fn channel(
    capacity: usize,
    pools: DbPools,
    clock: Arc<dyn Clock>,
) -> (AuditService, AuditDrain) {
    let (sender, receiver) = mpsc::channel(capacity);
    (
        AuditService {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
            clock: Arc::clone(&clock),
        },
        AuditDrain {
            receiver,
            pools,
            clock,
        },
    )
}

impl AuditService {
    pub async fn record_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        event: &AuditEvent,
    ) -> Result<(), AuditError> {
        let path = event.action().write_path();
        if path != WritePath::InTransaction {
            return Err(AuditError::WrongWritePath {
                action: event.action(),
                expected: path,
            });
        }
        repo::insert(tx, self.clock.as_ref(), event).await
    }

    pub fn record_async(&self, event: AuditEvent) {
        if event.action().write_path() != WritePath::Enqueued {
            self.note_drop(&event);
            return;
        }
        match self.sender.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event) | TrySendError::Closed(event)) => {
                self.note_drop(&event);
            }
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn note_drop(&self, event: &AuditEvent) {
        let dropped_total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            action = event.action().as_str(),
            dropped_total,
            "audit event dropped instead of blocking the caller"
        );
    }
}

impl JobAuditSink for AuditService {
    fn record(&self, event: JobAuditEvent) {
        let JobAuditEvent::DeadLettered {
            job_id,
            kind,
            attempts,
            failure,
        } = event;
        let Ok(occurred_at) = Timestamp::try_from(self.clock.now()) else {
            tracing::warn!(
                action = AuditAction::JobDeadLettered.as_str(),
                "audit event dropped because the server clock is outside the supported range"
            );
            return;
        };
        let spec = actions::job_dead_lettered(kind, attempts, kind.policy().max_attempts, failure);
        let target = Target::new(TargetType::Job)
            .id(&job_id.to_string())
            .label(kind.as_str());
        let audit = AuditEvent::new(
            spec,
            Actor::system(),
            Outcome::Failure(AuditCode::from(failure)),
            occurred_at,
        )
        .with_target(target);
        self.record_async(audit);
    }
}

impl AuditDrain {
    async fn insert_events(&self, events: &[AuditEvent]) -> Result<(), AuditError> {
        if events.is_empty() {
            return Ok(());
        }
        self.pools
            .write_tx(self.clock.as_ref(), "audit.record_batch", async |tx| {
                for event in events {
                    repo::insert(tx, self.clock.as_ref(), event).await?;
                }
                Ok::<(), AuditError>(())
            })
            .await
    }

    pub async fn flush_batch(&mut self, max: usize) -> Result<usize, AuditError> {
        let mut buffer = Vec::with_capacity(max.min(AUDIT_BATCH_MAX));
        while buffer.len() < max {
            match self.receiver.try_recv() {
                Ok(event) => buffer.push(event),
                Err(_) => break,
            }
        }
        let flushed = buffer.len();
        if flushed > 0 {
            self.insert_events(&buffer).await?;
        }
        Ok(flushed)
    }

    pub async fn run(mut self, cancel: CancellationToken) {
        let mut buffer = Vec::with_capacity(AUDIT_BATCH_MAX);
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    loop {
                        match self.flush_batch(AUDIT_BATCH_MAX).await {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error_kind = error.kind(),
                                    "audit channel could not be drained during shutdown"
                                );
                                break;
                            }
                        }
                    }
                    return;
                }
                received = self.receiver.recv_many(&mut buffer, AUDIT_BATCH_MAX) => {
                    if received == 0 {
                        return;
                    }
                    if let Err(error) = self.insert_events(&buffer).await {
                        tracing::warn!(
                            error_kind = error.kind(),
                            "an audit batch could not be persisted and is lost"
                        );
                    }
                    buffer.clear();
                }
            }
        }
    }

    pub fn into_background(self) -> BackgroundDrain {
        BackgroundDrain::new("audit", move |cancel| Box::pin(self.run(cancel)))
    }
}

pub async fn sweep_expired(
    pools: &DbPools,
    clock: &dyn Clock,
    retention_days: u32,
) -> Result<u64, AuditError> {
    let cutoff = Timestamp::try_from(
        clock.now() - Duration::from_secs(u64::from(retention_days) * 24 * 60 * 60),
    )?;
    let mut deleted = 0_u64;
    loop {
        let batch = repo::delete_expired_batch(pools, clock, cutoff).await?;
        deleted = deleted.saturating_add(batch);
        if batch < u64::from(MAX_ROWS_PER_TX) {
            return Ok(deleted);
        }
    }
}

#[derive(Clone)]
struct RetentionContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
}

pub fn register_jobs(registry: Registry, pools: DbPools, clock: Arc<dyn Clock>) -> Registry {
    let context = RetentionContext { pools, clock };
    registry.register(
        JobKind::AuditRetentionSweep,
        Idempotency::key("audit.retention_sweep date bucket"),
        move |job: ClaimedJob| {
            let context = context.clone();
            async move { retention_sweep(job, context).await }
        },
    )
}

async fn retention_sweep(_job: ClaimedJob, context: RetentionContext) -> anyhow::Result<()> {
    let retention_days = repo::retention_days(&context.pools).await?;
    let deleted = sweep_expired(&context.pools, context.clock.as_ref(), retention_days).await?;
    tracing::info!(deleted, retention_days, "audit retention sweep finished");
    let recurring = Recurring::new(JobKind::AuditRetentionSweep, AUDIT_RETENTION_PERIOD)
        .map_err(|_| AuditError::InvalidSchedule)?;
    let payload = JobPayload::empty();
    context
        .pools
        .write_tx(
            context.clock.as_ref(),
            "audit.retention_successor",
            async |tx| {
                recurring
                    .schedule_successor(tx, context.clock.as_ref(), &payload)
                    .await
            },
        )
        .await?;
    Ok(())
}

pub async fn ensure_retention_scheduled(
    pools: &DbPools,
    clock: &dyn Clock,
) -> Result<(), AuditError> {
    let recurring = Recurring::new(JobKind::AuditRetentionSweep, AUDIT_RETENTION_PERIOD)
        .map_err(|_| AuditError::InvalidSchedule)?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, "audit.schedule_retention", async |tx| {
            recurring.schedule(tx, clock, &payload).await
        })
        .await
        .map(drop)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;
    use time::macros::datetime;

    use super::*;
    use crate::config::SqliteSynchronous;
    use crate::domain::clock::{Clock, TestClock};
    use crate::domain::id::Id;
    use crate::infra::db::{DbPools, InstanceId, MIGRATOR};
    use crate::infra::jobs::backoff::Jitter;
    use crate::infra::jobs::claim::enqueue;
    use crate::infra::jobs::cli::run_once;
    use crate::infra::jobs::runtime::{Dispatcher, RuntimeTiming};
    use crate::infra::jobs::{Claimant, JobAudit, JobId, NewJob};
    use crate::infra::jobs::{FailureClass, Registry};

    use super::super::actions::{Presence, SettingKey};

    const START: time::OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
    const SENTINEL: &str = "palmr-audit-sentinel-9f2c";
    const ACTOR_ID: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d";

    type AuditRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );

    struct Harness {
        _root: TempDir,
        pools: DbPools,
        clock: TestClock,
    }

    impl Harness {
        async fn open() -> Self {
            let root = TempDir::new().unwrap();
            let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
                .await
                .unwrap();
            pools.migrate(&MIGRATOR).await.unwrap();
            Self {
                _root: root,
                pools,
                clock: TestClock::new(START),
            }
        }

        fn shared_clock(&self) -> Arc<dyn Clock> {
            Arc::new(self.clock.clone())
        }

        fn dispatcher(&self, registry: Registry) -> Dispatcher {
            Dispatcher::new(
                self.pools.clone(),
                self.shared_clock(),
                registry,
                Jitter::from_fn(|| 0),
                JobAudit::detached(),
                RuntimeTiming::DEFAULT.lease_renewal,
            )
        }

        async fn audit_count(&self) -> i64 {
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
                .fetch_one(self.pools.reader().executor())
                .await
                .unwrap()
        }

        async fn job_count(&self, predicate: &str) -> i64 {
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM jobs WHERE {predicate}"))
                .fetch_one(self.pools.reader().executor())
                .await
                .unwrap()
        }

        async fn seed_audit(&self, occurred_at: Timestamp, count: usize) {
            self.pools
                .write_tx(&self.clock, "test.seed_audit", async |tx| {
                    for _ in 0..count {
                        sqlx::query(
                            "INSERT INTO audit_events (id, occurred_at, action, actor_type, result)
                             VALUES (?1, ?2, 'JOB_DEAD_LETTERED', 'system', 'success')",
                        )
                        .bind(Id::<AuditEvent>::generate(&self.clock).to_string())
                        .bind(occurred_at.to_string())
                        .execute(tx.executor())
                        .await?;
                    }
                    Ok::<(), AuditError>(())
                })
                .await
                .unwrap();
        }

        async fn seed_user(&self, id: &str) {
            self.pools
                .write_tx(&self.clock, "test.seed_user", async |tx| {
                    sqlx::query(
                        "INSERT INTO users (id, email, email_normalized, username,
                                            username_normalized, created_at, updated_at)
                         VALUES (?1, 'ada@example.test', 'ada@example.test', 'ada', 'ada', ?2, ?2)",
                    )
                    .bind(id)
                    .bind(Timestamp::try_from(self.clock.now()).unwrap().to_string())
                    .execute(tx.executor())
                    .await?;
                    Ok::<(), AuditError>(())
                })
                .await
                .unwrap();
        }

        async fn seed_retention_setting(&self, days: i64) {
            sqlx::query(
                "INSERT INTO app_settings (key, group_name, value_type, value_json, is_secret, updated_at)
                 VALUES ('audit_retention_days', 'audit', 'integer', ?1, 0, ?2)",
            )
            .bind(days.to_string())
            .bind(Timestamp::try_from(self.clock.now()).unwrap().to_string())
            .execute(self.pools.reader().executor())
            .await
            .unwrap();
        }
    }

    fn claimant(clock: &TestClock, worker: u16) -> Claimant {
        Claimant::worker(InstanceId::generate(clock), worker)
    }

    fn setting_event(clock: &TestClock) -> AuditEvent {
        let spec = actions::setting_changed(
            &SettingKey::new(SettingKey::SMTP_PASSWORD).unwrap(),
            Presence::Set,
            Presence::Set,
        );
        AuditEvent::new(
            spec,
            Actor::user(ACTOR_ID, "ada@example.test"),
            Outcome::Success,
            Timestamp::try_from(clock.now()).unwrap(),
        )
        .with_target(Target::new(TargetType::Setting).id("smtp_password"))
    }

    fn dead_letter_event(clock: &TestClock, job_id: &str) -> AuditEvent {
        let spec =
            actions::job_dead_lettered(JobKind::TokensPrune, 3, 3, FailureClass::HandlerFailed);
        AuditEvent::new(
            spec,
            Actor::system(),
            Outcome::Failure(AuditCode::HandlerFailed),
            Timestamp::try_from(clock.now()).unwrap(),
        )
        .with_target(Target::new(TargetType::Job).id(job_id))
    }

    #[tokio::test]
    async fn it_audit_in_tx_rolls_back_with_change() {
        let harness = Harness::open().await;
        harness.seed_user(ACTOR_ID).await;
        let (service, _drain) = channel(
            AUDIT_CHANNEL_CAPACITY,
            harness.pools.clone(),
            harness.shared_clock(),
        );
        let event = setting_event(&harness.clock);
        let rolled_back_event = event.clone();

        let rolled_back = harness
            .pools
            .write_tx(&harness.clock, "test.audit_rollback", async |tx| {
                enqueue(
                    tx,
                    &harness.clock,
                    &NewJob::new(JobKind::TokensPrune, JobPayload::empty()),
                )
                .await?;
                service.record_in_tx(tx, &rolled_back_event).await?;
                Err::<(), AuditError>(AuditError::InvalidSchedule)
            })
            .await;
        assert!(matches!(rolled_back, Err(AuditError::InvalidSchedule)));
        assert_eq!(harness.job_count("1 = 1").await, 0);
        assert_eq!(harness.audit_count().await, 0);

        harness
            .pools
            .write_tx(&harness.clock, "test.audit_commit", async |tx| {
                enqueue(
                    tx,
                    &harness.clock,
                    &NewJob::new(JobKind::TokensPrune, JobPayload::empty()),
                )
                .await?;
                service.record_in_tx(tx, &event).await?;
                Ok::<(), AuditError>(())
            })
            .await
            .unwrap();
        assert_eq!(harness.job_count("1 = 1").await, 1);
        assert_eq!(harness.audit_count().await, 1);
    }

    #[tokio::test]
    async fn it_audit_async_drops_when_full_without_blocking() {
        let harness = Harness::open().await;
        let (service, mut drain) = channel(1, harness.pools.clone(), harness.shared_clock());

        service.record_async(dead_letter_event(&harness.clock, "job-0001"));
        service.record_async(dead_letter_event(&harness.clock, "job-0002"));
        assert_eq!(service.dropped(), 1);
        assert_eq!(harness.audit_count().await, 0);

        assert_eq!(drain.flush_batch(AUDIT_BATCH_MAX).await.unwrap(), 1);
        assert_eq!(harness.audit_count().await, 1);
        let target: Option<String> =
            sqlx::query_scalar("SELECT target_id FROM audit_events ORDER BY occurred_at LIMIT 1")
                .fetch_one(harness.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(target.as_deref(), Some("job-0001"));
        assert_eq!(drain.flush_batch(AUDIT_BATCH_MAX).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn it_audit_rows_contain_no_secrets() {
        let harness = Harness::open().await;
        harness.seed_user(ACTOR_ID).await;
        let (service, mut drain) = channel(
            AUDIT_CHANNEL_CAPACITY,
            harness.pools.clone(),
            harness.shared_clock(),
        );
        let secret_value = format!("{SENTINEL}-smtp-password");

        let event = setting_event(&harness.clock);
        harness
            .pools
            .write_tx(&harness.clock, "test.audit_setting", async |tx| {
                service.record_in_tx(tx, &event).await
            })
            .await
            .unwrap();

        let job_id = JobId::generate(&harness.clock);
        harness
            .pools
            .write_tx(&harness.clock, "test.seed_secret_job", async |tx| {
                sqlx::query(
                    "INSERT INTO jobs (id, kind, payload_json, state, priority, run_at, attempts,
                                       max_attempts, created_at, updated_at)
                     VALUES (?1, 'tokens.prune', ?2, 'dead', 200, ?3, 3, 3, ?3, ?3)",
                )
                .bind(job_id.to_string())
                .bind(format!("{{\"marker\":\"{SENTINEL}\"}}"))
                .bind(
                    Timestamp::try_from(harness.clock.now())
                        .unwrap()
                        .to_string(),
                )
                .execute(tx.executor())
                .await?;
                Ok::<(), AuditError>(())
            })
            .await
            .unwrap();

        let sink: Arc<dyn JobAuditSink> = Arc::new(service.clone());
        sink.record(JobAuditEvent::DeadLettered {
            job_id,
            kind: JobKind::TokensPrune,
            attempts: 3,
            failure: FailureClass::HandlerFailed,
        });
        let mut flushed = 0;
        loop {
            let batch = drain.flush_batch(AUDIT_BATCH_MAX).await.unwrap();
            if batch == 0 {
                break;
            }
            flushed += batch;
        }
        assert_eq!(flushed, 1);

        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT action, actor_label, target_label, error_code, metadata_json
                   FROM audit_events ORDER BY occurred_at, id",
        )
        .fetch_all(harness.pools.reader().executor())
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        for (action, actor_label, target_label, error_code, metadata_json) in &rows {
            for column in [
                Some(action.as_str()),
                actor_label.as_deref(),
                target_label.as_deref(),
                error_code.as_deref(),
                Some(metadata_json.as_str()),
            ]
            .into_iter()
            .flatten()
            {
                assert!(!column.contains(SENTINEL), "{column}");
                assert!(!column.contains(&secret_value), "{column}");
            }
        }

        for action in AuditAction::ALL {
            assert!(action
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'));
        }

        let long_key = SettingKey::new("k".repeat(64)).unwrap();
        let widest_setting = actions::setting_changed(&long_key, Presence::Unset, Presence::Set);
        let widest_job = actions::job_dead_lettered(
            JobKind::ReverseShareDeleteCascade,
            u32::MAX,
            u32::MAX,
            FailureClass::NoHandler,
        );
        for spec in [&widest_setting, &widest_job] {
            assert!(spec.metadata().bytes() <= actions::MAX_METADATA_BYTES);
            assert!(serde_json::from_str::<serde_json::Value>(spec.metadata().as_str()).is_ok());
        }

        let oversized = format!("{{\"k\":\"{}\"}}", "a".repeat(actions::MAX_METADATA_BYTES));
        let rejected = sqlx::query(
            "INSERT INTO audit_events (id, occurred_at, action, actor_type, result, metadata_json)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d', ?1, 'SETTING_CHANGED', 'system',
                     'success', ?2)",
        )
        .bind(
            Timestamp::try_from(harness.clock.now())
                .unwrap()
                .to_string(),
        )
        .bind(&oversized)
        .execute(harness.pools.reader().executor())
        .await;
        assert!(rejected.is_err());
    }

    #[tokio::test]
    async fn it_audit_retention_sweep_batches() {
        let harness = Harness::open().await;
        harness.seed_retention_setting(7).await;
        let expired_at =
            Timestamp::try_from(harness.clock.now() - time::Duration::days(30)).unwrap();
        let retained_at =
            Timestamp::try_from(harness.clock.now() - time::Duration::hours(1)).unwrap();
        harness.seed_audit(expired_at, 1_500).await;
        harness.seed_audit(retained_at, 3).await;
        assert_eq!(harness.audit_count().await, 1_503);

        let registry = register_jobs(
            Registry::production(),
            harness.pools.clone(),
            harness.shared_clock(),
        );
        let dispatcher = harness.dispatcher(registry);
        let worker = claimant(&harness.clock, 0);
        ensure_retention_scheduled(&harness.pools, &harness.clock)
            .await
            .unwrap();
        assert_eq!(harness.job_count("kind = 'audit.retention_sweep'").await, 1);

        let report = run_once(&dispatcher, &worker, JobKind::AuditRetentionSweep)
            .await
            .unwrap();
        assert_eq!(report.executed, 1);
        assert_eq!(harness.audit_count().await, 3);

        let rerun = run_once(&dispatcher, &worker, JobKind::AuditRetentionSweep)
            .await
            .unwrap();
        assert_eq!(rerun.executed, 0);
        assert_eq!(harness.audit_count().await, 3);

        harness.seed_audit(expired_at, 1_500).await;
        let cutoff = Timestamp::try_from(harness.clock.now() - time::Duration::days(7)).unwrap();
        assert_eq!(
            repo::delete_expired_batch(&harness.pools, &harness.clock, cutoff)
                .await
                .unwrap(),
            1_000
        );
        assert_eq!(
            repo::delete_expired_batch(&harness.pools, &harness.clock, cutoff)
                .await
                .unwrap(),
            500
        );
        assert_eq!(
            repo::delete_expired_batch(&harness.pools, &harness.clock, cutoff)
                .await
                .unwrap(),
            0
        );
        assert_eq!(harness.audit_count().await, 3);

        harness.clock.advance(Duration::from_secs(86_400));
        let next = run_once(&dispatcher, &worker, JobKind::AuditRetentionSweep)
            .await
            .unwrap();
        assert_eq!(next.executed, 1);
        assert_eq!(harness.audit_count().await, 3);
    }
}
