use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::s3_multipart::{upstream, PartGate, S3Rig};
use super::transfer_reaper::{DAY, HOUR};
use super::transfers::sized;
use super::tus::{pattern, Gate};
use super::*;
use crate::features::transfers::finalize::Point as FinalizePoint;
use crate::features::transfers::s3::reapers::{
    self as reapers, Point, ReaperHooks, S3ReaperContext, S3ReaperParts,
};
use crate::infra::db::InstanceId;
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{
    Claimant, DedupKey, Dispatcher, Jitter, JobAudit, JobKind, JobPayload, NewJob, Registry,
};
use crate::storage::s3::profile::MIB;

pub(super) const ABORT: JobKind = JobKind::S3AbortAbandonedMultipart;
pub(super) const RECONCILE: JobKind = JobKind::S3ReconcileMultipart;
pub(super) const MILLI: Duration = Duration::from_millis(1);
pub(super) const GRACE: Duration = Duration::from_secs(2 * 60);

pub(super) fn data(size: u64) -> Vec<u8> {
    pattern(usize::try_from(size).unwrap())
}

#[derive(Default)]
pub(super) struct Points {
    gates: Mutex<HashMap<Point, Arc<Gate>>>,
    crashes: Mutex<HashSet<Point>>,
    hits: Mutex<Vec<Point>>,
}

impl Points {
    pub(super) fn gate_at(&self, point: Point) -> Arc<Gate> {
        let gate = Gate::new();
        self.gates.lock().unwrap().insert(point, Arc::clone(&gate));
        gate
    }

    pub(super) fn crash_at(&self, point: Point) {
        self.crashes.lock().unwrap().insert(point);
    }

    pub(super) fn hits(&self, point: Point) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|hit| **hit == point)
            .count()
    }
}

#[async_trait]
impl ReaperHooks for Points {
    async fn proceeds(&self, point: Point) -> bool {
        self.hits.lock().unwrap().push(point);
        let gate = self.gates.lock().unwrap().remove(&point);
        if let Some(gate) = gate {
            gate.pass().await;
        }
        !self.crashes.lock().unwrap().remove(&point)
    }
}

pub(super) struct Reaper {
    pub(super) context: S3ReaperContext,
    pub(super) points: Arc<Points>,
    pub(super) kinds: Vec<JobKind>,
    dispatcher: Dispatcher,
    claimant: Claimant,
}

impl Reaper {
    pub(super) fn new(rig: &S3Rig) -> Self {
        Self::stoppable(rig, CancellationToken::new())
    }

    pub(super) fn stoppable(rig: &S3Rig, stop: CancellationToken) -> Self {
        let points = Arc::new(Points::default());
        let clock: Arc<dyn crate::domain::clock::Clock> = Arc::new(rig.stack.clock.clone());
        let context = S3ReaperContext::new(S3ReaperParts {
            pools: rig.stack.pools.clone(),
            clock: Arc::clone(&clock),
            settings: rig.stack.settings.handle(),
            audit: rig.stack.audit.clone(),
            storage: rig.storage.clone(),
            proxy: rig.stack.s3_handle.as_ref().map(|s3| s3.proxy_uploads()),
            stop,
            hooks: Some(Arc::clone(&points) as Arc<dyn ReaperHooks>),
        });
        let registry = reapers::register_jobs(Registry::production(), context.clone());
        let kinds = registry.kinds();
        let dispatcher = Dispatcher::new(
            rig.stack.pools.clone(),
            clock,
            registry,
            Jitter::from_fn(|| u32::MAX / 2),
            JobAudit::new(Arc::new(rig.stack.audit.clone())),
            Duration::from_secs(60),
        );
        Self {
            context,
            points,
            kinds,
            dispatcher,
            claimant: Claimant::worker(InstanceId::generate(&rig.stack.clock), 0),
        }
    }

    pub(super) async fn run_all(&self, kind: JobKind) -> u64 {
        let mut executed = 0;
        while self
            .dispatcher
            .run_next_kind(&self.claimant, kind, || true)
            .await
            .unwrap()
            .is_some()
        {
            executed += 1;
        }
        executed
    }

    pub(super) async fn drain(&self) -> u64 {
        let mut total = 0;
        for _ in 0..12 {
            let round = self.run_all(RECONCILE).await + self.run_all(ABORT).await;
            total += round;
            if round == 0 {
                break;
            }
        }
        total
    }

