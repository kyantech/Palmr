use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tempfile::TempDir;
use time::macros::datetime;
use time::OffsetDateTime;

use super::delete_blob::{
    delete_object, DeleteOutcome, CODE_INVALID_KEY, CODE_NOT_TOMBSTONED, CODE_PROVIDER_MISMATCH,
};
use super::sweep::{
    reap_deleted_rows, refuses_reap, sweep, SweepOutcome, SweepReport, DELETED_ROW_RETENTION,
    ORPHAN_MIN_AGE, SWEEP_PAGE_SIZE,
};
use super::thumbnails::ThumbnailCache;
use super::{
    dedup_key, register_jobs, tombstone, tombstone_uncommitted, DeletionReason, LifecycleContext,
    LifecycleError, PlacedObject, StorageObjectId, TombstoneReport, UncommittedOutcome,
    ABANDONED_UPLOAD_GRACE, MAX_TOMBSTONE_BATCH,
};
use crate::config::SqliteSynchronous;
use crate::domain::clock::{Clock, TestClock};
use crate::domain::time::Timestamp;
use crate::features::audit::service::{AuditDrain, AuditService};
use crate::features::audit::{self, AUDIT_BATCH_MAX};
use crate::infra::db::{DbPools, InstanceId, MIGRATOR};
use crate::infra::jobs::backoff::RETRY_CAP;
use crate::infra::jobs::claim::{claim, sweep_expired_leases};
use crate::infra::jobs::kinds::DEFAULT_LEASE;
use crate::infra::jobs::runtime::Outcome;
use crate::infra::jobs::{Claimant, Dispatcher, Jitter, JobAudit, JobKind, Registry};
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::{Retryable, StorageError};
use crate::storage::health::{ProbeDepth, SelfTestReport};
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::local::LocalProvider;
use crate::storage::provider::{
    ListCursor, ListEntry, ListPage, ObjectBody, ObjectStat, PutHint, StorageDescriptor,
    StorageProvider, MAX_LIST_PAGE_SIZE,
};
use crate::storage::ProviderKind;

const START: OffsetDateTime = datetime!(2026-09-25 12:00 UTC);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const SECRET: &str = "palmr-lifecycle-secret-sentinel";
const USER: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d";
const SESSION: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7e";
const TRANSFER_FILE: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7f";

enum Scripted {
    Fail(fn() -> StorageError),
    PanicBeforeRemove,
    RemoveThenPanic,
}

struct MemoryProvider {
    kind: ProviderKind,
    objects: Mutex<BTreeMap<String, (u64, OffsetDateTime)>>,
    script: Mutex<VecDeque<Scripted>>,
    deletes: Mutex<Vec<String>>,
    largest_page_request: AtomicU64,
    writer_probe: Mutex<Option<(DbPools, TestClock)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MemoryProvider {
    fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            objects: Mutex::new(BTreeMap::new()),
            script: Mutex::new(VecDeque::new()),
            deletes: Mutex::new(Vec::new()),
            largest_page_request: AtomicU64::new(0),
            writer_probe: Mutex::new(None),
        }
    }

    fn put(&self, key: &str, size: u64, modified_at: OffsetDateTime) {
        lock(&self.objects).insert(key.to_owned(), (size, modified_at));
    }

    fn has(&self, key: &str) -> bool {
        lock(&self.objects).contains_key(key)
    }

    fn remove(&self, key: &str) -> bool {
        lock(&self.objects).remove(key).is_some()
    }

    fn script(&self, step: Scripted) {
        lock(&self.script).push_back(step);
    }

    fn deletes(&self) -> Vec<String> {
        lock(&self.deletes).clone()
    }

    async fn assert_writer_free(&self) {
        let probe = lock(&self.writer_probe).clone();
        if let Some((pools, clock)) = probe {
            let acquired = tokio::time::timeout(
                Duration::from_secs(5),
                pools.write_tx(&clock, "test.writer_probe", async |tx| {
                    sqlx::query("SELECT 1").execute(tx.executor()).await?;
                    Ok::<(), crate::infra::db::DbError>(())
                }),
            )
            .await;
            assert!(
                matches!(acquired, Ok(Ok(()))),
                "provider.delete ran while a write transaction held the single writer"
            );
        }
    }
}

#[async_trait]
impl StorageProvider for MemoryProvider {
    fn caps(&self) -> &StorageCapabilities {
        &StorageCapabilities::LOCAL
    }

    fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: self.kind,
            local: None,
        }
    }

    async fn put_stream(
        &self,
        _key: &ObjectKey,
        _body: ObjectBody,
        _hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        panic!("the deletion lifecycle never writes objects")
    }

    async fn open_read(&self, _key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        panic!("the deletion lifecycle never reads objects")
    }

    async fn open_range(
        &self,
        _key: &ObjectKey,
        _start: u64,
        _len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        panic!("the deletion lifecycle never reads objects")
    }

    async fn stat(&self, _key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        panic!("the deletion lifecycle never stats objects")
    }

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.assert_writer_free().await;
        lock(&self.deletes).push(key.as_str().to_owned());
        let step = lock(&self.script).pop_front();
        match step {
            Some(Scripted::Fail(error)) => Err(error()),
            Some(Scripted::PanicBeforeRemove) => panic!("injected crash before provider delete"),
            Some(Scripted::RemoveThenPanic) => {
                self.remove(key.as_str());
                panic!("injected crash after provider delete")
            }
            None => Ok(self.remove(key.as_str())),
        }
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        Ok(self.has(key.as_str()))
    }

    async fn copy(&self, _src: &ObjectKey, _dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        panic!("the deletion lifecycle never copies objects")
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        self.largest_page_request
            .fetch_max(u64::from(page_size), Ordering::Relaxed);
        let limit = usize::try_from(page_size.min(MAX_LIST_PAGE_SIZE)).unwrap_or(0);
        let objects = lock(&self.objects);
        let after = cursor.map(|cursor| cursor.as_str().to_owned());
        let mut entries: Vec<ListEntry> = objects
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .filter(|(key, _)| {
                after
                    .as_ref()
                    .is_none_or(|after| key.as_str() > after.as_str())
            })
            .take(limit + 1)
            .map(|(key, (size, modified_at))| ListEntry {
                key: key.clone(),
                size: *size,
                modified_at: *modified_at,
            })
            .collect();
        let next = if entries.len() > limit {
            entries.truncate(limit);
            entries
                .last()
                .map(|entry| ListCursor::new(entry.key.clone()))
        } else {
            None
        };
        Ok(ListPage { entries, next })
    }

    async fn self_test(&self, _depth: ProbeDepth) -> SelfTestReport {
        panic!("the deletion lifecycle never self-tests")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectRow {
    state: String,
    refcount: i64,
    size_bytes: i64,
    finalized_at: Option<String>,
    tombstoned_at: Option<String>,
    deleted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueRow {
    reason: String,
    state: String,
    attempts: i64,
    not_before: String,
    completed_at: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobRow {
    state: String,
    attempts: i64,
    run_at: String,
    payload: String,
    last_error: Option<String>,
}

struct Harness {
    root: TempDir,
    pools: DbPools,
    clock: TestClock,
    memory: Arc<MemoryProvider>,
    provider: Arc<dyn StorageProvider>,
    audit: AuditService,
    drain: AuditDrain,
    claimant: Claimant,
}

impl Harness {
    async fn open() -> Self {
        Self::with_provider(ProviderKind::Local).await
    }

    async fn with_provider(kind: ProviderKind) -> Self {
        let memory = Arc::new(MemoryProvider::new(kind));
        let provider: Arc<dyn StorageProvider> = memory.clone();
        Self::build(memory, provider).await
    }

    async fn with_local_disk() -> (Self, Arc<LocalProvider>) {
        let memory = Arc::new(MemoryProvider::new(ProviderKind::Local));
        let local = Arc::new(LocalProvider::temporary());
        let provider: Arc<dyn StorageProvider> = local.clone();
        (Self::build(memory, provider).await, local)
    }

    async fn build(memory: Arc<MemoryProvider>, provider: Arc<dyn StorageProvider>) -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let clock = TestClock::new(START);
        let (audit, drain) = audit::channel(64, pools.clone(), Arc::new(clock.clone()));
        *lock(&memory.writer_probe) = Some((pools.clone(), clock.clone()));
        let claimant = Claimant::worker(InstanceId::generate(&clock), 0);
        let harness = Self {
            root,
            pools,
            clock,
            memory,
            provider,
            audit,
            drain,
            claimant,
        };
        harness.seed_user().await;
        harness
    }

    fn shared_clock(&self) -> Arc<dyn Clock> {
        Arc::new(self.clock.clone())
    }

    fn thumbnails(&self) -> ThumbnailCache {
        ThumbnailCache::under(self.root.path())
    }

    fn context(&self, orphan_reap: bool) -> LifecycleContext {
        LifecycleContext::new(
            self.pools.clone(),
            self.shared_clock(),
            Arc::clone(&self.provider),
            self.thumbnails(),
            self.audit.clone(),
            orphan_reap,
        )
    }

    fn dispatcher(&self, orphan_reap: bool) -> Dispatcher {
        Dispatcher::new(
            self.pools.clone(),
            self.shared_clock(),
            register_jobs(Registry::production(), self.context(orphan_reap)),
            Jitter::from_fn(|| u32::MAX / 2),
            JobAudit::new(Arc::new(self.audit.clone())),
            Duration::from_secs(60),
        )
    }

    async fn run_next(&self, kind: JobKind, orphan_reap: bool) -> Option<Outcome> {
        self.dispatcher(orphan_reap)
            .run_next_kind(&self.claimant, kind, || true)
            .await
            .unwrap()
    }

    fn now(&self) -> String {
        Timestamp::try_from(self.clock.now()).unwrap().to_string()
    }

    async fn exec(&self, sql: &str, binds: &[&str]) {
        let mut query = sqlx::query(sql);
        for bind in binds {
            query = query.bind(*bind);
        }
        query.execute(self.pools.reader().executor()).await.unwrap();
    }

    async fn count(&self, sql: &str, bind: &str) -> i64 {
        sqlx::query_scalar(sql)
            .bind(bind)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn seed_user(&self) {
        self.exec(
            "INSERT INTO users (id, email, email_normalized, username, username_normalized,
                                created_at, updated_at)
             VALUES (?1, 'alice@example.test', 'alice@example.test', 'alice', 'alice', ?2, ?2)",
            &[USER, &self.now()],
        )
        .await;
    }

    async fn insert_row(&self, key: &ObjectKey, state: &str, size: u64) -> StorageObjectId {
        let id = StorageObjectId::generate(&self.clock);
        let now = self.now();
        let (refcount, finalized, tombstoned) = match state {
            "active" => (1, Some(now.as_str()), None),
            _ => (0, None, Some(now.as_str())),
        };
        sqlx::query(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                                          created_at, updated_at, finalized_at, tombstoned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)",
        )
        .bind(id.to_string())
        .bind(key.as_str())
        .bind(self.provider.describe().provider.as_str())
        .bind(i64::try_from(size).unwrap())
        .bind(state)
        .bind(refcount)
        .bind(&now)
        .bind(finalized)
        .bind(tombstoned)
        .execute(self.pools.reader().executor())
        .await
        .unwrap();
        id
    }

    async fn active_object(&self, size: u64) -> (StorageObjectId, ObjectKey) {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        self.memory.put(key.as_str(), size, self.clock.now());
        (self.insert_row(&key, "active", size).await, key)
    }

    async fn visible_file(&self, id: StorageObjectId, name: &str) -> String {
        let file = StorageObjectId::generate(&self.clock).to_string();
        self.exec(
            "INSERT INTO files (id, owner_id, storage_object_id, name, name_normalized, size_bytes,
                                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4, 0, ?5, ?5)",
            &[&file, USER, &id.to_string(), name, &self.now()],
        )
        .await;
        file
    }

    async fn tombstone(&self, ids: &[StorageObjectId], reason: DeletionReason) -> TombstoneReport {
        self.pools
            .write_tx(&self.clock, "test.tombstone", async |tx| {
                tombstone(tx, &self.clock, ids, reason).await
            })
            .await
            .unwrap()
    }

    async fn object(&self, id: StorageObjectId) -> Option<ObjectRow> {
        sqlx::query_as::<
            _,
            (
                String,
                i64,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT state, refcount, size_bytes, finalized_at, tombstoned_at, deleted_at
               FROM storage_objects WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(self.pools.reader().executor())
        .await
        .unwrap()
        .map(
            |(state, refcount, size_bytes, finalized_at, tombstoned_at, deleted_at)| ObjectRow {
                state,
                refcount,
                size_bytes,
                finalized_at,
                tombstoned_at,
                deleted_at,
            },
        )
    }

    async fn state(&self, id: StorageObjectId) -> String {
        self.object(id).await.unwrap().state
    }

    async fn queue(&self, id: StorageObjectId) -> Option<QueueRow> {
        sqlx::query_as::<_, (String, String, i64, String, Option<String>, Option<String>)>(
            "SELECT reason, state, attempts, not_before, completed_at, last_error
               FROM file_deletion_queue WHERE storage_object_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(self.pools.reader().executor())
        .await
        .unwrap()
        .map(
            |(reason, state, attempts, not_before, completed_at, last_error)| QueueRow {
                reason,
                state,
                attempts,
                not_before,
                completed_at,
                last_error,
            },
        )
    }

    async fn job(&self, id: StorageObjectId) -> Option<JobRow> {
        let rows = sqlx::query_as::<_, (String, i64, String, String, Option<String>)>(
            "SELECT state, attempts, run_at, payload_json, last_error FROM jobs
              WHERE dedup_key = ?1",
        )
        .bind(dedup_key(id).unwrap().as_str())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap();
        assert!(rows.len() <= 1, "one storage.delete_blob job per object");
        rows.into_iter()
            .next()
            .map(|(state, attempts, run_at, payload, last_error)| JobRow {
                state,
                attempts,
                run_at,
                payload,
                last_error,
            })
    }

    async fn invisible_rows_pointing_at_non_active_objects(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM files f JOIN storage_objects o ON o.id = f.storage_object_id
                      WHERE o.state <> 'active')
                  + (SELECT COUNT(*) FROM received_files r
                       JOIN storage_objects o ON o.id = r.storage_object_id
                      WHERE o.state <> 'active')",
        )
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn assert_visible_rows_have_bytes(&self) {
        assert_eq!(
            self.invisible_rows_pointing_at_non_active_objects().await,
            0
        );
        let keys: Vec<String> = sqlx::query_scalar(
            "SELECT o.object_key FROM files f JOIN storage_objects o ON o.id = f.storage_object_id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap();
        for key in keys {
            assert!(
                self.memory.has(&key),
                "a visible file points at absent bytes"
            );
        }
    }

    async fn audit_rows(&mut self, action: &str) -> Vec<(String, Option<String>, Value)> {
        while self.drain.flush_batch(AUDIT_BATCH_MAX).await.unwrap() > 0 {}
        sqlx::query_as::<_, (String, Option<String>, String)>(
            "SELECT result, error_code, metadata_json FROM audit_events WHERE action = ?1
              ORDER BY occurred_at, id",
        )
        .bind(action)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
        .into_iter()
        .map(|(result, code, metadata)| (result, code, serde_json::from_str(&metadata).unwrap()))
        .collect()
    }

    async fn live_transfer(&self, key: &ObjectKey, state: &str) {
        let now = self.now();
        let later = Timestamp::try_from(self.clock.now() + DAY)
            .unwrap()
            .to_string();
        self.exec(
            "INSERT INTO transfer_sessions (id, context, user_id, provider, state, created_at,
                                            updated_at, expires_at)
             VALUES (?1, 'my_files', ?2, 'local', 'uploading', ?3, ?3, ?4)",
            &[SESSION, USER, &now, &later],
        )
        .await;
        let object_id = StorageObjectId::generate(&self.clock).to_string();
        self.exec(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, upload_kind,
                  state, finalize_stage, final_object_id, final_object_key, created_at, updated_at)
             VALUES (?1, ?2, 0, 'client-1', 'report.pdf', 'tus', ?3, 'placing', ?4, ?5, ?6, ?6)",
            &[
                TRANSFER_FILE,
                SESSION,
                state,
                &object_id,
                key.as_str(),
                &now,
            ],
        )
        .await;
    }

    async fn sweep(&self, orphan_reap: bool) -> SweepReport {
        sweep(&self.context(orphan_reap)).await.unwrap()
    }

    fn orphan(&self, age: Duration, size: u64) -> ObjectKey {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        self.memory.put(key.as_str(), size, self.clock.now() - age);
        key
    }

    async fn tracked(&self, count: usize) {
        for _ in 0..count {
            let key = ObjectKey::allocate(KeyNamespace::Objects);
            self.memory
                .put(key.as_str(), 1, self.clock.now() - 30 * DAY);
            self.insert_row(&key, "active", 1).await;
        }
    }
}

