use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use tokio_util::sync::CancellationToken;

use super::folders::{Member, HOST_A};
use super::tus::{
    assert_tus_error, channel_body, header, pattern, small_limits, wait_for_offset, Gate,
    TestStaging, TusReq, TUS,
};
use super::tus_patch::{checksum_of, framed_patch, head_offset, patch, stack_small, upload_for};
use super::tus_stream::{assert_terminal_and_quiet, stack_with_limits, upload_of};
use super::*;
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::transfers::TusLimits;

const LOCKED: &str = "UPLOAD_LOCKED";

async fn wait_for(what: &str, mut condition: impl AsyncFnMut() -> bool) {
    for _ in 0..1_000 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn wait_for_lease(stack: &Stack, id: &str) {
    wait_for("the lease", async || {
        stack.tus_row(id).await.locked_by.is_some()
    })
    .await;
}

fn timestamp_at(stack: &Stack, offset: Duration) -> String {
    Timestamp::try_from(stack.clock.now() + offset)
        .unwrap()
        .to_string()
}

async fn lease_expiry(stack: &Stack, id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT lock_expires_at FROM tus_uploads WHERE id = ?1")
        .bind(id)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap()
}

async fn force_lease(stack: &Stack, id: &str, holder: &str, until: &str) {
    stack
        .execute(&format!(
            "UPDATE tus_uploads SET locked_by = '{holder}', lock_expires_at = '{until}' WHERE id = '{id}'"
        ))
        .await;
}

fn blob_len(stack: &Stack, id: &str) -> u64 {
    std::fs::metadata(
        stack
            .data_dir
            .join("uploads")
            .join(id.replace('-', ""))
            .join("blob"),
    )
    .unwrap()
    .len()
}

#[tokio::test]
async fn it_tus_patch_locked_423() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(30_000)).await;
    let data = pattern(30_000);
    let (sender, body) = channel_body();

    let first = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..2_000])))
            .unwrap();
        wait_for_lease(&stack, &id).await;
        assert_eq!(stack.tus.upload_lock_entries(), 1);

        let second = stack
            .tus_send(patch(&alice, &id, 0).bytes(&data[2_000..2_100]))
            .await;
        assert_tus_error(&second, StatusCode::LOCKED, LOCKED);
        assert!(second.json()["error"]["details"]
            .as_object()
            .unwrap()
            .is_empty());
        let overridden = stack
            .tus_send(
                TusReq::new(Method::POST, &format!("{TUS}/{id}"), &alice)
                    .header("x-http-method-override", "PATCH")
                    .header("upload-offset", "0")
                    .bytes(&data[2_000..2_100]),
            )
            .await;
        assert_tus_error(&overridden, StatusCode::LOCKED, LOCKED);
        assert!(
            stack.tus_row(&id).await.locked_by.is_some(),
            "the rejected requests did not disturb the writer's lease"
        );

        sender
            .send(Ok(Bytes::copy_from_slice(&data[2_000..9_000])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(first, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(header(&response, "upload-offset"), "9000");
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        &data[..9_000],
        "only the first writer appended"
    );
    assert!(stack.tus_row(&id).await.locked_by.is_none());
    assert_eq!(stack.tus.upload_lock_entries(), 0);
    assert_eq!(head_offset(&stack, &alice, &id).await, 9_000);

    let retried = stack
        .tus_send(patch(&alice, &id, 9_000).bytes(&data[9_000..]))
        .await;
    assert_eq!(retried.status, StatusCode::NO_CONTENT, "{}", retried.text());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
    assert_eq!(stack.item_state(&planned.item).await, "uploading");
}

#[tokio::test]
async fn it_tus_patch_stale_lease_is_taken_over_only_after_it_expires() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(10_000)).await;
    let data = pattern(10_000);

    force_lease(
        &stack,
        &id,
        "dead-instance",
        &timestamp_at(&stack, Duration::from_secs(30)),
    )
    .await;
    let held = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..1_000]))
        .await;
    assert_tus_error(&held, StatusCode::LOCKED, LOCKED);
    let row = stack.tus_row(&id).await;
    assert_eq!(row.locked_by.as_deref(), Some("dead-instance"));
    assert_eq!(blob_len(&stack, &id), 0);
    assert_eq!(
        head_offset(&stack, &alice, &id).await,
        0,
        "HEAD ignores leases"
    );

    stack.clock.advance(Duration::from_secs(31));
    let recovered = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..1_000]))
        .await;
    assert_eq!(
        recovered.status,
        StatusCode::NO_CONTENT,
        "{}",
        recovered.text()
    );
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 1_000);
    assert!(row.locked_by.is_none());

    force_lease(
        &stack,
        &id,
        "live-peer",
        &timestamp_at(&stack, Duration::from_secs(50)),
    )
    .await;
    stack.clock.advance(Duration::from_secs(10));
    let still_held = stack
        .tus_send(patch(&alice, &id, 1_000).bytes(&data[1_000..2_000]))
        .await;
    assert_tus_error(&still_held, StatusCode::LOCKED, LOCKED);
    assert_eq!(
        stack.tus_row(&id).await.locked_by.as_deref(),
        Some("live-peer"),
        "a lease that has not expired is never cleared"
    );
}