    pub(super) async fn startup(&self, rig: &S3Rig) -> u64 {
        reapers::schedule_startup(
            &rig.stack.pools,
            &rig.stack.clock,
            InstanceId::generate(&rig.stack.clock),
        )
        .await
        .unwrap()
    }
}

pub(super) async fn enqueue_raw(rig: &S3Rig, kind: JobKind, payload: Value, dedup: &str) {
    let payload = JobPayload::new(&payload).unwrap();
    let key = DedupKey::new(dedup).unwrap();
    rig.stack
        .pools
        .write_tx(&rig.stack.clock, "test.enqueue", async |tx| {
            enqueue(
                tx,
                &rig.stack.clock,
                &NewJob::new(kind, payload.clone()).dedup_key(key.clone()),
            )
            .await
        })
        .await
        .unwrap();
}

pub(super) struct Row {
    pub(super) state: String,
    pub(super) expires_at: String,
    pub(super) completed_at: Option<String>,
    pub(super) upload_id: String,
    pub(super) key: String,
}

pub(super) async fn row(rig: &S3Rig, item: &str) -> Row {
    let (state, expires_at, completed_at, upload_id, key): (
        String,
        String,
        Option<String>,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT state, expires_at, completed_at, s3_upload_id, object_key
           FROM s3_multipart_uploads WHERE transfer_session_file_id = ?1",
    )
    .bind(item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    Row {
        state,
        expires_at,
        completed_at,
        upload_id,
        key,
    }
}

pub(super) fn instant(text: &str) -> OffsetDateTime {
    text.parse::<crate::domain::time::Timestamp>()
        .unwrap()
        .get()
}

pub(super) fn aborted(rig: &S3Rig) -> Vec<(String, String)> {
    rig.faults.abort_log.lock().unwrap().clone()
}

pub(super) async fn jobs_of(rig: &S3Rig, kind: JobKind, state: &str) -> i64 {
    rig.stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM jobs WHERE kind = '{}' AND state = '{state}'",
            kind.as_str()
        ))
        .await
}

pub(super) async fn audit_count(rig: &S3Rig, action: &str) -> i64 {
    rig.stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
}

pub(super) async fn live_upload(rig: &S3Rig, name: &str, size: u64) -> (String, String, Vec<u8>) {
    let bytes = data(size);
    let (session, item) = rig.one(name, size).await;
    rig.open(&session, &item, &bytes).await;
    (session, item, bytes)
}

