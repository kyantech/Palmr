use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::finalize::Rig;
use super::folders::{HOST_A, HOST_B};
use super::transfer_audit::transfer_rows;
use super::transfer_reaper::{pass, start_in_session, tus_marker, Jobs, DAY, HOUR};
use super::transfers::{request_body, s3_stack_storage, sized};
use super::*;
use crate::features::transfers::model::SESSION_TTL;
use crate::features::transfers::reaper::{self, ReapReport, ReaperContext, PASS_LIMIT};
use crate::features::transfers::state::TransferSessionState;
use crate::features::transfers::ErrorParts;
use crate::infra::jobs::JobKind;
use crate::storage::s3::profile::ProviderProfile;

const OLD: &str = "2026-07-01T00:00:00.000Z";

async fn seed_bulk_terminal_uploads(rig: &Rig, owner: UserId, sessions: u32, per_session: u32) {
    for session in 0..sessions {
        let session_id = format!("session-{session:04}");
        let first = session * per_session;
        rig.stack
            .execute(&format!(
                "INSERT INTO transfer_sessions
                     (id, context, user_id, provider, state, created_at, updated_at, expires_at, completed_at)
                 VALUES ('{session_id}', 'my_files', '{owner}', 'local', 'expired', '{OLD}', '{OLD}', '{OLD}', '{OLD}');
                 WITH RECURSIVE seq(n) AS (SELECT {first} UNION ALL SELECT n + 1 FROM seq WHERE n < {})
                 INSERT INTO transfer_session_files
                     (id, transfer_session_id, ordinal, client_file_key, display_name, upload_kind, state,
                      final_object_id, final_object_key, created_at, updated_at)
                 SELECT printf('item-%06d', n), '{session_id}', n - {first}, printf('k%06d', n), 'f.bin', 'tus', 'expired',
                        printf('%036d', n), 'objects/00/00/' || printf('%032x', n), '{OLD}', '{OLD}'
                   FROM seq;
                 WITH RECURSIVE seq(n) AS (SELECT {first} UNION ALL SELECT n + 1 FROM seq WHERE n < {})
                 INSERT INTO tus_uploads
                     (id, transfer_session_file_id, owner_user_id, upload_length, upload_offset, staging_path,
                      state, created_at, updated_at, expires_at)
                 SELECT printf('0192f3a1-%04x-7000-8000-%012x', n / 65536, n), printf('item-%06d', n), '{owner}', 10, 4,
                        'uploads/x/blob', 'expired', '{OLD}', '{OLD}', '{OLD}'
                   FROM seq;",
                first + per_session - 1,
                first + per_session - 1,
            ))
            .await;
    }
}

#[tokio::test]
async fn it_reaper_drains_a_ten_thousand_row_backlog_in_bounded_passes() {
    let rig = Rig::manual().await;
    let owner = rig.alice.id;
    seed_bulk_terminal_uploads(&rig, owner, 5, 2_000).await;
    assert_eq!(rig.stack.tus_count().await, 10_000);

    let mut passes = 0;
    let mut removed = 0;
    loop {
        let report = pass(&rig).await;
        passes += 1;
        assert!(
            report.staging_removed <= u64::from(PASS_LIMIT),
            "a pass is bounded: {report:?}"
        );
        removed += report.staging_removed;
        assert_eq!(report.errors, 0, "{report:?}");
        if !report.more {
            break;
        }
        assert!(passes < 40, "the backlog must drain");
    }
    assert_eq!(removed, 10_000);
    assert!(passes >= 10, "{passes}");
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM tus_uploads WHERE staging_removed_at IS NULL")
            .await,
        0
    );
    assert_eq!(
        rig.stack.tus_count().await,
        10_000,
        "marking is not pruning"
    );

    rig.stack.clock.advance(8 * DAY);
    let mut pruned = 0;
    let mut sessions = 0;
    for _ in 0..40 {
        let report = pass(&rig).await;
        assert!(report.uploads_pruned <= u64::from(PASS_LIMIT), "{report:?}");
        assert!(
            report.sessions_pruned <= u64::from(PASS_LIMIT),
            "{report:?}"
        );
        pruned += report.uploads_pruned;
        sessions += report.sessions_pruned;
        if !report.more && report.uploads_pruned == 0 {
            break;
        }
    }
    assert_eq!(pruned, 10_000);
    assert_eq!(rig.stack.tus_count().await, 0);
    assert_eq!(
        sessions, 5,
        "terminal sessions follow once their children are gone"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_session_files")
            .await,
        0
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions")
            .await,
        0
    );
}

