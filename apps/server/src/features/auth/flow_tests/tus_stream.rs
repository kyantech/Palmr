use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::folders::HOST_A;
use super::tus::{
    assert_tus_error, broken_stream, channel_body, frames, header, pattern, small_limits,
    stream_of, upload_id_of, wait_for_offset, TestStaging, TusReq, TUS,
};
use super::*;

async fn streaming_stack(root: &Path) -> (Stack, Arc<TestStaging>) {
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root, &clock);
    let stack = Stack::start_with_tus(
        root,
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
        },
    )
    .await;
    *staging.probe.lock().unwrap() = Some(stack.pools.clone());
    (stack, staging)
}

fn framed_post<'a>(
    stack: &Stack,
    member: &'a super::folders::Member,
    planned: &super::tus::Planned,
    length: Option<u64>,
    body: Body,
    declared: Option<usize>,
) -> TusReq<'a> {
    let request = stack.create_req(member, planned, length).raw_body(body);
    let request = request.header("content-type", super::tus::OCTET);
    match declared {
        Some(declared) => request.header("content-length", &declared.to_string()),
        None => request.header("transfer-encoding", "chunked"),
    }
}

#[tokio::test]
async fn it_tus_creation_with_upload_streams_through_a_bounded_buffer() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(20_000);
    let planned = stack.planned(&alice, "a.bin", Some(20_000)).await;
    let held = stack.reservation(&planned.session).await;

    let body = stream_of(frames(&data, &[1_000, 5_000, 4_096, 100, 9_000, 804]));
    let created = stack
        .tus_send(framed_post(
            &stack,
            &alice,
            &planned,
            Some(20_000),
            body,
            Some(20_000),
        ))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(header(&created, "upload-offset"), "20000");
    let id = upload_id_of(&created);

    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
    let row = stack.tus_row(&id).await;
    assert_eq!((row.offset, row.length), (20_000, Some(20_000)));
    assert_eq!(row.state, "in_progress");
    assert!(row.locked_by.is_none(), "the creation lease is released");
    assert!(
        staging.max_chunk.load(Ordering::SeqCst) <= 4_096,
        "writes never exceed the buffer"
    );
    assert!(
        staging.flushes.load(Ordering::SeqCst) >= 2,
        "offsets are flushed in windows"
    );

    assert_eq!(
        stack.item_state(&planned.item).await,
        "uploading",
        "full bytes are not completion"
    );
    assert_eq!(stack.session_row(&planned.session).await.0, "uploading");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);
    assert_eq!(stack.used_bytes(alice.id).await, 0);
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert!(!staging.overlapped_transaction.load(Ordering::SeqCst));

    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(header(&head, "upload-offset"), "20000");
    assert_eq!(header(&head, "upload-length"), "20000");
}

#[tokio::test]
async fn it_tus_creation_with_upload_persists_offsets_while_the_body_is_still_open() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10_000)).await;
    let (sender, body) = channel_body();
    let data = pattern(10_000);

    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &planned,
        Some(10_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..8_192])))
            .unwrap();
        let id: String = loop {
            let found: Option<String> = sqlx::query_scalar("SELECT id FROM tus_uploads")
                .fetch_optional(stack.pools.reader().executor())
                .await
                .unwrap();
            if let Some(found) = found {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        wait_for_offset(&stack, &id, 8_192).await;
        let mid = stack.tus_row(&id).await;
        assert_eq!(
            mid.offset, 8_192,
            "the server observed progress before the body ended"
        );
        assert!(
            mid.locked_by.is_some(),
            "the creation lease is held while streaming"
        );
        assert_eq!(stack.staged_bytes(&id).unwrap(), data[..8_192]);

        let duplicate = stack.tus_create(&alice, &planned, Some(10_000)).await;
        assert_eq!(duplicate.status, StatusCode::CREATED);
        assert_eq!(header(&duplicate, "upload-offset"), "8192");

        sender
            .send(Ok(Bytes::copy_from_slice(&data[8_192..])))
            .unwrap();
        drop(sender);
        id
    };
    let (created, id) = futures_util::future::join(request, driver).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(header(&created, "upload-offset"), "10000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
    assert!(stack.tus_row(&id).await.locked_by.is_none());
}

#[tokio::test]
async fn it_tus_creation_with_upload_keeps_an_honest_offset_after_a_disconnect() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(20_000)).await;
    let data = pattern(5_000);

    let body = broken_stream(frames(&data, &[3_000, 2_000]));
    let response = stack
        .tus_send(framed_post(
            &stack,
            &alice,
            &planned,
            Some(20_000),
            body,
            Some(20_000),
        ))
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    let id = upload_id_of(&response);
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 5_000);
    assert_eq!(row.state, "in_progress");
    assert!(row.locked_by.is_none());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);

    drop(stack);
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = super::folders::Member {
        id: alice.id,
        creds: stack.signed_in("alice", HOST_A).await,
    };
    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(header(&head, "upload-offset"), "5000");
    assert_eq!(header(&head, "upload-length"), "20000");
    assert_eq!(
        header(&head, "upload-expires"),
        "Sat, 26 Sep 2026 12:00:00 GMT"
    );
    let resumed = stack
        .tus_create(&alice, &planned_after(&planned), Some(20_000))
        .await;
    assert_eq!(
        header(&resumed, "location"),
        format!("https://files.example.test{TUS}/{id}")
    );
    assert_eq!(header(&resumed, "upload-offset"), "5000");
}