#[tokio::test]
async fn it_tus_patch_heartbeat_renews_the_lease_while_the_client_is_idle() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.lease_ttl = Duration::from_millis(300);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(10_000)).await;
    let data = pattern(10_000);
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        wait_for_lease(&stack, &id).await;
        let mut previous = lease_expiry(&stack, &id).await.unwrap();
        for _ in 0..3 {
            stack.clock.advance(Duration::from_millis(200));
            let expected = timestamp_at(&stack, Duration::from_millis(300));
            wait_for("a heartbeat", async || {
                lease_expiry(&stack, &id).await.as_deref() == Some(expected.as_str())
            })
            .await;
            let renewed = lease_expiry(&stack, &id).await.unwrap();
            assert!(renewed > previous);
            previous = renewed;
        }
        let contender = stack
            .tus_send(patch(&alice, &id, 0).bytes(&data[..10]))
            .await;
        assert_tus_error(&contender, StatusCode::LOCKED, LOCKED);
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..1_000])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert!(stack.tus_row(&id).await.locked_by.is_none());
}

#[tokio::test]
async fn it_tus_patch_stale_writer_cannot_persist_after_losing_its_lease() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(40_000)).await;
    let data = pattern(40_000);
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..3_000])))
            .unwrap();
        wait_for_lease(&stack, &id).await;
        force_lease(
            &stack,
            &id,
            "usurper",
            &timestamp_at(&stack, Duration::from_secs(60)),
        )
        .await;
        sender
            .send(Ok(Bytes::copy_from_slice(&data[3_000..20_000])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::LOCKED, LOCKED);
    let row = stack.tus_row(&id).await;
    assert_eq!(row.locked_by.as_deref(), Some("usurper"));
    assert_eq!(row.offset, 0, "the stale writer persisted nothing");
}