#[tokio::test]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R007_abandoned_multipart_abort() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "r007.bin", size).await;
    let key = rig.final_key(&item).await;
    let before = row(&rig, &item).await;
    assert_eq!(before.state, "in_progress");
    assert_eq!(rig.open_uploads(&key).await, 1);
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;

    rig.stack.clock.advance(DAY - Duration::from_secs(1));
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "in_progress");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert!(aborted(&rig).is_empty());

    rig.stack.clock.advance(Duration::from_secs(1));
    reaper.drain().await;

    let after = row(&rig, &item).await;
    assert_eq!(after.state, "aborted");
    assert!(after.completed_at.is_some());
    assert_eq!(
        rig.open_uploads(&key).await,
        0,
        "ListMultipartUploads ends empty"
    );
    assert_eq!(
        aborted(&rig),
        [(before.key.clone(), before.upload_id.clone())],
        "the abort used the exact recorded identity, once"
    );
    assert_eq!(rig.item_row(&item).await.0, "expired");
    assert_eq!(
        rig.stack.reservation(&session).await,
        ("released".to_owned(), 0, None, Some("expired".to_owned()))
    );
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    rig.stack.flush_audit().await;
    assert_eq!(audit_count(&rig, "TRANSFER_EXPIRED").await, 1);

    rig.stack.clock.advance(DAY);
    reaper.drain().await;
    assert_eq!(aborted(&rig).len(), 1, "a second run issues nothing");
    assert_eq!(row(&rig, &item).await.state, "aborted");
    rig.stack.flush_audit().await;
    assert_eq!(audit_count(&rig, "TRANSFER_EXPIRED").await, 1);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_expiry_boundary_is_exact_to_the_millisecond() {
    let rig = S3Rig::new().await;
    let size = 12 * MIB;
    let (_, first, _) = live_upload(&rig, "a.bin", size).await;
    let (_, second, _) = live_upload(&rig, "b.bin", size).await;
    let (_, third, _) = live_upload(&rig, "c.bin", size).await;
    let reaper = Reaper::new(&rig);
    let expires = instant(&row(&rig, &first).await.expires_at);
    for item in [&second, &third] {
        assert_eq!(instant(&row(&rig, item).await.expires_at), expires);
    }

    rig.stack.clock.set(expires - MILLI);
    enqueue_raw(
        &rig,
        ABORT,
        serde_json::json!({"mode": "sweep"}),
        "t:before",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert_eq!(
        row(&rig, &first).await.state,
        "in_progress",
        "expires_at - 1 ms"
    );
    assert!(aborted(&rig).is_empty());

    rig.stack.clock.set(expires);
    enqueue_raw(&rig, ABORT, serde_json::json!({"mode": "sweep"}), "t:at").await;
    reaper.run_all(ABORT).await;
    for item in [&first, &second, &third] {
        let state = row(&rig, item).await.state;
        assert!(
            ["abandoned", "aborted"].contains(&state.as_str()),
            "{state}"
        );
    }
    assert_eq!(rig.item_row(&first).await.0, "expired", "expires_at");
    rig.stack.clock.set(expires + MILLI);
    reaper.drain().await;
    assert_eq!(rig.open_uploads(&row(&rig, &second).await.key).await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_part_activity_extends_the_upload_past_the_ttl() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("slow.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED);
    rig.upload_parts(&session, &item, &bytes, &[1], true).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    let first_expiry = instant(&row(&rig, &item).await.expires_at);

    rig.stack.clock.advance(DAY - HOUR);
    rig.upload_parts(&session, &item, &bytes, &[2], true).await;
    let renewed = instant(&row(&rig, &item).await.expires_at);
    assert_eq!(
        renewed,
        first_expiry + DAY - HOUR,
        "created_at is not the clock"
    );

    rig.stack.clock.advance(HOUR * 2);
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "in_progress");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert!(aborted(&rig).is_empty());

    rig.stack.clock.advance(DAY);
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(rig.open_uploads(&key).await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_renewal_after_discovery_wins_the_guarded_transition() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("renewed.bin", size).await;
    rig.create(&session, &item).await;
    rig.upload_parts(&session, &item, &bytes, &[1], true).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    let gate = reaper.points.gate_at(Point::ExpireDiscovered);
    rig.stack.clock.advance(DAY);
    enqueue_raw(&rig, ABORT, serde_json::json!({"mode": "sweep"}), "t:race").await;

    let sweep = reaper.run_all(ABORT);
    let renew = async {
        gate.wait_until_entered().await;
        let presigned = rig.presign(&session, &item, &[2]).await;
        assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
        gate.open();
    };
    let (executed, ()) = tokio::join!(sweep, renew);
    assert!(executed >= 1);
    assert_eq!(
        row(&rig, &item).await.state,
        "in_progress",
        "the renewal committed first, so the stale snapshot cannot expire it"
    );
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert!(aborted(&rig).is_empty());
    assert_eq!(rig.stack.counts().await.held, i64::try_from(size).unwrap());
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_completing_upload_is_protected_from_the_reaper() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "completing.bin", size).await;
    let key = rig.final_key(&item).await;
    rig.hooks.crash_at(FinalizePoint::BeforePlace);
    let completed = rig.complete(&session, &item).await;
    assert_ne!(completed.status, StatusCode::OK);
    assert_eq!(row(&rig, &item).await.state, "completing");
    assert_eq!(rig.item_row(&item).await.0, "finalizing");

    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY * 3);
    reaper.drain().await;

    assert_eq!(
        row(&rig, &item).await.state,
        "completing",
        "a completing row belongs to the finalizer"
    );
    assert_eq!(rig.item_row(&item).await.0, "finalizing");
    assert!(
        aborted(&rig)
            .iter()
            .all(|(aborted_key, _)| *aborted_key != key),
        "no abort for the completing upload"
    );
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert_eq!(rig.files().await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_finalization_wins_and_the_reaper_never_touches_the_file() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, bytes) = live_upload(&rig, "kept.bin", size).await;
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY * 3);
    reaper.drain().await;

    assert_eq!(
        rig.read_object(&key).await.1,
        Sha256::digest(&bytes).to_vec()
    );
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(row(&rig, &item).await.state, "completed");
    assert!(aborted(&rig)
        .iter()
        .all(|(aborted_key, _)| *aborted_key != key));
    rig.stack.flush_audit().await;
    assert_eq!(rig.uploads_audited().await, 1);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_expiry_wins_before_finalization_ownership() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "late.bin", size).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "aborted");

    let late = rig.complete(&session, &item).await;
    assert_eq!(late.status, StatusCode::NOT_FOUND, "{}", late.text());
    assert_eq!((rig.files().await, rig.objects().await), (0, 0));
    assert_eq!(rig.stack.counts().await.held, 0);
    assert_eq!(rig.used().await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_explicit_abort_failure_is_recovered_by_the_sweep() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "explicit.bin", size).await;
    let key = rig.final_key(&item).await;
    rig.faults.abort.lock().unwrap().push_back(upstream);
    let aborted_now = rig.abort(&session, &item).await;
    assert!(
        aborted_now.status.is_server_error(),
        "{}",
        aborted_now.text()
    );
    assert_eq!(row(&rig, &item).await.state, "abandoned");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert_eq!(rig.item_row(&item).await.0, "canceled");
    assert_eq!(rig.stack.reservation(&session).await.0, "released");

    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    reaper.drain().await;
    assert_eq!(
        row(&rig, &item).await.state,
        "abandoned",
        "inside the grace window the route's own retry owns it"
    );

    rig.stack.clock.advance(GRACE + Duration::from_secs(1));
    enqueue_raw(&rig, ABORT, serde_json::json!({"mode": "sweep"}), "t:grace").await;
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(rig.open_uploads(&key).await, 0);
    assert_eq!(rig.stack.reservation(&session).await.0, "released");
    assert_eq!(rig.used().await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_explicit_abort_succeeding_leaves_the_worker_idempotent() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "done.bin", size).await;
    let aborted_now = rig.abort(&session, &item).await;
    assert_eq!(
        aborted_now.status,
        StatusCode::NO_CONTENT,
        "{}",
        aborted_now.text()
    );
    assert_eq!(row(&rig, &item).await.state, "aborted");
    let calls = aborted(&rig).len();

    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY * 2);
    enqueue_raw(
        &rig,
        ABORT,
        serde_json::json!({"mode": "multipart", "multipartId": multipart_id(&rig, &item).await}),
        "t:again",
    )
    .await;
    reaper.drain().await;
    assert_eq!(
        aborted(&rig).len(),
        calls,
        "a terminal row issues no provider call"
    );
    rig.stack.flush_audit().await;
    assert_eq!(audit_count(&rig, "TRANSFER_CANCELED").await, 1);
    assert_eq!(rig.stack.reservation(&session).await.0, "released");
    rig.assert_no_overlap();
}