fn permission_denied() -> StorageError {
    StorageError::PermissionDenied
}

fn not_found() -> StorageError {
    StorageError::NotFound
}

fn secret_bearing_s3_failure() -> StorageError {
    StorageError::S3(Box::new(io::Error::other(format!(
        "AccessDenied for AKIA{SECRET} at https://minio.internal/bucket?X-Amz-Signature={SECRET}"
    ))))
}

fn unavailable() -> StorageError {
    StorageError::ProviderUnavailable(Retryable::new(io::Error::other(SECRET)))
}

#[tokio::test]
async fn it_tombstone_crash_matrix() {
    let harness = Harness::open().await;

    let (first, first_key) = harness.active_object(4_096).await;
    let file = harness.visible_file(first, "first.bin").await;
    let crashed: Result<(), LifecycleError> = harness
        .pools
        .write_tx(&harness.clock, "test.crash_before_commit", async |tx| {
            sqlx::query("DELETE FROM files WHERE id = ?1")
                .bind(&file)
                .execute(tx.executor())
                .await?;
            let report =
                tombstone(tx, &harness.clock, &[first], DeletionReason::FileDeleted).await?;
            assert_eq!(report.tombstoned, 1);
            Err(LifecycleError::CorruptRow("injected crash before commit"))
        })
        .await;
    assert!(crashed.is_err());
    assert_eq!(harness.state(first).await, "active");
    assert_eq!(
        harness
            .count("SELECT COUNT(*) FROM files WHERE id = ?1", &file)
            .await,
        1
    );
    assert_eq!(harness.queue(first).await, None);
    assert_eq!(harness.job(first).await, None);
    assert!(harness.memory.has(first_key.as_str()));
    harness.assert_visible_rows_have_bytes().await;

    let committed: TombstoneReport = harness
        .pools
        .write_tx(&harness.clock, "test.delete_file", async |tx| {
            sqlx::query("DELETE FROM files WHERE id = ?1")
                .bind(&file)
                .execute(tx.executor())
                .await?;
            tombstone(tx, &harness.clock, &[first], DeletionReason::FileDeleted).await
        })
        .await
        .unwrap();
    assert_eq!(
        committed,
        TombstoneReport {
            tombstoned: 1,
            tombstoned_bytes: 4_096,
            already_tombstoned: 0,
            already_deleted: 0,
            queued: 1,
            jobs_enqueued: 1,
        }
    );
    assert!(
        harness.memory.deletes().is_empty(),
        "tombstoning performs no storage i/o"
    );
    let row = harness.object(first).await.unwrap();
    assert_eq!((row.state.as_str(), row.refcount), ("tombstoned", 0));
    assert_eq!(row.tombstoned_at.as_deref(), Some(harness.now().as_str()));
    let queue = harness.queue(first).await.unwrap();
    assert_eq!(
        (queue.state.as_str(), queue.reason.as_str()),
        ("pending", "file_deleted")
    );
    assert_eq!(queue.not_before, harness.now());
    let job = harness.job(first).await.unwrap();
    assert_eq!(job.state, "pending");
    let payload: Value = serde_json::from_str(&job.payload).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({ "storage_object_id": first.to_string() })
    );
    assert!(harness.memory.has(first_key.as_str()));
    harness.assert_visible_rows_have_bytes().await;

    let abandoned = claim(
        &harness.pools,
        &harness.clock,
        &Claimant::worker(InstanceId::generate(&harness.clock), 7),
        &[JobKind::StorageDeleteBlob],
        1,
        || true,
    )
    .await
    .unwrap();
    assert_eq!(abandoned.len(), 1);
    assert_eq!(harness.job(first).await.unwrap().state, "claimed");
    assert_eq!(harness.state(first).await, "tombstoned");
    assert!(harness.memory.has(first_key.as_str()));
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        None
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
    assert_eq!(harness.job(first).await.unwrap().state, "pending");

    harness.memory.script(Scripted::PanicBeforeRemove);
    let outcome = harness.run_next(JobKind::StorageDeleteBlob, false).await;
    assert!(
        matches!(outcome, Some(Outcome::Retrying { .. })),
        "{outcome:?}"
    );
    assert_eq!(harness.state(first).await, "tombstoned");
    let queue = harness.queue(first).await.unwrap();
    assert_eq!((queue.state.as_str(), queue.attempts), ("deleting", 1));
    assert!(harness.memory.has(first_key.as_str()));

    harness.clock.advance(RETRY_CAP);
    harness.memory.script(Scripted::RemoveThenPanic);
    let outcome = harness.run_next(JobKind::StorageDeleteBlob, false).await;
    assert!(
        matches!(outcome, Some(Outcome::Retrying { .. })),
        "{outcome:?}"
    );
    assert!(
        !harness.memory.has(first_key.as_str()),
        "the bytes are gone"
    );
    assert_eq!(
        harness.state(first).await,
        "tombstoned",
        "confirmation never ran"
    );
    assert_eq!(harness.queue(first).await.unwrap().state, "deleting");
    harness.assert_visible_rows_have_bytes().await;

    harness.clock.advance(RETRY_CAP);
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    let row = harness.object(first).await.unwrap();
    assert_eq!((row.state.as_str(), row.refcount), ("deleted", 0));
    assert_eq!(row.deleted_at.as_deref(), Some(harness.now().as_str()));
    let queue = harness.queue(first).await.unwrap();
    assert_eq!((queue.state.as_str(), queue.attempts), ("done", 3));
    assert_eq!(queue.completed_at.as_deref(), Some(harness.now().as_str()));
    assert_eq!(harness.job(first).await.unwrap().state, "succeeded");
    assert_eq!(harness.memory.deletes().len(), 3);

    assert_eq!(
        delete_object(&harness.context(false), first).await.unwrap(),
        DeleteOutcome::AlreadyDone
    );
    assert_eq!(
        harness.memory.deletes().len(),
        3,
        "a confirmed object is never deleted again"
    );
    let again = harness
        .tombstone(&[first], DeletionReason::FileDeleted)
        .await;
    assert_eq!(
        (again.already_deleted, again.queued, again.jobs_enqueued),
        (1, 0, 0)
    );
    assert_eq!(harness.state(first).await, "deleted", "no restore edge");

    harness
        .clock
        .advance(DELETED_ROW_RETENTION - Duration::from_secs(1));
    assert_eq!(
        reap_deleted_rows(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        0
    );
    assert!(harness.object(first).await.is_some());
    harness.clock.advance(Duration::from_secs(1));
    assert_eq!(
        reap_deleted_rows(&harness.pools, &harness.clock)
            .await
            .unwrap(),
        1
    );
    assert_eq!(harness.object(first).await, None);
    assert_eq!(harness.queue(first).await, None);
}

