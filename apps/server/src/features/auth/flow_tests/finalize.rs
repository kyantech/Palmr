use std::collections::HashMap;
use std::io::Read as _;
use std::sync::Mutex;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::folders::{Member, HOST_A};
use super::tus::{header, pattern, small_limits, Gate, Planned, TestStaging, TusReq, TUS};
use super::tus_patch::patch;
use super::*;
use crate::app::reconcile::{ReconcileContext, ReconcileError, ReconcileRegistry, RedriveStarter};
use crate::features::transfers::finalize::{
    FinalizeHooks, FinalizeLimits, FinalizeOutcome, Finalizer, FinalizerParts, Flow, Point,
};
use crate::infra::db::InstanceId;

pub(super) enum Action {
    Crash,
    Gate(Arc<Gate>),
}

#[derive(Default)]
pub(super) struct Hooks {
    actions: Mutex<HashMap<Point, Action>>,
    hits: Mutex<Vec<Point>>,
}

impl Hooks {
    pub(super) fn crash_at(&self, point: Point) {
        self.actions.lock().unwrap().insert(point, Action::Crash);
    }

    pub(super) fn gate_at(&self, point: Point) -> Arc<Gate> {
        let gate = Gate::new();
        self.actions
            .lock()
            .unwrap()
            .insert(point, Action::Gate(Arc::clone(&gate)));
        gate
    }

    pub(super) fn hits(&self) -> Vec<Point> {
        self.hits.lock().unwrap().clone()
    }
}

#[async_trait]
impl FinalizeHooks for Hooks {
    async fn reached(&self, point: Point) -> Flow {
        self.hits.lock().unwrap().push(point);
        let action = self.actions.lock().unwrap().remove(&point);
        match action {
            Some(Action::Crash) => Flow::Crash,
            Some(Action::Gate(gate)) => {
                gate.pass().await;
                Flow::Continue
            }
            None => Flow::Continue,
        }
    }
}

pub(super) struct Rig {
    pub(super) stack: Stack,
    pub(super) staging: Arc<TestStaging>,
    pub(super) hooks: Arc<Hooks>,
    pub(super) alice: Member,
    pub(super) root: std::path::PathBuf,
    _dir: TempDir,
}

pub(super) struct Pushed {
    pub(super) planned: Planned,
    pub(super) upload: String,
}

type FileTuple = (String, String, i64, String, String, Option<String>, String);

pub(super) struct FileRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) size: i64,
    pub(super) mime: String,
    pub(super) mime_source: String,
    pub(super) folder: Option<String>,
    pub(super) object: String,
}

impl Rig {
    pub(super) async fn new() -> Self {
        Self::with_limits(small_limits(), None).await
    }

    pub(super) async fn manual() -> Self {
        Self::build(small_limits(), None, false).await
    }

    pub(super) async fn with_limits(
        tus: crate::features::transfers::TusLimits,
        finalize: Option<FinalizeLimits>,
    ) -> Self {
        Self::build(tus, finalize, true).await
    }

    async fn build(
        tus: crate::features::transfers::TusLimits,
        finalize: Option<FinalizeLimits>,
        trigger: bool,
    ) -> Self {
        let dir = TempDir::new().unwrap();
        let clock = TestClock::new(START);
        let staging = TestStaging::new(dir.path(), &clock);
        let hooks = Arc::new(Hooks::default());
        let mut setup = FinalizeSetup {
            hooks: Some(hooks.clone()),
            ..FinalizeSetup::default()
        };
        if let Some(limits) = finalize {
            setup.limits = limits;
        }
        let stack = Stack::start_with_tus(
            dir.path(),
            &clock,
            TusSetup {
                staging: Some(staging.clone()),
                limits: tus,
                finalize: trigger.then_some(setup),
                ..TusSetup::default()
            },
        )
        .await;
        *staging.probe.lock().unwrap() = Some(stack.pools.clone());
        let alice = stack.member("alice", HOST_A).await;
        Self {
            stack,
            staging,
            hooks,
            alice,
            root: dir.path().to_path_buf(),
            _dir: dir,
        }
    }

    pub(super) fn finalizer(&self) -> &Finalizer {
        self.stack.finalizer.as_ref().unwrap()
    }

    pub(super) async fn settle(&self) {
        self.finalizer().idle().await;
    }

