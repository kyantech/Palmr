use tokio_util::sync::CancellationToken;

use super::finalize::Rig;
use super::transfers::sized;
use super::tus::{header, pattern, Planned};
use super::tus_patch::patch;
use super::*;
use crate::app::reconcile::{ReconcileContext, ReconcileRegistry};
use crate::domain::clock::Clock;
use crate::features::transfers::finalize::Point;
use crate::features::transfers::model::SESSION_TTL;
use crate::features::transfers::reaper::{self, ReapReport, ReaperContext};
use crate::infra::db::InstanceId;
use crate::infra::jobs::{Claimant, Dispatcher, Jitter, JobAudit, JobKind, Registry};
use crate::storage::lifecycle::{self as storage_lifecycle, thumbnails::ThumbnailCache};
use crate::storage::provider::StorageProvider;
use crate::storage::staging::StagingStorage;

pub(super) const HOUR: Duration = Duration::from_secs(60 * 60);
pub(super) const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const MILLI: Duration = Duration::from_millis(1);

pub(super) fn context(rig: &Rig) -> ReaperContext {
    let staging: Arc<dyn StagingStorage> = rig.staging.clone();
    ReaperContext::new(
        rig.stack.pools.clone(),
        Arc::new(rig.stack.clock.clone()),
        rig.stack.settings.handle(),
        rig.stack.audit.clone(),
        Some(staging),
        CancellationToken::new(),
    )
}

pub(super) async fn pass(rig: &Rig) -> ReapReport {
    reaper::run_pass(&context(rig)).await.unwrap()
}

pub(super) struct Jobs {
    dispatcher: Dispatcher,
    claimant: Claimant,
}

impl Jobs {
    pub(super) fn new(rig: &Rig) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(rig.stack.clock.clone());
        let provider: Arc<dyn StorageProvider> =
            Arc::new(crate::storage::local::LocalProvider::open(&rig.root, 262_144).unwrap());
        let registry = storage_lifecycle::register_jobs(
            reaper::register_jobs(Registry::production(), context(rig)),
            storage_lifecycle::LifecycleContext::new(
                rig.stack.pools.clone(),
                Arc::clone(&clock),
                provider,
                ThumbnailCache::under(&rig.root),
                rig.stack.audit.clone(),
                false,
            ),
        );
        let dispatcher = Dispatcher::new(
            rig.stack.pools.clone(),
            Arc::clone(&clock),
            registry,
            Jitter::from_fn(|| u32::MAX / 2),
            JobAudit::new(Arc::new(rig.stack.audit.clone())),
            Duration::from_secs(60),
        );
        Self {
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
}

pub(super) async fn startup_reconcile(rig: &Rig) {
    let report = ReconcileRegistry::production()
        .run(&ReconcileContext::new(
            rig.stack.pools.clone(),
            Arc::new(rig.stack.clock.clone()),
            InstanceId::generate(&rig.stack.clock),
        ))
        .await;
    assert_eq!(report.failed(), 0, "{report:?}");
}

pub(super) struct Upload {
    pub(super) planned: Planned,
    pub(super) id: String,
}

pub(super) async fn start_upload(rig: &Rig, name: &str, length: u64, sent: usize) -> Upload {
    let planned = rig.plan(name, Some(length)).await;
    let id = rig
        .stack
        .tus_created(&rig.alice, &planned, Some(length))
        .await;
    if sent > 0 {
        let patched = rig
            .stack
            .tus_send(patch(&rig.alice, &id, 0).bytes(&pattern(sent)))
            .await;
        assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
        assert_eq!(header(&patched, "upload-offset"), sent.to_string());
    }
    Upload { planned, id }
}

pub(super) async fn start_in_session(
    rig: &Rig,
    session: &str,
    item: &str,
    name: &str,
    length: u64,
    sent: usize,
) -> String {
    let planned = Planned {
        session: session.to_owned(),
        item: item.to_owned(),
        name: name.to_owned(),
    };
    let id = rig
        .stack
        .tus_created(&rig.alice, &planned, Some(length))
        .await;
    if sent > 0 {
        let patched = rig
            .stack
            .tus_send(patch(&rig.alice, &id, 0).bytes(&pattern(sent)))
            .await;
        assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
    }
    id
}

pub(super) async fn tus_marker(rig: &Rig, upload: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, staging_removed_at FROM tus_uploads WHERE id = ?1")
        .bind(upload)
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap()
}

pub(super) async fn job_count(rig: &Rig, kind: JobKind) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = ?1")
        .bind(kind.as_str())
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap()
}

