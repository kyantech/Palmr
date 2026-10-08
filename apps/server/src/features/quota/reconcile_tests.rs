use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::Row;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use super::error::QuotaError;
use super::model::{ReleaseReason, ReservationState, TransferSessionId};
use super::reconcile::{
    confirm_drift, ensure_scheduled, expected_usage, reconcile, register_jobs,
    QuotaReconcileContext, QuotaReconcileReport, MAX_AUDITED_DRIFTS_PER_EXECUTION,
    QUOTA_RECONCILE_PERIOD, USER_PAGE_SIZE,
};
use super::repo;
use super::service::increment_used;
use super::tests::{bytes, Harness, UserSpec};
use crate::config::LogFormat;
use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::audit::service::{AuditDrain, AuditService};
use crate::features::audit::{self, AUDIT_BATCH_MAX};
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, InstanceId};
use crate::infra::jobs::claim::{claim, sweep_expired_leases};
use crate::infra::jobs::kinds::DEFAULT_LEASE;
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::runtime::Outcome;
use crate::infra::jobs::{Claimant, Dispatcher, Jitter, JobAudit, JobKind, JobPayload, Registry};
use crate::infra::telemetry::build_dispatch;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn lines(&self) -> Vec<Map<String, Value>> {
        let text = String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn with_message(&self, message: &str) -> Vec<Map<String, Value>> {
        self.lines()
            .into_iter()
            .filter(|line| line["message"] == message)
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

fn capture_logs() -> (Capture, tracing::dispatcher::DefaultGuard) {
    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("info"),
        LogFormat::Json,
        output.clone(),
        SystemTime,
        false,
    );
    (output, tracing::dispatcher::set_default(&dispatch))
}

const DRIFT_MESSAGE: &str = "quota usage drift detected; users.used_bytes was left unchanged";
const STALE_MESSAGE: &str =
    "stale quota reservation released by the reaper; the owning transfer path did not settle it";

struct Fixture {
    harness: Arc<Harness>,
    audit: AuditService,
    drain: tokio::sync::Mutex<AuditDrain>,
    claimant: Claimant,
    objects: AtomicU32,
}

impl Fixture {
    async fn open() -> Self {
        let harness = Harness::open().await;
        let (audit, drain) =
            audit::channel(1024, harness.pools.clone(), Arc::new(harness.clock.clone()));
        let claimant = Claimant::worker(InstanceId::generate(&harness.clock), 0);
        Self {
            harness,
            audit,
            drain: tokio::sync::Mutex::new(drain),
            claimant,
            objects: AtomicU32::new(1),
        }
    }

    fn context(&self) -> QuotaReconcileContext {
        QuotaReconcileContext::new(
            self.harness.pools.clone(),
            Arc::new(self.harness.clock.clone()),
            self.audit.clone(),
        )
    }

    fn dispatcher(&self) -> Dispatcher {
        Dispatcher::new(
            self.harness.pools.clone(),
            Arc::new(self.harness.clock.clone()),
            register_jobs(Registry::production(), self.context()),
            Jitter::from_fn(|| u32::MAX / 2),
            JobAudit::new(Arc::new(self.audit.clone())),
            Duration::from_secs(60),
        )
    }

    async fn schedule_due(&self) {
        let recurring = Recurring::new(JobKind::QuotaReconcile, QUOTA_RECONCILE_PERIOD).unwrap();
        self.harness
            .pools
            .write_tx(&self.harness.clock, "quota.test_schedule_due", async |tx| {
                recurring
                    .schedule(tx, &self.harness.clock, &JobPayload::empty())
                    .await
            })
            .await
            .unwrap();
    }

    async fn run_next(&self) -> Option<Outcome> {
        self.dispatcher()
            .run_next_kind(&self.claimant, JobKind::QuotaReconcile, || true)
            .await
            .unwrap()
    }

    async fn run_job(&self) -> Option<Outcome> {
        self.schedule_due().await;
        self.run_next().await
    }

    async fn audit_rows(&self, action: &str) -> Vec<Map<String, Value>> {
        let mut drain = self.drain.lock().await;
        while drain.flush_batch(AUDIT_BATCH_MAX).await.unwrap() > 0 {}
        sqlx::query(
            "SELECT action, actor_type, actor_user_id, target_type, target_id, result,
                    metadata_json
               FROM audit_events WHERE action = ?1 ORDER BY occurred_at, id",
        )
        .bind(action)
        .fetch_all(self.harness.pools.reader().executor())
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            let metadata: Option<String> = row.get("metadata_json");
            let mut object = Map::new();
            for column in ["action", "actor_type", "target_type", "target_id", "result"] {
                let value: Option<String> = row.get(column);
                object.insert(column.to_owned(), value.map_or(Value::Null, Value::from));
            }
            let actor: Option<String> = row.get("actor_user_id");
            object.insert(
                "actor_user_id".to_owned(),
                actor.map_or(Value::Null, Value::from),
            );
            object.insert(
                "metadata".to_owned(),
                metadata.map_or(Value::Null, |text| serde_json::from_str(&text).unwrap()),
            );
            object
        })
        .collect()
    }

    async fn pending_jobs(&self) -> Vec<(String, String)> {
        sqlx::query(
            "SELECT state, dedup_key FROM jobs WHERE kind = 'quota.reconcile' ORDER BY run_at, id",
        )
        .fetch_all(self.harness.pools.reader().executor())
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.get("state"), row.get("dedup_key")))
        .collect()
    }

    async fn object(&self, state: &str) -> String {
        let n = self.objects.fetch_add(1, Ordering::Relaxed);
        let id = Id::<ObjectMarker>::generate(&self.harness.clock).to_string();
        let key = format!("objects/{:02x}/{:02x}/{n:032x}", (n >> 8) & 0xff, n & 0xff);
        let now = self.harness.now().to_string();
        let active = state == "active";
        self.harness
            .pools
            .write_tx(&self.harness.clock, "quota.test_object", async |tx| {
                sqlx::query(
                    "INSERT INTO storage_objects
                         (id, object_key, provider, size_bytes, state, refcount, created_at,
                          updated_at, finalized_at, tombstoned_at)
                     VALUES (?1, ?2, 'local', 0, ?3, ?4, ?5, ?5, ?6, ?7)",
                )
                .bind(&id)
                .bind(&key)
                .bind(state)
                .bind(i64::from(active))
                .bind(&now)
                .bind(active.then_some(&now))
                .bind((!active).then_some(&now))
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    async fn file(&self, owner: UserId, size: i64, object_state: &str) {
        let object = self.object(object_state).await;
        let id = Id::<ObjectMarker>::generate(&self.harness.clock).to_string();
        let name = format!("file-{id}");
        let now = self.harness.now().to_string();
        self.harness
            .pools
            .write_tx(&self.harness.clock, "quota.test_file", async |tx| {
                sqlx::query(
                    "INSERT INTO files
                         (id, owner_id, storage_object_id, name, name_normalized, size_bytes,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?6)",
                )
                .bind(&id)
                .bind(owner.to_string())
                .bind(&object)
                .bind(&name)
                .bind(size)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
    }

    async fn received(&self, owner: UserId, share: &str, size: i64) {
        let object = self.object("active").await;
        let id = Id::<ObjectMarker>::generate(&self.harness.clock).to_string();
        let name = format!("received-{id}");
        let now = self.harness.now().to_string();
        self.harness
            .pools
            .write_tx(&self.harness.clock, "quota.test_received", async |tx| {
                sqlx::query(
                    "INSERT INTO received_files
                         (id, owner_id, reverse_share_id, storage_object_id, name,
                          name_normalized, size_bytes, received_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?7)",
                )
                .bind(&id)
                .bind(owner.to_string())
                .bind(share)
                .bind(&object)
                .bind(&name)
                .bind(size)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
    }

    async fn seed(
        &self,
        owner: UserId,
        reserved: i64,
        state: ReservationState,
        expires: Timestamp,
    ) -> TransferSessionId {
        let session = self.harness.session_expiring(owner, expires).await;
        let id = Id::<super::model::Reservation>::generate(&self.harness.clock).to_string();
        let created = Timestamp::try_from(self.harness.clock.now() - DAY * 40).unwrap();
        let settled = Timestamp::try_from(self.harness.clock.now() - DAY).unwrap();
        let (committed, settled_at, reason) = match state {
            ReservationState::Held => (None, None, None),
            ReservationState::Committed => (Some(reserved), Some(settled.to_string()), None),
            ReservationState::Released => (
                None,
                Some(settled.to_string()),
                Some(ReleaseReason::Canceled.as_str()),
            ),
        };
        self.harness
            .pools
            .write_tx(
                &self.harness.clock,
                "quota.test_seed_reservation",
                async |tx| {
                    sqlx::query(
                        "INSERT INTO quota_reservations
                         (id, user_id, transfer_session_id, context, reserved_bytes,
                          committed_bytes, state, created_at, expires_at, settled_at,
                          release_reason)
                     VALUES (?1, ?2, ?3, 'my_files', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    )
                    .bind(&id)
                    .bind(owner.to_string())
                    .bind(session.to_string())
                    .bind(reserved)
                    .bind(committed)
                    .bind(state.as_str())
                    .bind(created.to_string())
                    .bind(expires.to_string())
                    .bind(settled_at)
                    .bind(reason)
                    .execute(tx.executor())
                    .await
                    .map_err(DbError::from)?;
                    Ok::<(), DbError>(())
                },
            )
            .await
            .unwrap();
        session
    }
}

enum ObjectMarker {}

#[tokio::test]
async fn it_quota_reconcile_releases_stale_holds() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let expired_at = harness.after(Duration::ZERO);
    harness.clock.advance(Duration::from_secs(3_600));
    let live_until = harness.after(DAY * 30);

    let stale = fixture
        .seed(owner, 70, ReservationState::Held, expired_at)
        .await;
    let live = fixture
        .seed(owner, 30, ReservationState::Held, live_until)
        .await;
    let committed = fixture
        .seed(owner, 20, ReservationState::Committed, expired_at)
        .await;
    let released = fixture
        .seed(owner, 10, ReservationState::Released, expired_at)
        .await;
    assert_eq!(harness.held_sum(owner).await, 100);

    let live_before = harness.raw_state(live).await;
    let committed_before = harness.raw_state(committed).await;
    let released_before = harness.raw_state(released).await;

    let (logs, guard) = capture_logs();
    let outcome = fixture.run_job().await;
    drop(guard);
    assert_eq!(outcome, Some(Outcome::Succeeded));

    let reaped = harness.raw_state(stale).await;
    assert_eq!(reaped.state, "released");
    assert_eq!(reaped.release_reason.as_deref(), Some("reaped"));
    assert_eq!(
        reaped.settled_at.as_deref(),
        Some(harness.now().to_string().as_str())
    );
    assert_eq!(reaped.reserved, 70);
    assert_eq!(reaped.committed, None);
    assert_eq!(harness.held_sum(owner).await, 30);
    assert_eq!(harness.raw_state(live).await, live_before);
    assert_eq!(harness.raw_state(committed).await, committed_before);
    assert_eq!(harness.raw_state(released).await, released_before);
    assert_eq!(harness.used(owner).await, 0);

    let warnings = logs.with_message(STALE_MESSAGE);
    assert_eq!(warnings.len(), 1, "{:?}", logs.lines());
    let warning = &warnings[0];
    assert_eq!(warning["level"], "WARN");
    assert_eq!(warning["transfer_session_id"], stale.to_string());
    assert_eq!(warning["owner_id"], owner.to_string());
    assert_eq!(warning["reserved_bytes"], 70);
    assert_eq!(warning["expires_at"], expired_at.to_string());
    assert!(warning.contains_key("reservation_id"));

    harness.clock.advance(DAY + Duration::from_secs(1));
    let (logs, guard) = capture_logs();
    let second = fixture.run_job().await;
    drop(guard);
    assert_eq!(second, Some(Outcome::Succeeded));
    assert!(logs.with_message(STALE_MESSAGE).is_empty());
    assert_eq!(
        harness.raw_state(stale).await,
        reaped,
        "a second run must not settle the row again"
    );
    assert_eq!(harness.raw_state(live).await, live_before);
    assert_eq!(harness.raw_state(committed).await, committed_before);
    assert_eq!(harness.raw_state(released).await, released_before);
    assert_eq!(harness.held_sum(owner).await, 30);
}

#[tokio::test]
async fn it_quota_reconcile_expiry_boundary_is_inclusive() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let exactly_now = fixture
        .seed(owner, 1, ReservationState::Held, harness.now())
        .await;
    let a_moment_later = fixture
        .seed(
            owner,
            2,
            ReservationState::Held,
            harness.after(Duration::from_millis(1)),
        )
        .await;

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(report.reaped, 1);
    assert_eq!(harness.raw_state(exactly_now).await.state, "released");
    assert_eq!(harness.raw_state(a_moment_later).await.state, "held");
}

#[tokio::test]
async fn it_quota_reconcile_reaper_never_overwrites_a_winner() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let service = harness.service(None);
    let owner = harness.user(UserSpec::inherit(0)).await;
    let past = harness.now();
    harness.clock.advance(Duration::from_secs(60));

    let release_wins = fixture.seed(owner, 5, ReservationState::Held, past).await;
    let commit_wins = fixture.seed(owner, 6, ReservationState::Held, past).await;
    let reaper_wins = fixture.seed(owner, 7, ReservationState::Held, past).await;

    harness
        .release(&service, release_wins, ReleaseReason::Canceled)
        .await
        .unwrap();
    harness.commit(&service, commit_wins, 6).await.unwrap();
    let release_before = harness.raw_state(release_wins).await;
    let commit_before = harness.raw_state(commit_wins).await;

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(report.reaped, 1);
    assert_eq!(harness.raw_state(release_wins).await, release_before);
    assert_eq!(harness.raw_state(commit_wins).await, commit_before);
    assert_eq!(
        harness
            .raw_state(release_wins)
            .await
            .release_reason
            .as_deref(),
        Some("canceled")
    );
    assert_eq!(harness.raw_state(commit_wins).await.state, "committed");

    let reaped = harness.raw_state(reaper_wins).await;
    assert_eq!(reaped.release_reason.as_deref(), Some("reaped"));
    let late_release = harness
        .release(&service, reaper_wins, ReleaseReason::Expired)
        .await
        .unwrap();
    assert!(matches!(
        late_release,
        super::model::Settlement::AlreadySettled(_)
    ));
    assert!(matches!(
        harness.commit(&service, reaper_wins, 7).await,
        Err(QuotaError::StateConflict {
            current: ReservationState::Released
        })
    ));
    assert_eq!(harness.raw_state(reaper_wins).await, reaped);
}

#[tokio::test]
async fn it_quota_reconcile_reaps_in_bounded_batches() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let now = harness.now().to_string();
    let past = Timestamp::try_from(harness.clock.now() - DAY)
        .unwrap()
        .to_string();
    harness
        .pools
        .write_tx(&harness.clock, "quota.test_bulk_seed", async |tx| {
            sqlx::query(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2500)
                 INSERT INTO transfer_sessions
                     (id, context, user_id, provider, state, created_at, updated_at, expires_at)
                 SELECT printf('session-%05d', i), 'my_files', ?1, 'local', 'created', ?2, ?2, ?3
                   FROM n",
            )
            .bind(owner.to_string())
            .bind(&now)
            .bind(&past)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
            sqlx::query(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2500)
                 INSERT INTO quota_reservations
                     (id, user_id, transfer_session_id, context, reserved_bytes, state,
                      created_at, expires_at)
                 SELECT printf('reservation-%05d', i), ?1, printf('session-%05d', i),
                        'my_files', 3, 'held', ?2, ?3
                   FROM n",
            )
            .bind(owner.to_string())
            .bind(&now)
            .bind(&past)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();
    assert_eq!(harness.held_sum(owner).await, 7_500);

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(report.reaped, 2_500);
    assert_eq!(report.reap_transactions, 3);
    assert_eq!(harness.held_sum(owner).await, 0);
    let reaped: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quota_reservations
          WHERE state = 'released' AND release_reason = 'reaped' AND settled_at IS NOT NULL",
    )
    .fetch_one(harness.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(reaped, 2_500);
}

#[tokio::test]
async fn it_quota_reconcile_removes_settled_rows_after_thirty_days() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let future = harness.after(DAY * 5);
    let old = fixture
        .seed(owner, 1, ReservationState::Committed, future)
        .await;
    let old_released = fixture
        .seed(owner, 1, ReservationState::Released, future)
        .await;
    let held = fixture.seed(owner, 1, ReservationState::Held, future).await;
    harness.clock.advance(DAY * 29);
    let recent = fixture
        .seed(owner, 1, ReservationState::Released, future)
        .await;
    harness.clock.advance(DAY * 2);

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(report.settled_deleted, 2);
    let remaining: Vec<String> = sqlx::query_scalar(
        "SELECT transfer_session_id FROM quota_reservations ORDER BY transfer_session_id",
    )
    .fetch_all(harness.pools.reader().executor())
    .await
    .unwrap();
    let mut expected = vec![held.to_string(), recent.to_string()];
    expected.sort();
    assert_eq!(remaining, expected);
    assert!(![old, old_released]
        .iter()
        .any(|session| remaining.contains(&session.to_string())));
}

#[tokio::test]
async fn it_quota_reconcile_expected_usage_sums_files_and_received_per_owner() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(100)).await;
    let other = harness.user(UserSpec::inherit(999)).await;
    let owner_share = harness.reverse_share(owner).await;
    let other_share = harness.reverse_share(other).await;

    fixture.file(owner, 25, "active").await;
    fixture.file(owner, 15, "active").await;
    fixture.received(owner, &owner_share, 60).await;
    fixture.file(other, 1_000, "active").await;
    fixture.received(other, &other_share, 2_000).await;

    let snapshot = repo::usage_for(harness.pools.reader().executor(), &owner.to_string())
        .await
        .unwrap()
        .unwrap();
    let expected = expected_usage(&snapshot).unwrap();
    assert_eq!(expected.my_files, bytes(40));
    assert_eq!(expected.received, bytes(60));
    assert_eq!(expected.total, bytes(100));
    assert_eq!(snapshot.files.rows, 2);
    assert_eq!(snapshot.received.rows, 1);
    assert_eq!(snapshot.used, 100);

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(
        report.drifted_users, 1,
        "only the unrelated user is out of step"
    );
    let drift = fixture.audit_rows("QUOTA_DRIFT_DETECTED").await;
    assert_eq!(drift.len(), 1);
    assert_eq!(drift[0]["target_id"], other.to_string());
    assert_eq!(drift[0]["metadata"]["expected_bytes"], 3_000);
    assert_eq!(drift[0]["metadata"]["used_bytes"], 999);
}