#[tokio::test]
async fn it_delete_blob_notfound_success() {
    let (harness, local) = Harness::with_local_disk().await;
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    assert!(!StorageProvider::exists(local.as_ref(), &key).await.unwrap());
    let id = harness.insert_row(&key, "active", 2_048).await;
    harness.tombstone(&[id], DeletionReason::FileDeleted).await;

    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    let row = harness.object(id).await.unwrap();
    assert_eq!((row.state.as_str(), row.refcount), ("deleted", 0));
    let queue = harness.queue(id).await.unwrap();
    assert_eq!((queue.state.as_str(), queue.attempts), ("done", 1));
    let job = harness.job(id).await.unwrap();
    assert_eq!((job.state.as_str(), job.attempts), ("succeeded", 0));
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        None
    );

    let memory = Harness::open().await;
    let (id, key) = memory.active_object(10).await;
    memory.memory.remove(key.as_str());
    memory.memory.script(Scripted::Fail(not_found));
    memory.tombstone(&[id], DeletionReason::FileDeleted).await;
    assert_eq!(
        memory.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    assert_eq!(memory.state(id).await, "deleted");
    assert_eq!(memory.job(id).await.unwrap().attempts, 0);
}

#[tokio::test]
async fn it_orphan_sweep_report_only_default() {
    let mut harness = Harness::open().await;
    harness.tracked(3).await;
    let orphan = harness.orphan(2 * DAY, 777);
    let (stale, stale_key) = harness.active_object(55).await;
    harness
        .tombstone(&[stale], DeletionReason::FileDeleted)
        .await;
    harness.clock.advance(DAY + Duration::from_secs(1));
    super::sweep::ensure_scheduled(&harness.pools, &harness.clock)
        .await
        .unwrap();

    assert_eq!(
        harness.run_next(JobKind::StorageOrphanSweep, false).await,
        Some(Outcome::Succeeded)
    );
    assert!(
        harness.memory.has(orphan.as_str()),
        "report-only never deletes"
    );
    assert!(harness.memory.deletes().is_empty());
    assert_eq!(
        harness.state(stale).await,
        "tombstoned",
        "stale tombstones are only reported"
    );
    assert!(harness.memory.has(stale_key.as_str()));

    let rows = harness.audit_rows("STORAGE_ORPHAN_DETECTED").await;
    assert_eq!(rows.len(), 1);
    let (result, code, metadata) = &rows[0];
    assert_eq!((result.as_str(), code.as_deref()), ("success", None));
    assert_eq!(metadata["outcome"], "report_only");
    assert_eq!(metadata["reap_enabled"], false);
    assert_eq!(metadata["candidates"], 1);
    assert_eq!(metadata["candidate_bytes"], 777);
    assert_eq!(metadata["stale_tombstones"], 1);
    assert_eq!(metadata["reaped"], 0);
    assert!(!metadata.to_string().contains(orphan.as_str()));

    let successors: i64 = harness
        .count(
            "SELECT COUNT(*) FROM jobs WHERE kind = ?1 AND state = 'pending'",
            "storage.orphan_sweep",
        )
        .await;
    assert_eq!(successors, 1);

    let report = harness.sweep(false).await;
    assert_eq!(report.outcome, SweepOutcome::ReportOnly);
    assert_eq!((report.scan.candidates, report.stale_tombstones), (1, 1));
    assert_eq!(report.stale_tombstone_bytes, 55);
}