#[tokio::test]
async fn it_tus_patch_never_shares_a_writer_with_creation_with_upload() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(20_000)).await;
    let data = pattern(20_000);
    let (sender, body) = channel_body();

    let creation = stack.tus_send(
        stack
            .create_req(&alice, &planned, Some(20_000))
            .raw_body(body)
            .header("content-type", super::tus::OCTET)
            .header("transfer-encoding", "chunked"),
    );
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..3_000])))
            .unwrap();
        let id = upload_of(&stack, &planned.item).await;
        wait_for_lease(&stack, &id).await;

        let patched = stack
            .tus_send(patch(&alice, &id, 0).bytes(&data[..100]))
            .await;
        assert_tus_error(&patched, StatusCode::LOCKED, LOCKED);
        let repeated = stack
            .tus_send(
                stack
                    .create_req(&alice, &planned, Some(20_000))
                    .raw_body(Body::from(vec![7_u8; 100]))
                    .header("content-type", super::tus::OCTET)
                    .header("content-length", "100"),
            )
            .await;
        assert_eq!(repeated.status, StatusCode::CREATED);
        assert_eq!(
            super::tus::upload_id_of(&repeated),
            id,
            "a second creation resumes the same resource"
        );
        sender
            .send(Ok(Bytes::copy_from_slice(&data[3_000..9_000])))
            .unwrap();
        drop(sender);
        id
    };
    let (created, id) = futures_util::future::join(creation, driver).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(header(&created, "upload-offset"), "9000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..9_000]);
    assert_eq!(stack.tus_count().await, 1);
    let resumed = stack
        .tus_send(patch(&alice, &id, 9_000).bytes(&data[9_000..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
    assert_eq!(stack.tus.upload_lock_entries(), 0);
}

async fn many_uploads(stack: &Stack, alice: &Member, count: usize) -> Vec<(String, String)> {
    let files: Vec<Value> = (0..count)
        .map(|n| super::transfers::sized(&format!("c{n}"), &format!("f{n}.bin"), 1_000))
        .collect();
    let session = stack.opened(alice, &files).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let items = stack.item_ids(&session).await;
    let mut uploads = Vec::new();
    for (n, item) in items.into_iter().enumerate() {
        let planned = super::tus::Planned {
            session: session.clone(),
            item,
            name: format!("f{n}.bin"),
        };
        let id = stack.tus_created(alice, &planned, Some(1_000)).await;
        uploads.push((id, planned.item));
    }
    uploads
}

#[tokio::test]
async fn it_tus_backpressure_503() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.patch_concurrency = TusLimits::PATCH_CONCURRENCY;
    limits.permit_wait = Duration::from_millis(150);
    let (stack, staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let uploads = many_uploads(&stack, &alice, 65).await;
    let data = pattern(1_000);
    assert_eq!(stack.tus.available_patch_permits(), 64);

    let mut senders = Vec::new();
    let mut requests = Vec::new();
    for (id, _item) in uploads.iter().take(64) {
        let (sender, body) = channel_body();
        senders.push(sender);
        requests.push(stack.tus_send(framed_patch(&alice, id, 0, body)));
    }
    let all = join_all(requests);
    let driver = async {
        wait_for("every permit to be taken", async || {
            stack.tus.available_patch_permits() == 0
        })
        .await;
        let (extra_id, _) = &uploads[64];
        let rejected = stack
            .tus_send(patch(&alice, extra_id, 0).bytes(&data))
            .await;
        assert_tus_error(
            &rejected,
            StatusCode::SERVICE_UNAVAILABLE,
            "TRANSFER_BACKPRESSURE",
        );
        let retry: u64 = header(&rejected, "retry-after").parse().unwrap();
        assert!(retry >= 1);
        assert_eq!(blob_len(&stack, extra_id), 0, "no byte was accepted");
        assert_eq!(stack.tus_row(extra_id).await.offset, 0);
        assert!(stack.tus_row(extra_id).await.locked_by.is_none());
        assert_eq!(stack.tus.available_patch_permits(), 0, "no permit leaked");

        senders[0].send(Ok(Bytes::copy_from_slice(&data))).unwrap();
        let first = senders.remove(0);
        drop(first);
        wait_for("a permit to come back", async || {
            stack.tus.available_patch_permits() == 1
        })
        .await;
        let accepted = stack
            .tus_send(patch(&alice, extra_id, 0).bytes(&data))
            .await;
        assert_eq!(
            accepted.status,
            StatusCode::NO_CONTENT,
            "{}",
            accepted.text()
        );
        for sender in senders.drain(..) {
            drop(sender);
        }
    };
    let (responses, ()) = futures_util::future::join(all, driver).await;
    assert_eq!(responses.len(), 64);
    for response in &responses {
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{}",
            response.text()
        );
    }
    assert_eq!(
        stack.tus.available_patch_permits(),
        64,
        "every permit is back"
    );
    assert_eq!(stack.tus.upload_lock_entries(), 0);
    assert!(!staging.overlapped_transaction.load(Ordering::SeqCst));
}

#[tokio::test]
async fn it_tus_patch_permits_are_released_on_every_exit() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.patch_concurrency = 1;
    limits.idle_timeout = Duration::from_millis(300);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(50_000)).await;
    let data = pattern(50_000);
    let free = || stack.tus.available_patch_permits();

    let ok = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..1_000]))
        .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert_eq!(free(), 1);
    let mismatch = stack
        .tus_send(patch(&alice, &id, 5).bytes(&data[..10]))
        .await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
    assert_eq!(free(), 1);
    let bad_checksum = stack
        .tus_send(
            patch(&alice, &id, 1_000)
                .header("upload-checksum", &checksum_of(b"nope"))
                .bytes(&data[1_000..2_000]),
        )
        .await;
    assert_eq!(bad_checksum.status.as_u16(), 460);
    assert_eq!(free(), 1);
    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[1_000..1_500])))
        .unwrap();
    let idle = stack.tus_send(framed_patch(&alice, &id, 1_000, body)).await;
    assert_eq!(idle.status, StatusCode::REQUEST_TIMEOUT);
    drop(sender);
    assert_eq!(free(), 1);
    let (sender, body) = channel_body();
    sender
        .send(Err(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )))
        .unwrap();
    let _ = stack.tus_send(framed_patch(&alice, &id, 1_500, body)).await;
    drop(sender);
    assert_eq!(free(), 1);

    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 1_500, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[1_500..2_000])))
            .unwrap();
        wait_for_lease(&stack, &id).await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        drop(sender);
    };
    let (_response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(free(), 1, "a canceled upload frees its permit");
    let after = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..10]))
        .await;
    assert_eq!(after.status, StatusCode::GONE);
    assert_eq!(free(), 1);
    assert_eq!(stack.item_state(&planned.item).await, "canceled");
}