#[tokio::test]
async fn it_quota_reconcile_detects_drift_without_repairing_it() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let service = harness.service(None);
    let drifting = harness.user(UserSpec::inherit(80)).await;
    let consistent = harness.user(UserSpec::inherit(7)).await;
    let empty = harness.user(UserSpec::inherit(0)).await;
    let share = harness.reverse_share(drifting).await;
    fixture.file(drifting, 40, "active").await;
    fixture.received(drifting, &share, 60).await;
    fixture.file(consistent, 7, "active").await;
    let live_hold = harness.held(&service, drifting, 11).await;
    let hold_before = harness.raw_state(live_hold).await;

    let (logs, guard) = capture_logs();
    let outcome = fixture.run_job().await;
    drop(guard);
    assert_eq!(outcome, Some(Outcome::Succeeded));

    assert_eq!(
        harness.used(drifting).await,
        80,
        "the counter must not be rewritten"
    );
    assert_eq!(harness.used(consistent).await, 7);
    assert_eq!(harness.used(empty).await, 0);
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(harness.pools.reader().executor())
        .await
        .unwrap();
    let received: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM received_files")
        .fetch_one(harness.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!((files, received), (2, 1));
    assert_eq!(harness.raw_state(live_hold).await, hold_before);

    let rows = fixture.audit_rows("QUOTA_DRIFT_DETECTED").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let event = &rows[0];
    assert_eq!(event["actor_type"], "system");
    assert_eq!(event["actor_user_id"], Value::Null);
    assert_eq!(event["target_type"], "user");
    assert_eq!(event["target_id"], drifting.to_string());
    assert_eq!(event["result"], "success");
    let metadata = &event["metadata"];
    assert_eq!(metadata["used_bytes"], 80);
    assert_eq!(metadata["expected_bytes"], 100);
    assert_eq!(metadata["difference_bytes"], 20);
    assert_eq!(metadata["my_files_bytes"], 40);
    assert_eq!(metadata["received_bytes"], 60);
    assert_eq!(metadata["file_count"], 1);
    assert_eq!(metadata["received_file_count"], 1);
    assert_eq!(metadata["inactive_object_rows"], 0);
    let text = serde_json::to_string(metadata).unwrap();
    assert!(
        !text.contains("objects/"),
        "object keys must not reach the audit row"
    );

    let drifts = logs.with_message(DRIFT_MESSAGE);
    assert_eq!(drifts.len(), 1, "{:?}", logs.lines());
    assert_eq!(drifts[0]["level"], "WARN");
    assert_eq!(drifts[0]["user_id"], drifting.to_string());
    assert_eq!(drifts[0]["used_bytes"], 80);
    assert_eq!(drifts[0]["expected_bytes"], 100);
    assert_eq!(drifts[0]["difference_bytes"], 20);
    assert_eq!(drifts[0]["my_files_bytes"], 40);
    assert_eq!(drifts[0]["received_bytes"], 60);

    harness.clock.advance(DAY + Duration::from_secs(1));
    let second = fixture.run_job().await;
    assert_eq!(second, Some(Outcome::Succeeded));
    assert_eq!(
        harness.used(drifting).await,
        80,
        "drift stays observable, never repaired"
    );
    assert_eq!(fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.len(), 2);
}