#[tokio::test]
async fn it_reaper_job_continues_a_capped_pass_until_the_backlog_is_gone() {
    let rig = Rig::manual().await;
    seed_bulk_terminal_uploads(&rig, rig.alice.id, 3, 1_500).await;
    let jobs = Jobs::new(&rig);
    reaper::ensure_scheduled(&rig.stack.pools, &rig.stack.clock)
        .await
        .unwrap();
    let executed = jobs.run_all(JobKind::TusExpireStale).await;
    assert!(
        executed >= 5,
        "a capped pass enqueues continuations: {executed}"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM tus_uploads WHERE staging_removed_at IS NULL")
            .await,
        0
    );
    assert_eq!(
        rig.stack
            .scalar_i64(
                "SELECT COUNT(*) FROM jobs WHERE kind = 'tus.expire_stale' AND state = 'pending'"
            )
            .await,
        1,
        "exactly the next recurring bucket remains"
    );
}

#[tokio::test]
async fn it_reaper_prune_respects_foreign_keys_and_never_touches_completed_files() {
    let rig = Rig::manual().await;
    let owner = rig.alice.id;
    let seed_session = |id: &'static str, state: &'static str| {
        format!(
            "INSERT INTO transfer_sessions
                 (id, context, user_id, provider, state, created_at, updated_at, expires_at, completed_at)
             VALUES ('{id}', 'my_files', '{owner}', 'local', '{state}', '{OLD}', '{OLD}', '{OLD}', '{OLD}');"
        )
    };
    let seed_item = |session: &'static str, n: u32| {
        format!(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, upload_kind, state,
                  final_object_id, final_object_key, created_at, updated_at)
             VALUES ('{session}-item', '{session}', {n}, 'k{n}', 'f.bin', 'tus', 'expired',
                     printf('%036d', {n}), 'objects/00/00/' || printf('%032x', {n}), '{OLD}', '{OLD}');"
        )
    };
    let statements = [
            seed_session("live-tus", "expired"),
            seed_item("live-tus", 1),
            format!(
                "INSERT INTO tus_uploads (id, transfer_session_file_id, owner_user_id, upload_length, upload_offset,
                     staging_path, state, created_at, updated_at, expires_at)
                 VALUES ('0192f3a1-0000-7000-8000-000000000001', 'live-tus-item', '{owner}', 10, 4,
                         'uploads/x/blob', 'expired', '{OLD}', '{OLD}', '{OLD}');"
            ),
            seed_session("abandoned-s3", "expired"),
            seed_item("abandoned-s3", 2),
            format!(
                "INSERT INTO s3_multipart_uploads (id, transfer_session_file_id, s3_upload_id, bucket,
                     object_key, owner_user_id, total_size_bytes, part_size_bytes, part_count, state,
                     created_at, updated_at, expires_at)
                 VALUES ('s3mp-1', 'abandoned-s3-item', 'upload-id', 'bucket', 'objects/00/00/{:032x}', '{owner}',
                         10, 5242880, 1, 'abandoned', '{OLD}', '{OLD}', '{OLD}');",
                2
            ),
            seed_session("held", "canceled"),
            seed_item("held", 3),
            format!(
                "INSERT INTO quota_reservations (id, user_id, transfer_session_id, context, reserved_bytes, state,
                     created_at, expires_at)
                 VALUES ('res-held', '{owner}', 'held', 'my_files', 10, 'held', '{OLD}', '{OLD}');"
            ),
            seed_session("settled", "completed"),
    ];
    rig.stack.execute(&statements.concat()).await;
    rig.stack
        .execute(&format!(
            "{}INSERT INTO quota_reservations (id, user_id, transfer_session_id, context, reserved_bytes,
                  committed_bytes, state, created_at, expires_at, settled_at)
              VALUES ('res-settled', '{owner}', 'settled', 'my_files', 0, 0, 'committed', '{OLD}', '{OLD}', '{OLD}');
              {}{}",
            seed_item("settled", 4),
            seed_session("clean", "completed"),
            seed_item("clean", 5),
        ))
        .await;
    let file = rig
        .stack
        .seed_completed_item(owner, "clean-item", 1, 10)
        .await;
    rig.stack
        .execute("UPDATE tus_uploads SET staging_removed_at = NULL")
        .await;
    rig.stack.clock.advance(60 * DAY);

    let first = pass(&rig).await;
    assert_eq!(first.errors, 0, "{first:?}");
    let remaining = |sql: &'static str| {
        let stack = &rig.stack;
        async move { stack.scalar_i64(sql).await }
    };
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'live-tus'").await,
        1,
        "a session whose protocol row still awaits staging cleanup stays"
    );

    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM tus_uploads WHERE staging_removed_at IS NOT NULL")
            .await,
        1,
        "the first pass proved the removal"
    );
    rig.stack
        .execute("UPDATE tus_uploads SET staging_removed_at = '2026-07-02T00:00:00.000Z'")
        .await;
    let second = pass(&rig).await;
    assert_eq!(second.errors, 0, "{second:?}");
    assert_eq!(second.uploads_pruned, 1);
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'live-tus'").await,
        0,
        "children first, then the parent"
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'abandoned-s3'").await,
        1,
        "an abandoned multipart row keeps its session until M14-T09 cleans it"
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'held'").await,
        1,
        "an unsettled reservation is never pruned"
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM quota_reservations WHERE id = 'res-held'").await,
        1
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'settled'").await,
        1,
        "a settled reservation row is quota.reconcile's to retire first"
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'clean'").await,
        0
    );
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_session_files WHERE id = 'clean-item'").await,
        0
    );
    assert_eq!(
        rig.stack
            .scalar_i64(&format!("SELECT COUNT(*) FROM files WHERE id = '{file}'"))
            .await,
        1,
        "completed files survive history pruning"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await,
        1
    );
    assert_eq!(
        rig.stack.scalar_i64("PRAGMA foreign_keys").await,
        1,
        "foreign keys stay enforced"
    );

    rig.stack
        .execute("DELETE FROM quota_reservations WHERE id = 'res-settled'")
        .await;
    let third = pass(&rig).await;
    assert_eq!(third.sessions_pruned, 1);
    assert_eq!(
        remaining("SELECT COUNT(*) FROM transfer_sessions WHERE id = 'settled'").await,
        0
    );
    assert_eq!(pass(&rig).await.sessions_pruned, 0);
}