#[tokio::test]
async fn it_orphan_reap_guards_24h_5pct_unparseable() {
    for (valid, orphans, refused) in [
        (100, 5, false),
        (100, 6, true),
        (20, 1, false),
        (20, 2, true),
    ] {
        assert_eq!(refuses_reap(orphans, valid), refused, "{orphans}/{valid}");
    }
    assert!(!refuses_reap(0, 0));

    let mut harness = Harness::open().await;
    harness.tracked(95).await;
    let old: Vec<ObjectKey> = (0..4).map(|_| harness.orphan(ORPHAN_MIN_AGE, 10)).collect();
    let young = harness.orphan(ORPHAN_MIN_AGE - Duration::from_secs(1), 10);
    let unparseable = [
        "objects/README.txt",
        "objects/01/92/.tmp-0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
        "objects/00/00/0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
    ];
    for key in unparseable {
        harness.memory.put(key, 3, harness.clock.now() - 90 * DAY);
    }

    let report = harness.sweep(false).await;
    assert_eq!(report.outcome, SweepOutcome::ReportOnly);
    assert_eq!(report.scan.valid, 100);
    assert_eq!(report.scan.candidates, 4);
    assert_eq!(report.scan.too_young, 1);
    assert_eq!(
        (report.scan.unparseable, report.scan.unparseable_bytes),
        (3, 9)
    );
    assert!(
        harness.memory.deletes().is_empty(),
        "reap=false never deletes"
    );

    let extra = harness.orphan(2 * DAY, 10);
    let report = harness.sweep(true).await;
    assert_eq!((report.scan.valid, report.scan.candidates), (101, 5));
    assert_eq!(
        report.outcome,
        SweepOutcome::Reaped,
        "5 of 101 is within the guard"
    );
    assert_eq!((report.reaped, report.reaped_bytes), (5, 50));
    for key in old.iter().chain([&extra]) {
        assert!(!harness.memory.has(key.as_str()));
    }
    assert!(
        harness.memory.has(young.as_str()),
        "younger than 24 h is never reaped"
    );
    for key in unparseable {
        assert!(harness.memory.has(key), "unparseable keys are never reaped");
    }
    assert_eq!(harness.memory.deletes().len(), 5);
    let tracked_rows: i64 = harness
        .count(
            "SELECT COUNT(*) FROM storage_objects WHERE state = ?1",
            "active",
        )
        .await;
    assert_eq!(tracked_rows, 95, "reaping creates no storage_objects rows");

    harness.clock.advance(Duration::from_secs(1));
    let report = harness.sweep(true).await;
    assert_eq!(report.outcome, SweepOutcome::Reaped);
    assert_eq!(report.reaped, 1);
    assert!(
        !harness.memory.has(young.as_str()),
        "once 24 h old it is an ordinary candidate"
    );

    let deletes_before = harness.memory.deletes().len();
    let refused: Vec<ObjectKey> = (0..6).map(|_| harness.orphan(3 * DAY, 10)).collect();
    let report = harness.sweep(true).await;
    assert_eq!((report.scan.valid, report.scan.candidates), (101, 6));
    assert_eq!(report.outcome, SweepOutcome::Refused);
    assert_eq!(report.reaped, 0);
    assert_eq!(
        harness.memory.deletes().len(),
        deletes_before,
        "a refused run deletes nothing"
    );
    for key in &refused {
        assert!(harness.memory.has(key.as_str()));
    }

    let outcomes: Vec<Value> = harness
        .audit_rows("STORAGE_ORPHAN_DETECTED")
        .await
        .into_iter()
        .map(|(_, _, metadata)| metadata["outcome"].clone())
        .collect();
    assert_eq!(outcomes, ["report_only", "reaped", "reaped", "refused"]);
}