#[tokio::test]
async fn it_quota_reconcile_reports_inactive_objects_instead_of_zeroing_them() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(30)).await;
    fixture.file(owner, 30, "tombstoned").await;

    let (logs, guard) = capture_logs();
    let report = reconcile(&fixture.context()).await.unwrap();
    drop(guard);
    assert_eq!(report.inactive_object_rows, 1);
    assert_eq!(
        report.drifted_users, 0,
        "the tombstoned row still counts at its size"
    );
    let findings =
        logs.with_message("live content rows reference storage objects that are not active");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["level"], "ERROR");
    assert_eq!(findings[0]["user_id"], owner.to_string());
    assert_eq!(findings[0]["inactive_object_rows"], 1);
    assert_eq!(harness.used(owner).await, 30);
}

#[tokio::test]
async fn it_quota_reconcile_arithmetic_overflow_fails_visibly() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(5)).await;
    let share = harness.reverse_share(owner).await;
    fixture.file(owner, i64::MAX, "active").await;
    fixture.received(owner, &share, 1).await;
    let healthy = harness.user(UserSpec::inherit(0)).await;

    let (logs, guard) = capture_logs();
    let outcome = fixture.run_job().await;
    drop(guard);
    assert!(
        matches!(outcome, Some(Outcome::DeadLettered { .. })),
        "{outcome:?}"
    );

    assert_eq!(
        harness.used(owner).await,
        5,
        "the counter must not be touched"
    );
    assert_eq!(harness.used(healthy).await, 0);
    assert!(fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.is_empty());
    let errors = logs.with_message(
        "live usage could not be summed in 64-bit arithmetic; users.used_bytes was left unchanged",
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["user_id"], owner.to_string());
    assert_eq!(errors[0]["error_kind"], "quota_arithmetic_overflow");
    let dead = fixture.audit_rows("JOB_DEAD_LETTERED").await;
    assert_eq!(dead.len(), 1);

    let chain = fixture.pending_jobs().await;
    assert_eq!(
        chain.len(),
        2,
        "the chain survives the integrity failure: {chain:?}"
    );
    assert_eq!(chain[0].0, "dead");
    assert_eq!(chain[1].0, "pending");
}

