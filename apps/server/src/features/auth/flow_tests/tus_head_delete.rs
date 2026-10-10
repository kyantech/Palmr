use std::sync::atomic::Ordering;

use super::folders::{HOST_A, HOST_B};
use super::transfers::sized;
use super::tus::{
    assert_head_error, assert_tus_error, header, pattern, small_limits, upload_id_of, TestStaging,
    TusReq, TUS,
};
use super::*;
use crate::features::transfers::cleanup::terminated_tus_page;

fn head<'a>(member: &'a super::folders::Member, id: &str) -> TusReq<'a> {
    TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), member)
}

fn delete<'a>(member: &'a super::folders::Member, id: &str) -> TusReq<'a> {
    TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), member)
}

impl Stack {
    async fn terminated_page(&self) -> Vec<String> {
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
async fn it_tus_head_reports_the_authoritative_offset() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let known = stack.planned(&alice, "a.bin", Some(1_000)).await;
    let id = stack.tus_created(&alice, &known, Some(1_000)).await;
    let response = stack.tus_send(head(&alice, &id)).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.is_empty());
    assert_eq!(header(&response, "tus-resumable"), "1.0.0");
    assert_eq!(header(&response, "upload-offset"), "0");
    assert_eq!(header(&response, "upload-length"), "1000");
    assert!(!response.headers.contains_key("upload-defer-length"));
    assert_eq!(
        header(&response, "upload-expires"),
        "Sat, 26 Sep 2026 12:00:00 GMT"
    );
    assert_eq!(header(&response, "cache-control"), "no-store");
    assert!(response.headers.contains_key("x-request-id"));
    for leaked in ["location", "upload-metadata", "x-palmr-storage"] {
        assert!(!response.headers.contains_key(leaked), "{leaked}");
    }

    let deferred = stack.planned(&alice, "d.bin", None).await;
    let deferred_id = stack.tus_created(&alice, &deferred, None).await;
    let response = stack.tus_send(head(&alice, &deferred_id)).await;
    assert_eq!(header(&response, "upload-defer-length"), "1");
    assert!(!response.headers.contains_key("upload-length"));

    let data = pattern(600);
    let with_body = stack.planned(&alice, "b.bin", None).await;
    let body = stack
        .tus_send(stack.create_req(&alice, &with_body, None).bytes(&data))
        .await;
    assert_eq!(header(&body, "upload-offset"), "600");
    let body_id = upload_id_of(&body);
    let response = stack.tus_send(head(&alice, &body_id)).await;
    assert_eq!(header(&response, "upload-offset"), "600");
}

#[tokio::test]
async fn it_tus_head_never_reports_more_than_the_disk_justifies() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", None).await;
    let data = pattern(1_000);
    let created = stack
        .tus_send(stack.create_req(&alice, &planned, None).bytes(&data))
        .await;
    let id = upload_id_of(&created);

    stack
        .execute(&format!(
            "UPDATE tus_uploads SET upload_offset = 900 WHERE id = '{id}'"
        ))
        .await;
    let behind = stack.tus_send(head(&alice, &id)).await;
    assert_eq!(
        header(&behind, "upload-offset"),
        "900",
        "min(db, disk) when the disk is ahead"
    );
    assert_eq!(
        stack.staged_bytes(&id).unwrap().len(),
        1_000,
        "a read never truncates"
    );
    assert_eq!(
        stack.tus_row(&id).await.offset,
        900,
        "a read never rewrites the row"
    );

    stack
        .execute(&format!(
            "UPDATE tus_uploads SET upload_offset = 5000 WHERE id = '{id}'"
        ))
        .await;
    let phantom = stack.tus_send(head(&alice, &id)).await;
    assert_eq!(
        header(&phantom, "upload-offset"),
        "1000",
        "a phantom offset is not believed"
    );
    assert_eq!(stack.tus_row(&id).await.offset, 5_000);
}

