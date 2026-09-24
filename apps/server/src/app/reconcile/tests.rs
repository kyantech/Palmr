use std::sync::Arc;

use tempfile::TempDir;
use time::macros::datetime;

use super::{
    jobs_step, ReconcileContext, ReconcileError, ReconcileRegistry, StepFuture, JOBS_STEP,
};
use crate::config::SqliteSynchronous;
use crate::domain::clock::TestClock;
use crate::infra::db::{DbPools, InstanceId, MIGRATOR};
use crate::infra::jobs::JobsError;

const NOW: &str = "2026-09-24T12:00:00.000Z";
const PAST: &str = "2026-09-24T11:00:00.000Z";
const FUTURE: &str = "2026-09-24T13:00:00.000Z";

const OBJECT_A: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b80";
const KEY_A: &str = "objects/ab/cd/0123456789abcdef0123456789abcdef";
const KEY_B: &str = "objects/ef/01/fedcba9876543210fedcba9876543210";
const KEY_C: &str = "objects/23/45/00112233445566778899aabbccddeeff";
const OBJECT_ID_B: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b90";
const OBJECT_ID_C: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b91";
const USER: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b85";
const EXPIRED_JOB: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5c01";
const FOREIGN_JOB: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5c02";
const OWN_JOB: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5c03";
const FUTURE_JOB: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5c04";
const FOREIGN_INSTANCE: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

struct Harness {
    _root: TempDir,
    pools: DbPools,
    clock: TestClock,
    instance: InstanceId,
}

impl Harness {
    async fn open() -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let clock = TestClock::new(datetime!(2026-09-24 12:00 UTC));
        let instance = InstanceId::generate(&clock);
        Self {
            _root: root,
            pools,
            clock,
            instance,
        }
    }

    async fn seed(&self, sql: &str) {
        self.pools
            .write_tx(&self.clock, "test.seed", async |tx| {
                sqlx::raw_sql(sql).execute(tx.executor()).await?;
                Ok::<_, JobsError>(())
            })
            .await
            .unwrap();
    }

    async fn strings(&self, sql: &str) -> Vec<String> {
        sqlx::query_scalar(sql)
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    fn context(&self) -> ReconcileContext {
        ReconcileContext::new(
            self.pools.clone(),
            Arc::new(self.clock.clone()),
            self.instance,
        )
    }
}

fn injected_failure(_context: ReconcileContext) -> StepFuture {
    Box::pin(async { Err(ReconcileError::new("RECONCILE_TEST_FAILURE", "injected")) })
}