#[tokio::test]
async fn it_quota_reconcile_handler_failure_is_not_recorded_as_success() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    harness.user(UserSpec::inherit(0)).await;
    harness
        .execute("DROP TABLE quota_reservations", Vec::new())
        .await;

    let outcome = fixture.run_job().await;
    assert!(
        matches!(outcome, Some(Outcome::Retrying { .. })),
        "{outcome:?}"
    );
    let chain = fixture.pending_jobs().await;
    assert_eq!(
        chain.len(),
        1,
        "no successor before the run succeeds: {chain:?}"
    );
    assert_eq!(chain[0].0, "pending");
    let attempts: i64 =
        sqlx::query_scalar("SELECT attempts FROM jobs WHERE kind = 'quota.reconcile'")
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn it_quota_reconcile_job_registers_schedules_once_and_survives_reclaim() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    assert!(register_jobs(Registry::production(), fixture.context())
        .kinds()
        .contains(&JobKind::QuotaReconcile));
    assert_eq!(QUOTA_RECONCILE_PERIOD, DAY);

    for _ in 0..3 {
        ensure_scheduled(&harness.pools, &harness.clock)
            .await
            .unwrap();
    }
    assert_eq!(fixture.pending_jobs().await.len(), 1);
    assert!(
        fixture.run_next().await.is_none(),
        "the first schedule is in the future"
    );

    harness.clock.advance(DAY + Duration::from_secs(1));
    let claimed = claim(
        &harness.pools,
        &harness.clock,
        &fixture.claimant,
        &[JobKind::QuotaReconcile],
        1,
        || true,
    )
    .await
    .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        ensure_scheduled(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        crate::infra::jobs::claim::Enqueued::Deduplicated,
        "a claimed job is live"
    );
    harness
        .clock
        .advance(DEFAULT_LEASE + Duration::from_secs(1));
    assert_eq!(
        sweep_expired_leases(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        1
    );

    let first = fixture.run_next().await;
    assert_eq!(first, Some(Outcome::Succeeded));
    let chain = fixture.pending_jobs().await;
    assert_eq!(chain.len(), 2, "{chain:?}");
    assert_eq!(chain[0].0, "succeeded");
    assert_eq!(chain[1].0, "pending");
    assert!(chain[0].1.starts_with("quota.reconcile:"));
    assert_ne!(chain[0].1, chain[1].1);

    for _ in 0..3 {
        ensure_scheduled(&harness.pools, &harness.clock)
            .await
            .unwrap();
    }
    assert_eq!(
        fixture.pending_jobs().await.len(),
        2,
        "no second chain after a restart"
    );
}