#[tokio::test]
async fn it_abandoned_placed_object_tombstone_inserted() {
    let mut harness = Harness::open().await;
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    harness.memory.put(key.as_str(), 9_000, harness.clock.now());
    let id = StorageObjectId::generate(&harness.clock);
    let placed = PlacedObject {
        id,
        key: &key,
        provider: ProviderKind::Local,
        measured_size: Some(9_000),
    };

    let outcome = harness
        .pools
        .write_tx(&harness.clock, "test.tombstone_uncommitted", async |tx| {
            tombstone_uncommitted(tx, &harness.clock, &placed, DeletionReason::UploadAbandoned)
                .await
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        UncommittedOutcome::Queued {
            queued: true,
            job_enqueued: true
        }
    );
    let row = harness.object(id).await.unwrap();
    assert_eq!(
        row,
        ObjectRow {
            state: "tombstoned".into(),
            refcount: 0,
            size_bytes: 9_000,
            finalized_at: None,
            tombstoned_at: Some(harness.now()),
            deleted_at: None,
        }
    );
    let id_text = id.to_string();
    assert_eq!(
        harness
            .count(
                "SELECT COUNT(*) FROM files WHERE storage_object_id = ?1",
                &id_text
            )
            .await,
        0
    );
    assert_eq!(
        harness
            .count(
                "SELECT COUNT(*) FROM received_files WHERE storage_object_id = ?1",
                &id_text
            )
            .await,
        0
    );
    let grace_end = Timestamp::try_from(harness.clock.now() + ABANDONED_UPLOAD_GRACE)
        .unwrap()
        .to_string();
    let queue = harness.queue(id).await.unwrap();
    assert_eq!(
        (queue.reason.as_str(), queue.state.as_str()),
        ("upload_abandoned", "pending")
    );
    assert_eq!(queue.not_before, grace_end);
    let job = harness.job(id).await.unwrap();
    assert_eq!(
        (job.state.as_str(), job.run_at.as_str()),
        ("pending", grace_end.as_str())
    );

    let replay = harness
        .pools
        .write_tx(
            &harness.clock,
            "test.tombstone_uncommitted_again",
            async |tx| {
                tombstone_uncommitted(tx, &harness.clock, &placed, DeletionReason::UploadAbandoned)
                    .await
            },
        )
        .await
        .unwrap();
    assert_eq!(
        replay,
        UncommittedOutcome::Queued {
            queued: false,
            job_enqueued: false
        }
    );

    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        None
    );
    assert!(
        harness.memory.has(key.as_str()),
        "a resuming client is never raced"
    );
    harness.clock.advance(ABANDONED_UPLOAD_GRACE);
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    assert!(!harness.memory.has(key.as_str()));
    assert_eq!(harness.state(id).await, "deleted");
    assert_eq!(harness.queue(id).await.unwrap().state, "done");

    let (active, active_key) = harness.active_object(1).await;
    let committed = PlacedObject {
        id: active,
        key: &active_key,
        provider: ProviderKind::Local,
        measured_size: None,
    };
    let refused = harness
        .pools
        .write_tx(
            &harness.clock,
            "test.tombstone_uncommitted_active",
            async |tx| {
                tombstone_uncommitted(
                    tx,
                    &harness.clock,
                    &committed,
                    DeletionReason::UploadRejected,
                )
                .await
            },
        )
        .await;
    assert!(
        matches!(refused, Err(LifecycleError::CommittedObject)),
        "{refused:?}"
    );
    assert_eq!(harness.state(active).await, "active");
    assert_eq!(harness.queue(active).await, None);
    assert!(harness.audit_rows("JOB_DEAD_LETTERED").await.is_empty());
}

#[tokio::test]
async fn it_orphan_sweep_skips_live_transfer_keys() {
    let harness = Harness::open().await;
    harness.tracked(40).await;
    let key = harness.orphan(10 * DAY, 123);
    harness.live_transfer(&key, "uploading").await;

    let report = harness.sweep(true).await;
    assert_eq!(report.scan.live_transfer, 1);
    assert_eq!(report.scan.candidates, 0);
    assert_eq!(report.outcome, SweepOutcome::Clean);
    assert!(harness.memory.has(key.as_str()));

    for state in ["pending", "finalizing"] {
        harness
            .exec(
                "UPDATE transfer_session_files SET state = ?1 WHERE id = ?2",
                &[state, TRANSFER_FILE],
            )
            .await;
        assert_eq!(harness.sweep(true).await.scan.live_transfer, 1, "{state}");
        assert!(harness.memory.has(key.as_str()));
    }

    harness
        .exec(
            "UPDATE transfer_session_files SET state = 'failed' WHERE id = ?1",
            &[TRANSFER_FILE],
        )
        .await;
    let report = harness.sweep(true).await;
    assert_eq!(report.scan.live_transfer, 0);
    assert_eq!(report.scan.candidates, 1);
    assert_eq!(report.outcome, SweepOutcome::Reaped);
    assert!(!harness.memory.has(key.as_str()));
}

