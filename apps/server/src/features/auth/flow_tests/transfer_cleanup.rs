use std::io::Cursor;

use super::deletion_tests::World;
use super::folders::{Member, HOST_A, SEEDED_AT};
use super::profile::assert_code;
use super::transfers::sized;
use super::*;
use crate::features::transfers::cleanup::{abandoned_multipart_page, terminated_tus_page};
use crate::features::transfers::model::SESSION_TTL;
use crate::infra::jobs::JobKind;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{PutHint, StorageProvider};

const INVALID: &str = "TRANSFER_SESSION_STATE_INVALID";

impl Stack {
    pub(super) async fn relogin(&self, member: &Member) -> Member {
        Member {
            id: member.id,
            creds: self.signed_in("alice", HOST_A).await,
        }
    }

    async fn multipart_identity(&self) -> Vec<(String, String, String, String)> {
        sqlx::query_as(
            "SELECT id, s3_upload_id, bucket, object_key FROM s3_multipart_uploads ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn abandoned_ids(&self) -> Vec<String> {
        let mut connection = self.pools.reader().executor().acquire().await.unwrap();
        abandoned_multipart_page(&mut connection, None, 1_000)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect()
    }

    async fn terminated_ids(&self) -> Vec<String> {
        let mut connection = self.pools.reader().executor().acquire().await.unwrap();
        terminated_tus_page(&mut connection, None, 1_000)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect()
    }
}

#[tokio::test]
async fn it_transfer_cancel_keeps_every_protocol_row_discoverable_for_cleanup() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("a", "a.bin", 100),
                sized("b", "b.bin", 100),
                sized("c", "c.bin", 100),
                sized("d", "d.bin", 100),
                sized("e", "e.bin", 100),
            ],
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
    stack.set_item_state(&items[1], "finalizing").await;
    stack
        .seed_multipart(
            alice.id,
            &items[1],
            &[(1, 5_242_880, "uploaded")],
            "completing",
        )
        .await;
    stack.set_item_state(&items[3], "uploading").await;
    stack
        .seed_tus(alice.id, &items[3], 100, 40, "in_progress")
        .await;
    stack.set_item_state(&items[4], "uploading").await;
    stack
        .seed_multipart(
            alice.id,
            &items[4],
            &[(1, 5_242_880, "uploaded")],
            "in_progress",
        )
        .await;
    stack.set_session_state(&session, "uploading").await;
    let identity = stack.multipart_identity().await;
    assert_eq!(identity.len(), 3);
    assert!(stack.abandoned_ids().await.is_empty());

    let item_cancel = stack.cancel_item(&alice, &session, &items[4]).await;
    assert_eq!(item_cancel.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.abandoned_ids().await, [format!("s3mp-{}", items[4])]);

    let canceled = stack.cancel_session(&alice, &session).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );

    let mut expected: Vec<String> = items[0..2]
        .iter()
        .chain(&items[4..5])
        .map(|item| format!("s3mp-{item}"))
        .collect();
    expected.sort();
    assert_eq!(
        stack.abandoned_ids().await,
        expected,
        "every live multipart is discoverable"
    );
    assert_eq!(stack.terminated_ids().await, [format!("tus-{}", items[3])]);
    assert_eq!(
        stack.multipart_identity().await,
        identity,
        "the provider identity is never altered by the cancel"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind IN ('s3.abort_abandoned_multipart', 'tus.expire_stale')")
            .await,
        0,
        "no job runtime is invented before the owning handler exists"
    );

    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN SELECT id, transfer_session_file_id FROM s3_multipart_uploads
          WHERE state = 'abandoned' AND id > '' ORDER BY id LIMIT 100",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(
        plan.iter().any(|row| row.3.contains("ix_s3mp_abandoned")),
        "{plan:?}"
    );
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN SELECT id, transfer_session_file_id FROM tus_uploads
          WHERE state = 'terminated' AND id > '' ORDER BY id LIMIT 100",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(
        plan.iter()
            .any(|row| row.3.contains("ix_tus_uploads_terminated")),
        "{plan:?}"
    );

    let settled = stack.dump().await;
    let repeated = stack.cancel_session(&alice, &session).await;
    assert_eq!(repeated.status, StatusCode::NO_CONTENT);
    let repeated_item = stack.cancel_item(&alice, &session, &items[0]).await;
    assert_eq!(repeated_item.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.dump().await,
        settled,
        "a repeated cancel does no second unit of work"
    );
    assert_eq!(stack.abandoned_ids().await, expected);

    let mut connection = stack.pools.reader().executor().acquire().await.unwrap();
    let first = abandoned_multipart_page(&mut connection, None, 1)
        .await
        .unwrap();
    let second = abandoned_multipart_page(&mut connection, Some(&first[0].id), 1)
        .await
        .unwrap();
    let third = abandoned_multipart_page(&mut connection, Some(&second[0].id), 5)
        .await
        .unwrap();
    assert_eq!(
        [
            first[0].id.clone(),
            second[0].id.clone(),
            third[0].id.clone()
        ],
        expected[..]
    );
    assert!(
        abandoned_multipart_page(&mut connection, Some(&third[0].id), 5)
            .await
            .unwrap()
            .is_empty()
    );
    drop(connection);

    clock.advance(SESSION_TTL + SESSION_TTL);
    stack
        .execute(&format!(
            "UPDATE transfer_sessions SET completed_at = '{SEEDED_AT}' WHERE id = '{session}'"
        ))
        .await;
    let retention = stack
        .pools
        .write_tx(&stack.clock, "test.retention", async |tx| {
            sqlx::query("DELETE FROM quota_reservations WHERE transfer_session_id = ?1")
                .bind(&session)
                .execute(tx.executor())
                .await
                .map_err(crate::infra::db::DbError::from)?;
            sqlx::query("DELETE FROM transfer_sessions WHERE id = ?1")
                .bind(&session)
                .execute(tx.executor())
                .await
                .map_err(crate::infra::db::DbError::from)?;
            Ok::<(), crate::infra::db::DbError>(())
        })
        .await;
    assert!(
        retention.is_err(),
        "terminal-session retention cannot remove a session that still awaits cleanup"
    );
    assert_eq!(stack.abandoned_ids().await, expected);
    assert_eq!(stack.multipart_identity().await, identity);

    stack.stop().await;
    let restarted = Stack::start(root.path(), &clock).await;
    assert_eq!(
        restarted.abandoned_ids().await,
        expected,
        "the rows survive a restart"
    );
    assert_eq!(
        restarted.terminated_ids().await,
        [format!("tus-{}", items[3])]
    );
    assert_eq!(restarted.multipart_identity().await, identity);
    assert_eq!(restarted.dump().await, settled);

    restarted
        .execute(&format!(
            "UPDATE s3_multipart_uploads SET state = 'aborted' WHERE id IN (SELECT id FROM s3_multipart_uploads);
             DELETE FROM s3_multipart_parts;
             DELETE FROM s3_multipart_uploads;
             DELETE FROM tus_uploads;
             DELETE FROM quota_reservations WHERE transfer_session_id = '{session}';
             DELETE FROM transfer_sessions WHERE id = '{session}'"
        ))
        .await;
    assert!(restarted.abandoned_ids().await.is_empty());
    assert_eq!(
        restarted
            .scalar_i64("SELECT COUNT(*) FROM transfer_session_files")
            .await,
        0,
        "once the adapters have cleaned up, retention removes the session and its items"
    );
    restarted.stop().await;
}