    pub(super) async fn settle_until_item(&self, item: &str, state: &str) {
        for _ in 0..800 {
            if self.item_row(item).await.0 == state {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("item {item} never reached {state}");
    }

    pub(super) async fn restart(&self) -> Finalizer {
        let finalizer = self.second_finalizer("restarted");
        let instance = InstanceId::generate(&self.stack.clock);
        let starter = {
            let finalizer = finalizer.clone();
            RedriveStarter::new(move || {
                let finalizer = finalizer.clone();
                Box::pin(async move {
                    finalizer
                        .start_redrive()
                        .await
                        .map_err(|error| ReconcileError::new(error.kind(), error.to_string()))
                })
            })
        };
        let report = ReconcileRegistry::production()
            .run(
                &ReconcileContext::new(
                    self.stack.pools.clone(),
                    Arc::new(self.stack.clock.clone()),
                    instance,
                )
                .with_redrive(starter),
            )
            .await;
        assert_eq!(report.failed(), 0, "{report:?}");
        finalizer.idle().await;
        finalizer
    }

    pub(super) fn second_finalizer(&self, holder: &str) -> Finalizer {
        self.second_finalizer_with(holder, FinalizeSetup::default().limits)
    }

    pub(super) fn second_finalizer_with(&self, holder: &str, limits: FinalizeLimits) -> Finalizer {
        Finalizer::new(FinalizerParts {
            pools: self.stack.pools.clone(),
            clock: Arc::new(self.stack.clock.clone()),
            settings: self.stack.settings.handle(),
            storage: self.stack.transfer_storage(),
            audit: self.stack.audit.clone(),
            holder: holder.to_owned(),
            stop: tokio_util::sync::CancellationToken::new(),
            limits,
            hooks: None,
        })
    }

    pub(super) async fn plan(&self, name: &str, size: Option<u64>) -> Planned {
        self.stack.planned(&self.alice, name, size).await
    }

    pub(super) async fn push(&self, name: &str, data: &[u8]) -> Pushed {
        let planned = self.plan(name, Some(data.len() as u64)).await;
        self.push_planned(planned, data).await
    }

    pub(super) async fn push_planned(&self, planned: Planned, data: &[u8]) -> Pushed {
        let length = Some(data.len() as u64);
        let upload = self.stack.tus_created(&self.alice, &planned, length).await;
        if !data.is_empty() {
            let patched = self
                .stack
                .tus_send(patch(&self.alice, &upload, 0).bytes(data))
                .await;
            assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
            assert_eq!(header(&patched, "upload-offset"), data.len().to_string());
        }
        Pushed { planned, upload }
    }

    pub(super) async fn push_and_settle(&self, name: &str, data: &[u8]) -> Pushed {
        let pushed = self.push(name, data).await;
        self.settle().await;
        pushed
    }

    pub(super) async fn item_row(
        &self,
        item: &str,
    ) -> (String, String, Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT state, finalize_stage, resulting_file_id, error_code
               FROM transfer_session_files WHERE id = ?1",
        )
        .bind(item)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn file_rows(&self) -> Vec<FileRow> {
        let rows: Vec<FileTuple> = sqlx::query_as(
            "SELECT id, name, size_bytes, mime_type, mime_source, folder_id, storage_object_id
                   FROM files ORDER BY created_at, name",
        )
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(
                |(id, name, size, mime, mime_source, folder, object)| FileRow {
                    id,
                    name,
                    size,
                    mime,
                    mime_source,
                    folder,
                    object,
                },
            )
            .collect()
    }

    pub(super) async fn object_state(&self, item: &str) -> Option<(String, String, i64)> {
        sqlx::query_as(
            "SELECT o.state, o.object_key, o.size_bytes
               FROM storage_objects o
               JOIN transfer_session_files t ON t.final_object_id = o.id
              WHERE t.id = ?1",
        )
        .bind(item)
        .fetch_optional(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn final_key(&self, item: &str) -> String {
        sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
            .bind(item)
            .fetch_one(self.stack.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn final_path(&self, item: &str) -> std::path::PathBuf {
        self.root.join("storage").join(self.final_key(item).await)
    }

    pub(super) fn staging_exists(&self, upload: &str) -> bool {
        self.stack.staged_bytes(upload).is_some()
    }

    pub(super) fn digest_of(path: &std::path::Path) -> Option<(u64, Vec<u8>)> {
        let mut file = std::fs::File::open(path).ok()?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            let read = file.read(&mut buffer).unwrap();
            if read == 0 {
                return Some((total, hasher.finalize().to_vec()));
            }
            hasher.update(&buffer[..read]);
            total += read as u64;
        }
    }

    pub(super) fn digest(data: &[u8]) -> Vec<u8> {
        Sha256::digest(data).to_vec()
    }

    pub(super) async fn audit_uploads(&self) -> i64 {
        self.stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
            .await
    }

    pub(super) async fn used(&self) -> i64 {
        self.stack.used_bytes(self.alice.id).await
    }

    pub(super) async fn jobs_for_delete(&self) -> i64 {
        self.stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob'")
            .await
    }

    pub(super) async fn queue_rows(&self) -> i64 {
        self.stack
            .scalar_i64("SELECT COUNT(*) FROM file_deletion_queue")
            .await
    }

    pub(super) fn assert_no_overlap(&self) {
        assert!(
            !self
                .staging
                .overlapped_transaction
                .load(std::sync::atomic::Ordering::SeqCst),
            "a storage call ran while a write transaction was open"
        );
    }
}

pub(super) fn bytes(length: usize) -> Vec<u8> {
    pattern(length)
}

pub(super) fn png() -> Vec<u8> {
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    data.extend(pattern(64));
    data
}

pub(super) fn tus_head<'a>(rig: &'a Rig, id: &str) -> TusReq<'a> {
    TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &rig.alice)
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-006")]
async fn regression_R006_blob_metadata_ordering_and_measured_size() {
    let rig = Rig::new().await;
    let data = bytes(40_000);
    let pushed = rig.push("report.bin", &data).await;
    let item = pushed.planned.item.clone();
    let place = rig.hooks.gate_at(Point::BeforeCommit);

    place.wait_until_entered().await;
    let key = rig.final_path(&item).await;
    let (stage_state, stage, file, _) = rig.item_row(&item).await;
    assert_eq!(
        (stage_state.as_str(), stage.as_str(), file),
        ("finalizing", "placing", None),
        "the blob is placed and the commit is owed"
    );
    assert_eq!(
        Rig::digest_of(&key).unwrap(),
        (data.len() as u64, Rig::digest(&data)),
        "the exact bytes sit at the recorded key"
    );
    assert!(
        rig.file_rows().await.is_empty(),
        "no visible file exists while only the blob is placed"
    );
    assert!(rig.object_state(&item).await.is_none(), "no object row yet");
    assert_eq!(rig.used().await, 0, "no usage is charged before the commit");
    assert!(
        !rig.staging_exists(&pushed.upload),
        "the staging blob moved"
    );
    place.open();
    rig.settle().await;

    let rows = rig.file_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].size, 40_000,
        "the committed size is the measured size"
    );
    assert_eq!(rig.used().await, 40_000);
    let (state, key_state, size) = rig.object_state(&item).await.unwrap();
    assert_eq!(
        (state.as_str(), key_state.as_str(), size),
        ("active", rig.final_key(&item).await.as_str(), 40_000)
    );
    let (row_state, stage, file, _) = rig.item_row(&item).await;
    assert_eq!(
        (row_state.as_str(), stage.as_str(), file.as_deref()),
        ("completed", "committed", Some(rows[0].id.as_str()))
    );
    rig.assert_no_overlap();
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-006")]
async fn regression_R006_a_database_failure_leaves_no_visible_file_and_retries_once() {
    let rig = Rig::new().await;
    let data = bytes(10_000);
    rig.stack
        .execute(
            "CREATE TRIGGER fail_file_insert BEFORE INSERT ON files
             BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END",
        )
        .await;
    let pushed = rig.push("a.bin", &data).await;
    let item = pushed.planned.item.clone();
    rig.settle().await;

    assert!(rig.file_rows().await.is_empty(), "the commit rolled back");
    assert!(
        rig.object_state(&item).await.is_none(),
        "no object row survived"
    );
    assert_eq!(rig.used().await, 0, "no quota was charged");
    let (state, stage, _, _) = rig.item_row(&item).await;
    assert_eq!((state.as_str(), stage.as_str()), ("finalizing", "placing"));
    assert_eq!(
        Rig::digest_of(&rig.final_path(&item).await).unwrap().0,
        10_000,
        "the placed blob is owned by the durable intent"
    );

    rig.stack.execute("DROP TRIGGER fail_file_insert").await;
    let outcome = rig
        .finalizer()
        .finalize(item.parse().unwrap(), None)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        FinalizeOutcome::Committed { created: true, .. }
    ));
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(rig.used().await, 10_000, "charged exactly once");
    assert_eq!(rig.audit_uploads().await, 1);
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-006")]
async fn regression_R006_a_client_declared_size_cannot_replace_the_measured_size() {
    let rig = Rig::new().await;
    let data = bytes(5_000);
    let planned = rig.plan("lie.bin", Some(5_000)).await;
    let upload = rig
        .stack
        .tus_created(&rig.alice, &planned, Some(5_000))
        .await;
    let patched = rig
        .stack
        .tus_send(patch(&rig.alice, &upload, 0).bytes(&data[..3_000]))
        .await;
    assert_eq!(patched.status, StatusCode::NO_CONTENT);
    let blob = rig.stack.staged_bytes(&upload).unwrap();
    assert_eq!(blob.len(), 3_000);
    rig.stack
        .execute(&format!(
            "UPDATE tus_uploads SET upload_offset = 5000 WHERE id = '{upload}'"
        ))
        .await;
    rig.finalizer()
        .finalize(planned.item.parse().unwrap(), None)
        .await
        .unwrap();

    assert!(
        rig.file_rows().await.is_empty(),
        "a short blob never becomes a file"
    );
    assert_eq!(rig.used().await, 0);
    assert!(rig.object_state(&planned.item).await.is_none());
    let (state, _, _, code) = rig.item_row(&planned.item).await;
    assert_eq!(state, "failed");
    assert_eq!(code.as_deref(), Some("UPLOAD_LENGTH_MISMATCH"));
}

#[tokio::test]
async fn it_finalize_idempotent_same_file_id() {
    let rig = Rig::new().await;
    let data = bytes(12_345);
    let pushed = rig.push_and_settle("same.bin", &data).await;
    let item = pushed.planned.item.clone();
    let files = rig.file_rows().await;
    assert_eq!(files.len(), 1);
    let before = (
        rig.used().await,
        rig.audit_uploads().await,
        rig.stack.counts().await.objects,
        rig.reservation_of(&pushed.planned).await,
    );
    let placed_before = Rig::digest_of(&rig.final_path(&item).await).unwrap();

    for _ in 0..3 {
        let outcome = rig
            .finalizer()
            .finalize(item.parse().unwrap(), None)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            FinalizeOutcome::Committed {
                file_id: files[0].id.clone(),
                created: false
            }
        );
    }
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(rig.file_rows().await[0].object, files[0].object);
    assert_eq!(
        before,
        (
            rig.used().await,
            rig.audit_uploads().await,
            rig.stack.counts().await.objects,
            rig.reservation_of(&pushed.planned).await,
        ),
        "no second charge, audit event, object or settlement"
    );
    assert_eq!(
        placed_before,
        Rig::digest_of(&rig.final_path(&item).await).unwrap(),
        "the placed blob was not overwritten"
    );
}