#[tokio::test]
async fn it_quota_reconcile_pages_through_every_user() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let total = i64::from(USER_PAGE_SIZE) * 2 + 50;
    let now = harness.now().to_string();
    harness
        .pools
        .write_tx(&harness.clock, "quota.test_many_users", async |tx| {
            for index in 0..total {
                let used = i64::from(index == total - 1) * 5;
                sqlx::query(
                    "INSERT INTO users
                         (id, email, email_normalized, username, username_normalized, used_bytes,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?5, ?5)",
                )
                .bind(format!("user-{index:05}"))
                .bind(format!("user{index}@example.test"))
                .bind(format!("user{index}"))
                .bind(used)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
            }
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();

    let report = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(i64::try_from(report.users_scanned).unwrap(), total);
    assert_eq!(report.drifted_users, 1);
    let rows = fixture.audit_rows("QUOTA_DRIFT_DETECTED").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["target_id"], format!("user-{:05}", total - 1));
}

#[tokio::test]
async fn it_quota_reconcile_recheck_suppresses_drift_that_resolved_meanwhile() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    fixture.file(owner, 10, "active").await;

    let stale_page = repo::usage_page(harness.pools.reader().executor(), "", USER_PAGE_SIZE)
        .await
        .unwrap();
    let stale = stale_page
        .iter()
        .find(|row| row.user_id == owner.to_string())
        .unwrap();
    assert_ne!(stale.used, expected_usage(stale).unwrap().total.to_i64());

    harness
        .pools
        .write_tx(&harness.clock, "quota.test_late_commit", async |tx| {
            increment_used(tx, owner, bytes(10)).await
        })
        .await
        .unwrap();

    let mut report = QuotaReconcileReport::default();
    confirm_drift(&fixture.context(), &mut report, &owner.to_string())
        .await
        .unwrap();
    assert_eq!(report.drifted_users, 0);
    assert!(fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.is_empty());

    harness
        .execute(
            "UPDATE users SET used_bytes = 3 WHERE id = ?1",
            vec![owner.to_string()],
        )
        .await;
    confirm_drift(&fixture.context(), &mut report, &owner.to_string())
        .await
        .unwrap();
    assert_eq!(report.drifted_users, 1);
    assert_eq!(fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.len(), 1);
    assert_eq!(harness.used(owner).await, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_quota_reconcile_concurrent_commits_never_look_like_drift() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let context = fixture.context();

    let writer = async {
        for _ in 0..120 {
            let object = fixture.object("active").await;
            let id = Id::<ObjectMarker>::generate(&harness.clock).to_string();
            let now = harness.now().to_string();
            harness
                .pools
                .write_tx(&harness.clock, "quota.test_legit_commit", async |tx| {
                    sqlx::query(
                        "INSERT INTO files
                             (id, owner_id, storage_object_id, name, name_normalized, size_bytes,
                              created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?1, ?1, 3, ?4, ?4)",
                    )
                    .bind(&id)
                    .bind(owner.to_string())
                    .bind(&object)
                    .bind(&now)
                    .execute(tx.executor())
                    .await?;
                    increment_used(tx, owner, bytes(3)).await
                })
                .await
                .unwrap();
        }
    };
    let reader = async {
        let mut drifted = 0;
        let mut scans = 0;
        for _ in 0..25 {
            let report = reconcile(&context).await.unwrap();
            drifted += report.drifted_users;
            scans += report.users_scanned;
            tokio::task::yield_now().await;
        }
        (drifted, scans)
    };
    let ((), (drifted, scans)) = tokio::join!(writer, reader);
    assert!(scans >= 25);
    assert_eq!(
        drifted, 0,
        "a legitimate accounting commit was reported as drift"
    );
    assert!(fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.is_empty());
    assert_eq!(harness.used(owner).await, 360);
}