#[tokio::test]
async fn it_reaper_settles_each_item_share_once_and_keeps_a_retryable_hold() {
    let rig = Rig::manual().await;
    let alice = &rig.alice;
    rig.stack.set_quota(alice.id, Some(1_000)).await;
    let created = rig
        .stack
        .opened(
            alice,
            &[
                sized("a", "a.bin", 100),
                sized("b", "b.bin", 200),
                sized("c", "c.bin", 300),
                sized("d", "d.bin", 400),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    rig.stack
        .seed_completed_item(alice.id, &items[0], 1, 100)
        .await;
    let b = start_in_session(&rig, &session, &items[1], "b.bin", 200, 20).await;
    let c = start_in_session(&rig, &session, &items[2], "c.bin", 300, 30).await;
    let d = start_in_session(&rig, &session, &items[3], "d.bin", 400, 40).await;
    assert_eq!(rig.stack.reservation(&session).await.1, 900);

    rig.stack.clock.advance(12 * HOUR);
    let renewed_d = rig
        .stack
        .tus_send(super::tus_patch::patch(alice, &d, 40).bytes(&super::tus::pattern(10)))
        .await;
    assert_eq!(renewed_d.status, StatusCode::NO_CONTENT);
    rig.stack.clock.advance(11 * HOUR);
    let renewed_b = rig
        .stack
        .tus_send(super::tus_patch::patch(alice, &b, 20).bytes(&super::tus::pattern(10)))
        .await;
    assert_eq!(renewed_b.status, StatusCode::NO_CONTENT);
    rig.stack
        .transfers
        .fail_item(
            alice.id,
            session.parse().unwrap(),
            items[3].parse().unwrap(),
            ErrorParts::of(
                crate::domain::error_code::ErrorCode::StorageUnavailable,
                None,
            ),
        )
        .await
        .unwrap();
    let states_before = rig.stack.item_states(&session).await;
    for negotiation in [
        crate::domain::error_code::ErrorCode::UploadLocked,
        crate::domain::error_code::ErrorCode::UploadOffsetMismatch,
        crate::domain::error_code::ErrorCode::TransferBackpressure,
    ] {
        let recorded = rig
            .stack
            .transfers
            .fail_item(
                alice.id,
                session.parse().unwrap(),
                items[1].parse().unwrap(),
                ErrorParts::of(negotiation, None),
            )
            .await;
        assert!(
            matches!(
                recorded,
                Err(
                    crate::features::transfers::error::TransferError::Invariant {
                        what: "failure_code"
                    }
                )
            ),
            "{negotiation:?} is a retry signal, never a failed upload: {recorded:?}"
        );
    }
    assert_eq!(
        rig.stack.item_states(&session).await,
        states_before,
        "the refused recordings changed no item"
    );

    let refused = rig
        .stack
        .open(alice, &request_body(None, &[sized("n", "n.bin", 300)]))
        .await;
    assert_eq!(refused.status, StatusCode::INSUFFICIENT_STORAGE);

    rig.stack.clock.advance(HOUR);
    let first = pass(&rig).await;
    assert_eq!(first.uploads_expired, 1, "only the idle upload: {first:?}");
    assert_eq!(tus_marker(&rig, &c).await.0, "expired");
    assert_eq!(tus_marker(&rig, &b).await.0, "in_progress");
    assert_eq!(
        tus_marker(&rig, &d).await.0,
        "in_progress",
        "a retryable failure keeps its row"
    );
    assert_eq!(
        rig.stack.item_states(&session).await,
        ["completed", "uploading", "expired", "failed"]
    );
    let (state, held, _, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), held),
        ("held", 600),
        "only C's share was released"
    );
    assert_eq!(rig.stack.used_bytes(alice.id).await, 100);
    assert_eq!(
        rig.stack.session_row(&session).await.0,
        "uploading",
        "B is still uploading, so D's retryable failure does not fail the session"
    );

    let admitted = rig
        .stack
        .open(alice, &request_body(None, &[sized("n", "n.bin", 300)]))
        .await;
    assert_eq!(admitted.status, StatusCode::CREATED, "{}", admitted.text());
    let fresh = admitted.json()["id"].as_str().unwrap().to_owned();
    let settled = rig.stack.dump().await;
    assert_eq!(pass(&rig).await, ReapReport::default());
    assert_eq!(
        rig.stack.dump().await,
        settled,
        "no share is released twice"
    );

    rig.stack.clock.advance(12 * HOUR);
    let second = pass(&rig).await;
    assert_eq!(second.uploads_expired, 1, "{second:?}");
    assert_eq!(tus_marker(&rig, &d).await.0, "expired");
    assert_eq!(
        rig.stack.reservation(&session).await.1,
        200,
        "D's share is gone, B's remains"
    );

    rig.stack.clock.advance(12 * HOUR);
    let third = pass(&rig).await;
    assert_eq!(third.uploads_expired, 1, "{third:?}");
    let (state, held, _, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), held),
        ("held", 0),
        "the completed item keeps the session open"
    );
    assert_eq!(
        rig.stack.session_row(&session).await.0,
        "uploading",
        "the reaper expires items without failing the session"
    );

    rig.stack.clock.advance(SESSION_TTL);
    let last = pass(&rig).await;
    assert_eq!(last.sessions_expired, 2, "{last:?}");
    let (state, _, _, reason) = rig.stack.reservation(&fresh).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("released", Some("expired"))
    );
    let (state, _, committed, reason) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), committed, reason),
        ("committed", Some(100), None)
    );
    assert_eq!(rig.stack.used_bytes(alice.id).await, 100);
    assert_eq!(pass(&rig).await.sessions_expired, 0);
}