impl Rig {
    pub(super) async fn reservation_of(&self, planned: &Planned) -> (String, i64, Option<i64>) {
        let (state, reserved, committed, _) = self.stack.reservation(&planned.session).await;
        (state, reserved, committed)
    }
}

#[tokio::test]
async fn it_storage_object_created_only_at_finalization() {
    let rig = Rig::new().await;
    let data = bytes(30_000);
    let planned = rig.plan("late.bin", Some(30_000)).await;
    let item = planned.item.clone();
    assert!(rig.object_state(&item).await.is_none(), "not at admission");

    let upload = rig
        .stack
        .tus_created(&rig.alice, &planned, Some(30_000))
        .await;
    assert!(
        rig.object_state(&item).await.is_none(),
        "not at TUS creation"
    );
    assert_eq!(rig.stack.counts().await.objects, 0);

    let partial = rig
        .stack
        .tus_send(patch(&rig.alice, &upload, 0).bytes(&data[..10_000]))
        .await;
    assert_eq!(partial.status, StatusCode::NO_CONTENT);
    assert_eq!(
        rig.stack.counts().await.objects,
        0,
        "not after a partial PATCH"
    );

    let intent = rig.hooks.gate_at(Point::IntentCommitted);
    let placed = rig.hooks.gate_at(Point::AfterPlace);
    let full = rig
        .stack
        .tus_send(patch(&rig.alice, &upload, 10_000).bytes(&data[10_000..]))
        .await;
    assert_eq!(full.status, StatusCode::NO_CONTENT);

    intent.wait_until_entered().await;
    assert_eq!(
        rig.item_row(&item).await.1,
        "placing",
        "the intent stage is durable"
    );
    assert_eq!(
        rig.stack.counts().await.objects,
        0,
        "not at the intent stage"
    );
    intent.open();
    placed.wait_until_entered().await;
    assert!(rig.final_path(&item).await.exists());
    assert_eq!(
        rig.stack.counts().await.objects,
        0,
        "not at the placement stage"
    );
    placed.open();
    rig.settle().await;

    let counts = rig.stack.counts().await;
    assert_eq!(
        counts.objects, 1,
        "exactly one active object after the commit"
    );
    let (state, _, size) = rig.object_state(&item).await.unwrap();
    assert_eq!((state.as_str(), size), ("active", 30_000));
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT refcount FROM storage_objects")
            .await,
        1
    );
}