async fn job_row(
    harness: &Harness,
    id: &str,
) -> (String, String, Option<String>, Option<String>, i64) {
    sqlx::query_as(
        "SELECT state, run_at, claimed_by, lease_expires_at, attempts FROM jobs WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(harness.pools.reader().executor())
    .await
    .unwrap()
}

const SNAPSHOTS: [&str; 8] = [
    "SELECT id || '|' || state || '|' || refcount || '|' || COALESCE(finalized_at, '') || '|' || COALESCE(tombstoned_at, '') || '|' || COALESCE(deleted_at, '') FROM storage_objects ORDER BY id",
    "SELECT id || '|' || state || '|' || attempts || '|' || not_before FROM file_deletion_queue ORDER BY id",
    "SELECT id || '|' || state || '|' || key_hash || '|' || COALESCE(lease_expires_at, '') FROM idempotency_records ORDER BY id",
    "SELECT id || '|' || action || '|' || result FROM audit_events ORDER BY id",
    "SELECT id || '|' || state || '|' || COALESCE(last_reconciled_at, '') || '|' || COALESCE(completed_at, '') FROM s3_multipart_uploads ORDER BY id",
    "SELECT id || '|' || state || '|' || COALESCE(locked_by, '') || '|' || COALESCE(lock_expires_at, '') || '|' || upload_offset FROM tus_uploads ORDER BY id",
    "SELECT id || '|' || state FROM transfer_sessions ORDER BY id",
    "SELECT id || '|' || state || '|' || finalize_stage FROM transfer_session_files ORDER BY id",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_startup_reconcile_never_deletes() {
    let harness = Harness::open().await;
    let own_claim = format!("{}#0", harness.instance);
    let foreign_claim = format!("{FOREIGN_INSTANCE}#0");

    harness
        .seed(&format!(
            "INSERT INTO storage_objects
                 (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             VALUES ('{OBJECT_A}', '{KEY_A}', 'local', 10, 'active', 1, '{NOW}', '{NOW}', '{NOW}');
             INSERT INTO file_deletion_queue
                 (id, storage_object_id, reason, state, attempts, requested_at, not_before)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b81', '{OBJECT_A}', 'file_deleted',
                     'pending', 0, '{NOW}', '{NOW}');
             INSERT INTO idempotency_records
                 (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
                  state, lease_expires_at, created_at, expires_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b82', 'user', '{USER}', 'POST',
                     '/api/v1/files/upload', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     'in_progress', '{FUTURE}', '{NOW}', '{FUTURE}');
             INSERT INTO audit_events (id, occurred_at, action, actor_type, result)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b84', '{NOW}', 'FILE_UPLOADED', 'system', 'success');
             INSERT INTO users (id, email, email_normalized, username, username_normalized, created_at, updated_at)
             VALUES ('{USER}', 'u@example.test', 'u@example.test', 'user', 'user', '{NOW}', '{NOW}');
             INSERT INTO transfer_sessions
                 (id, context, user_id, provider, state, created_at, updated_at, expires_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b86', 'my_files', '{USER}', 's3', 'uploading',
                     '{NOW}', '{NOW}', '{FUTURE}');
             INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
                  upload_kind, state, finalize_stage, final_object_id, final_object_key, created_at, updated_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b87', '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b86', 0,
                     'k1', 'a.bin', '', 's3_multipart', 'uploading', 'none', '{OBJECT_ID_B}', '{KEY_B}',
                     '{NOW}', '{NOW}');
             INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
                  upload_kind, state, finalize_stage, final_object_id, final_object_key, created_at, updated_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b89', '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b86', 1,
                     'k2', 'b.bin', '', 'tus', 'uploading', 'none', '{OBJECT_ID_C}', '{KEY_C}',
                     '{NOW}', '{NOW}');
             INSERT INTO s3_multipart_uploads
                 (id, transfer_session_file_id, s3_upload_id, bucket, object_key, owner_user_id,
                  part_size_bytes, part_count, state, created_at, updated_at, expires_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b88', '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b87',
                     's3-up-1', 'bucket', '{KEY_B}', '{USER}', 8388608, 2, 'in_progress',
                     '{NOW}', '{NOW}', '{FUTURE}');
             INSERT INTO tus_uploads
                 (id, transfer_session_file_id, owner_user_id, upload_length, staging_path, state,
                  locked_by, lock_expires_at, created_at, updated_at, expires_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b8a', '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b89',
                     '{USER}', 100, 'uploads/01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b8a/blob', 'in_progress',
                     'foreign-lock#0', '{PAST}', '{NOW}', '{NOW}', '{FUTURE}');
             INSERT INTO jobs
                 (id, kind, payload_json, state, priority, run_at, attempts, max_attempts,
                  claimed_by, lease_expires_at, created_at, updated_at)
             VALUES
                 ('{EXPIRED_JOB}', 'tokens.prune', '{{}}', 'claimed', 100, '{PAST}', 2, 3,
                  '{foreign_claim}', '{PAST}', '{NOW}', '{NOW}'),
                 ('{FOREIGN_JOB}', 'sessions.prune', '{{}}', 'claimed', 100, '{PAST}', 0, 3,
                  '{foreign_claim}', '{FUTURE}', '{NOW}', '{NOW}'),
                 ('{OWN_JOB}', 'audit.retention_sweep', '{{}}', 'claimed', 100, '{PAST}', 0, 3,
                  '{own_claim}', '{FUTURE}', '{NOW}', '{NOW}'),
                 ('{FUTURE_JOB}', 'tokens.prune', '{{}}', 'pending', 100, '{FUTURE}', 0, 3,
                  NULL, NULL, '{NOW}', '{NOW}');"
        ))
        .await;

    let before: Vec<Vec<String>> = {
        let mut snapshots = Vec::new();
        for sql in SNAPSHOTS {
            snapshots.push(harness.strings(sql).await);
        }
        snapshots
    };
    assert!(before.iter().all(|rows| !rows.is_empty()));

    let report = ReconcileRegistry::new()
        .register("injected", injected_failure)
        .register(JOBS_STEP, jobs_step)
        .run(&harness.context())
        .await;

    assert_eq!(report.steps(), 2);
    assert_eq!(report.failed(), 1);
    assert_eq!(report.requeued(), 2);
    assert!(matches!(report.outcome("injected"), Some(Err(_))));
    assert_eq!(report.outcome(JOBS_STEP), Some(&Ok(2)));

    let expired = job_row(&harness, EXPIRED_JOB).await;
    assert_eq!(expired.0, "pending");
    assert_eq!(expired.1, PAST);
    assert_eq!(expired.2, None);
    assert_eq!(expired.3, None);
    assert_eq!(expired.4, 2);

    let foreign = job_row(&harness, FOREIGN_JOB).await;
    assert_eq!(foreign.0, "pending");
    assert_eq!(foreign.2, None);
    assert_eq!(foreign.3, None);

    let own = job_row(&harness, OWN_JOB).await;
    assert_eq!(own.0, "claimed");
    assert_eq!(own.2.as_deref(), Some(own_claim.as_str()));
    assert_eq!(own.3.as_deref(), Some(FUTURE));

    let future = job_row(&harness, FUTURE_JOB).await;
    assert_eq!(future.0, "pending");
    assert_eq!(future.1, FUTURE);
    assert_eq!(future.2, None);

    let after: Vec<Vec<String>> = {
        let mut snapshots = Vec::new();
        for sql in SNAPSHOTS {
            snapshots.push(harness.strings(sql).await);
        }
        snapshots
    };
    assert_eq!(before, after);

    assert_eq!(
        harness
            .strings("SELECT state FROM storage_objects WHERE id = '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b80'")
            .await,
        ["active"]
    );
    assert_eq!(
        harness
            .strings("SELECT state FROM s3_multipart_uploads WHERE id = '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b88'")
            .await,
        ["in_progress"]
    );
    assert_eq!(
        harness
            .strings(
                "SELECT state FROM tus_uploads WHERE id = '01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b8a'"
            )
            .await,
        ["in_progress"]
    );
}