fn planned_after(planned: &super::tus::Planned) -> super::tus::Planned {
    super::tus::Planned {
        session: planned.session.clone(),
        item: planned.item.clone(),
        name: planned.name.clone(),
    }
}

#[tokio::test]
async fn it_tus_creation_with_upload_rejects_bodies_beyond_the_declared_length() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(100)).await;
    let data = pattern(150);

    let body = stream_of(frames(&data, &[60, 90]));
    let response = stack
        .tus_send(framed_post(
            &stack,
            &alice,
            &planned,
            Some(100),
            body,
            Some(150),
        ))
        .await;
    assert_tus_error(&response, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 100, "nothing past the declared length is kept");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data[..100]);
    let (state, code): (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM transfer_session_files WHERE id = ?1")
            .bind(&planned.item)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), code.as_deref()),
        ("failed", Some("FILE_TOO_LARGE"))
    );
    assert_eq!(stack.session_row(&planned.session).await.0, "failed");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);

    let zero = stack.planned(&alice, "z.bin", Some(0)).await;
    let body = stream_of(frames(&[1], &[1]));
    let response = stack
        .tus_send(framed_post(&stack, &alice, &zero, Some(0), body, Some(1)))
        .await;
    assert_tus_error(&response, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
}

#[tokio::test]
async fn it_tus_creation_with_upload_enforces_the_size_policy_for_deferred_lengths() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_max_file_size(5_000).await;
    let planned = stack.planned(&alice, "a.bin", None).await;
    let data = pattern(6_000);

    let body = stream_of(frames(&data, &[3_000, 3_000]));
    let response = stack
        .tus_send(framed_post(&stack, &alice, &planned, None, body, None))
        .await;
    assert_tus_error(&response, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(response.json()["error"]["details"]["maxBytes"], 5_000);
    let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(stack.tus_row(&id).await.offset, 5_000);
}

#[tokio::test]
async fn it_tus_creation_with_upload_runs_the_quota_check_for_unreserved_sizes() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(3_000)).await;
    let planned = stack.planned(&alice, "a.bin", None).await;
    assert_eq!(
        stack.reservation(&planned.session).await.1,
        0,
        "unlimited size reserves nothing"
    );
    let data = pattern(5_000);

    let body = stream_of(frames(&data, &[2_000, 3_000]));
    let response = stack
        .tus_send(framed_post(&stack, &alice, &planned, None, body, None))
        .await;
    assert_tus_error(
        &response,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        stack.tus_row(&id).await.offset,
        3_000,
        "bytes stop at the quota"
    );
    assert_eq!(stack.used_bytes(alice.id).await, 0, "nothing is committed");
    let (state, code): (String, Option<String>) =
        sqlx::query_as("SELECT state, error_code FROM transfer_session_files WHERE id = ?1")
            .bind(&planned.item)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), code.as_deref()),
        ("failed", Some("QUOTA_EXCEEDED"))
    );
}