#[tokio::test]
async fn it_tus_head_refuses_other_owners_unknown_terminated_and_expired_uploads() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let id = stack.tus_created(&alice, &planned, Some(10)).await;

    let foreign = stack.tus_send(head(&bob, &id)).await;
    let unknown = stack
        .tus_send(head(&bob, "0192f3a7-2a01-7c4d-8e11-aa0192f3a799"))
        .await;
    let malformed = stack.tus_send(head(&bob, "not-an-id")).await;
    for response in [&foreign, &unknown, &malformed] {
        assert_head_error(response, StatusCode::NOT_FOUND);
    }
    let comparable = |fetched: &Fetched| {
        let mut headers = fetched.headers.clone();
        headers.remove("x-request-id");
        headers.remove("date");
        headers.remove("content-security-policy");
        format!("{headers:?}")
    };
    assert_eq!(comparable(&foreign), comparable(&unknown));
    assert_eq!(comparable(&foreign), comparable(&malformed));

    let anonymous = stack
        .tus_send(TusReq::anonymous(Method::HEAD, &format!("{TUS}/{id}")))
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(header(&anonymous, "tus-resumable"), "1.0.0");
    let no_version = stack.tus_send(head(&alice, &id).no_version()).await;
    assert_head_error(&no_version, StatusCode::PRECONDITION_FAILED);
    assert_eq!(header(&no_version, "tus-version"), "1.0.0");

    stack
        .execute(&format!(
            "UPDATE tus_uploads SET expires_at = '2026-09-25T11:59:59.999Z' WHERE id = '{id}'"
        ))
        .await;
    let past_due = stack.tus_send(head(&alice, &id)).await;
    assert_head_error(&past_due, StatusCode::GONE);
    stack
        .execute(&format!(
            "UPDATE tus_uploads SET expires_at = '2026-09-26T12:00:00.000Z', state = 'expired' WHERE id = '{id}'"
        ))
        .await;
    let expired = stack.tus_send(head(&alice, &id)).await;
    assert_head_error(&expired, StatusCode::GONE);

    let other = stack.planned(&alice, "b.bin", Some(10)).await;
    let other_id = stack.tus_created(&alice, &other, Some(10)).await;
    assert_eq!(
        stack.tus_send(delete(&alice, &other_id)).await.status,
        StatusCode::NO_CONTENT
    );
    let terminated = stack.tus_send(head(&alice, &other_id)).await;
    assert_head_error(&terminated, StatusCode::GONE);
    let overridden = stack
        .tus_send(
            TusReq::new(Method::POST, &format!("{TUS}/{other_id}"), &alice)
                .header("x-http-method-override", "HEAD"),
        )
        .await;
    assert_tus_error(&overridden, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
}

#[tokio::test]
async fn it_tus_delete_terminates_once_and_releases_the_quota_share() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root.path(), &clock);
    let stack = Stack::start_with_tus(
        root.path(),
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
        },
    )
    .await;
    *staging.probe.lock().unwrap() = Some(stack.pools.clone());
    let alice = stack.member("alice", HOST_A).await;
    let created = stack
        .open(
            &alice,
            &super::transfers::request_body(
                None,
                &[sized("c1", "a.bin", 100), sized("c2", "b.bin", 50)],
            ),
        )
        .await;
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    let first = super::tus::Planned {
        session: session.clone(),
        item: items[0].clone(),
        name: "a.bin".into(),
    };
    let first_id = stack.tus_created(&alice, &first, Some(100)).await;
    assert_eq!(stack.reservation(&session).await.1, 150);

    let deleted = stack.tus_send(delete(&alice, &first_id)).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.body.is_empty());
    assert_eq!(header(&deleted, "tus-resumable"), "1.0.0");
    assert_eq!(stack.tus_row(&first_id).await.state, "terminated");
    assert_eq!(stack.item_state(&items[0]).await, "canceled");
    assert_eq!(
        stack.item_state(&items[1]).await,
        "pending",
        "siblings are unaffected"
    );
    assert_eq!(
        stack.reservation(&session).await.1,
        50,
        "only this item's share is released"
    );
    assert_eq!(stack.session_row(&session).await.0, "uploading");
    assert!(
        stack.staged_bytes(&first_id).is_none(),
        "staging is removed after the commit"
    );
    assert!(!staging.overlapped_transaction.load(Ordering::SeqCst));
    assert_eq!(stack.terminated_page().await, vec![first_id.clone()]);

    let before = stack.dump().await;
    let again = stack.tus_send(delete(&alice, &first_id)).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.dump().await,
        before,
        "a repeated DELETE changes nothing"
    );

    let head_after = stack.tus_send(head(&alice, &first_id)).await;
    assert_head_error(&head_after, StatusCode::GONE);
    let recreate = stack.tus_create(&alice, &first, Some(100)).await;
    assert_tus_error(
        &recreate,
        StatusCode::CONFLICT,
        "TRANSFER_SESSION_STATE_INVALID",
    );

    let second = super::tus::Planned {
        session: session.clone(),
        item: items[1].clone(),
        name: "b.bin".into(),
    };
    let second_id = stack.tus_created(&alice, &second, Some(50)).await;
    assert_eq!(
        stack.tus_send(delete(&alice, &second_id)).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        stack.session_row(&session).await.0,
        "canceled",
        "the last item closes the session"
    );
    assert_eq!(stack.reservation(&session).await.0, "released");
    assert_eq!(stack.counts().await.held, 0);
}