pub(super) async fn multipart_id(rig: &S3Rig, item: &str) -> String {
    sqlx::query_scalar("SELECT id FROM s3_multipart_uploads WHERE transfer_session_file_id = ?1")
        .bind(item)
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap()
}

#[tokio::test]
async fn it_same_multipart_abort_race_settles_once() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "race.bin", size).await;
    let key = rig.final_key(&item).await;
    let gate = PartGate::new();
    *rig.faults.abort_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let reaper = Reaper::new(&rig);

    let route = rig.abort(&session, &item);
    let worker = async {
        gate.entered.acquire().await.unwrap().forget();
        rig.stack.clock.advance(GRACE + Duration::from_secs(1));
        enqueue_raw(
            &rig,
            ABORT,
            serde_json::json!({"mode": "multipart", "multipartId": multipart_id(&rig, &item).await}),
            "t:race",
        )
        .await;
        let second = reaper.run_all(ABORT);
        let release = async {
            gate.entered.acquire().await.unwrap().forget();
            gate.release.add_permits(2);
        };
        let ((), executed) = tokio::join!(release, second);
        executed
    };
    let (response, executed) = tokio::join!(route, worker);
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(executed, 1);
    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(rig.open_uploads(&key).await, 0);
    assert_eq!(rig.stack.reservation(&session).await.0, "released");
    assert_eq!(rig.used().await, 0);
    rig.stack.flush_audit().await;
    assert_eq!(audit_count(&rig, "TRANSFER_CANCELED").await, 1);
    assert_eq!(audit_count(&rig, "TRANSFER_EXPIRED").await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_retry_after_provider_failure_remains_discoverable() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "retry.bin", size).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    rig.faults.abort.lock().unwrap().push_back(upstream);
    rig.faults.abort.lock().unwrap().push_back(upstream);

    reaper.drain().await;
    assert_eq!(
        row(&rig, &item).await.state,
        "abandoned",
        "the provider failed"
    );
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert!(
        jobs_of(&rig, ABORT, "pending").await >= 1,
        "the targeted job is waiting on its backoff"
    );

    rig.stack.clock.advance(HOUR);
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(rig.open_uploads(&key).await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_provider_success_followed_by_a_lost_ack_is_retried_as_no_such_upload() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "ack.bin", size).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    rig.faults.abort_after.lock().unwrap().push_back(upstream);

    reaper.drain().await;
    assert_eq!(rig.open_uploads(&key).await, 0, "the provider did abort");
    assert_eq!(row(&rig, &item).await.state, "abandoned");
    rig.stack.clock.advance(HOUR);
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(
        aborted(&rig).len(),
        2,
        "NoSuchUpload on the retry is success"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_multi_file_session_settles_each_share_once() {
    let mut rig = S3Rig::new().await;
    let size = 12 * MIB;
    let (session, mut items) = rig
        .session(&[
            sized("c1", "a.bin", size),
            sized("c2", "b.bin", size),
            sized("c3", "c.bin", size),
        ])
        .await;
    let (third, second, first) = (items.remove(2), items.remove(1), items.remove(0));
    let bytes = data(size);
    for item in [&first, &second, &third] {
        rig.open(&session, item, &bytes).await;
    }
    let done = rig.complete(&session, &first).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert_eq!(
        rig.stack.counts().await.held,
        2 * i64::try_from(size).unwrap()
    );

    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    reaper.drain().await;

    assert_eq!(rig.item_row(&first).await.0, "completed");
    assert_eq!(rig.item_row(&second).await.0, "expired");
    assert_eq!(rig.item_row(&third).await.0, "expired");
    assert_eq!(
        rig.stack.counts().await.held,
        0,
        "no share is lost or released twice"
    );
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(aborted(&rig).len(), 2);
    assert_eq!(row(&rig, &second).await.state, "aborted");
    assert_eq!(row(&rig, &third).await.state, "aborted");
    assert_eq!(row(&rig, &first).await.state, "completed");
    rig.stack.flush_audit().await;
    assert_eq!(audit_count(&rig, "TRANSFER_EXPIRED").await, 2);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_terminal_rows_are_kept_seven_days_after_confirmed_abort() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "retained.bin", size).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    reaper.drain().await;
    let settled = instant(&row(&rig, &item).await.completed_at.unwrap());

    let week = Duration::from_secs(7 * 24 * 60 * 60);
    rig.stack.clock.set(settled + week - MILLI);
    enqueue_raw(
        &rig,
        ABORT,
        serde_json::json!({"mode": "prune", "continuation": 0}),
        "t:prune1",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        1
    );

    rig.stack.clock.set(settled + week);
    enqueue_raw(
        &rig,
        ABORT,
        serde_json::json!({"mode": "prune", "continuation": 0}),
        "t:prune2",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        0
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts")
            .await,
        0,
        "parts cascade with the row"
    );
    assert_eq!(
        rig.item_row(&item).await.0,
        "expired",
        "the item survives its protocol row"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_unconfirmed_abort_and_completing_rows_are_never_pruned() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, stuck, _) = live_upload(&rig, "stuck.bin", size).await;
    let (session, finalizing, _) = live_upload(&rig, "fin.bin", size).await;
    rig.hooks.crash_at(FinalizePoint::BeforePlace);
    assert_ne!(
        rig.complete(&session, &finalizing).await.status,
        StatusCode::OK
    );
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    for _ in 0..8 {
        rig.faults.abort.lock().unwrap().push_back(upstream);
    }
    reaper.drain().await;
    assert_eq!(row(&rig, &stuck).await.state, "abandoned");

    rig.stack.clock.advance(DAY * 40);
    enqueue_raw(
        &rig,
        ABORT,
        serde_json::json!({"mode": "prune", "continuation": 0}),
        "t:prune",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert_eq!(row(&rig, &stuck).await.state, "abandoned");
    assert_eq!(row(&rig, &finalizing).await.state, "completing");
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_dead_lettered_targeted_job_does_not_strand_an_abandoned_row() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "stranded.bin", size).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.points.crash_at(Point::BeforeProviderAbort);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    reaper.drain().await;

    assert_eq!(
        reaper.points.hits(Point::BeforeProviderAbort),
        1,
        "the process died after the terminal commit and before the provider call"
    );
    assert_eq!(row(&rig, &item).await.state, "abandoned");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert_eq!(rig.item_row(&item).await.0, "expired");
    assert_eq!(
        jobs_of(&rig, ABORT, "dead").await,
        1,
        "the targeted job is dead-lettered"
    );

    rig.stack.clock.advance(HOUR);
    reaper.drain().await;
    assert_eq!(
        row(&rig, &item).await.state,
        "aborted",
        "the next sweep rediscovers the row"
    );
    assert_eq!(rig.open_uploads(&key).await, 0);
    assert_eq!(
        rig.stack
            .reservation(&item_session(&rig, &item).await)
            .await
            .0,
        "released"
    );
    rig.assert_no_overlap();
}