#[tokio::test]
async fn it_tombstone_is_idempotent_and_bounded() {
    let harness = Harness::open().await;
    let (first, _) = harness.active_object(10).await;
    let (second, _) = harness.active_object(20).await;

    let report = harness
        .tombstone(&[first, second, first], DeletionReason::FolderDeleted)
        .await;
    assert_eq!((report.tombstoned, report.tombstoned_bytes), (2, 30));
    assert_eq!((report.queued, report.jobs_enqueued), (2, 2));

    let replay = harness
        .tombstone(&[first, second], DeletionReason::FileDeleted)
        .await;
    assert_eq!(
        replay,
        TombstoneReport {
            tombstoned: 0,
            tombstoned_bytes: 0,
            already_tombstoned: 2,
            already_deleted: 0,
            queued: 0,
            jobs_enqueued: 0,
        }
    );
    assert_eq!(harness.queue(first).await.unwrap().reason, "folder_deleted");
    assert_eq!(
        harness
            .count(
                "SELECT COUNT(*) FROM jobs WHERE kind = ?1",
                "storage.delete_blob"
            )
            .await,
        2
    );

    let missing = StorageObjectId::generate(&harness.clock);
    let (third, _) = harness.active_object(5).await;
    let unknown = harness
        .pools
        .write_tx(&harness.clock, "test.tombstone_unknown", async |tx| {
            tombstone(
                tx,
                &harness.clock,
                &[third, missing],
                DeletionReason::FileDeleted,
            )
            .await
        })
        .await;
    assert!(
        matches!(
            unknown,
            Err(LifecycleError::UnknownObjects {
                requested: 2,
                found: 1
            })
        ),
        "{unknown:?}"
    );
    assert_eq!(
        harness.state(third).await,
        "active",
        "the caller's transaction rolled back"
    );

    let oversized = vec![third; MAX_TOMBSTONE_BATCH + 1];
    let too_large = harness
        .pools
        .write_tx(&harness.clock, "test.tombstone_oversized", async |tx| {
            tombstone(tx, &harness.clock, &oversized, DeletionReason::FileDeleted).await
        })
        .await;
    assert!(matches!(
        too_large,
        Err(LifecycleError::BatchTooLarge { .. })
    ));
    assert_eq!(
        harness.tombstone(&[], DeletionReason::FileDeleted).await,
        TombstoneReport::default()
    );

    let reasons: Vec<&str> = DeletionReason::ALL
        .iter()
        .map(|reason| reason.as_str())
        .collect();
    for reason in DeletionReason::ALL {
        let (id, _) = harness.active_object(1).await;
        harness.tombstone(&[id], reason).await;
        assert_eq!(harness.queue(id).await.unwrap().reason, reason.as_str());
    }
    assert_eq!(reasons.len(), 13);
}

#[tokio::test]
async fn it_delete_blob_refusals_and_retry_classification() {
    let mut harness = Harness::open().await;

    let (active, active_key) = harness.active_object(10).await;
    harness
        .exec(
            "INSERT INTO file_deletion_queue (id, storage_object_id, reason, requested_at, not_before)
             VALUES (?1, ?2, 'file_deleted', ?3, ?3)",
            &[
                &StorageObjectId::generate(&harness.clock).to_string(),
                &active.to_string(),
                &harness.now(),
            ],
        )
        .await;
    harness
        .exec(
            "INSERT INTO jobs (id, kind, payload_json, state, priority, run_at, attempts,
                               max_attempts, dedup_key, created_at, updated_at)
             VALUES (?1, 'storage.delete_blob', ?2, 'pending', 100, ?3, 0, 12, ?4, ?3, ?3)",
            &[
                &StorageObjectId::generate(&harness.clock).to_string(),
                &serde_json::json!({ "storage_object_id": active.to_string() }).to_string(),
                &harness.now(),
                dedup_key(active).unwrap().as_str(),
            ],
        )
        .await;
    let outcome = harness.run_next(JobKind::StorageDeleteBlob, false).await;
    assert_eq!(outcome, Some(Outcome::DeadLettered { attempts: 1 }));
    assert_eq!(harness.state(active).await, "active");
    assert!(harness.memory.has(active_key.as_str()));
    assert!(harness.memory.deletes().is_empty());
    let queue = harness.queue(active).await.unwrap();
    assert_eq!(
        (queue.state.as_str(), queue.last_error.as_deref()),
        ("failed", Some(CODE_NOT_TOMBSTONED))
    );
    let job = harness.job(active).await.unwrap();
    assert!(job.last_error.unwrap().starts_with("JOB_HANDLER_REJECTED"));
    let dead = harness.audit_rows("JOB_DEAD_LETTERED").await;
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].1.as_deref(), Some("JOB_HANDLER_REJECTED"));

    let corrupt = ObjectKey::parse("objects/01/92/0192f3c8d7e94a1b8f0c2d5e6a7b8c9d").unwrap();
    let corrupt_text = format!("objects/00/00/{}", &corrupt.as_str()[14..]);
    let bad = StorageObjectId::generate(&harness.clock);
    harness
        .exec(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                                          created_at, updated_at, finalized_at)
             VALUES (?1, ?2, 'local', 1, 'active', 1, ?3, ?3, ?3)",
            &[&bad.to_string(), &corrupt_text, &harness.now()],
        )
        .await;
    harness.tombstone(&[bad], DeletionReason::FileDeleted).await;
    let outcome = harness.run_next(JobKind::StorageDeleteBlob, false).await;
    assert_eq!(
        outcome,
        Some(Outcome::DeadLettered { attempts: 1 }),
        "never hammered"
    );
    assert_eq!(harness.state(bad).await, "tombstoned");
    assert_eq!(
        harness.queue(bad).await.unwrap().last_error.as_deref(),
        Some(CODE_INVALID_KEY)
    );
    assert!(harness.memory.deletes().is_empty());

    let (failing, failing_key) = harness.active_object(10).await;
    harness
        .tombstone(&[failing], DeletionReason::FileDeleted)
        .await;
    for (script, expected) in [
        (
            permission_denied as fn() -> StorageError,
            "STORAGE_UNAVAILABLE permission_denied",
        ),
        (secret_bearing_s3_failure, "STORAGE_UNAVAILABLE s3"),
        (unavailable, "STORAGE_UNAVAILABLE provider_unavailable"),
    ] {
        harness.memory.script(Scripted::Fail(script));
        let outcome = harness.run_next(JobKind::StorageDeleteBlob, false).await;
        assert!(
            matches!(outcome, Some(Outcome::Retrying { .. })),
            "{outcome:?}"
        );
        let queue = harness.queue(failing).await.unwrap();
        assert_eq!(queue.state, "failed");
        assert_eq!(queue.last_error.as_deref(), Some(expected));
        let job = harness.job(failing).await.unwrap();
        assert!(!job.last_error.unwrap_or_default().contains(SECRET));
        assert_eq!(harness.state(failing).await, "tombstoned");
        assert!(harness.memory.has(failing_key.as_str()));
        harness.clock.advance(RETRY_CAP);
    }
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    let queue = harness.queue(failing).await.unwrap();
    assert_eq!(
        (queue.state.as_str(), queue.attempts, queue.last_error),
        ("done", 4, None)
    );
    assert_eq!(harness.state(failing).await, "deleted");

    let mismatch = Harness::with_provider(ProviderKind::S3).await;
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    mismatch.memory.put(key.as_str(), 1, mismatch.clock.now());
    let foreign = StorageObjectId::generate(&mismatch.clock);
    mismatch
        .exec(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                                          created_at, updated_at, finalized_at)
             VALUES (?1, ?2, 'local', 1, 'active', 1, ?3, ?3, ?3)",
            &[&foreign.to_string(), key.as_str(), &mismatch.now()],
        )
        .await;
    mismatch
        .tombstone(&[foreign], DeletionReason::FileDeleted)
        .await;
    let outcome = mismatch.run_next(JobKind::StorageDeleteBlob, false).await;
    assert!(
        matches!(outcome, Some(Outcome::Retrying { .. })),
        "{outcome:?}"
    );
    assert!(
        mismatch.memory.deletes().is_empty(),
        "the other provider is never tried"
    );
    assert!(mismatch.memory.has(key.as_str()));
    assert_eq!(
        mismatch.state(foreign).await,
        "tombstoned",
        "a mismatch is not NotFound"
    );
    assert_eq!(
        mismatch.queue(foreign).await.unwrap().last_error.as_deref(),
        Some(CODE_PROVIDER_MISMATCH)
    );
}

