use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rand::Rng;
use serde_json::{json, Map, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use super::backoff::{base_delay, retry_delay, Jitter, RETRY_CAP};
use super::claim::{
    bounded_error, claim, enqueue, prune_succeeded, renew, settle_success, sweep_expired_leases,
    Enqueued, Settled, SUCCEEDED_RETENTION,
};
use super::cli::run_once;
use super::kinds::{JobKind, Priority, DEFAULT_LEASE};
use super::recurring::Recurring;
use super::runtime::{
    Dispatcher, FailureClass, Idempotency, JobAudit, JobAuditEvent, JobAuditSink, JobRuntime,
    JobsDrain, NonRetryable, Outcome, Registry, RuntimeTiming,
};
use super::{
    Claimant, ClaimedJob, DedupKey, JobId, JobPayload, JobsError, NewJob, MAX_LAST_ERROR_BYTES,
};
use crate::config::{LogFormat, SqliteSynchronous};
use crate::domain::clock::{Clock, TestClock};
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, InstanceId, MIGRATOR};
use crate::infra::telemetry::build_dispatch;

const START: time::OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
const PAYLOAD_MARKER: &str = "payload-marker-4f1e";
const ERROR_MARKER: &str = "handler-error-detail-9c2d";

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

    async fn enqueue(&self, job: &NewJob) -> Enqueued {
        self.pools
            .write_tx(&self.clock, "test.enqueue", async |tx| {
                enqueue(tx, &self.clock, job).await
            })
            .await
            .unwrap()
    }

    async fn enqueue_many(&self, kind: JobKind, count: usize) -> Vec<JobId> {
        self.pools
            .write_tx(&self.clock, "test.enqueue_many", async |tx| {
                let mut ids = Vec::with_capacity(count);
                for index in 0..count {
                    let payload = JobPayload::new(&json!({ "index": index })).unwrap();
                    match enqueue(tx, &self.clock, &NewJob::new(kind, payload)).await? {
                        Enqueued::Inserted(id) => ids.push(id),
                        Enqueued::Deduplicated => unreachable!("no dedup key was given"),
                    }
                }
                Ok::<_, JobsError>(ids)
            })
            .await
            .unwrap()
    }

    async fn claim(&self, claimant: &Claimant, kinds: &[JobKind], limit: u32) -> Vec<ClaimedJob> {
        claim(&self.pools, &self.clock, claimant, kinds, limit, || true)
            .await
            .unwrap()
    }

    async fn row(&self, id: JobId) -> JobRow {
        let (
            state,
            attempts,
            run_at,
            claimed_by,
            lease_expires_at,
            last_error,
            created_at,
            updated_at,
        ) = sqlx::query_as::<
            _,
            (
                String,
                i64,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
                String,
            ),
        >(
            "SELECT state, attempts, run_at, claimed_by, lease_expires_at, last_error,
                        created_at, updated_at
                   FROM jobs WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        JobRow {
            state,
            attempts,
            run_at,
            claimed_by,
            lease_expires_at,
            last_error,
            created_at,
            updated_at,
        }
    }

    async fn count(&self, predicate: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM jobs WHERE {predicate}"))
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    fn dispatcher(&self, registry: Registry, jitter: Jitter, audit: JobAudit) -> Dispatcher {
        Dispatcher::new(
            self.pools.clone(),
            self.shared_clock(),
            registry,
            jitter,
            audit,
            RuntimeTiming::DEFAULT.lease_renewal,
        )
    }

    async fn reopen_pools(&mut self) {
        self.pools = DbPools::open(self._root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
    }

    async fn schedule_recurring(&self, recurring: &Recurring, payload: &JobPayload) -> Enqueued {
        self.pools
            .write_tx(&self.clock, "test.schedule_recurring", async |tx| {
                recurring.schedule(tx, &self.clock, payload).await
            })
            .await
            .unwrap()
    }

    async fn schedule_recurring_successor(
        &self,
        recurring: &Recurring,
        payload: &JobPayload,
    ) -> Enqueued {
        self.pools
            .write_tx(
                &self.clock,
                "test.schedule_recurring_successor",
                async |tx| recurring.schedule_successor(tx, &self.clock, payload).await,
            )
            .await
            .unwrap()
    }
}

#[derive(Debug)]
struct JobRow {
    state: String,
    attempts: i64,
    run_at: String,
    claimed_by: Option<String>,
    lease_expires_at: Option<String>,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
}

fn at(clock: &TestClock, offset: Duration) -> String {
    Timestamp::try_from(clock.now() + offset)
        .unwrap()
        .to_string()
}

fn claimant(clock: &TestClock, worker: u16) -> Claimant {
    Claimant::worker(InstanceId::generate(clock), worker)
}

fn idempotency() -> Idempotency {
    Idempotency::key("fixture job id")
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap()
    }

    fn json_lines(&self) -> Vec<Map<String, Value>> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_jobs_enqueue_dedup() {
    let harness = Arc::new(Harness::open().await);
    let key = DedupKey::new("storage.delete_blob:object-0001").unwrap();
    let job = NewJob::new(
        JobKind::StorageDeleteBlob,
        JobPayload::new(&json!({ "storage_object_id": "object-0001" })).unwrap(),
    )
    .dedup_key(key.clone());

    let mut racers = JoinSet::new();
    for _ in 0..16 {
        let harness = Arc::clone(&harness);
        let job = job.clone();
        racers.spawn(async move { harness.enqueue(&job).await });
    }
    let outcomes = racers.join_all().await;
    let inserted: Vec<JobId> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Enqueued::Inserted(id) => Some(*id),
            Enqueued::Deduplicated => None,
        })
        .collect();
    assert_eq!(inserted.len(), 1, "{outcomes:?}");
    assert_eq!(harness.enqueue(&job).await, Enqueued::Deduplicated);
    assert_eq!(
        harness
            .count("dedup_key = 'storage.delete_blob:object-0001'")
            .await,
        1
    );

    let stored = harness.row(inserted[0]).await;
    assert_eq!(stored.state, "pending");
    assert_eq!(stored.attempts, 0);
    assert_eq!(stored.run_at, at(&harness.clock, Duration::ZERO));
    assert_eq!(stored.claimed_by, None);
    let (kind, priority, max_attempts): (String, i64, i64) =
        sqlx::query_as("SELECT kind, priority, max_attempts FROM jobs WHERE id = ?")
            .bind(inserted[0].to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(kind, "storage.delete_blob");
    let policy = JobKind::StorageDeleteBlob.policy();
    assert_eq!(priority, i64::from(policy.priority.get()));
    assert_eq!(max_attempts, i64::from(policy.max_attempts));

    let in_one_tx = harness
        .pools
        .write_tx(&harness.clock, "test.enqueue_twice", async |tx| {
            let fresh = job
                .clone()
                .dedup_key(DedupKey::new("tokens.prune:2026-09-24").unwrap());
            let first = enqueue(tx, &harness.clock, &fresh).await?;
            let second = enqueue(tx, &harness.clock, &fresh).await?;
            Ok::<_, JobsError>((first, second))
        })
        .await
        .unwrap();
    assert!(matches!(in_one_tx.0, Enqueued::Inserted(_)));
    assert_eq!(in_one_tx.1, Enqueued::Deduplicated);

    let rolled_back = harness
        .pools
        .write_tx(&harness.clock, "test.enqueue_rolled_back", async |tx| {
            let doomed = job.clone().dedup_key(DedupKey::new("rolled-back").unwrap());
            enqueue(tx, &harness.clock, &doomed).await?;
            Err::<(), _>(JobsError::CorruptRow("fixture"))
        })
        .await;
    assert!(matches!(rolled_back, Err(JobsError::CorruptRow("fixture"))));
    assert_eq!(harness.count("dedup_key = 'rolled-back'").await, 0);

    let unkeyed = NewJob::new(JobKind::TokensPrune, JobPayload::empty());
    let mut unkeyed_ids = HashSet::new();
    for _ in 0..3 {
        match harness.enqueue(&unkeyed).await {
            Enqueued::Inserted(id) => assert!(unkeyed_ids.insert(id)),
            Enqueued::Deduplicated => panic!("a null dedup key never collapses"),
        }
    }
    assert_eq!(harness.count("dedup_key IS NULL").await, 3);

    assert!(DedupKey::new("").is_err());
    assert!(DedupKey::new("k".repeat(257)).is_err());
    assert!(DedupKey::new("k".repeat(256)).is_ok());
    assert!(JobPayload::new(&json!(["not", "an", "object"])).is_err());
    assert!(JobPayload::new(&json!({ "blob": "x".repeat(16 * 1024) })).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_jobs_claim_single_writer() {
    let harness = Arc::new(Harness::open().await);
    let kinds = [JobKind::TokensPrune, JobKind::EmailSend];

    let low = harness
        .enqueue(&NewJob::new(JobKind::TokensPrune, JobPayload::empty()).priority(Priority::LOW))
        .await;
    let high = harness
        .enqueue(&NewJob::new(JobKind::EmailSend, JobPayload::empty()))
        .await;
    let first = harness.claim(&claimant(&harness.clock, 0), &kinds, 1).await;
    assert_eq!(first.len(), 1);
    assert_eq!(Enqueued::Inserted(first[0].id()), high);
    let second = harness.claim(&claimant(&harness.clock, 0), &kinds, 1).await;
    assert_eq!(Enqueued::Inserted(second[0].id()), low);

    let future = harness
        .enqueue(
            &NewJob::new(JobKind::TokensPrune, JobPayload::empty())
                .run_at(Timestamp::try_from(START + Duration::from_secs(60)).unwrap()),
        )
        .await;
    let unregistered = harness
        .enqueue(&NewJob::new(JobKind::QuotaReconcile, JobPayload::empty()))
        .await;

    let expected: HashSet<JobId> = harness
        .enqueue_many(JobKind::TokensPrune, 240)
        .await
        .into_iter()
        .collect();

    let mut claimers = JoinSet::new();
    for worker in 0..8 {
        let harness = Arc::clone(&harness);
        claimers.spawn(async move {
            let me = claimant(&harness.clock, worker);
            let mut mine = Vec::new();
            loop {
                let batch = harness.claim(&me, &kinds, 7).await;
                if batch.is_empty() {
                    return (me, mine);
                }
                mine.extend(batch.iter().map(ClaimedJob::id));
            }
        });
    }

    let mut seen = HashSet::new();
    for (me, mine) in claimers.join_all().await {
        for id in mine {
            assert!(seen.insert(id), "job {id} was claimed twice");
            let row = harness.row(id).await;
            assert_eq!(row.state, "claimed");
            assert_eq!(row.claimed_by.as_deref(), Some(me.as_str()));
            assert_eq!(
                row.lease_expires_at,
                Some(at(&harness.clock, DEFAULT_LEASE))
            );
        }
    }
    assert_eq!(seen, expected);

    assert!(harness
        .claim(&claimant(&harness.clock, 99), &kinds, 1_000)
        .await
        .is_empty());
    for untouched in [future, unregistered] {
        let Enqueued::Inserted(id) = untouched else {
            panic!("fixture jobs are unkeyed");
        };
        assert_eq!(harness.row(id).await.state, "pending");
    }
}

#[tokio::test]
async fn it_jobs_lease_reclaim_after_crash() {
    let harness = Harness::open().await;
    let kinds = [JobKind::SessionsPrune];
    let ids = harness.enqueue_many(JobKind::SessionsPrune, 2).await;

    let crashed = claimant(&harness.clock, 0);
    let orphaned = harness.claim(&crashed, &kinds, 1).await.remove(0);
    assert_eq!(orphaned.id(), ids[0]);
    let renewing = harness.claim(&crashed, &kinds, 1).await.remove(0);
    drop(crashed);

    let restarted = claimant(&harness.clock, 0);
    harness
        .clock
        .advance(DEFAULT_LEASE - Duration::from_secs(60));
    assert!(renew(&harness.pools, &harness.clock, &renewing)
        .await
        .unwrap());
    harness.clock.advance(Duration::from_millis(59_999));

    assert_eq!(
        sweep_expired_leases(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        0
    );
    let held = harness.row(orphaned.id()).await;
    assert_eq!(held.state, "claimed");
    assert_eq!(
        held.claimed_by.as_deref(),
        Some(orphaned.claimant().as_str())
    );
    assert!(harness.claim(&restarted, &kinds, 10).await.is_empty());

    harness.clock.advance(Duration::from_millis(1));
    assert_eq!(
        sweep_expired_leases(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        1
    );
    let reclaimed = harness.row(orphaned.id()).await;
    assert_eq!(reclaimed.state, "pending");
    assert_eq!(reclaimed.claimed_by, None);
    assert_eq!(reclaimed.lease_expires_at, None);
    assert_eq!(reclaimed.attempts, 0);
    assert_eq!(harness.row(renewing.id()).await.state, "claimed");

    let rerun = harness.claim(&restarted, &kinds, 10).await;
    assert_eq!(rerun.len(), 1);
    assert_eq!(rerun[0].id(), orphaned.id());

    assert_eq!(
        settle_success(&harness.pools, &harness.clock, &orphaned)
            .await
            .unwrap(),
        Settled::LeaseLost
    );
    assert_eq!(
        settle_success(&harness.pools, &harness.clock, &rerun[0])
            .await
            .unwrap(),
        Settled::Succeeded
    );
    let settled = harness.row(orphaned.id()).await;
    assert_eq!(settled.state, "succeeded");
    assert_eq!(settled.claimed_by, None);
    assert_eq!(settled.lease_expires_at, None);

    harness.clock.advance(DEFAULT_LEASE);
    assert_eq!(
        sweep_expired_leases(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        1
    );
    assert_eq!(harness.row(renewing.id()).await.state, "pending");
    assert!(!renew(&harness.pools, &harness.clock, &renewing)
        .await
        .unwrap());
}

#[tokio::test]
async fn it_jobs_backoff_jitter_bounds() {
    assert_eq!(base_delay(0), Duration::from_secs(30));
    assert_eq!(base_delay(1), Duration::from_secs(60));
    assert_eq!(base_delay(9), Duration::from_secs(15_360));
    assert_eq!(base_delay(10), RETRY_CAP);
    assert_eq!(RETRY_CAP, Duration::from_secs(6 * 60 * 60));

    let mut rng = seeded("it_jobs_backoff_jitter_bounds");
    let attempts = (0..=16).chain([31, 32, 33, 63, 64, 65, 1_000, u32::MAX - 1, u32::MAX]);
    for attempts in attempts {
        let base = base_delay(attempts);
        assert!(base <= RETRY_CAP, "attempts={attempts}");
        let base_ms = base.as_millis();
        let fixed = [0, 1, u32::MAX / 2, u32::MAX - 1, u32::MAX];
        let random: Vec<u32> = (0..256).map(|_| rng.random()).collect();
        for sample in fixed.into_iter().chain(random) {
            let delay = retry_delay(attempts, sample).as_millis();
            assert!(
                delay * 5 >= base_ms * 4 && delay * 5 <= base_ms * 6,
                "attempts={attempts} sample={sample} delay={delay} base={base_ms}"
            );
        }
        assert_eq!(retry_delay(attempts, 0).as_millis() * 5, base_ms * 4);
        assert_eq!(retry_delay(attempts, u32::MAX).as_millis() * 5, base_ms * 6);
    }

    let harness = Harness::open().await;
    let registry =
        Registry::default().register(JobKind::ImageNormalize, idempotency(), |_job| async {
            Err(anyhow::anyhow!("fixture failure"))
        });
    let dispatcher =
        harness.dispatcher(registry, Jitter::from_fn(|| u32::MAX), JobAudit::detached());
    let Enqueued::Inserted(id) = harness
        .enqueue(&NewJob::new(JobKind::ImageNormalize, JobPayload::empty()))
        .await
    else {
        panic!("unkeyed enqueue inserts");
    };
    let worker = claimant(&harness.clock, 0);

    let expected = [36_000, 72_000, 144_000, 288_000];
    for (attempt, delay_ms) in expected.into_iter().enumerate() {
        let outcome = dispatcher.run_next(&worker, || true).await.unwrap();
        let run_at = at(&harness.clock, Duration::from_millis(delay_ms));
        assert_eq!(
            outcome.map(|outcome| match outcome {
                Outcome::Retrying { run_at } => run_at.to_string(),
                other => format!("{other:?}"),
            }),
            Some(run_at.clone())
        );
        let row = harness.row(id).await;
        assert_eq!(row.state, "pending");
        assert_eq!(row.run_at, run_at);
        assert_eq!(row.attempts, i64::try_from(attempt).unwrap() + 1);

        harness.clock.advance(Duration::from_millis(delay_ms - 1));
        assert_eq!(dispatcher.run_next(&worker, || true).await.unwrap(), None);
        harness.clock.advance(Duration::from_millis(1));
    }
}

fn seeded(test_name: &str) -> rand::rngs::StdRng {
    use rand::SeedableRng;
    use sha2::{Digest, Sha256};
    rand::rngs::StdRng::from_seed(Sha256::digest(test_name.as_bytes()).into())
}

#[derive(Default)]
struct CapturedAudit(Mutex<Vec<JobAuditEvent>>);

impl JobAuditSink for CapturedAudit {
    fn record(&self, event: JobAuditEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn it_jobs_non_retryable_dead_letters_once() {
    let harness = Harness::open().await;
    let calls = Arc::new(AtomicU32::new(0));
    let registry = Registry::default().register(JobKind::StorageDeleteBlob, idempotency(), {
        let calls = Arc::clone(&calls);
        move |_job: ClaimedJob| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Err(NonRetryable::new("FIXTURE_CORRUPT_ROW").into()) }
        }
    });
    let captured = Arc::new(CapturedAudit::default());
    let dispatcher = harness.dispatcher(
        registry,
        Jitter::from_fn(|| 0),
        JobAudit::new(captured.clone()),
    );
    let Enqueued::Inserted(id) = harness
        .enqueue(&NewJob::new(
            JobKind::StorageDeleteBlob,
            JobPayload::empty(),
        ))
        .await
    else {
        panic!("unkeyed enqueue inserts");
    };
    let worker = claimant(&harness.clock, 0);

    let dead = dispatcher.run_next(&worker, || true).await.unwrap();
    assert_eq!(dead, Some(Outcome::DeadLettered { attempts: 1 }));
    harness.clock.advance(RETRY_CAP * 4);
    assert_eq!(dispatcher.run_next(&worker, || true).await.unwrap(), None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let row = harness.row(id).await;
    assert_eq!((row.state.as_str(), row.attempts), ("dead", 1));
    assert_eq!(
        row.last_error.as_deref(),
        Some(format!("JOB_HANDLER_REJECTED job_id={id} attempt=1").as_str())
    );
    assert_eq!(
        *captured.0.lock().unwrap(),
        [JobAuditEvent::DeadLettered {
            job_id: id,
            kind: JobKind::StorageDeleteBlob,
            attempts: 1,
            failure: FailureClass::HandlerRejected,
        }]
    );
}

#[tokio::test]
async fn it_jobs_dead_letter_audited() {
    let harness = Harness::open().await;
    let calls = Arc::new(AtomicU32::new(0));
    let registry = Registry::default().register(JobKind::TokensPrune, idempotency(), {
        let calls = Arc::clone(&calls);
        move |job: ClaimedJob| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(job.payload()["marker"], PAYLOAD_MARKER);
                assert!(call > 0, "fixture handler panic");
                Err(anyhow::anyhow!("upstream rejected: {ERROR_MARKER}"))
            }
        }
    });
    let captured = Arc::new(CapturedAudit::default());
    let dispatcher = harness.dispatcher(
        registry,
        Jitter::from_fn(|| 0),
        JobAudit::new(captured.clone()),
    );
    let Enqueued::Inserted(id) = harness
        .enqueue(&NewJob::new(
            JobKind::TokensPrune,
            JobPayload::new(&json!({ "marker": PAYLOAD_MARKER })).unwrap(),
        ))
        .await
    else {
        panic!("unkeyed enqueue inserts");
    };
    let worker = claimant(&harness.clock, 0);
    let max_attempts = JobKind::TokensPrune.policy().max_attempts;
    assert_eq!(max_attempts, 3);

    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("info"),
        LogFormat::Json,
        output.clone(),
        SystemTime,
        false,
    );
    let guard = tracing::dispatcher::set_default(&dispatch);

    let panicked = dispatcher.run_next(&worker, || true).await.unwrap();
    assert!(matches!(panicked, Some(Outcome::Retrying { .. })));
    assert_eq!(
        harness.row(id).await.last_error.as_deref(),
        Some(format!("JOB_HANDLER_PANICKED job_id={id} attempt=1").as_str())
    );

    harness.clock.advance(RETRY_CAP * 2);
    let failed = dispatcher.run_next(&worker, || true).await.unwrap();
    assert!(matches!(failed, Some(Outcome::Retrying { .. })));

    harness.clock.advance(RETRY_CAP * 2);
    let dead = dispatcher.run_next(&worker, || true).await.unwrap();
    assert_eq!(dead, Some(Outcome::DeadLettered { attempts: 3 }));

    harness.clock.advance(RETRY_CAP * 4);
    assert_eq!(dispatcher.run_next(&worker, || true).await.unwrap(), None);
    assert!(harness
        .claim(&worker, &[JobKind::TokensPrune], 10)
        .await
        .is_empty());
    assert_eq!(
        sweep_expired_leases(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        0
    );
    drop(guard);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let row = harness.row(id).await;
    assert_eq!(row.state, "dead");
    assert_eq!(row.attempts, i64::from(max_attempts));
    assert_eq!(row.claimed_by, None);
    assert_eq!(row.lease_expires_at, None);
    let last_error = row.last_error.unwrap();
    assert_eq!(
        last_error,
        format!("JOB_HANDLER_FAILED job_id={id} attempt=3")
    );
    assert!(last_error.len() <= MAX_LAST_ERROR_BYTES);
    assert!(!last_error.contains(ERROR_MARKER));
    assert!(!last_error.contains(PAYLOAD_MARKER));

    let events = captured.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    let event = events[0];
    assert_eq!(
        event,
        JobAuditEvent::DeadLettered {
            job_id: id,
            kind: JobKind::TokensPrune,
            attempts: 3,
            failure: FailureClass::HandlerFailed,
        }
    );
    assert_eq!(event.action(), "JOB_DEAD_LETTERED");

    let text = output.text();
    assert!(!text.contains(ERROR_MARKER));
    assert!(!text.contains(PAYLOAD_MARKER));
    let errors: Vec<_> = output
        .json_lines()
        .into_iter()
        .filter(|line| line["level"] == "ERROR")
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let error = &errors[0];
    assert_eq!(error["message"], "job.dead_lettered");
    assert_eq!(error["job_id"], id.to_string());
    assert_eq!(error["kind"], "tokens.prune");
    assert_eq!(error["attempts"], 3);
    assert_eq!(error["max_attempts"], 3);
    assert_eq!(error["error_class"], "JOB_HANDLER_FAILED");

    let oversized = "é".repeat(MAX_LAST_ERROR_BYTES);
    let bounded = bounded_error(&oversized);
    assert!(bounded.len() <= MAX_LAST_ERROR_BYTES);
    assert!(oversized.starts_with(&bounded));
    assert_eq!(bounded_error("short"), "short");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_jobs_stop_claiming_on_shutdown() {
    let harness = Harness::open().await;
    let ids = harness.enqueue_many(JobKind::SessionsPrune, 5).await;
    let (started, mut starts) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let registry = Registry::default().register(JobKind::SessionsPrune, idempotency(), {
        let release = Arc::clone(&release);
        move |job: ClaimedJob| {
            let started = started.clone();
            let release = Arc::clone(&release);
            async move {
                started.send(job.id()).unwrap();
                release.acquire().await?.forget();
                Ok(())
            }
        }
    });
    let dispatcher = harness.dispatcher(registry, Jitter::from_fn(|| 0), JobAudit::detached());
    let timing = RuntimeTiming {
        poll: Duration::from_millis(5),
        lease_sweep: Duration::from_secs(3_600),
        lease_renewal: Duration::from_secs(3_600),
    };
    let runtime = JobRuntime::start(&dispatcher, InstanceId::generate(&harness.clock), 2, timing);

    let running: HashSet<JobId> = [starts.recv().await.unwrap(), starts.recv().await.unwrap()]
        .into_iter()
        .collect();
    assert_eq!(running.len(), 2);

    runtime.stop_claiming();
    release.add_permits(ids.len());
    assert_eq!(
        runtime.shutdown(Duration::from_secs(30)).await,
        JobsDrain::Completed
    );
    assert!(starts.try_recv().is_err());

    let mut waiting = Vec::new();
    for id in &ids {
        let row = harness.row(*id).await;
        assert_eq!(row.claimed_by, None);
        if running.contains(id) {
            assert_eq!(row.state, "succeeded");
        } else {
            assert_eq!(row.state, "pending");
            assert_eq!(row.updated_at, row.created_at);
            waiting.push(*id);
        }
    }
    assert_eq!(waiting.len(), 3);

    let stuck_registry =
        Registry::default().register(JobKind::SessionsPrune, idempotency(), |_job| {
            std::future::pending::<anyhow::Result<()>>()
        });
    let stuck = harness.dispatcher(stuck_registry, Jitter::from_fn(|| 0), JobAudit::detached());
    let runtime = JobRuntime::start(&stuck, InstanceId::generate(&harness.clock), 1, timing);
    loop {
        if harness.count("state = 'claimed'").await == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        runtime.shutdown(Duration::from_millis(50)).await,
        JobsDrain::GraceElapsed { interrupted: 1 }
    );
    let interrupted: String = sqlx::query_scalar("SELECT id FROM jobs WHERE state = 'claimed'")
        .fetch_one(harness.pools.reader().executor())
        .await
        .unwrap();
    let interrupted = harness.row(interrupted.parse().unwrap()).await;
    assert_eq!(interrupted.state, "claimed");
    assert_eq!(interrupted.attempts, 0);
    assert_eq!(
        interrupted.lease_expires_at,
        Some(at(&harness.clock, DEFAULT_LEASE))
    );
    assert_eq!(harness.count("state = 'claimed'").await, 1);
}

#[tokio::test]
async fn it_jobs_prune_succeeded_after_retention() {
    let harness = Harness::open().await;
    let ids = harness.enqueue_many(JobKind::QuotaReconcile, 4).await;
    let worker = claimant(&harness.clock, 0);
    for job in harness.claim(&worker, &[JobKind::QuotaReconcile], 2).await {
        assert_eq!(
            settle_success(&harness.pools, &harness.clock, &job)
                .await
                .unwrap(),
            Settled::Succeeded
        );
    }
    harness
        .pools
        .write_tx(&harness.clock, "test.terminal_fixtures", async |tx| {
            sqlx::query(
                "UPDATE jobs SET state = 'dead', last_error = 'JOB_HANDLER_FAILED'
                  WHERE id = (SELECT id FROM jobs WHERE state = 'pending' LIMIT 1)",
            )
            .execute(tx.executor())
            .await?;
            sqlx::query(
                "UPDATE jobs SET state = 'failed', last_error = 'JOB_HANDLER_FAILED'
                  WHERE id = (SELECT id FROM jobs WHERE state = 'pending' LIMIT 1)",
            )
            .execute(tx.executor())
            .await?;
            Ok::<_, JobsError>(())
        })
        .await
        .unwrap();
    let live = harness.enqueue_many(JobKind::QuotaReconcile, 1).await;
    harness.claim(&worker, &[JobKind::QuotaReconcile], 1).await;

    harness
        .clock
        .advance(SUCCEEDED_RETENTION - Duration::from_millis(1));
    assert_eq!(
        prune_succeeded(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        0
    );
    harness.clock.advance(Duration::from_millis(1));
    assert_eq!(
        prune_succeeded(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        2
    );
    harness.clock.advance(SUCCEEDED_RETENTION * 30);
    assert_eq!(
        prune_succeeded(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        0
    );
    assert_eq!(harness.count("1 = 1").await, 3);
    assert_eq!(harness.count("state = 'dead'").await, 1);
    assert_eq!(harness.count("state = 'failed'").await, 1);
    assert_eq!(harness.row(live[0]).await.state, "claimed");
    assert_eq!(ids.len(), 4);
}

fn execution_counter(
    calls: &Arc<AtomicU32>,
) -> impl Fn(ClaimedJob) -> std::future::Ready<anyhow::Result<()>> + Send + Sync + 'static {
    let calls = Arc::clone(calls);
    move |_job: ClaimedJob| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(()))
    }
}

fn recurring_registry(
    pools: &DbPools,
    clock: &TestClock,
    recurring: Recurring,
    payload: &JobPayload,
    calls: &Arc<AtomicU32>,
) -> Registry {
    let pools = pools.clone();
    let clock = clock.clone();
    let payload = payload.clone();
    let calls = Arc::clone(calls);
    Registry::default().register(recurring.kind(), idempotency(), move |_job: ClaimedJob| {
        let pools = pools.clone();
        let clock = clock.clone();
        let payload = payload.clone();
        calls.fetch_add(1, Ordering::SeqCst);
        async move {
            pools
                .write_tx(&clock, "test.recurring_successor", async |tx| {
                    recurring.schedule_successor(tx, &clock, &payload).await
                })
                .await?;
            Ok(())
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_jobs_run_once_is_idempotent() {
    let harness = Harness::open().await;
    let target = JobKind::EmailSend;
    let other = JobKind::SessionsPrune;

    let target_ids = harness.enqueue_many(target, 5).await;
    let other_ids = harness.enqueue_many(other, 2).await;
    let Enqueued::Inserted(future_id) = harness
        .enqueue(
            &NewJob::new(target, JobPayload::empty())
                .run_at(Timestamp::try_from(START + Duration::from_secs(600)).unwrap()),
        )
        .await
    else {
        panic!("unkeyed enqueue inserts");
    };

    let calls = Arc::new(AtomicU32::new(0));
    let registry = Registry::default().register(target, idempotency(), execution_counter(&calls));
    let dispatcher = harness.dispatcher(registry, Jitter::from_fn(|| 0), JobAudit::detached());
    let worker = claimant(&harness.clock, 0);

    let first = run_once(&dispatcher, &worker, target).await.unwrap();
    assert_eq!(first.kind, target);
    assert_eq!(first.executed, 5);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    for id in &target_ids {
        assert_eq!(harness.row(*id).await.state, "succeeded");
    }
    for id in &other_ids {
        assert_eq!(harness.row(*id).await.state, "pending");
    }
    assert_eq!(harness.row(future_id).await.state, "pending");

    let second = run_once(&dispatcher, &worker, target).await.unwrap();
    assert_eq!(second.executed, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    for id in &other_ids {
        assert_eq!(harness.row(*id).await.state, "pending");
    }

    harness.clock.advance(Duration::from_secs(599));
    assert_eq!(
        run_once(&dispatcher, &worker, target)
            .await
            .unwrap()
            .executed,
        0
    );
    assert_eq!(harness.row(future_id).await.state, "pending");

    harness.clock.advance(Duration::from_secs(1));
    assert_eq!(
        run_once(&dispatcher, &worker, target)
            .await
            .unwrap()
            .executed,
        1
    );
    assert_eq!(harness.row(future_id).await.state, "succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 6);

    for id in &other_ids {
        assert_eq!(harness.row(*id).await.state, "pending");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_recurring_job_single_chain_after_restart() {
    let mut harness = Harness::open().await;
    let kind = JobKind::TokensPrune;
    let recurring = Recurring::new(kind, Duration::from_secs(3_600)).unwrap();
    let payload = JobPayload::empty();
    let calls = Arc::new(AtomicU32::new(0));

    let Enqueued::Inserted(current_id) = harness.schedule_recurring(&recurring, &payload).await
    else {
        panic!("the first bucket schedules a fresh chain");
    };
    let now = Timestamp::try_from(harness.clock.now()).unwrap();
    let current = recurring.bucket(now).unwrap();
    let successor = recurring.successor_bucket(now).unwrap();
    assert!(current.start() <= now);
    assert!(successor.start() > now);

    let first = harness.dispatcher(
        recurring_registry(&harness.pools, &harness.clock, recurring, &payload, &calls),
        Jitter::from_fn(|| 0),
        JobAudit::detached(),
    );
    let worker = claimant(&harness.clock, 0);
    assert_eq!(run_once(&first, &worker, kind).await.unwrap().executed, 1);
    assert_eq!(harness.row(current_id).await.state, "succeeded");

    let successor_key = recurring.dedup_key(successor).unwrap();
    assert_eq!(
        successor_key.as_str(),
        format!("{kind}:{}", successor.start().get().unix_timestamp())
    );
    assert_eq!(
        harness
            .count(&format!("dedup_key = '{}'", successor_key.as_str()))
            .await,
        1
    );
    let (state, run_at, attempts): (String, String, i64) =
        sqlx::query_as("SELECT state, run_at, attempts FROM jobs WHERE dedup_key = ?1")
            .bind(successor_key.as_str())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(state, "pending");
    assert_eq!(run_at, successor.start().to_string());
    assert_eq!(attempts, 0);

    harness.reopen_pools().await;
    let second = harness.dispatcher(
        recurring_registry(&harness.pools, &harness.clock, recurring, &payload, &calls),
        Jitter::from_fn(|| 0),
        JobAudit::detached(),
    );

    assert_eq!(
        harness
            .schedule_recurring_successor(&recurring, &payload)
            .await,
        Enqueued::Deduplicated
    );
    assert_eq!(
        harness.schedule_recurring(&recurring, &payload).await,
        Enqueued::Deduplicated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness
            .count(&format!("dedup_key = '{}'", successor_key.as_str()))
            .await,
        1
    );
    assert_eq!(
        harness
            .count(&format!(
                "dedup_key = '{}'",
                recurring.dedup_key(current).unwrap().as_str()
            ))
            .await,
        1
    );
    assert_eq!(harness.count("kind = 'tokens.prune'").await, 2);

    harness.clock.advance(Duration::from_secs(3_600));
    assert_eq!(run_once(&second, &worker, kind).await.unwrap().executed, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(harness.count("kind = 'tokens.prune'").await, 3);
}