#[tokio::test]
async fn it_tus_creation_with_upload_survives_a_storage_write_failure() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10_000)).await;
    staging.fail_write_after.store(4_096, Ordering::SeqCst);
    let data = pattern(10_000);

    let body = stream_of(frames(&data, &[5_000, 5_000]));
    let response = stack
        .tus_send(framed_post(
            &stack,
            &alice,
            &planned,
            Some(10_000),
            body,
            Some(10_000),
        ))
        .await;
    assert_tus_error(
        &response,
        StatusCode::INSUFFICIENT_STORAGE,
        "STORAGE_WRITE_FAILED",
    );
    assert!(response.json()["error"]["details"]
        .as_object()
        .unwrap()
        .is_empty());
    let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    let row = stack.tus_row(&id).await;
    assert_eq!(
        row.offset, 4_096,
        "only bytes that reached disk are acknowledged"
    );
    assert!(row.locked_by.is_none());
    assert_eq!(
        stack.item_state(&planned.item).await,
        "uploading",
        "a retryable fault is not a failure"
    );
    assert_eq!(stack.staged_bytes(&id).unwrap().len(), 4_096);
}

#[tokio::test]
async fn it_tus_creation_with_upload_handles_empty_and_unframed_bodies() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;

    let empty = stack.planned(&alice, "empty.bin", Some(10)).await;
    let response = stack
        .tus_send(
            stack
                .create_req(&alice, &empty, Some(10))
                .header("content-type", super::tus::OCTET)
                .header("content-length", "0"),
        )
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(header(&response, "upload-offset"), "0");
    assert_eq!(
        stack.tus_row(&upload_id_of(&response)).await.state,
        "created"
    );

    let chunked = stack.planned(&alice, "chunked.bin", None).await;
    let data = pattern(300);
    let body = stream_of(frames(&data, &[100, 200]));
    let response = stack
        .tus_send(framed_post(&stack, &alice, &chunked, None, body, None))
        .await;
    assert_eq!(response.status, StatusCode::CREATED);
    assert_eq!(header(&response, "upload-offset"), "300");

    let wrong_type = stack.planned(&alice, "typed.bin", Some(5)).await;
    let before = stack.tus_count().await;
    let response = stack
        .tus_send(
            stack
                .create_req(&alice, &wrong_type, Some(5))
                .raw_body(Body::from("hello"))
                .header("content-type", "text/plain")
                .header("content-length", "5"),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(response.error_code(), "UNSUPPORTED_MEDIA_TYPE");
    let untyped = stack
        .tus_send(
            stack
                .create_req(&alice, &wrong_type, Some(5))
                .raw_body(Body::from("hello"))
                .header("content-length", "5"),
        )
        .await;
    assert_eq!(untyped.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(stack.tus_count().await, before);
    assert_eq!(stack.item_state(&wrong_type.item).await, "pending");
}

#[tokio::test]
async fn it_tus_creation_with_upload_stops_when_the_upload_is_terminated() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(30_000)).await;
    let (sender, body) = channel_body();
    let data = pattern(30_000);

    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &planned,
        Some(30_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..8_192])))
            .unwrap();
        let id: String = loop {
            if let Some(found) = sqlx::query_scalar::<_, String>("SELECT id FROM tus_uploads")
                .fetch_optional(stack.pools.reader().executor())
                .await
                .unwrap()
            {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        wait_for_offset(&stack, &id, 8_192).await;
        let terminated = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(terminated.status, StatusCode::NO_CONTENT);
        sender
            .send(Ok(Bytes::copy_from_slice(&data[8_192..16_384])))
            .unwrap();
        drop(sender);
        id
    };
    let (response, id) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    let row = stack.tus_row(&id).await;
    assert_eq!(
        row.state, "terminated",
        "a terminated upload is never resurrected"
    );
    assert_eq!(row.offset, 8_192);
    assert_eq!(stack.item_state(&planned.item).await, "canceled");
}

async fn stack_with_limits(
    root: &Path,
    limits: crate::features::transfers::TusLimits,
) -> (Stack, Arc<TestStaging>) {
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root, &clock);
    let stack = Stack::start_with_tus(
        root,
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits,
        },
    )
    .await;
    (stack, staging)
}