#[tokio::test]
async fn it_tus_delete_during_patch_stops_the_writer() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(60_000);

    let (after, id) = upload_for(&stack, &alice, "after.bin", Some(60_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..8_192])))
            .unwrap();
        wait_for_offset(&stack, &id, 8_192).await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        sender
            .send(Ok(Bytes::copy_from_slice(&data[8_192..24_576])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &after, &id, 8_192, &alice).await;

    let write_gate = Gate::new();
    *staging.write_gate.lock().unwrap() = Some(write_gate.clone());
    let (before, id) = upload_for(&stack, &alice, "before.bin", Some(60_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..4_096])))
            .unwrap();
        write_gate.wait_until_entered().await;
        assert_eq!(stack.tus_row(&id).await.offset, 0, "the writer is parked");
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        write_gate.open();
        sender
            .send(Ok(Bytes::copy_from_slice(&data[4_096..20_000])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &before, &id, 0, &alice).await;
    *staging.write_gate.lock().unwrap() = None;

    let flush_gate = Gate::new();
    *staging.flush_gate.lock().unwrap() = Some(flush_gate.clone());
    let (during, id) = upload_for(&stack, &alice, "during.bin", Some(60_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..9_000])))
            .unwrap();
        flush_gate.wait_until_entered().await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        flush_gate.open();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &during, &id, 0, &alice).await;
    *staging.flush_gate.lock().unwrap() = None;

    let (waiting, id) = upload_for(&stack, &alice, "waiting.bin", Some(60_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..1_000])))
            .unwrap();
        wait_for_lease(&stack, &id).await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &waiting, &id, 0, &alice).await;

    let (racing, id) = upload_for(&stack, &alice, "racing.bin", Some(60_000)).await;
    let (sender, body) = channel_body();
    let claimed = checksum_of(b"never matches");
    let request =
        stack.tus_send(framed_patch(&alice, &id, 0, body).header("upload-checksum", &claimed));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..12_000])))
            .unwrap();
        wait_for("the staged bytes", async || blob_len(&stack, &id) >= 4_096).await;
        let deleted = stack
            .tus_send(TusReq::new(Method::DELETE, &format!("{TUS}/{id}"), &alice))
            .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(&response, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_terminal_and_quiet(&stack, &racing, &id, 0, &alice).await;
}

#[tokio::test]
async fn it_tus_patch_idle_timeout_408_keeps_the_durable_bytes() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.idle_timeout = Duration::from_millis(500);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(20_000)).await;
    let held = stack.reservation(&planned.session).await;
    let data = pattern(20_000);
    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[..5_000])))
        .unwrap();

    let response = stack.tus_send(framed_patch(&alice, &id, 0, body)).await;
    assert_tus_error(
        &response,
        StatusCode::REQUEST_TIMEOUT,
        "TRANSFER_IDLE_TIMEOUT",
    );
    drop(sender);
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 5_000, "the buffered tail was made durable");
    assert!(row.locked_by.is_none());
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..5_000]);
    assert_eq!(stack.item_state(&planned.item).await, "uploading");
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert_eq!(head_offset(&stack, &alice, &id).await, 5_000);

    let resumed = stack
        .tus_send(patch(&alice, &id, 5_000).bytes(&data[5_000..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
}

#[tokio::test]
async fn it_tus_patch_checkpoint_timer_fires_without_another_frame() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.flush_bytes = u64::MAX;
    limits.flush_interval = Duration::from_millis(150);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(20_000)).await;
    let data = pattern(20_000);
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..1_000])))
            .unwrap();
        wait_for_offset(&stack, &id, 1_000).await;
        assert_eq!(
            blob_len(&stack, &id),
            1_000,
            "a small tail below the buffer size reached the disk with no further frame"
        );
        assert_eq!(head_offset(&stack, &alice, &id).await, 1_000);
        assert!(stack.tus_row(&id).await.locked_by.is_some());
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(header(&response, "upload-offset"), "1000");
}