#[tokio::test]
async fn it_delete_blob_removes_thumbnail_best_effort() {
    let harness = Harness::open().await;
    let thumbnails = harness.thumbnails();

    let (id, key) = harness.active_object(10).await;
    let path = thumbnails.path_for(&key).unwrap();
    let oid = key.as_str().rsplit('/').next().unwrap();
    assert!(path.ends_with(format!("{}/{}/{oid}.webp", &oid[..2], &oid[2..4])));
    assert!(path.starts_with(harness.root.path().join("thumbnails")));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"webp").unwrap();
    harness.tombstone(&[id], DeletionReason::FileDeleted).await;
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    assert!(!path.exists());

    let (stuck, stuck_key) = harness.active_object(10).await;
    let stuck_path: PathBuf = thumbnails.path_for(&stuck_key).unwrap();
    std::fs::create_dir_all(stuck_path.join("not-removable")).unwrap();
    harness
        .tombstone(&[stuck], DeletionReason::FileDeleted)
        .await;
    assert_eq!(
        harness.run_next(JobKind::StorageDeleteBlob, false).await,
        Some(Outcome::Succeeded)
    );
    assert_eq!(harness.state(stuck).await, "deleted");
    assert!(
        stuck_path.exists(),
        "a thumbnail failure never fails the deletion"
    );

    let branding = ObjectKey::allocate(KeyNamespace::Branding(
        crate::storage::key::BrandingKind::Logo,
    ));
    assert_eq!(thumbnails.path_for(&branding), None);
}

#[tokio::test]
async fn it_orphan_sweep_is_paged_and_batched() {
    let harness = Harness::open().await;
    for _ in 0..2_345 {
        harness.orphan(Duration::from_secs(60), 1);
    }
    let report = harness.sweep(true).await;
    assert_eq!(report.pages, 3);
    assert!(report.largest_page <= u64::from(SWEEP_PAGE_SIZE));
    assert_eq!(SWEEP_PAGE_SIZE, 1_000);
    assert!(harness.memory.largest_page_request.load(Ordering::Relaxed) <= 1_000);
    assert_eq!(report.scan.listed, 2_345);
    assert_eq!(
        report.db_lookups,
        2 * report.pages,
        "two batched lookups per page"
    );
    assert_eq!(report.scan.too_young, 2_345);
    assert_eq!(report.outcome, SweepOutcome::Clean);
    assert!(harness.memory.deletes().is_empty());

    let empty = Harness::open().await;
    let report = empty.sweep(true).await;
    assert_eq!((report.pages, report.db_lookups), (1, 0));
}

#[test]
fn unit_lifecycle_has_no_restore_edge() {
    for source in [
        include_str!("../lifecycle.rs"),
        include_str!("delete_blob.rs"),
        include_str!("sweep.rs"),
    ] {
        let compact: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!compact.contains("SET state = 'active'"));
        assert!(!compact.contains("state = 'active', "));
    }
    let id = StorageObjectId::generate(&TestClock::new(START));
    assert_eq!(
        dedup_key(id).unwrap().as_str(),
        format!("storage_object:{id}")
    );
}