#[tokio::test]
async fn it_tus_creation_stream_idle_timeout_408() {
    assert_eq!(
        crate::features::transfers::TusLimits::production(262_144).idle_timeout,
        Duration::from_secs(60),
        "the production idle timeout is per frame"
    );
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.idle_timeout = Duration::from_millis(600);
    let (stack, staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(1_000_000)).await;
    let held = stack.reservation(&planned.session).await;
    let data = pattern(9_692);
    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[..8_192])))
        .unwrap();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[8_192..])))
        .unwrap();

    let response = stack
        .tus_send(framed_post(
            &stack,
            &alice,
            &planned,
            Some(1_000_000),
            body,
            None,
        ))
        .await;
    assert_tus_error(
        &response,
        StatusCode::REQUEST_TIMEOUT,
        "TRANSFER_IDLE_TIMEOUT",
    );
    assert!(response.json()["error"]["details"]
        .as_object()
        .unwrap()
        .is_empty());

    let id: String = sqlx::query_scalar("SELECT id FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    let row = stack.tus_row(&id).await;
    assert_eq!(
        row.offset, 9_692,
        "every byte accepted before the stall is acknowledged"
    );
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        data,
        "and exactly those bytes are on disk"
    );
    assert_eq!(row.state, "in_progress");
    assert!(
        row.locked_by.is_none(),
        "the stalled request released its lease"
    );
    assert!(
        staging.max_chunk.load(Ordering::SeqCst) <= 4_096,
        "nothing was collected whole"
    );
    assert_eq!(
        stack.item_state(&planned.item).await,
        "uploading",
        "a stall is retryable, not a failure"
    );
    assert_eq!(stack.session_row(&planned.session).await.0, "uploading");
    assert_eq!(
        stack.reservation(&planned.session).await,
        held,
        "the hold is untouched"
    );
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);

    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(header(&head, "upload-offset"), "9692");
    let resumed = stack.tus_create(&alice, &planned, Some(1_000_000)).await;
    assert_eq!(resumed.status, StatusCode::CREATED);
    assert_eq!(header(&resumed, "upload-offset"), "9692");
    let deleted = stack
        .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(
        deleted.status,
        StatusCode::NO_CONTENT,
        "the stalled upload is still terminable"
    );
    assert_eq!(stack.reservation(&planned.session).await.0, "released");
}

#[tokio::test]
async fn it_tus_creation_stream_has_no_whole_transfer_deadline() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.idle_timeout = Duration::from_millis(600);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(16_000);
    let planned = stack.planned(&alice, "slow.bin", Some(16_000)).await;
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &planned,
        Some(16_000),
        body,
        None,
    ));
    let driver = async {
        for frame in data.chunks(2_000) {
            sender.send(Ok(Bytes::copy_from_slice(frame))).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(sender);
    };
    let started = tokio::time::Instant::now();
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert!(
        started.elapsed() > Duration::from_millis(600),
        "the transfer outlived a single idle window"
    );
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(header(&response, "upload-offset"), "16000");
}