#[tokio::test]
async fn it_tus_patch_has_no_whole_transfer_deadline() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.idle_timeout = Duration::from_millis(600);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "slow.bin", Some(16_000)).await;
    let data = pattern(16_000);
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        for frame in data.chunks(2_000) {
            sender.send(Ok(Bytes::copy_from_slice(frame))).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(sender);
    };
    let started = tokio::time::Instant::now();
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert!(started.elapsed() > Duration::from_millis(600));
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(header(&response, "upload-offset"), "16000");
}

async fn shutdown_stack(root: &Path) -> (Stack, Arc<TestStaging>, CancellationToken) {
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root, &clock);
    let token = CancellationToken::new();
    let stack = Stack::start_with_tus(
        root,
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
            shutdown: token.clone(),
        },
    )
    .await;
    *staging.probe.lock().unwrap() = Some(stack.pools.clone());
    (stack, staging, token)
}

#[tokio::test]
async fn it_tus_patch_shutdown_stops_at_a_safe_boundary() {
    let root = TempDir::new().unwrap();
    let (stack, staging, token) = shutdown_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(50_000)).await;
    let data = pattern(50_000);
    let gate = Gate::new();
    *staging.write_gate.lock().unwrap() = Some(gate.clone());
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..5_000])))
            .unwrap();
        gate.wait_until_entered().await;
        token.cancel();
        gate.open();
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(header(&response, "upload-offset"), "5000");
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 5_000);
    assert!(row.locked_by.is_none());
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..5_000]);
    assert_eq!(stack.item_state(&planned.item).await, "uploading");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);
    drop(sender);

    let refused = stack
        .tus_send(patch(&alice, &id, 5_000).bytes(&data[5_000..6_000]))
        .await;
    assert_tus_error(
        &refused,
        StatusCode::SERVICE_UNAVAILABLE,
        "SERVICE_UNAVAILABLE",
    );
    assert_eq!(head_offset(&stack, &alice, &id).await, 5_000);
}

#[tokio::test]
async fn it_tus_patch_shutdown_rolls_back_an_unverified_chunk() {
    let root = TempDir::new().unwrap();
    let (stack, staging, token) = shutdown_stack(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(50_000)).await;
    let data = pattern(50_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..2_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
    let gate = Gate::new();
    *staging.write_gate.lock().unwrap() = Some(gate.clone());
    let (sender, body) = channel_body();
    let claimed = checksum_of(&data[2_000..30_000]);

    let request =
        stack.tus_send(framed_patch(&alice, &id, 2_000, body).header("upload-checksum", &claimed));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[2_000..8_000])))
            .unwrap();
        gate.wait_until_entered().await;
        token.cancel();
        gate.open();
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(
        header(&response, "upload-offset"),
        "2000",
        "an unfinished checksummed chunk is not acknowledged"
    );
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..2_000]);
    drop(sender);
}

#[tokio::test]
async fn it_tus_patch_extends_the_expiry_but_never_past_the_session() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(10_000)).await;
    let data = pattern(10_000);
    let session_expiry: String =
        sqlx::query_scalar("SELECT expires_at FROM transfer_sessions WHERE id = ?1")
            .bind(&planned.session)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    let session_expiry: Timestamp = session_expiry.parse().unwrap();
    let imf = crate::features::transfers::tus::headers::imf_fixdate;

    stack.clock.advance(Duration::from_secs(2 * 60 * 60));
    let first = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..1_000]))
        .await;
    assert_eq!(first.status, StatusCode::NO_CONTENT);
    let expected =
        Timestamp::try_from(stack.clock.now() + Duration::from_secs(24 * 60 * 60)).unwrap();
    assert_eq!(header(&first, "upload-expires"), imf(expected));
    assert_eq!(stack.tus_row(&id).await.expires_at, expected.to_string());

    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(
        header(&head, "upload-expires"),
        imf(expected),
        "HEAD does not extend"
    );

    let offset = 1_000_u64;
    let mut last = expected;
    let remaining = |stack: &Stack| session_expiry.get() - stack.clock.now();
    while remaining(&stack) > time::Duration::hours(21) {
        stack.clock.advance(Duration::from_secs(20 * 60 * 60));
        let response = stack.tus_send(patch(&alice, &id, offset).bytes(&[])).await;
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{}",
            response.text()
        );
        let stored: Timestamp = stack.tus_row(&id).await.expires_at.parse().unwrap();
        assert!(stored >= last, "an expiry never moves back");
        assert!(
            stored <= session_expiry,
            "a child never outlives its session"
        );
        assert_eq!(header(&response, "upload-expires"), imf(stored));
        last = stored;
    }
    let step = (remaining(&stack) - time::Duration::hours(1))
        .try_into()
        .unwrap();
    stack.clock.advance(step);
    let clamped = stack.tus_send(patch(&alice, &id, offset).bytes(&[])).await;
    assert_eq!(clamped.status, StatusCode::NO_CONTENT, "{}", clamped.text());
    assert_eq!(
        header(&clamped, "upload-expires"),
        imf(session_expiry),
        "the last extension is clamped to the session"
    );

    stack.clock.advance(Duration::from_secs(2 * 60 * 60));
    let expired = stack.tus_send(patch(&alice, &id, offset).bytes(&[])).await;
    assert_tus_error(&expired, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_eq!(
        stack.tus_row(&id).await.expires_at,
        session_expiry.to_string(),
        "an expired session is not revived"
    );
}