#[tokio::test]
async fn it_reaper_cancels_and_expires_a_two_thousand_item_session() {
    let mut rig = Rig::manual().await;
    let alice = &rig.alice;
    let files: Vec<Value> = (0..2_000)
        .map(|index| sized(&format!("c{index:04}"), &format!("f{index:04}.bin"), 1))
        .collect();
    let canceled = rig.stack.open(alice, &request_body(None, &files)).await;
    assert_eq!(canceled.status, StatusCode::CREATED, "{}", canceled.text());
    let session = canceled.json()["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    assert_eq!(items.len(), 2_000);
    rig.stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'uploading' WHERE transfer_session_id = '{session}';
             UPDATE transfer_sessions SET state = 'uploading' WHERE id = '{session}'"
        ))
        .await;
    for item in items.iter().take(500) {
        rig.stack.seed_tus(alice.id, item, 1, 0, "created").await;
    }
    let started = Instant::now();
    let response = rig.stack.cancel_session(alice, &session).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    let cancel_took = started.elapsed();
    eprintln!("2000-item session cancel (one transaction, one audit event): {cancel_took:?}");
    assert!(cancel_took < Duration::from_secs(20), "{cancel_took:?}");
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1, "one event, not one per item");
    assert_eq!(events[0].action, "TRANSFER_CANCELED");
    assert_eq!(events[0].metadata["items_ended"], 2_000);
    assert_eq!(
        rig.stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM transfer_session_files WHERE transfer_session_id = '{session}' AND state = 'canceled'"
            ))
            .await,
        2_000
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM tus_uploads WHERE state = 'terminated'")
            .await,
        500
    );
    let (state, _, _, reason) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("released", Some("canceled"))
    );

    let expiring = rig
        .stack
        .open(
            alice,
            &request_body(
                None,
                &(0..2_000)
                    .map(|index| sized(&format!("e{index:04}"), &format!("e{index:04}.bin"), 1))
                    .collect::<Vec<_>>(),
            ),
        )
        .await;
    assert_eq!(expiring.status, StatusCode::CREATED, "{}", expiring.text());
    let second = expiring.json()["id"].as_str().unwrap().to_owned();
    rig.stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'uploading' WHERE transfer_session_id = '{second}';
             UPDATE transfer_sessions SET state = 'uploading' WHERE id = '{second}'"
        ))
        .await;
    rig.stack.clock.advance(SESSION_TTL);
    let started = Instant::now();
    let report = pass(&rig).await;
    assert_eq!(report.sessions_expired, 1, "{report:?}");
    let expire_took = started.elapsed();
    eprintln!("2000-item session expiry (one transaction, one audit event): {expire_took:?}");
    assert!(expire_took < Duration::from_secs(20), "{expire_took:?}");
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 2, "one more event for the whole session");
    assert_eq!(events[1].action, "TRANSFER_EXPIRED");
    assert_eq!(events[1].metadata["items_ended"], 2_000);
    assert_eq!(
        rig.stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM transfer_session_files WHERE transfer_session_id = '{second}' AND state = 'expired'"
            ))
            .await,
        2_000
    );
    assert_eq!(
        rig.stack.reservation(&second).await.3.as_deref(),
        Some("expired")
    );
    assert_eq!(rig.stack.counts().await.held, 0);
}