#[tokio::test]
async fn it_transfer_cancel_recovers_a_placed_object_through_the_deletion_lifecycle() {
    let world = World::start().await;
    let stack = &world.stack;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(10_000)).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("pending", "pending.bin", 10),
                sized("uploading", "uploading.bin", 20),
                sized("placing", "placing.bin", 30),
                sized("done", "done.bin", 40),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    stack.set_item_state(&items[1], "uploading").await;
    stack
        .seed_tus(alice.id, &items[1], 20, 5, "in_progress")
        .await;
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'finalizing', finalize_stage = 'placing' WHERE id = '{}'",
            items[2]
        ))
        .await;
    stack.seed_completed_item(alice.id, &items[3], 1, 40).await;
    stack.set_session_state(&session, "finalizing").await;

    let (placed_id, placed_key): (String, String) = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files WHERE id = ?1",
    )
    .bind(&items[2])
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    let key = ObjectKey::parse(&placed_key).unwrap();
    world
        .provider
        .put_stream(
            &key,
            Box::pin(Cursor::new(vec![7_u8; 30])),
            PutHint {
                declared_len: Some(30),
                content_type: None,
            },
        )
        .await
        .unwrap();
    assert!(world.provider.exists(&key).unwrap());
    let used = stack.used_bytes(alice.id).await;
    let objects_before = stack.counts().await.objects;

    let canceled = stack.cancel_session(&alice, &session).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );

    let never_placed: i64 = stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM storage_objects WHERE id IN (SELECT final_object_id FROM transfer_session_files WHERE id IN ('{}', '{}'))",
            items[0], items[1]
        ))
        .await;
    assert_eq!(
        never_placed, 0,
        "a resource that never reached placing gets no object row"
    );
    let tombstone: (String, String) =
        sqlx::query_as("SELECT state, object_key FROM storage_objects WHERE id = ?1")
            .bind(&placed_id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(tombstone, ("tombstoned".to_owned(), placed_key.clone()));
    assert_eq!(stack.counts().await.objects, objects_before + 1);
    let queued: Vec<(String, String)> = sqlx::query_as(
        "SELECT reason, state FROM file_deletion_queue WHERE storage_object_id = ?1",
    )
    .bind(&placed_id)
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        queued,
        [("upload_abandoned".to_owned(), "pending".to_owned())]
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob' AND state = 'pending'")
            .await,
        1
    );
    assert!(
        world.provider.exists(&key).unwrap(),
        "the request itself deleted nothing: the bytes await the durable job"
    );
    assert_eq!(
        stack.used_bytes(alice.id).await,
        used,
        "cancel never adjusts committed usage"
    );
    assert_eq!(stack.reservation(&session).await.0, "committed");
    assert_eq!(
        stack.item_states(&session).await,
        ["canceled", "canceled", "canceled", "completed"]
    );

    let settled = stack.dump().await;
    assert_eq!(
        stack.cancel_session(&alice, &session).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        stack.cancel_item(&alice, &session, &items[2]).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        stack.dump().await,
        settled,
        "retrying the cancel adds no queue row, job or quota change"
    );

    assert_eq!(
        world.run_all(JobKind::StorageDeleteBlob).await,
        0,
        "the grace period has not passed"
    );
    stack.clock.advance(Duration::from_secs(61 * 60));
    assert_eq!(world.run_all(JobKind::StorageDeleteBlob).await, 1);
    assert!(
        !world.provider.exists(&key).unwrap(),
        "the placed bytes are removed by the registered deletion job"
    );
    let finished: (String,) = sqlx::query_as("SELECT state FROM storage_objects WHERE id = ?1")
        .bind(&placed_id)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(finished.0, "deleted");
    assert_eq!(
        stack.used_bytes(alice.id).await,
        used,
        "recovery does not touch usage"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM files WHERE name = 'done-1.bin'")
            .await,
        1,
        "the completed item stays intact"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await,
        1
    );
}