#[tokio::test]
async fn it_tus_patch_on_an_idle_expired_resource_is_410() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(10_000)).await;
    let data = pattern(1_000);
    stack.clock.advance(Duration::from_secs(25 * 60 * 60));

    let expired = stack.tus_send(patch(&alice, &id, 0).bytes(&data)).await;
    assert_tus_error(&expired, StatusCode::GONE, "UPLOAD_SESSION_EXPIRED");
    assert_eq!(stack.tus_row(&id).await.offset, 0);
    assert!(stack.tus_row(&id).await.locked_by.is_none());
    assert_eq!(
        stack.tus_row(&id).await.state,
        "created",
        "the reaper, not PATCH, expires rows"
    );
    assert_eq!(
        head_offset_status(&stack, &alice, &id).await,
        StatusCode::GONE
    );
}

async fn head_offset_status(stack: &Stack, member: &Member, id: &str) -> StatusCode {
    stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), member))
        .await
        .status
}

#[tokio::test]
async fn it_tus_patch_connection_reset_keeps_exactly_the_accepted_bytes() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(40_000);
    let reset = || Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset));

    let (_plan, after) = upload_for(&stack, &alice, "after.bin", Some(40_000)).await;
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &after, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..9_000])))
            .unwrap();
        wait_for_offset(&stack, &after, 8_192).await;
        sender
            .send(Ok(Bytes::copy_from_slice(&data[9_000..9_700])))
            .unwrap();
        sender.send(reset()).unwrap();
        drop(sender);
    };
    let (_response, ()) = futures_util::future::join(request, driver).await;
    let row = stack.tus_row(&after).await;
    assert_eq!(
        row.offset, 9_700,
        "bytes accepted after a checkpoint are kept"
    );
    assert!(row.locked_by.is_none());
    assert_eq!(stack.staged_bytes(&after).unwrap(), &data[..9_700]);
    assert_eq!(head_offset(&stack, &alice, &after).await, 9_700);

    let (_plan, before) = upload_for(&stack, &alice, "before.bin", Some(40_000)).await;
    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[..1_000])))
        .unwrap();
    sender.send(reset()).unwrap();
    drop(sender);
    let _ = stack.tus_send(framed_patch(&alice, &before, 0, body)).await;
    let row = stack.tus_row(&before).await;
    assert_eq!(
        row.offset, 1_000,
        "bytes accepted before any checkpoint are kept"
    );
    assert!(row.locked_by.is_none());
    let resumed = stack
        .tus_send(patch(&alice, &before, 1_000).bytes(&data[1_000..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(stack.staged_bytes(&before).unwrap(), data);
}

#[tokio::test]
async fn it_tus_head_during_an_active_patch_reports_only_proven_bytes() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_plan, id) = upload_for(&stack, &alice, "a.bin", Some(40_000)).await;
    let data = pattern(40_000);
    let (sender, body) = channel_body();

    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..3_000])))
            .unwrap();
        wait_for_lease(&stack, &id).await;
        assert_eq!(
            head_offset(&stack, &alice, &id).await,
            0,
            "buffered bytes are not proven"
        );
        sender
            .send(Ok(Bytes::copy_from_slice(&data[3_000..20_000])))
            .unwrap();
        wait_for_offset(&stack, &id, 8_192).await;
        let row = stack.tus_row(&id).await;
        let head = head_offset(&stack, &alice, &id).await;
        assert!(
            head <= row.offset as u64 || head <= blob_len(&stack, &id),
            "HEAD never exceeds what is persisted and on disk"
        );
        assert!(head <= blob_len(&stack, &id));
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(head_offset(&stack, &alice, &id).await, 20_000);
}