#[tokio::test]
async fn it_tus_delete_keeps_completed_content_and_refuses_other_owners() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let created = stack
        .open(
            &alice,
            &super::transfers::request_body(
                None,
                &[sized("c1", "done.bin", 10), sized("c2", "live.bin", 20)],
            ),
        )
        .await;
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    let live = super::tus::Planned {
        session: session.clone(),
        item: items[1].clone(),
        name: "live.bin".into(),
    };
    let live_id = stack.tus_created(&alice, &live, Some(20)).await;
    let file_id = stack.seed_completed_item(alice.id, &items[0], 1, 10).await;

    let foreign = stack.tus_send(delete(&bob, &live_id)).await;
    assert_tus_error(&foreign, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(stack.tus_row(&live_id).await.state, "created");

    assert_eq!(
        stack.tus_send(delete(&alice, &live_id)).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stack.item_state(&items[0]).await, "completed");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 1);
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?1")
        .bind(&file_id)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(kept, 1);
    assert_eq!(stack.counts().await.objects, 1);
    assert_eq!(stack.used_bytes(alice.id).await, 10);
}

#[tokio::test]
async fn it_tus_delete_is_atomic_and_cleanup_survives_a_restart() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root.path(), &clock);
    let stack = Stack::start_with_tus(
        root.path(),
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
        },
    )
    .await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let id = stack.tus_created(&alice, &planned, Some(10)).await;
    let before = stack.dump().await;

    stack
        .execute(
            "CREATE TRIGGER fail_terminate BEFORE UPDATE OF state ON tus_uploads
             WHEN NEW.state = 'terminated' BEGIN SELECT RAISE(ABORT, 'injected'); END",
        )
        .await;
    let failed = stack.tus_send(delete(&alice, &id)).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        stack.dump().await,
        before,
        "a failed termination rolls back completely"
    );
    assert!(
        stack.staged_bytes(&id).is_some(),
        "staging is untouched by a failed termination"
    );
    stack.execute("DROP TRIGGER fail_terminate").await;

    staging.fail_remove.store(true, Ordering::SeqCst);
    let deleted = stack.tus_send(delete(&alice, &id)).await;
    assert_eq!(
        deleted.status,
        StatusCode::NO_CONTENT,
        "cleanup trouble never fails the termination"
    );
    assert!(
        stack.staged_bytes(&id).is_some(),
        "the bytes are still there"
    );
    assert_eq!(stack.tus_row(&id).await.state, "terminated");

    drop(stack);
    let restarted = Stack::start(root.path(), &clock).await;
    assert_eq!(
        restarted.terminated_page().await,
        vec![id.clone()],
        "the terminated row is discoverable after a restart"
    );
    assert!(restarted.staged_bytes(&id).is_some());
    let again = restarted
        .tus_send(delete(
            &super::folders::Member {
                id: alice.id,
                creds: restarted.signed_in("alice", HOST_A).await,
            },
            &id,
        ))
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    assert!(
        restarted.staged_bytes(&id).is_none(),
        "a repeated DELETE retries the removal"
    );
}

#[tokio::test]
async fn it_tus_delete_answers_expired_uploads_with_gone_and_is_gated() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let id = stack.tus_created(&alice, &planned, Some(10)).await;

    let no_csrf = stack.tus_send(delete(&alice, &id).no_csrf()).await;
    assert_tus_error(&no_csrf, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING");
    assert_eq!(stack.tus_row(&id).await.state, "created");
    let no_version = stack.tus_send(delete(&alice, &id).no_version()).await;
    assert_tus_error(
        &no_version,
        StatusCode::PRECONDITION_FAILED,
        "TUS_VERSION_UNSUPPORTED",
    );
    let anonymous = stack
        .tus_send(TusReq::anonymous(Method::DELETE, &format!("{TUS}/{id}")))
        .await;
    assert_eq!(
        anonymous.status,
        StatusCode::FORBIDDEN,
        "the CSRF gate precedes authentication"
    );
    assert_eq!(header(&anonymous, "tus-resumable"), "1.0.0");

    stack
        .execute(&format!(
            "UPDATE tus_uploads SET state = 'expired' WHERE id = '{id}'"
        ))
        .await;
    let expired = stack.tus_send(delete(&alice, &id)).await;
    assert_tus_error(&expired, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
}

#[tokio::test]
async fn it_tus_delete_between_row_commit_and_staging_leaves_no_staging() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root.path(), &clock);
    let stack = Stack::start_with_tus(
        root.path(),
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
        },
    )
    .await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let gate = super::tus::Gate::new();
    *staging.ensure_gate.lock().unwrap() = Some(gate.clone());

    let request = stack.tus_create(&alice, &planned, Some(10));
    let driver = async {
        gate.wait_until_entered().await;
        let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
        let deleted = stack.tus_send(delete(&alice, &id)).await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        assert!(
            stack.staging_dirs().is_empty(),
            "nothing exists yet to remove"
        );
        gate.open();
        id
    };
    let (response, id) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert!(
        stack.staging_dirs().is_empty(),
        "staging created after the termination is removed: {:?}",
        stack.staging_dirs()
    );
    assert_eq!(stack.tus_row(&id).await.state, "terminated");
    assert_eq!(stack.item_state(&planned.item).await, "canceled");
    assert_eq!(stack.terminated_page().await, vec![id]);
}