#[tokio::test]
async fn it_transfer_cancel_tombstone_survives_a_restart() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let created = stack.opened(&alice, &[sized("p", "p.bin", 10)]).await;
    let session = created["id"].as_str().unwrap().to_owned();
    let item = stack.item_ids(&session).await.remove(0);
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'finalizing', finalize_stage = 'placing' WHERE id = '{item}';
             UPDATE transfer_sessions SET state = 'finalizing' WHERE id = '{session}'"
        ))
        .await;
    assert_eq!(
        stack.cancel_session(&alice, &session).await.status,
        StatusCode::NO_CONTENT
    );
    let before = stack.dump().await;
    stack.stop().await;

    let restarted = Stack::start(root.path(), &clock).await;
    assert_eq!(restarted.dump().await, before);
    assert_eq!(
        restarted
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned'")
            .await,
        1
    );
    assert_eq!(
        restarted
            .scalar_i64("SELECT COUNT(*) FROM file_deletion_queue WHERE state = 'pending'")
            .await,
        1
    );
    assert_eq!(
        restarted
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob'")
            .await,
        1
    );
    let again = restarted.cancel_session(&alice, &session).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    assert_eq!(restarted.dump().await, before);
    restarted.stop().await;
}

#[tokio::test]
async fn it_transfer_complete_partial_success_settles_once() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("ok", "ok.bin", 100),
                sized("bad", "bad.bin", 200),
                sized("gone", "gone.bin", 300),
                sized("late", "late.bin", 50),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    assert_eq!(stack.reservation(&session).await.1, 650);

    let file_id = stack.seed_completed_item(alice.id, &items[0], 1, 100).await;
    stack.set_item_state(&items[1], "failed").await;
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET error_code = 'STORAGE_UNAVAILABLE', error_request_id = 'req-9' WHERE id = '{}'",
            items[1]
        ))
        .await;
    assert_eq!(
        stack.cancel_item(&alice, &session, &items[2]).await.status,
        StatusCode::NO_CONTENT
    );
    stack.set_session_state(&session, "uploading").await;
    assert_eq!(
        stack.reservation(&session).await.1,
        250,
        "completed and canceled shares are already out of the hold"
    );

    let blocked = stack.complete_session(&alice, &session).await;
    assert_code(&blocked, StatusCode::CONFLICT, INVALID);
    assert_eq!(stack.item_states(&session).await[3], "pending");

    assert_eq!(
        stack.cancel_item(&alice, &session, &items[3]).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stack.reservation(&session).await.1, 200);
    assert_eq!(stack.used_bytes(alice.id).await, 100);

    let closed = stack.complete_session(&alice, &session).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    let body = closed.json();
    assert_eq!(body["state"], "completed");
    assert_eq!(body["reservedBytes"], 0);
    let states: Vec<&str> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["completed", "failed", "canceled", "canceled"]);
    assert_eq!(body["files"][0]["fileId"], file_id);
    for index in 1..4 {
        assert_eq!(
            body["files"][index]["fileId"],
            Value::Null,
            "no fileId is fabricated"
        );
    }
    assert_eq!(body["files"][1]["error"]["code"], "STORAGE_UNAVAILABLE");
    assert_eq!(body["files"][1]["error"]["requestId"], "req-9");
    assert_eq!(body["files"][1]["uploadedBytes"], 0);
    assert_eq!(body["uploadedBytes"], 100);
    assert_eq!(
        stack.reservation(&session).await,
        ("committed".to_owned(), 200, Some(100), None),
        "the committed bytes are the authoritative completed bytes, never a declared size"
    );
    assert_eq!(stack.used_bytes(alice.id).await, 100);
    assert_eq!(stack.counts().await.held, 0);

    let dump = stack.dump().await;
    let again = stack.complete_session(&alice, &session).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        again.body, closed.body,
        "a repeated close is the same representation"
    );
    assert_eq!(stack.dump().await, dump);

    let fits = stack
        .open(
            &alice,
            &super::transfers::request_body(None, &[sized("n", "n.bin", 900)]),
        )
        .await;
    assert_eq!(
        fits.status,
        StatusCode::CREATED,
        "failed and canceled shares consume no quota after the close"
    );
    let refused = stack
        .open(
            &alice,
            &super::transfers::request_body(None, &[sized("m", "m.bin", 1)]),
        )
        .await;
    assert_code(&refused, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    assert_eq!(refused.json()["error"]["details"]["usedBytes"], 100);
    assert_eq!(refused.json()["error"]["details"]["heldBytes"], 900);

    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_complete_without_any_completed_item_releases_the_hold() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;

    let created = stack
        .opened(
            &alice,
            &[sized("a", "a.bin", 400), sized("b", "b.bin", 500)],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    stack.set_item_state(&items[0], "failed").await;
    stack.set_item_state(&items[1], "failed").await;
    stack.set_session_state(&session, "failed").await;

    let closed = stack.complete_session(&alice, &session).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    assert_eq!(
        stack.reservation(&session).await,
        ("released".to_owned(), 900, None, Some("failed".to_owned())),
        "declared sizes never become committed usage"
    );
    assert_eq!(stack.used_bytes(alice.id).await, 0);
    assert_eq!(stack.counts().await.held, 0);
    for file in closed.json()["files"].as_array().unwrap() {
        assert_eq!(file["fileId"], Value::Null);
        assert_eq!(file["state"], "failed");
    }
    assert_eq!(stack.counts().await.objects, 0);
    let again = stack.complete_session(&alice, &session).await;
    assert_eq!(again.body, closed.body);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_ttl_is_seven_days_and_the_hold_expires_with_it() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;

    assert_eq!(SESSION_TTL, Duration::from_secs(7 * 24 * 60 * 60));
    let created = stack
        .opened(&alice, &[sized("a", "a.bin", 10), sized("b", "b.bin", 20)])
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["createdAt"], "2026-09-25T12:00:00.000Z");
    assert_eq!(created["expiresAt"], "2026-10-02T12:00:00.000Z");

    let (hold_expiry, session_expiry): (String, String) = sqlx::query_as(
        "SELECT q.expires_at, s.expires_at FROM quota_reservations q
           JOIN transfer_sessions s ON s.id = q.transfer_session_id WHERE s.id = ?1",
    )
    .bind(&session)
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        hold_expiry, session_expiry,
        "the hold expires with its session"
    );
    assert_eq!(session_expiry, "2026-10-02T12:00:00.000Z");
    let day = Duration::from_secs(24 * 60 * 60);

    clock.advance(day);
    let alice = stack.relogin(&alice).await;
    let after_a_day = stack.complete_session(&alice, &session).await;
    assert_code(&after_a_day, StatusCode::CONFLICT, INVALID);

    clock.advance(day * 6 - Duration::from_millis(1));
    let alice = stack.relogin(&alice).await;
    let last_instant = stack.complete_session(&alice, &session).await;
    assert_code(&last_instant, StatusCode::CONFLICT, INVALID);

    clock.advance(Duration::from_millis(1));
    let alice = stack.relogin(&alice).await;
    let at_expiry = stack.complete_session(&alice, &session).await;
    assert_code(&at_expiry, StatusCode::GONE, "TRANSFER_SESSION_EXPIRED");
    assert_eq!(
        stack.session_detail(&alice, &session).await.json()["state"],
        "created",
        "reading never expires a session"
    );
    stack.stop().await;
}