#[tokio::test]
async fn it_quota_reconcile_audits_every_drifting_user_across_bounded_jobs() {
    let fixture = Fixture::open().await;
    let harness = &fixture.harness;
    let drifting: i64 = 150;
    let now = harness.now().to_string();
    harness
        .pools
        .write_tx(&harness.clock, "quota.test_drifting_users", async |tx| {
            for index in 0..drifting {
                sqlx::query(
                    "INSERT INTO users
                         (id, email, email_normalized, username, username_normalized, used_bytes,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?5, ?5)",
                )
                .bind(format!("user-{index:05}"))
                .bind(format!("user{index}@example.test"))
                .bind(format!("user{index}"))
                .bind(1_000 + index)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
            }
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();

    fixture.schedule_due().await;
    let first = fixture.run_next().await;
    assert_eq!(first, Some(Outcome::Succeeded));
    let after_first = fixture.audit_rows("QUOTA_DRIFT_DETECTED").await;
    assert_eq!(
        after_first.len(),
        MAX_AUDITED_DRIFTS_PER_EXECUTION as usize,
        "one execution audits at most its budget"
    );
    let chain = fixture.pending_jobs().await;
    assert_eq!(chain.len(), 2, "{chain:?}");
    assert_eq!(chain[1].0, "pending");
    assert!(chain[1].1.contains(":after:user-00099"), "{chain:?}");

    let second = fixture.run_next().await;
    assert_eq!(second, Some(Outcome::Succeeded));
    let findings = fixture.audit_rows("QUOTA_DRIFT_DETECTED").await;
    assert_eq!(findings.len(), 150);
    let mut targets: Vec<String> = findings
        .iter()
        .map(|row| row["target_id"].as_str().unwrap().to_owned())
        .collect();
    targets.sort();
    targets.dedup();
    assert_eq!(
        targets.len(),
        150,
        "every drifting user is audited exactly once"
    );
    for event in &findings {
        let index: i64 = event["target_id"].as_str().unwrap()[5..].parse().unwrap();
        assert_eq!(event["metadata"]["used_bytes"], 1_000 + index);
        assert_eq!(event["metadata"]["expected_bytes"], 0);
    }

    let chain = fixture.pending_jobs().await;
    let pending: Vec<_> = chain.iter().filter(|job| job.0 == "pending").collect();
    assert_eq!(
        pending.len(),
        1,
        "one successor, no more continuations: {chain:?}"
    );
    assert!(!pending[0].1.contains(":after:"));

    assert!(
        fixture.run_next().await.is_none(),
        "the successor is not due yet"
    );
    harness.clock.advance(Duration::from_secs(60));
    let repeat = reconcile(&fixture.context()).await.unwrap();
    assert_eq!(repeat.drifted_users, 150, "every finding is still reported");
    assert_eq!(repeat.already_audited, 150);
    assert_eq!(repeat.audited_drifts, 0);
    assert_eq!(repeat.resume_after, None);
    assert_eq!(
        fixture.audit_rows("QUOTA_DRIFT_DETECTED").await.len(),
        150,
        "an unchanged finding inside the period is not audited again"
    );

    for index in 0..drifting {
        let used: i64 = sqlx::query_scalar("SELECT used_bytes FROM users WHERE id = ?1")
            .bind(format!("user-{index:05}"))
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
        assert_eq!(used, 1_000 + index, "no user is repaired");
    }
}