#[tokio::test]
async fn it_tus_expire_stale_idempotent() {
    let rig = Rig::manual().await;
    let clock = rig.stack.clock.clone();
    let upload = start_upload(&rig, "a.bin", 1_000, 400).await;
    let session = &upload.planned.session;
    assert_eq!(rig.stack.reservation(session).await.1, 1_000);
    assert!(rig.stack.staged_bytes(&upload.id).is_some());

    clock.advance(DAY - MILLI);
    let early = pass(&rig).await;
    assert_eq!(
        early,
        ReapReport::default(),
        "one millisecond before the boundary"
    );
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "in_progress");

    clock.advance(MILLI);
    let due = pass(&rig).await;
    assert_eq!(due.uploads_expired, 1, "{due:?}");
    assert_eq!(due.staging_removed, 1, "{due:?}");
    assert_eq!(due.errors, 0);
    let (state, removed) = tus_marker(&rig, &upload.id).await;
    assert_eq!(state, "expired");
    assert!(removed.is_some());
    assert!(rig.stack.staged_bytes(&upload.id).is_none());
    assert_eq!(rig.stack.item_state(&upload.planned.item).await, "expired");
    let (reservation, held, _, reason) = rig.stack.reservation(session).await;
    assert_eq!(reservation, "released");
    assert_eq!(reason.as_deref(), Some("expired"), "{held}");
    assert_eq!(rig.stack.session_row(session).await.0, "expired");
    assert_eq!(rig.stack.counts().await.held, 0);

    let settled = rig.stack.dump().await;
    let again = pass(&rig).await;
    assert_eq!(again, ReapReport::default(), "a repeated pass is a no-op");
    assert_eq!(rig.stack.dump().await, settled);
    clock.advance(HOUR);
    assert_eq!(pass(&rig).await, ReapReport::default());
    assert_eq!(rig.stack.dump().await, settled);

    let jobs = Jobs::new(&rig);
    for _ in 0..2 {
        reaper::ensure_scheduled(&rig.stack.pools, &rig.stack.clock)
            .await
            .unwrap();
    }
    assert_eq!(
        job_count(&rig, JobKind::TusExpireStale).await,
        1,
        "scheduling twice yields one recurring chain"
    );
    assert_eq!(jobs.run_all(JobKind::TusExpireStale).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'tus.expire_stale' AND state = 'pending'"
        )
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap(),
        1,
        "the run reschedules exactly one successor"
    );
    assert_eq!(jobs.run_all(JobKind::TusExpireStale).await, 0);
    assert_eq!(rig.stack.dump().await.len(), settled.len());
}

#[tokio::test]
async fn it_active_upload_never_expires() {
    let rig = Rig::manual().await;
    let clock = rig.stack.clock.clone();
    let planned = rig.plan("big.bin", Some(10_000)).await;
    let id = rig
        .stack
        .tus_created(&rig.alice, &planned, Some(10_000))
        .await;
    let mut offset = 0_u64;
    for step in 0..6 {
        let patched = rig
            .stack
            .tus_send(patch(&rig.alice, &id, offset).bytes(&pattern(100)))
            .await;
        assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
        offset += 100;
        clock.advance(23 * HOUR);
        let report = pass(&rig).await;
        assert_eq!(
            (report.uploads_expired, report.sessions_expired),
            (0, 0),
            "step {step}: {report:?}"
        );
        assert_eq!(tus_marker(&rig, &id).await.0, "in_progress");
    }
    assert!(clock.now() - START > time::Duration::days(5));

    let head = rig
        .stack
        .tus_send(super::tus::TusReq::new(
            Method::HEAD,
            &format!("{}/{id}", super::tus::TUS),
            &rig.alice,
        ))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(header(&head, "upload-offset"), "600");

    clock.advance(SESSION_TTL);
    let outer = pass(&rig).await;
    assert_eq!(
        (outer.sessions_expired, outer.staging_removed),
        (1, 1),
        "the seven-day session bound ends even a progressing upload: {outer:?}"
    );
    assert_eq!(tus_marker(&rig, &id).await.0, "expired");
    assert_eq!(rig.stack.session_row(&planned.session).await.0, "expired");
}