async fn item_session(rig: &S3Rig, item: &str) -> String {
    sqlx::query_scalar("SELECT transfer_session_id FROM transfer_session_files WHERE id = ?1")
        .bind(item)
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap()
}

#[tokio::test]
async fn it_both_s3_job_kinds_have_a_production_handler() {
    let rig = S3Rig::new().await;
    let reaper = Reaper::new(&rig);
    assert_eq!(reaper.kinds, [ABORT, RECONCILE]);
    reaper.startup(&rig).await;
    let scheduled: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT kind FROM jobs WHERE kind LIKE 's3.%' ORDER BY kind")
            .fetch_all(rig.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        scheduled,
        ["s3.abort_abandoned_multipart", "s3.reconcile_multipart"],
        "everything that is scheduled has a registered handler"
    );
}

#[tokio::test]
async fn it_s3_reaper_queries_are_served_by_their_indexes() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    for (name, sql, literals, index) in reapers::PLANNED_QUERIES {
        let mut statement = sql.to_owned();
        for (position, literal) in literals.iter().enumerate() {
            statement = statement.replace(&format!("?{}", position + 1), literal);
        }
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {statement}"))
                .fetch_all(stack.pools.reader().executor())
                .await
                .unwrap_or_else(|error| panic!("{name}: {error}\n{statement}"));
        let text: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
        assert!(
            text.iter().any(|line| line.contains(index)),
            "{name} must use {index}: {text:?}"
        );
        assert!(
            !text
                .iter()
                .any(|line| line.starts_with("SCAN s3_multipart_uploads")),
            "{name} must not scan its table: {text:?}"
        );
    }
    let abandoned: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN SELECT id, transfer_session_file_id FROM s3_multipart_uploads
          WHERE state = 'abandoned' AND id > '' ORDER BY id LIMIT 100",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(abandoned
        .iter()
        .any(|row| row.3.contains("ix_s3mp_abandoned")));
    stack.stop().await;
}

#[tokio::test]
async fn it_permission_denied_abort_stays_discoverable_and_never_loops() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "denied.bin", size).await;
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    for _ in 0..64 {
        rig.faults
            .abort
            .lock()
            .unwrap()
            .push_back(|| crate::storage::error::StorageError::PermissionDenied);
    }
    reaper.drain().await;
    let attempts = rig.faults.aborts.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        (1..=24).contains(&attempts),
        "the retries are bounded by the job policy, not a loop: {attempts}"
    );
    assert_eq!(row(&rig, &item).await.state, "abandoned");
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert_eq!(
        jobs_of(&rig, ABORT, "dead").await,
        0,
        "a refused call is retried, not dropped"
    );

    rig.faults.abort.lock().unwrap().clear();
    rig.stack.clock.advance(HOUR * 2);
    reaper.drain().await;
    assert_eq!(
        row(&rig, &item).await.state,
        "aborted",
        "the next sweep succeeds once permitted"
    );
    rig.assert_no_overlap();
}