#[tokio::test]
async fn it_session_expiry_preserves_s3_resource_identity_and_enqueues_no_abort() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let storage = s3_stack_storage(ProviderProfile::Minio, false);
    let stack = Stack::start_with_storage(root.path(), &clock, storage).await;
    let alice = stack.member("alice", HOST_A).await;
    let created = stack
        .opened(
            &alice,
            &[sized("a", "a.bin", 10_000_000), sized("b", "b.bin", 5)],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    stack.set_item_state(&items[0], "uploading").await;
    stack
        .seed_multipart(
            alice.id,
            &items[0],
            &[(1, 5_242_880, "uploaded")],
            "in_progress",
        )
        .await;
    stack.set_session_state(&session, "uploading").await;
    let identity: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, s3_upload_id, bucket, object_key FROM s3_multipart_uploads ORDER BY id",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(identity.len(), 1);

    let reaper_context = ReaperContext::new(
        stack.pools.clone(),
        Arc::new(clock.clone()),
        stack.settings.handle(),
        stack.audit.clone(),
        None,
        CancellationToken::new(),
    );
    clock.advance(SESSION_TTL);
    let report = reaper::run_pass(&reaper_context).await.unwrap();
    assert_eq!(report.sessions_expired, 1, "{report:?}");
    assert_eq!(report.staging_removed, 0);
    assert_eq!(stack.session_row(&session).await.0, "expired");
    assert_eq!(stack.item_states(&session).await, ["expired", "expired"]);
    let after: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, s3_upload_id, bucket, object_key, state FROM s3_multipart_uploads ORDER BY id",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        (&after[0].0, &after[0].1, &after[0].2, &after[0].3),
        (
            &identity[0].0,
            &identity[0].1,
            &identity[0].2,
            &identity[0].3
        ),
        "provider identity is untouched"
    );
    assert_eq!(after[0].4, "abandoned");
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind LIKE 's3.%'")
            .await,
        0,
        "no abort job is invented before its handler exists"
    );

    clock.advance(40 * DAY);
    let retained = reaper::run_pass(&reaper_context).await.unwrap();
    assert_eq!(retained.sessions_pruned, 0, "{retained:?}");
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions")
            .await,
        1
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads WHERE state = 'abandoned'")
            .await,
        1
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_item_cancel_matrix_keeps_its_semantics() {
    let rig = Rig::manual().await;
    let alice = &rig.alice;
    let bob = rig.stack.member("bob", HOST_B).await;
    rig.stack.set_quota(alice.id, Some(10_000)).await;
    let created = rig
        .stack
        .opened(
            alice,
            &[
                sized("pending", "pending.bin", 10),
                sized("uploading", "uploading.bin", 20),
                sized("failed", "failed.bin", 30),
                sized("finalizing", "finalizing.bin", 40),
                sized("completed", "completed.bin", 50),
                sized("again", "again.bin", 60),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    rig.stack.set_item_state(&items[1], "uploading").await;
    rig.stack.set_item_state(&items[2], "failed").await;
    rig.stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'finalizing', finalize_stage = 'placing' WHERE id = '{}'",
            items[3]
        ))
        .await;
    rig.stack
        .seed_completed_item(alice.id, &items[4], 1, 50)
        .await;
    rig.stack.set_item_state(&items[5], "canceled").await;
    rig.stack.set_session_state(&session, "uploading").await;
    let held = rig.stack.reservation(&session).await.1;
    assert_eq!(held, 160, "{held}");
    let foreign = rig.stack.cancel_item(&bob, &session, &items[0]).await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    assert_eq!(rig.stack.item_states(&session).await[0], "pending");

    let mut expected = held;
    for (index, share) in [(0_usize, 10_i64), (1, 20), (2, 30), (3, 40)] {
        let response = rig.stack.cancel_item(alice, &session, &items[index]).await;
        assert_eq!(response.status, StatusCode::NO_CONTENT, "item {index}");
        expected -= share;
        assert_eq!(
            rig.stack.reservation(&session).await.1,
            expected,
            "item {index}"
        );
        let repeated = rig.stack.cancel_item(alice, &session, &items[index]).await;
        assert_eq!(repeated.status, StatusCode::NO_CONTENT);
        assert_eq!(
            rig.stack.reservation(&session).await.1,
            expected,
            "idempotent {index}"
        );
    }
    let completed = rig.stack.cancel_item(alice, &session, &items[4]).await;
    assert_eq!(
        completed.status,
        StatusCode::CONFLICT,
        "{}",
        completed.text()
    );
    let already = rig.stack.cancel_item(alice, &session, &items[5]).await;
    assert_eq!(already.status, StatusCode::NO_CONTENT);
    assert_eq!(
        rig.stack.item_states(&session).await,
        [
            "canceled",
            "canceled",
            "canceled",
            "canceled",
            "completed",
            "canceled"
        ]
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned'")
            .await,
        1,
        "the placed object of the finalizing item is tombstone-inserted"
    );
    assert_eq!(rig.stack.used_bytes(alice.id).await, 50);
    assert_eq!(
        TransferSessionState::Uploading.as_str(),
        rig.stack.session_row(&session).await.0
    );
}

#[tokio::test]
async fn it_reaper_queries_are_served_by_their_indexes() {
    let rig = Rig::manual().await;
    for (name, sql, literals, index) in reaper::PLANNED_QUERIES {
        let mut statement = sql.to_owned();
        for (position, literal) in literals.iter().enumerate() {
            statement = statement.replace(&format!("?{}", position + 1), literal);
        }
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {statement}"))
                .fetch_all(rig.stack.pools.reader().executor())
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
                .any(|line| line.starts_with("SCAN transfer_sessions")
                    || line.starts_with("SCAN tus_uploads")
                    || line.starts_with("SCAN s3_multipart_uploads")),
            "{name} must not scan its table: {text:?}"
        );
    }
}