#[tokio::test]
async fn it_cancel_releases_reservation() {
    let rig = Rig::manual().await;
    let alice = &rig.alice;
    rig.stack.set_quota(alice.id, Some(1_000)).await;

    let first = rig
        .stack
        .opened(alice, &[sized("a", "a.bin", 500), sized("b", "b.bin", 300)])
        .await;
    let first_id = first["id"].as_str().unwrap().to_owned();
    assert_eq!(rig.stack.counts().await.held, 800);

    let refused = rig
        .stack
        .open(
            alice,
            &super::transfers::request_body(None, &[sized("c", "c.bin", 500)]),
        )
        .await;
    assert_eq!(
        refused.status,
        StatusCode::INSUFFICIENT_STORAGE,
        "{}",
        refused.text()
    );

    let items = rig.stack.item_ids(&first_id).await;
    let item = rig.stack.cancel_item(alice, &first_id, &items[1]).await;
    assert_eq!(item.status, StatusCode::NO_CONTENT);
    assert_eq!(
        rig.stack.reservation(&first_id).await.1,
        500,
        "an item cancel releases only its own share"
    );
    assert_eq!(
        rig.stack.item_states(&first_id).await,
        ["pending", "canceled"]
    );

    let canceled = rig.stack.cancel_session(alice, &first_id).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    let (state, _, committed, reason) = rig.stack.reservation(&first_id).await;
    assert_eq!(
        (state.as_str(), committed, reason.as_deref()),
        ("released", None, Some("canceled"))
    );
    assert_eq!(rig.stack.counts().await.held, 0);
    assert_eq!(rig.stack.used_bytes(alice.id).await, 0);

    let admitted = rig
        .stack
        .open(
            alice,
            &super::transfers::request_body(None, &[sized("c", "c.bin", 900)]),
        )
        .await;
    assert_eq!(
        admitted.status,
        StatusCode::CREATED,
        "reclaimed quota admits a fresh session: {}",
        admitted.text()
    );

    let settled = rig.stack.dump().await;
    assert_eq!(
        rig.stack.cancel_session(alice, &first_id).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        rig.stack.dump().await,
        settled,
        "a repeated cancel settles nothing twice"
    );
}

#[tokio::test]
async fn it_startup_reconcile_releases_expired_holds() {
    let rig = Rig::manual().await;
    let clock = rig.stack.clock.clone();
    let alice = &rig.alice;
    rig.stack.set_quota(alice.id, Some(10_000)).await;

    let stale = start_upload(&rig, "stale.bin", 1_000, 100).await;
    assert_eq!(rig.stack.counts().await.held, 1_000);
    clock.advance(6 * DAY + 12 * HOUR);
    let live = start_upload(&rig, "live.bin", 2_000, 200).await;
    assert_eq!(rig.stack.counts().await.held, 3_000);
    clock.advance(12 * HOUR);
    clock.advance(Duration::from_secs(3_600));

    assert_eq!(
        rig.stack.session_row(&stale.planned.session).await.0,
        "uploading"
    );
    let before_live = rig.stack.dump().await;

    startup_reconcile(&rig).await;
    assert_eq!(
        job_count(&rig, JobKind::TusExpireStale).await,
        1,
        "startup only schedules the reaper"
    );
    assert_eq!(
        rig.stack.dump().await.len(),
        before_live.len(),
        "startup reconciliation itself changes no transfer row"
    );
    assert_eq!(
        rig.stack.session_row(&stale.planned.session).await.0,
        "uploading"
    );

    let jobs = Jobs::new(&rig);
    assert!(jobs.run_all(JobKind::TusExpireStale).await >= 1);
    let (state, _, _, reason) = rig.stack.reservation(&stale.planned.session).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("released", Some("expired"))
    );
    assert_eq!(
        rig.stack.session_row(&stale.planned.session).await.0,
        "expired"
    );
    assert_eq!(rig.stack.item_state(&stale.planned.item).await, "expired");
    assert!(rig.stack.staged_bytes(&stale.id).is_none());

    let (live_state, live_held, _, _) = rig.stack.reservation(&live.planned.session).await;
    assert_eq!((live_state.as_str(), live_held), ("held", 2_000));
    assert_eq!(rig.stack.item_state(&live.planned.item).await, "uploading");
    assert_eq!(tus_marker(&rig, &live.id).await.0, "in_progress");
    assert_eq!(rig.stack.staged_bytes(&live.id).unwrap().len(), 200);
    assert_eq!(rig.stack.counts().await.held, 2_000);

    startup_reconcile(&rig).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'tus.expire_stale' AND state = 'pending'"
        )
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap(),
        1,
        "a second startup never starts a second chain"
    );
}