#[tokio::test]
async fn it_tus_creation_with_upload_flushes_on_the_interval_not_only_on_size() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.flush_bytes = u64::MAX;
    limits.flush_interval = Duration::from_millis(150);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(5_000)).await;
    let (sender, body) = channel_body();
    let data = pattern(5_000);

    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &planned,
        Some(5_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..1_000])))
            .unwrap();
        let id: String = loop {
            if let Some(found) = sqlx::query_scalar::<_, String>("SELECT id FROM tus_uploads")
                .fetch_optional(stack.pools.reader().executor())
                .await
                .unwrap()
            {
                break found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(
            stack.tus_row(&id).await.offset,
            0,
            "no window has elapsed yet"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        sender
            .send(Ok(Bytes::copy_from_slice(&data[1_000..2_000])))
            .unwrap();
        wait_for_offset(&stack, &id, 2_000).await;
        sender
            .send(Ok(Bytes::copy_from_slice(&data[2_000..])))
            .unwrap();
        drop(sender);
        id
    };
    let (response, id) = futures_util::future::join(request, driver).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(stack.tus_row(&id).await.offset, 5_000);
}

async fn upload_of(stack: &Stack, item: &str) -> String {
    loop {
        if let Some(found) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM tus_uploads WHERE transfer_session_file_id = ?1",
        )
        .bind(item)
        .fetch_optional(stack.pools.reader().executor())
        .await
        .unwrap()
        {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn assert_terminal_and_quiet(
    stack: &Stack,
    planned: &super::tus::Planned,
    id: &str,
    offset: i64,
    alice: &super::folders::Member,
) {
    let row = stack.tus_row(id).await;
    assert_eq!(row.state, "terminated", "a canceled upload is terminal");
    assert_eq!(
        row.offset, offset,
        "no offset advanced past the termination"
    );
    assert_eq!(stack.item_state(&planned.item).await, "canceled");
    assert_eq!(stack.session_row(&planned.session).await.0, "canceled");
    let (state, _, _, reason) = stack.reservation(&planned.session).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("released", Some("canceled"))
    );
    assert_eq!(stack.counts().await.held, 0);
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);
    assert_eq!(stack.used_bytes(alice.id).await, 0);
    assert!(
        stack.staging_dirs().is_empty(),
        "staging is not recreated: {:?}",
        stack.staging_dirs()
    );

    let mut connection = stack.pools.reader().executor().acquire().await.unwrap();
    let pending: Vec<String> =
        crate::features::transfers::cleanup::terminated_tus_page(&mut connection, None, 1_000)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
    assert!(
        pending.contains(&id.to_owned()),
        "the terminal row stays discoverable"
    );
    drop(connection);

    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), alice))
        .await;
    assert_eq!(head.status, StatusCode::GONE);
    let before = stack.dump().await;
    let again = stack
        .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), alice))
        .await;
    assert_eq!(
        again.status,
        StatusCode::NO_CONTENT,
        "DELETE stays idempotent"
    );
    assert_eq!(stack.dump().await, before, "the repeat changes nothing");
}

#[tokio::test]
async fn it_tus_delete_during_creation_stream_stops_writer() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = streaming_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(40_000);

    let after_guarded_work = stack.planned(&alice, "first.bin", Some(40_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &after_guarded_work,
        Some(40_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..8_192])))
            .unwrap();
        let id = upload_of(&stack, &after_guarded_work.item).await;
        wait_for_offset(&stack, &id, 8_192).await;
        assert_eq!(stack.tus_row(&id).await.offset, 8_192);
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        sender
            .send(Ok(Bytes::copy_from_slice(&data[8_192..24_576])))
            .unwrap();
        drop(sender);
        id
    };
    let (response, id) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &after_guarded_work, &id, 8_192, &alice).await;

    let write_gate = super::tus::Gate::new();
    *staging.write_gate.lock().unwrap() = Some(write_gate.clone());
    let before_next_operation = stack.planned(&alice, "second.bin", Some(40_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &before_next_operation,
        Some(40_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..4_096])))
            .unwrap();
        write_gate.wait_until_entered().await;
        let id = upload_of(&stack, &before_next_operation.item).await;
        assert_eq!(
            stack.tus_row(&id).await.offset,
            0,
            "the writer is parked mid-write"
        );
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        write_gate.open();
        sender
            .send(Ok(Bytes::copy_from_slice(&data[4_096..20_000])))
            .unwrap();
        drop(sender);
        id
    };
    let (response, id) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &before_next_operation, &id, 0, &alice).await;
    *staging.write_gate.lock().unwrap() = None;

    let disconnected = stack.planned(&alice, "third.bin", Some(40_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_post(
        &stack,
        &alice,
        &disconnected,
        Some(40_000),
        body,
        None,
    ));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..3_000])))
            .unwrap();
        let id = upload_of(&stack, &disconnected.item).await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        sender
            .send(Err(std::io::Error::from(
                std::io::ErrorKind::ConnectionReset,
            )))
            .unwrap();
        drop(sender);
        id
    };
    let (_response, id) = futures_util::future::join(request, driver).await;
    assert_terminal_and_quiet(&stack, &disconnected, &id, 0, &alice).await;
    assert!(!staging.overlapped_transaction.load(Ordering::SeqCst));
}
