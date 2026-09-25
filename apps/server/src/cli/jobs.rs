use std::sync::Arc;

use super::error::CliError;
use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::features::audit;
use crate::features::email::{self, EmailService, SmtpTransport};
use crate::features::settings::SettingsService;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::DbPools;
use crate::infra::jobs::cli::{run_once, RunOnceReport};
use crate::infra::jobs::prune_tokens;
use crate::infra::jobs::{
    Claimant, Dispatcher, Jitter, JobAudit, JobKind, Registry, RuntimeTiming,
};
use crate::storage;
use crate::storage::lifecycle::{self as storage_lifecycle, thumbnails::ThumbnailCache};

pub async fn run_once_command(
    config: &OperatorConfig,
    kind: JobKind,
    clock: Arc<dyn Clock>,
) -> Result<RunOnceReport, CliError> {
    let access = DataAccess::claim(config, Access::Exclusive, clock.as_ref())?;
    let outcome = execute(config, &access, kind, clock).await;
    access.release();
    outcome
}

async fn execute(
    config: &OperatorConfig,
    access: &DataAccess,
    kind: JobKind,
    clock: Arc<dyn Clock>,
) -> Result<RunOnceReport, CliError> {
    access.existing_database()?;
    let pools = DbPools::open(
        access.root(),
        config.db_read_connections,
        config.db_synchronous,
    )
    .await
    .map_err(StartupError::from)?;
    let (audit_service, mut audit_drain) = audit::channel(
        audit::AUDIT_CHANNEL_CAPACITY,
        pools.clone(),
        Arc::clone(&clock),
    );
    let (instance_key, _) =
        InstanceKey::load_or_create(access.root()).map_err(StartupError::from)?;
    let settings = SettingsService::load(&pools, Arc::clone(&clock), &instance_key)
        .await
        .map_err(StartupError::from)?;
    let registry = audit::register_jobs(
        Registry::production(),
        pools.clone(),
        Arc::clone(&clock),
        settings.handle(),
    );
    let email = EmailService::new(
        pools.clone(),
        Arc::clone(&clock),
        settings.keys(),
        settings.handle(),
        config.base_url.clone(),
        Arc::new(SmtpTransport),
    );
    let registry = email::register_jobs(registry, email);
    let registry = prune_tokens::register_jobs(registry, pools.clone(), Arc::clone(&clock));
    let registry = if matches!(
        kind,
        JobKind::StorageDeleteBlob | JobKind::StorageOrphanSweep
    ) {
        let provider =
            storage::build_provider(config, Arc::clone(&clock)).map_err(StartupError::from)?;
        storage_lifecycle::register_jobs(
            registry,
            storage_lifecycle::LifecycleContext::new(
                pools.clone(),
                Arc::clone(&clock),
                provider,
                ThumbnailCache::under(access.root()),
                audit_service.clone(),
                config.storage_orphan_reap,
            ),
        )
    } else {
        registry
    };
    let dispatcher = Dispatcher::new(
        pools.clone(),
        Arc::clone(&clock),
        registry,
        Jitter::os(),
        JobAudit::new(Arc::new(audit_service)),
        RuntimeTiming::DEFAULT.lease_renewal,
    );
    let claimant = Claimant::worker(access.instance_id(), 0);
    let report = run_once(&dispatcher, &claimant, kind).await;
    loop {
        match audit_drain.flush_batch(audit::AUDIT_BATCH_MAX).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = pools.shutdown().await;
    report.map_err(|source| CliError::Jobs { source })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tempfile::TempDir;
    use time::macros::datetime;
    use time::OffsetDateTime;

    use super::run_once_command;
    use crate::cli::migrate::migrate;
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::clock::TestClock;
    use crate::domain::id::Id;
    use crate::domain::time::Timestamp;
    use crate::infra::crypto::hash::sha256_hex;
    use crate::infra::db::{DbError, DbPools};
    use crate::infra::http::idempotency::{IdempotencyRecord, REPLAY_WINDOW};
    use crate::infra::jobs::claim::MAX_ROWS_PER_TX;
    use crate::infra::jobs::prune_tokens::{
        ensure_scheduled, prune_step, PruneStep, StepReport, TOKENS_PRUNE_PERIOD,
    };
    use crate::infra::jobs::JobKind;

    const START: OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
    const LEASE: Duration = Duration::from_secs(30);
    const BULK: usize = 2 * MAX_ROWS_PER_TX as usize + 345;

    #[derive(Clone, Copy)]
    enum State {
        Completed,
        InProgress,
    }

    struct Seed {
        label: &'static str,
        created: OffsetDateTime,
        count: usize,
        state: State,
    }

    const fn seed(label: &'static str, created: OffsetDateTime, count: usize) -> Seed {
        Seed {
            label,
            created,
            count,
            state: State::Completed,
        }
    }

    fn stamp(at: OffsetDateTime) -> String {
        Timestamp::try_from(at).unwrap().to_string()
    }

    async fn open(config: &OperatorConfig) -> DbPools {
        DbPools::open(
            &config.data_dir,
            config.db_read_connections,
            config.db_synchronous,
        )
        .await
        .unwrap()
    }

    async fn insert(pools: &DbPools, clock: &TestClock, seeds: &[Seed]) {
        pools
            .write_tx(clock, "test.seed_idempotency", async |tx| {
                for seed in seeds {
                    for index in 0..seed.count {
                        let digest = sha256_hex(format!("{}-{index}", seed.label).as_bytes());
                        let (state, lease, status, json, completed) = match seed.state {
                            State::Completed => (
                                "completed",
                                None,
                                Some(201_i64),
                                Some(r#"{"body":{},"headers":{}}"#),
                                Some(stamp(seed.created)),
                            ),
                            State::InProgress => (
                                "in_progress",
                                Some(stamp(seed.created + LEASE)),
                                None,
                                None,
                                None,
                            ),
                        };
                        sqlx::query(
                            "INSERT INTO idempotency_records
                                 (id, scope_kind, scope_id, http_method, route_template, key_hash,
                                  request_hash, state, lease_expires_at, response_status,
                                  response_json, created_at, completed_at, expires_at)
                             VALUES (?1, 'user', ?2, 'POST', '/api/v1/test/prune', ?3, ?3,
                                     ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        )
                        .bind(Id::<IdempotencyRecord>::generate(clock).to_string())
                        .bind(seed.label)
                        .bind(digest.as_str())
                        .bind(state)
                        .bind(lease)
                        .bind(status)
                        .bind(json)
                        .bind(stamp(seed.created))
                        .bind(completed)
                        .bind(stamp(seed.created + REPLAY_WINDOW))
                        .execute(tx.executor())
                        .await
                        .map_err(DbError::from)?;
                    }
                }
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
    }

    async fn remaining(pools: &DbPools) -> Vec<(String, i64)> {
        sqlx::query_as(
            "SELECT scope_id, COUNT(*) FROM idempotency_records
              GROUP BY scope_id ORDER BY scope_id",
        )
        .fetch_all(pools.reader().executor())
        .await
        .unwrap()
    }

    fn counts(expected: &[(&str, i64)]) -> Vec<(String, i64)> {
        expected
            .iter()
            .map(|(label, count)| ((*label).to_owned(), *count))
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn it_idempotency_records_pruned_after_window() {
        let temp = TempDir::new().unwrap();
        let data = temp.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_DATA_DIR",
            data.to_str().unwrap(),
        )]))
        .unwrap()
        .config;
        let clock = TestClock::new(START);
        migrate(&config, &clock).await.unwrap();

        let pools = open(&config).await;
        let day = REPLAY_WINDOW;
        insert(
            &pools,
            &clock,
            &[
                seed("a-expired", START - day - Duration::from_secs(3600), BULK),
                seed("b-boundary", START - day, 1),
                Seed {
                    state: State::InProgress,
                    ..seed("c-abandoned", START - day - Duration::from_secs(60), 1)
                },
                seed("d-edge", START - day + Duration::from_millis(1), 1),
                seed("e-fresh", START - Duration::from_secs(3600), 1),
                Seed {
                    state: State::InProgress,
                    ..seed("f-running", START, 1)
                },
            ],
        )
        .await;
        ensure_scheduled(&pools, &clock).await.unwrap();
        let _ = pools.shutdown().await;

        let first = run_once_command(&config, JobKind::TokensPrune, Arc::new(clock.clone()))
            .await
            .unwrap();
        assert_eq!(first.executed, 1);
        let pools = open(&config).await;
        let survivors = counts(&[("d-edge", 1), ("e-fresh", 1), ("f-running", 1)]);
        assert_eq!(remaining(&pools).await, survivors);
        let successor: Vec<String> = sqlx::query_scalar(
            "SELECT run_at FROM jobs WHERE kind = 'tokens.prune' AND state = 'pending'",
        )
        .fetch_all(pools.reader().executor())
        .await
        .unwrap();
        assert_eq!(successor, [stamp(START + TOKENS_PRUNE_PERIOD)]);
        let _ = pools.shutdown().await;

        let rerun = run_once_command(&config, JobKind::TokensPrune, Arc::new(clock.clone()))
            .await
            .unwrap();
        assert_eq!(rerun.executed, 0);
        let pools = open(&config).await;
        assert_eq!(remaining(&pools).await, survivors);

        insert(
            &pools,
            &clock,
            &[seed(
                "a-expired",
                START - day - Duration::from_secs(1),
                BULK,
            )],
        )
        .await;
        let batched = prune_step(&pools, &clock, PruneStep::IdempotencyRecords)
            .await
            .unwrap();
        assert_eq!(
            batched,
            StepReport {
                deleted: BULK as u64,
                transactions: 3,
                largest_batch: u64::from(MAX_ROWS_PER_TX),
            }
        );
        assert_eq!(remaining(&pools).await, survivors);
        let idle = prune_step(&pools, &clock, PruneStep::IdempotencyRecords)
            .await
            .unwrap();
        assert_eq!(
            idle,
            StepReport {
                deleted: 0,
                transactions: 1,
                largest_batch: 0,
            }
        );
        assert_eq!(remaining(&pools).await, survivors);

        clock.advance(Duration::from_millis(1));
        let edge = prune_step(&pools, &clock, PruneStep::IdempotencyRecords)
            .await
            .unwrap();
        assert_eq!(edge.deleted, 1);
        assert_eq!(
            remaining(&pools).await,
            counts(&[("e-fresh", 1), ("f-running", 1)])
        );

        clock.set(START + day - Duration::from_secs(3600));
        prune_step(&pools, &clock, PruneStep::IdempotencyRecords)
            .await
            .unwrap();
        assert_eq!(remaining(&pools).await, counts(&[("f-running", 1)]));
        clock.set(START + day);
        prune_step(&pools, &clock, PruneStep::IdempotencyRecords)
            .await
            .unwrap();
        assert!(remaining(&pools).await.is_empty());
        let _ = pools.shutdown().await;
    }
}