#[tokio::test]
async fn it_abandoned_placed_object_tombstone_inserted() {
    let rig = Rig::new().await;
    let clock = rig.stack.clock.clone();
    let gate = rig.hooks.gate_at(Point::AfterPlace);
    let data = pattern(300);
    let pushed = rig.push("placed.bin", &data).await;
    gate.wait_until_entered().await;
    let item = pushed.planned.item.clone();
    let session = pushed.planned.session.clone();
    assert_eq!(rig.item_row(&item).await.1, "placing");
    let key = rig.final_key(&item).await;
    assert!(rig.final_path(&item).await.exists(), "the bytes are placed");
    assert!(
        rig.object_state(&item).await.is_none(),
        "but no object row yet"
    );

    clock.advance(SESSION_TTL + Duration::from_secs(1));
    let expiry = pass(&rig).await;
    assert_eq!(expiry.sessions_expired, 1, "{expiry:?}");

    let (state, stored_key, _) = rig.object_state(&item).await.unwrap();
    assert_eq!(
        (state.as_str(), stored_key.as_str()),
        ("tombstoned", key.as_str())
    );
    let queued: (String, String) = sqlx::query_as(
        "SELECT q.reason, q.state FROM file_deletion_queue q
           JOIN transfer_session_files t ON t.final_object_id = q.storage_object_id
          WHERE t.id = ?1",
    )
    .bind(&item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        queued,
        ("upload_abandoned".to_owned(), "pending".to_owned())
    );
    assert_eq!(job_count(&rig, JobKind::StorageDeleteBlob).await, 1);
    let (item_state, stage, file, _) = rig.item_row(&item).await;
    assert_eq!(
        (item_state.as_str(), stage.as_str(), file),
        ("expired", "placing", None)
    );
    assert!(
        rig.file_rows().await.is_empty(),
        "no incomplete files row is exposed"
    );
    let (reservation, _, _, reason) = rig.stack.reservation(&session).await;
    assert_eq!(
        (reservation.as_str(), reason.as_deref()),
        ("released", Some("expired"))
    );
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 0);

    gate.open();
    rig.settle().await;
    assert!(
        rig.file_rows().await.is_empty(),
        "the finalizer cannot commit afterwards"
    );
    assert_eq!(
        rig.item_row(&item).await.0,
        "expired",
        "no terminal-state resurrection"
    );
    assert_eq!(rig.object_state(&item).await.unwrap().0, "tombstoned");
    assert_eq!(rig.audit_uploads().await, 0);
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 0);
    assert_eq!(
        job_count(&rig, JobKind::StorageDeleteBlob).await,
        1,
        "one deletion job"
    );

    let jobs = Jobs::new(&rig);
    clock.advance(2 * HOUR);
    assert_eq!(jobs.run_all(JobKind::StorageDeleteBlob).await, 1);
    assert!(
        !rig.final_path(&item).await.exists(),
        "the placed bytes are removed"
    );
    assert_eq!(rig.object_state(&item).await.unwrap().0, "deleted");
}

#[tokio::test]
async fn it_cancel_keeps_completed_items() {
    let rig = Rig::manual().await;
    let clock = rig.stack.clock.clone();
    let alice = &rig.alice;
    rig.stack.set_quota(alice.id, Some(10_000)).await;

    let created = rig
        .stack
        .opened(
            alice,
            &[
                sized("c1", "done.bin", 100),
                sized("c2", "live.bin", 200),
                sized("c3", "idle.bin", 300),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    let file = rig
        .stack
        .seed_completed_item(alice.id, &items[0], 1, 100)
        .await;
    let upload = start_in_session(&rig, &session, &items[1], "live.bin", 200, 50).await;
    assert_eq!(rig.stack.used_bytes(alice.id).await, 100);

    clock.advance(SESSION_TTL);
    let report = pass(&rig).await;
    assert_eq!(report.sessions_expired, 1, "{report:?}");

    assert_eq!(
        rig.stack.item_states(&session).await,
        ["completed", "expired", "expired"]
    );
    assert_eq!(
        rig.stack
            .scalar_i64(&format!("SELECT COUNT(*) FROM files WHERE id = '{file}'"))
            .await,
        1,
        "the completed file survives the expiry of its session"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await,
        1
    );
    assert_eq!(rig.stack.used_bytes(alice.id).await, 100);
    let (state, _, committed, reason) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), committed, reason),
        ("committed", Some(100), None)
    );
    assert_eq!(rig.stack.counts().await.held, 0);
    assert_eq!(rig.stack.session_row(&session).await.0, "expired");
    assert_eq!(tus_marker(&rig, &upload).await.0, "expired");
    assert!(rig.stack.staged_bytes(&upload).is_none());

    clock.advance(31 * DAY);
    let retained = pass(&rig).await;
    assert_eq!(retained.errors, 0, "{retained:?}");
    assert_eq!(
        rig.stack
            .scalar_i64(&format!("SELECT COUNT(*) FROM files WHERE id = '{file}'"))
            .await,
        1,
        "history pruning never touches completed content"
    );
    assert_eq!(rig.stack.used_bytes(alice.id).await, 100);
}
