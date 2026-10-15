use std::io::Cursor;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64ct::{Base64, Encoding};
use futures_util::future::join_all;
use sha2::{Digest, Sha256};

use super::folders::Member;
use super::s3_multipart::{timeout, upstream, PartFault, PartGate, RigOptions, S3Rig};
use super::transfers::{request_body, sized};
use super::tus::{broken_stream, channel_body, frames, pattern, stream_of, TusReq};
use super::*;
use crate::config::S3Profile;
use crate::domain::bytes::ByteSize;
use crate::features::transfers::ProxyLimits;
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    MultipartStorage as _, PartChecksum, PutHint, StorageProvider as _,
};
use crate::storage::s3::profile::{GIB, MIB};

const PART: u64 = 8 * MIB;
const CAP: u64 = PART * 9_900;
const OCTET_STREAM: &str = "application/octet-stream";

fn base64_sha256(bytes: &[u8]) -> String {
    Base64::encode_string(&Sha256::digest(bytes))
}

fn limits(permit_wait: Duration, idle: Duration) -> ProxyLimits {
    ProxyLimits {
        concurrency: 8,
        permit_wait,
        idle_timeout: idle,
    }
}

pub(super) fn steady() -> ProxyLimits {
    limits(Duration::from_millis(150), Duration::from_secs(5))
}

pub(super) async fn proxied_rig(proxy: ProxyLimits) -> S3Rig {
    let rig = S3Rig::build(RigOptions {
        proxy: Some(proxy),
        ..RigOptions::default()
    })
    .await;
    rig.proxied.store(true, Ordering::SeqCst);
    rig
}

fn part_path(session: &str, item: &str, number: &str) -> String {
    S3Rig::path(session, item, &format!("/parts/{number}"))
}

impl S3Rig {
    pub(super) fn request<'a>(
        &'a self,
        who: &'a Member,
        session: &str,
        item: &str,
        number: &str,
    ) -> TusReq<'a> {
        TusReq::new(Method::PUT, &part_path(session, item, number), who)
            .no_version()
            .header("content-type", OCTET_STREAM)
    }

    pub(super) async fn put(&self, request: TusReq<'_>) -> Fetched {
        self.stack.tus_send(request).await
    }

    pub(super) async fn put_bytes(
        &self,
        session: &str,
        item: &str,
        number: &str,
        data: &Bytes,
    ) -> Fetched {
        let request = self
            .request(&self.alice, session, item, number)
            .header("content-length", &data.len().to_string())
            .raw_body(Body::from(data.clone()));
        self.put(request).await
    }

    pub(super) async fn part_row(
        &self,
        item: &str,
        number: i64,
    ) -> (String, Option<String>, i64, i64) {
        sqlx::query_as(
            "SELECT p.state, p.etag, p.size_bytes, p.attempts
               FROM s3_multipart_parts p
               JOIN s3_multipart_uploads u ON u.id = p.s3_multipart_upload_id
              WHERE u.transfer_session_file_id = ?1 AND p.part_number = ?2",
        )
        .bind(item)
        .bind(number)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn uploaded_parts(&self, item: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM s3_multipart_parts p
               JOIN s3_multipart_uploads u ON u.id = p.s3_multipart_upload_id
              WHERE u.transfer_session_file_id = ?1 AND p.state IN ('uploaded','verified')",
        )
        .bind(item)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) fn provider_part_calls(&self) -> usize {
        self.faults.part_calls.load(Ordering::SeqCst)
    }

    pub(super) fn proxy_snapshot(&self) -> crate::features::transfers::s3::proxied::BufferSnapshot {
        self.stack.s3_handle.as_ref().unwrap().proxy_snapshot()
    }

    pub(super) async fn multi(&self, count: usize, size: u64) -> (String, Vec<String>) {
        let files: Vec<Value> = (1..=count)
            .map(|n| sized(&format!("c{n}"), &format!("f{n}.bin"), size))
            .collect();
        let (session, items) = self.session(&files).await;
        for item in &items {
            let created = self.create(&session, item).await;
            assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        }
        (session, items)
    }
}

fn reason(fetched: &Fetched) -> String {
    fetched.json()["error"]["details"]["reason"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn expect_invalid_part(fetched: &Fetched, expected: &str) {
    assert_eq!(
        fetched.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{expected}: {}",
        fetched.text()
    );
    assert_eq!(fetched.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(reason(fetched), expected, "{}", fetched.text());
}

#[tokio::test]
async fn regression_381_provider_checksum_rejection() {
    checksum_flow(S3Rig::checksum_demanding().await).await;
}

#[tokio::test]
async fn regression_381_generic_provider_with_an_explicit_checksum_requirement() {
    let rig = S3Rig::build(RigOptions {
        checksum_gate: true,
        profile: Some(S3Profile::Generic),
        require_part_checksums: Some(true),
        ..RigOptions::default()
    })
    .await;
    assert_eq!(
        rig.stack
            .transfer_storage()
            .part_layout(ByteSize::try_from(1_u64).unwrap()),
        Some((PART, 1))
    );
    checksum_flow(rig).await;
}

async fn checksum_flow(rig: S3Rig) {
    let gate = rig.gate.as_ref().unwrap();
    assert!(
        rig.provider.caps().requires_checksum_headers,
        "the profile, not an error string, demands checksums"
    );
    let size = 2 * PART + 4 * MIB + 321;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let (session, item) = rig.one("checksum.bin", size).await;

    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["checksumHeadersRequired"], true);
    assert_eq!(created.json()["partSizeBytes"], PART);
    assert_eq!(created.json()["partCount"], 3);

    let signs = rig.faults.signs.load(Ordering::SeqCst);
    let presigned = rig.presign(&session, &item, &[1]).await;
    expect_invalid_part(&presigned, "server_proxied_part_upload");
    assert_eq!(rig.faults.signs.load(Ordering::SeqCst), signs);

    let control_key = ObjectKey::parse(&rig.final_key(&item).await).unwrap();
    let control = rig
        .provider
        .create_multipart(&control_key, PutHint::default())
        .await
        .unwrap();
    let rejected_before = gate.rejected();
    let refused = rig
        .provider
        .upload_part_stream(&control, 1, 3, Box::pin(Cursor::new(vec![1_u8, 2, 3])))
        .await;
    assert!(
        matches!(refused, Err(StorageError::S3(_))),
        "the provider layer must refuse a part without a checksum: {refused:?}"
    );
    assert_eq!(gate.rejected(), rejected_before + 1);
    let forged = rig
        .provider
        .upload_part_checksummed(
            &control,
            1,
            3,
            Box::pin(Cursor::new(vec![1_u8, 2, 3])),
            PartChecksum::sha256([0; 32]),
        )
        .await
        .unwrap_err();
    eprintln!("MinIO answer to a wrong part checksum: {forged:?}");
    assert!(
        crate::storage::s3::is_checksum_rejection(&forged),
        "the provider verifies the digest it is sent: {forged:?}"
    );
    rig.provider.abort_multipart(&control).await.unwrap();

    let mut expected = Vec::new();
    let mut etags = Vec::new();
    for number in 1..=3_usize {
        let start = (number - 1) * usize::try_from(PART).unwrap();
        let end = (number * usize::try_from(PART).unwrap()).min(data.len());
        let slice = data.slice(start..end);
        expected.push(base64_sha256(&slice));
        let stored = rig
            .put_bytes(&session, &item, &number.to_string(), &slice)
            .await;
        assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
        let view = stored.json();
        assert_eq!(view["partNumber"], number);
        assert_eq!(view["sizeBytes"], slice.len());
        etags.push(view["etag"].as_str().unwrap().to_owned());
        let (state, etag, size_bytes, attempts) = rig.part_row(&item, number as i64).await;
        assert_eq!(state, "uploaded");
        assert_eq!(etag.as_deref(), view["etag"].as_str());
        assert_eq!(size_bytes, i64::try_from(slice.len()).unwrap());
        assert_eq!(attempts, 1);
    }
    let forwarded = gate.forwarded_checksums();
    assert_eq!(
        forwarded[0],
        PartChecksum::sha256([0; 32]).to_base64(),
        "the control upload with a forged digest is the only other checksummed part"
    );
    assert_eq!(
        forwarded[1..],
        expected,
        "every UploadPart carried the SHA-256 of exactly the received bytes"
    );
    let logged: Vec<String> = rig
        .faults
        .part_log
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, checksum)| checksum.to_base64())
        .collect();
    assert_eq!(logged, expected);
    assert_eq!(gate.rejected(), rejected_before + 1);
    assert_eq!(rig.faults.signs.load(Ordering::SeqCst), signs);

    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.json()["missingPartNumbers"], json!([]));
    assert_eq!(listed.json()["uploadedBytes"], size);
    let provider_etags: Vec<String> = listed.json()["completedParts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| part["etag"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(provider_etags, etags, "ListParts reports the real parts");
    assert_eq!(rig.uploaded_parts(&item).await, 3);

    assert_eq!(rig.used().await, 0, "nothing is charged per part");
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(completed.json()["sizeBytes"], size);
    let key = rig.final_key(&item).await;
    let (stored_bytes, digest) = rig.read_object(&key).await;
    assert_eq!(stored_bytes, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());
    assert_eq!(rig.files().await, 1);
    assert_eq!(rig.objects().await, 1);
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(rig.uploads_audited().await, 1);
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("completed")
    );
    assert_eq!(rig.open_uploads(&key).await, 0);
    rig.assert_no_overlap();

    let upload_id: String = sqlx::query_scalar(
        "SELECT s3_upload_id FROM s3_multipart_uploads WHERE transfer_session_file_id = ?1",
    )
    .bind(&item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    rig.leaked(
        &[created.text(), listed.text(), completed.text()],
        &key,
        &upload_id,
    );
}

#[tokio::test]
async fn it_s3_proxied_part_memory_bounded() {
    let rig = proxied_rig(steady()).await;
    let (session, items) = rig.multi(10, PART).await;
    let body = Bytes::from(pattern(usize::try_from(PART).unwrap()));

    let single = rig.put_bytes(&session, &items[0], "1", &body).await;
    assert_eq!(single.status, StatusCode::OK, "{}", single.text());
    let one = rig.proxy_snapshot();
    assert_eq!(
        (
            one.active_permits,
            one.active_buffers,
            one.active_capacity_bytes
        ),
        (0, 0, 0),
        "every permit and buffer is returned"
    );
    assert_eq!(
        (one.peak_permits, one.peak_buffers, one.peak_capacity_bytes),
        (1, 1, PART)
    );
    assert_eq!(rig.provider_part_calls(), 1);

    let gate = PartGate::new();
    *rig.faults.part_gate.lock().unwrap() = Some(gate.clone());
    let eight = join_all((1..=8).map(|index| rig.put_bytes(&session, &items[index], "1", &body)));
    let contention = async {
        gate.entered.acquire_many(8).await.unwrap().forget();
        let held = rig.proxy_snapshot();
        assert_eq!(held.active_permits, 8, "eight proxied parts are in flight");
        assert_eq!(held.active_buffers, 8);
        assert_eq!(held.active_capacity_bytes, 8 * PART);
        assert_eq!(rig.provider_part_calls(), 9);

        let ninth = rig.put_bytes(&session, &items[9], "1", &body).await;
        assert_eq!(
            ninth.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            ninth.text()
        );
        assert_eq!(ninth.error_code(), "TRANSFER_BACKPRESSURE");
        assert_eq!(ninth.headers.get("retry-after").unwrap(), "1");
        let waiting = rig.proxy_snapshot();
        assert_eq!(
            (waiting.active_permits, waiting.active_buffers),
            (8, 8),
            "the ninth request allocated nothing while it waited"
        );
        assert_eq!(waiting.peak_buffers, 8);
        assert_eq!(rig.provider_part_calls(), 9, "the ninth never reached S3");
        assert_eq!(rig.part_row(&items[9], 1).await.0, "planned");
        gate.release.add_permits(8);
    };
    let (results, ()) = tokio::join!(eight, contention);
    for fetched in &results {
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    }
    *rig.faults.part_gate.lock().unwrap() = None;
    let drained = rig.proxy_snapshot();
    assert_eq!(
        (
            drained.active_permits,
            drained.active_buffers,
            drained.active_capacity_bytes
        ),
        (0, 0, 0)
    );
    assert_eq!(drained.peak_permits, 8);
    assert_eq!(drained.peak_buffers, 8);
    assert_eq!(drained.peak_capacity_bytes, 8 * PART);
    assert!(drained.peak_capacity_bytes <= 64 * MIB);

    let after = rig.put_bytes(&session, &items[9], "1", &body).await;
    assert_eq!(
        after.status,
        StatusCode::OK,
        "the refused part uploads once there is room"
    );

    let calls = rig.provider_part_calls();
    let oversized = Bytes::from(pattern(usize::try_from(PART).unwrap() + 1));
    let refused = rig
        .put(
            rig.request(&rig.alice, &session, &items[0], "1")
                .raw_body(Body::from(oversized)),
        )
        .await;
    expect_invalid_part(&refused, "body_too_long");
    assert_eq!(rig.provider_part_calls(), calls, "no byte reaches S3");

    let calls = rig.provider_part_calls();
    let cut = rig
        .put(
            rig.request(&rig.alice, &session, &items[0], "1")
                .raw_body(broken_stream(frames(
                    &body.slice(..71_000),
                    &[1_000, 70_000],
                ))),
        )
        .await;
    expect_invalid_part(&cut, "body_interrupted");
    assert_eq!(rig.provider_part_calls(), calls);

    rig.faults
        .part
        .lock()
        .unwrap()
        .push_back(PartFault::Before(upstream));
    let failed = rig.put_bytes(&session, &items[8], "1", &body).await;
    assert_eq!(
        failed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        failed.text()
    );
    assert_eq!(failed.error_code(), "STORAGE_UNAVAILABLE");
    rig.faults
        .part
        .lock()
        .unwrap()
        .push_back(PartFault::Before(timeout));
    let timed_out = rig.put_bytes(&session, &items[8], "1", &body).await;
    assert_eq!(timed_out.error_code(), "STORAGE_UNAVAILABLE");

    let end = rig.proxy_snapshot();
    assert_eq!(
        (
            end.active_permits,
            end.active_buffers,
            end.active_capacity_bytes
        ),
        (0, 0, 0),
        "failures return every permit and buffer"
    );
    assert!(end.peak_buffers <= 8 && end.peak_capacity_bytes <= 64 * MIB);
    eprintln!(
        "proxied part gauge: peak permits {}, peak buffers {}, peak buffer capacity {} bytes",
        end.peak_permits, end.peak_buffers, end.peak_capacity_bytes
    );
}

#[tokio::test]
async fn it_s3_proxied_shutdown_releases_permits_and_refuses_new_parts() {
    let token = tokio_util::sync::CancellationToken::new();
    let rig = S3Rig::build(RigOptions {
        proxy: Some(steady()),
        shutdown: Some(token.clone()),
        ..RigOptions::default()
    })
    .await;
    rig.proxied.store(true, Ordering::SeqCst);
    let (session, items) = rig.multi(2, PART).await;
    let (sender, stream) = channel_body();
    let calls = rig.provider_part_calls();
    let hanging = rig.put(
        rig.request(&rig.alice, &session, &items[0], "1")
            .raw_body(stream),
    );
    let stopper = async {
        sender
            .send(Ok(Bytes::from_static(b"partial")))
            .expect("the body is still open");
        while rig.proxy_snapshot().active_permits == 0 {
            tokio::task::yield_now().await;
        }
        token.cancel();
    };
    let (stopped, ()) = tokio::join!(hanging, stopper);
    assert_eq!(
        stopped.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        stopped.text()
    );
    assert_eq!(stopped.error_code(), "SERVICE_UNAVAILABLE");
    assert_eq!(rig.provider_part_calls(), calls);
    let after = rig.proxy_snapshot();
    assert_eq!((after.active_permits, after.active_buffers), (0, 0));

    let body = Bytes::from(pattern(usize::try_from(PART).unwrap()));
    let refused = rig.put_bytes(&session, &items[1], "1", &body).await;
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(rig.provider_part_calls(), calls);
    assert_eq!(rig.part_row(&items[0], 1).await.0, "planned");
}

#[tokio::test]
async fn it_s3_proxied_part_http_semantics() {
    let rig = proxied_rig(limits(
        Duration::from_millis(100),
        Duration::from_millis(300),
    ))
    .await;
    let part = usize::try_from(PART).unwrap();
    let size = 2 * PART + 4 * MIB + 7;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let slice = |number: usize| {
        let start = (number - 1) * part;
        data.slice(start..(number * part).min(data.len()))
    };
    let (session, item) = rig.one("semantics.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let calls = || rig.provider_part_calls();
    let chunked = |number: &str, body: Body| {
        rig.request(&rig.alice, &session, &item, number)
            .header("transfer-encoding", "chunked")
            .raw_body(body)
    };

    let first = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    assert_eq!(first.json()["sizeBytes"], PART);
    assert_eq!(calls(), 1);

    let short_header = slice(2).slice(..part - 1);
    let refused = rig.put_bytes(&session, &item, "2", &short_header).await;
    expect_invalid_part(&refused, "content_length_mismatch");
    let refused = rig
        .put(chunked("2", Body::from(short_header.clone())))
        .await;
    expect_invalid_part(&refused, "body_too_short");
    let refused = rig
        .put(chunked(
            "2",
            stream_of(frames(
                &slice(2).slice(..part - 5),
                &[1 << 20, 3 << 20, (4 << 20) - 5],
            )),
        ))
        .await;
    expect_invalid_part(&refused, "body_too_short");
    let mut longer = slice(2).to_vec();
    longer.push(0);
    let longer = Bytes::from(longer);
    expect_invalid_part(
        &rig.put_bytes(&session, &item, "2", &longer).await,
        "content_length_mismatch",
    );
    expect_invalid_part(
        &rig.put(chunked("2", Body::from(longer.clone()))).await,
        "body_too_long",
    );
    expect_invalid_part(
        &rig.put(chunked(
            "2",
            stream_of(vec![slice(2), Bytes::from_static(b"x")]),
        ))
        .await,
        "body_too_long",
    );
    assert_eq!(calls(), 1, "no short, long or truncated body reaches S3");

    let missing = rig.put(rig.request(&rig.alice, &session, &item, "2")).await;
    expect_invalid_part(&missing, "body_too_short");
    let empty = rig
        .put(
            rig.request(&rig.alice, &session, &item, "2")
                .header("content-length", "0"),
        )
        .await;
    expect_invalid_part(&empty, "content_length_mismatch");
    for malformed in ["abc", "-1", "8388608x", "0x800000", "+8388608", " "] {
        let refused = rig
            .put(
                rig.request(&rig.alice, &session, &item, "2")
                    .header("content-length", malformed)
                    .raw_body(Body::from(slice(2))),
            )
            .await;
        expect_invalid_part(&refused, "content_length_mismatch");
    }
    let repeated = rig
        .put(
            rig.request(&rig.alice, &session, &item, "2")
                .header("content-length", "8388608")
                .header("content-length", "8388608")
                .raw_body(Body::from(slice(2))),
        )
        .await;
    expect_invalid_part(&repeated, "content_length_mismatch");
    assert_eq!(calls(), 1);

    let framed = rig
        .put(chunked("2", stream_of(frames(&slice(2), &[1 << 20; 8]))))
        .await;
    assert_eq!(framed.status, StatusCode::OK, "{}", framed.text());
    assert_eq!(calls(), 2);

    let final_part = slice(3);
    let mut padded = final_part.to_vec();
    padded.push(1);
    expect_invalid_part(
        &rig.put(chunked("3", Body::from(padded))).await,
        "body_too_long",
    );
    let (sender, stream) = channel_body();
    let trickle = rig.put(chunked("3", stream));
    let feeder = async {
        let mut at = 0;
        let step = final_part.len().div_ceil(5);
        while at < final_part.len() {
            let end = (at + step).min(final_part.len());
            sender.send(Ok(final_part.slice(at..end))).unwrap();
            at = end;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(sender);
    };
    let (slow, ()) = tokio::join!(trickle, feeder);
    assert_eq!(slow.status, StatusCode::OK, "{}", slow.text());
    assert_eq!(slow.json()["sizeBytes"], final_part.len());
    assert_eq!(calls(), 3);

    let stalled_calls = calls();
    let (sender, stream) = channel_body();
    sender.send(Ok(Bytes::from_static(&[7; 1_000]))).unwrap();
    let stalled = rig.put(chunked("3", stream)).await;
    assert_eq!(
        stalled.status,
        StatusCode::REQUEST_TIMEOUT,
        "{}",
        stalled.text()
    );
    assert_eq!(stalled.error_code(), "TRANSFER_IDLE_TIMEOUT");
    drop(sender);
    assert_eq!(calls(), stalled_calls);

    let (sender, stream) = channel_body();
    let long_running = rig.put(chunked("3", stream));
    let began = std::time::Instant::now();
    let steady_feed = async {
        let step = final_part.len().div_ceil(20);
        let mut at = 0;
        while at < final_part.len() {
            let end = (at + step).min(final_part.len());
            sender.send(Ok(final_part.slice(at..end))).unwrap();
            at = end;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(sender);
    };
    let (progressing, ()) = tokio::join!(long_running, steady_feed);
    assert!(
        began.elapsed() > Duration::from_millis(1_800),
        "the part took longer than 6 idle windows"
    );
    assert_eq!(
        progressing.status,
        StatusCode::OK,
        "continuous progress is never cut off by the total duration: {}",
        progressing.text()
    );
    assert_eq!(calls(), stalled_calls + 1);

    let cut = rig
        .put(chunked(
            "3",
            broken_stream(frames(&final_part.slice(..30), &[10, 20])),
        ))
        .await;
    expect_invalid_part(&cut, "body_interrupted");

    rig.faults
        .part
        .lock()
        .unwrap()
        .push_back(PartFault::Before(timeout));
    let provider_timeout = rig.put_bytes(&session, &item, "3", &final_part).await;
    assert_eq!(
        provider_timeout.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        provider_timeout.text()
    );
    assert_eq!(provider_timeout.error_code(), "STORAGE_UNAVAILABLE");
    assert_eq!(rig.uploaded_parts(&item).await, 3, "parts 1, 2 and 3 stand");

    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.json()["missingPartNumbers"], json!([]));
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(stored, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());
    let end = rig.proxy_snapshot();
    assert_eq!((end.active_permits, end.active_buffers), (0, 0));
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3_proxied_part_route_is_authenticated_scoped_and_stateful() {
    let rig = proxied_rig(steady()).await;
    let part = usize::try_from(PART).unwrap();
    let body = Bytes::from(pattern(part));
    let (session, items) = rig.multi(5, PART).await;
    let item = &items[0];
    let length = part.to_string();
    let untouched = |rig: &S3Rig| {
        assert_eq!(rig.provider_part_calls(), 0, "no provider call");
        assert_eq!(rig.proxy_snapshot().peak_buffers, 0, "no buffer");
    };

    let proof = rig.alice.creds.csrf.clone();
    let anonymous = rig
        .put(
            TusReq::anonymous(Method::PUT, &part_path(&session, item, "1"))
                .no_version()
                .header("cookie", &format!("palmr_csrf={proof}"))
                .header("x-palmr-csrf", &proof)
                .header("content-type", OCTET_STREAM)
                .header("content-length", &length)
                .raw_body(Body::from(body.clone())),
        )
        .await;
    assert_eq!(
        anonymous.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        anonymous.text()
    );

    let forged = rig
        .put(
            rig.request(&rig.alice, &session, item, "1")
                .no_csrf()
                .header("content-length", &length)
                .raw_body(Body::from(body.clone())),
        )
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN, "{}", forged.text());
    assert_eq!(forged.error_code(), "CSRF_TOKEN_MISSING");

    let foreign_origin = rig
        .put(
            rig.request(&rig.alice, &session, item, "1")
                .origin("https://evil.example.test")
                .header("content-length", &length)
                .raw_body(Body::from(body.clone())),
        )
        .await;
    assert_eq!(foreign_origin.status, StatusCode::FORBIDDEN);
    assert_eq!(foreign_origin.error_code(), "ORIGIN_NOT_ALLOWED");

    for content_type in [
        "application/json",
        "application/offset+octet-stream",
        "text/plain",
    ] {
        let wrong = rig
            .put(
                TusReq::new(Method::PUT, &part_path(&session, item, "1"), &rig.alice)
                    .no_version()
                    .header("content-type", content_type)
                    .header("content-length", &length)
                    .raw_body(Body::from(body.clone())),
            )
            .await;
        assert_eq!(
            wrong.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type}: {}",
            wrong.text()
        );
    }
    let untyped = rig
        .put(
            TusReq::new(Method::PUT, &part_path(&session, item, "1"), &rig.alice)
                .no_version()
                .header("content-length", &length)
                .raw_body(Body::from(body.clone())),
        )
        .await;
    assert_eq!(untyped.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let stranger = rig
        .put(
            rig.request(&rig.bob, &session, item, "1")
                .header("content-length", &length)
                .raw_body(Body::from(body.clone())),
        )
        .await;
    assert_eq!(
        stranger.status,
        StatusCode::NOT_FOUND,
        "{}",
        stranger.text()
    );
    assert_eq!(stranger.error_code(), "MULTIPART_NOT_FOUND");
    let unknown = rig
        .put_bytes(&session, &rig.stack.fresh_id(), "1", &body)
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(
        unknown.error_without_request_id(),
        stranger.error_without_request_id(),
        "an unknown item and another owner's item are indistinguishable"
    );
    let garbage = rig.put_bytes("not-a-uuid", item, "1", &body).await;
    assert_eq!(garbage.status, StatusCode::NOT_FOUND);
    untouched(&rig);

    for number in [
        "0",
        "2",
        "9",
        "abc",
        "01",
        "-1",
        "1.5",
        "4294967296",
        "%20",
        "1e0",
    ] {
        let refused = rig.put_bytes(&session, item, number, &body).await;
        expect_invalid_part(&refused, "part_number");
    }
    untouched(&rig);

    rig.proxied.store(false, Ordering::SeqCst);
    let direct = rig.put_bytes(&session, item, "1", &body).await;
    expect_invalid_part(&direct, "direct_part_upload");
    rig.proxied.store(true, Ordering::SeqCst);
    untouched(&rig);

    let ok = rig.put_bytes(&session, item, "1", &body).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    let uploaded = rig.provider_part_calls();

    let completed = rig.complete(&session, item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let late = rig.put_bytes(&session, item, "1", &body).await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text());
    assert_eq!(late.error_code(), "MULTIPART_ALREADY_COMPLETED");
    assert_eq!(rig.provider_part_calls(), uploaded);

    let canceled = &items[1];
    let aborted = rig.abort(&session, canceled).await;
    assert_eq!(aborted.status, StatusCode::NO_CONTENT, "{}", aborted.text());
    let gone = rig.put_bytes(&session, canceled, "1", &body).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text());
    assert_eq!(gone.error_code(), "MULTIPART_NOT_FOUND");
    assert_eq!(rig.provider_part_calls(), uploaded);

    let expiring = &items[2];
    rig.stack
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-09-25T11:00:00.000Z' WHERE id = '{session}'"
        ))
        .await;
    let expired = rig.put_bytes(&session, expiring, "1", &body).await;
    assert_eq!(expired.status, StatusCode::GONE, "{}", expired.text());
    assert_eq!(expired.error_code(), "TRANSFER_SESSION_EXPIRED");
    assert_eq!(rig.provider_part_calls(), uploaded);
    rig.stack
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-10-25T11:00:00.000Z' WHERE id = '{session}'"
        ))
        .await;

    let inactive = &items[3];
    rig.stack
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z' WHERE id = '{}'",
            rig.alice.id
        ))
        .await;
    let blocked = rig.put_bytes(&session, inactive, "1", &body).await;
    assert!(
        matches!(
            (blocked.status, blocked.error_code().as_str()),
            (StatusCode::UNAUTHORIZED, "AUTH_REQUIRED")
                | (StatusCode::FORBIDDEN, "AUTH_ACCOUNT_INACTIVE")
        ),
        "{}",
        blocked.text()
    );
    assert_eq!(rig.provider_part_calls(), uploaded);
    rig.stack
        .execute(&format!(
            "UPDATE users SET is_active = 1, deactivated_at = NULL WHERE id = '{}'",
            rig.alice.id
        ))
        .await;
    let end = rig.proxy_snapshot();
    assert_eq!(
        end.peak_buffers, 1,
        "only the one valid part was ever buffered"
    );
    assert_eq!((end.active_permits, end.active_buffers), (0, 0));
}

#[tokio::test]
async fn it_s3_proxied_parts_retry_and_reconcile_with_provider_truth() {
    let rig = proxied_rig(steady()).await;
    let size = 2 * PART + MIB;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let part = usize::try_from(PART).unwrap();
    let slice = |number: usize| data.slice((number - 1) * part..(number * part).min(data.len()));
    let (session, item) = rig.one("retry.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    rig.faults
        .part
        .lock()
        .unwrap()
        .push_back(PartFault::After(timeout));
    let lost = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(
        lost.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        lost.text()
    );
    assert_eq!(
        rig.part_row(&item, 1).await.0,
        "planned",
        "a lost provider answer records nothing"
    );
    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.json()["missingPartNumbers"], json!([2, 3]));
    assert_eq!(listed.json()["completedParts"][0]["partNumber"], 1);
    let (state, held_etag, ..) = rig.part_row(&item, 1).await;
    assert_eq!(state, "verified", "ListParts repaired the cache");
    let provider_etag = held_etag.unwrap();

    let again = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    assert_eq!(again.json()["etag"], provider_etag.as_str());
    let (state, etag, _, attempts) = rig.part_row(&item, 1).await;
    assert_eq!(
        (state.as_str(), etag.as_deref(), attempts),
        ("uploaded", Some(provider_etag.as_str()), 1)
    );
    let duplicate = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(duplicate.status, StatusCode::OK, "{}", duplicate.text());
    assert_eq!(rig.part_row(&item, 1).await.3, 2);
    assert_eq!(
        rig.uploaded_parts(&item).await,
        1,
        "no second part row exists"
    );
    assert_eq!(rig.used().await, 0, "retries never charge quota");

    let gate = PartGate::new();
    *rig.faults.part_gate.lock().unwrap() = Some(gate.clone());
    let second_part = slice(2);
    let third_part = slice(3);
    let first = rig.put_bytes(&session, &item, "2", &second_part);
    let racing = async {
        gate.entered.acquire().await.unwrap().forget();
        let calls = rig.provider_part_calls();
        let second = rig.put_bytes(&session, &item, "2", &second_part).await;
        assert_eq!(second.status, StatusCode::LOCKED, "{}", second.text());
        assert_eq!(second.error_code(), "UPLOAD_LOCKED");
        assert_eq!(rig.provider_part_calls(), calls, "the loser sends nothing");
        let elsewhere = rig.put_bytes(&session, &item, "3", &third_part);
        gate.release.add_permits(2);
        let elsewhere = elsewhere.await;
        assert_eq!(elsewhere.status, StatusCode::OK, "{}", elsewhere.text());
    };
    let (first, ()) = tokio::join!(first, racing);
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    *rig.faults.part_gate.lock().unwrap() = None;

    let bogus = rig
        .report(
            &session,
            &item,
            json!([{ "partNumber": 3, "etag": "\"bogus\"", "sizeBytes": MIB }]),
        )
        .await;
    assert_eq!(bogus.status, StatusCode::NO_CONTENT);
    assert_eq!(rig.part_row(&item, 3).await.1.as_deref(), Some("\"bogus\""));
    let replaced = rig.put_bytes(&session, &item, "3", &slice(3)).await;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.text());
    assert_ne!(rig.part_row(&item, 3).await.1.as_deref(), Some("\"bogus\""));

    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(stored, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());
    assert_eq!(
        (
            rig.files().await,
            rig.objects().await,
            rig.uploads_audited().await
        ),
        (1, 1, 1)
    );
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    let repeated = rig.complete(&session, &item).await;
    assert_eq!(repeated.status, StatusCode::OK);
    assert_eq!(rig.uploads_audited().await, 1, "completion audits once");
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3_proxied_parts_race_abort_and_completion_safely() {
    let rig = proxied_rig(steady()).await;
    let size = PART + MIB;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let part = usize::try_from(PART).unwrap();
    let slice = |number: usize| data.slice((number - 1) * part..(number * part).min(data.len()));

    let (session, item) = rig.one("abort.bin", size).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        rig.put_bytes(&session, &item, "1", &slice(1)).await.status,
        StatusCode::OK
    );
    let key = rig.final_key(&item).await;
    let gate = PartGate::new();
    *rig.faults.part_gate.lock().unwrap() = Some(gate.clone());
    let second_part = slice(2);
    let in_flight = rig.put_bytes(&session, &item, "2", &second_part);
    let aborting = async {
        gate.entered.acquire().await.unwrap().forget();
        let aborted = rig.abort(&session, &item).await;
        assert_eq!(aborted.status, StatusCode::NO_CONTENT, "{}", aborted.text());
        gate.release.add_permits(1);
    };
    let (late, ()) = tokio::join!(in_flight, aborting);
    assert_eq!(late.status, StatusCode::NOT_FOUND, "{}", late.text());
    assert_eq!(late.error_code(), "MULTIPART_NOT_FOUND");
    assert_ne!(rig.part_row(&item, 2).await.0, "uploaded");
    assert_eq!(rig.item_row(&item).await.0, "canceled");
    assert_eq!(rig.multipart_state(&item).await.as_deref(), Some("aborted"));
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    assert_eq!(rig.open_uploads(&key).await, 0);
    *rig.faults.part_gate.lock().unwrap() = None;

    let (session, item) = rig.one("complete.bin", size).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        rig.put_bytes(&session, &item, "1", &slice(1)).await.status,
        StatusCode::OK
    );
    let gate = PartGate::new();
    *rig.faults.part_gate.lock().unwrap() = Some(gate.clone());
    let second_part = slice(2);
    let in_flight = rig.put_bytes(&session, &item, "2", &second_part);
    let completing = async {
        gate.entered.acquire().await.unwrap().forget();
        let early = rig.complete(&session, &item).await;
        assert_eq!(early.status, StatusCode::CONFLICT, "{}", early.text());
        assert_eq!(early.error_code(), "MULTIPART_INCOMPLETE");
        assert_eq!(
            early.json()["error"]["details"]["missingPartNumbers"],
            json!([2])
        );
        assert_eq!(rig.files().await, 0, "no partial file is visible");
        gate.release.add_permits(1);
    };
    let (landed, ()) = tokio::join!(in_flight, completing);
    assert_eq!(landed.status, StatusCode::OK, "{}", landed.text());
    *rig.faults.part_gate.lock().unwrap() = None;
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let key = rig.final_key(&item).await;
    let (stored, digest) = rig.read_object(&key).await;
    assert_eq!(stored, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());

    let calls = rig.provider_part_calls();
    let late = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text());
    assert_eq!(late.error_code(), "MULTIPART_ALREADY_COMPLETED");
    assert_eq!(rig.provider_part_calls(), calls);
    let (stored, digest) = rig.read_object(&key).await;
    assert_eq!((stored, digest), (size, Sha256::digest(&data).to_vec()));
    let aborted = rig.abort(&session, &item).await;
    assert_eq!(aborted.status, StatusCode::CONFLICT);
    assert_eq!((rig.files().await, rig.uploads_audited().await), (1, 1));
    rig.assert_no_overlap();
}

async fn kinds(rig: &S3Rig, session: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT upload_kind FROM transfer_session_files WHERE transfer_session_id = ?1 ORDER BY ordinal",
    )
    .bind(session)
    .fetch_all(rig.stack.pools.reader().executor())
    .await
    .unwrap()
}

async fn untouched_counts(rig: &S3Rig) -> (i64, i64, i64, i64, i64) {
    (
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions")
            .await,
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM quota_reservations")
            .await,
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        rig.files().await,
        rig.objects().await,
    )
}

#[tokio::test]
async fn it_s3_proxied_ceiling_preflight() {
    assert_eq!(
        CAP, 83_047_219_200,
        "8 MiB x 9 900 is 79 200 MiB = 77.34375 GiB"
    );
    let rig = proxied_rig(steady()).await;

    let sizes = [0, 1, PART - 1, PART, PART + 1, CAP - 1, CAP];
    let files: Vec<Value> = sizes
        .iter()
        .enumerate()
        .map(|(index, size)| sized(&format!("c{index}"), &format!("f{index}.bin"), *size))
        .collect();
    let (session, items) = rig.session(&files).await;
    assert_eq!(
        kinds(&rig, &session).await,
        [
            "s3_single",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart"
        ]
    );
    let plan = |size: u64| (size.div_ceil(PART), PART);
    for (index, size) in sizes.iter().enumerate().skip(1) {
        let created = rig.create(&session, &items[index]).await;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "{size}: {}",
            created.text()
        );
        let (count, part) = plan(*size);
        assert_eq!(created.json()["partSizeBytes"], part, "{size}");
        assert_eq!(created.json()["partCount"], count, "{size}");
        assert_eq!(created.json()["checksumHeadersRequired"], true);
    }
    assert_eq!(rig.faults.creates.load(Ordering::SeqCst), 6);
    let at_cap = rig.part_row(&items[6], 9_900).await;
    assert_eq!(
        at_cap.2,
        i64::try_from(PART).unwrap(),
        "the last part of the ceiling is a full part"
    );
    let below_cap = rig.part_row(&items[5], 9_900).await;
    assert_eq!(below_cap.2, i64::try_from(PART).unwrap() - 1);
    let zero = rig.create(&session, &items[0]).await;
    assert_eq!(
        zero.status,
        StatusCode::CONFLICT,
        "a zero-byte file has no multipart upload"
    );

    let before = untouched_counts(&rig).await;
    let creates = rig.faults.creates.load(Ordering::SeqCst);
    let held_before = rig
        .stack
        .scalar_i64(
            "SELECT COALESCE(SUM(reserved_bytes), 0) FROM quota_reservations WHERE state = 'held'",
        )
        .await;
    let over = rig
        .stack
        .open(
            &rig.alice,
            &request_body(None, &[sized("big", "big.bin", CAP + 1)]),
        )
        .await;
    assert_eq!(
        over.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        over.text()
    );
    assert_eq!(over.error_code(), "FILE_TOO_LARGE");
    let details = &over.json()["error"]["details"];
    assert_eq!(details["reason"], "proxy_part_count");
    assert_eq!(details["declaredBytes"], CAP + 1);
    assert_eq!(details["providerMaxObjectBytes"], CAP);
    assert_eq!(details["itemClientId"], "big");
    assert_eq!(
        untouched_counts(&rig).await,
        before,
        "a refused plan leaves no row"
    );
    assert_eq!(rig.faults.creates.load(Ordering::SeqCst), creates);
    assert_eq!(rig.provider_part_calls(), 0);
    assert_eq!(
        rig.stack.scalar_i64("SELECT COALESCE(SUM(reserved_bytes), 0) FROM quota_reservations WHERE state = 'held'").await,
        held_before,
        "a refused plan holds no quota"
    );
    let mixed = rig
        .stack
        .open(
            &rig.alice,
            &request_body(
                None,
                &[sized("a", "a.bin", 1), sized("b", "b.bin", CAP + 1)],
            ),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(mixed.json()["error"]["details"]["itemClientId"], "b");
    assert_eq!(untouched_counts(&rig).await, before);

    let tight = 40 * GIB;
    rig.stack
        .set_max_file_size(i64::try_from(tight).unwrap())
        .await;
    let policy = rig
        .stack
        .open(
            &rig.alice,
            &request_body(None, &[sized("p", "p.bin", tight + 1)]),
        )
        .await;
    assert_eq!(
        policy.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        policy.text()
    );
    assert_eq!(policy.json()["error"]["details"]["reason"], "max_file_size");
    assert_eq!(untouched_counts(&rig).await, before);

    rig.stack
        .set_max_file_size(i64::try_from(CAP * 2).unwrap())
        .await;
    rig.proxied.store(false, Ordering::SeqCst);
    let (session, item) = rig.one("late.bin", 100 * GIB).await;
    rig.proxied.store(true, Ordering::SeqCst);
    let creates = rig.faults.creates.load(Ordering::SeqCst);
    let refused = rig.create(&session, &item).await;
    assert_eq!(
        refused.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        refused.text()
    );
    assert_eq!(
        refused.json()["error"]["details"]["reason"],
        "proxy_part_count"
    );
    assert_eq!(
        rig.faults.creates.load(Ordering::SeqCst),
        creates,
        "multipart creation re-checks the ceiling before CreateMultipartUpload"
    );
    assert_eq!(rig.multipart_state(&item).await, None);
    assert_eq!(rig.item_row(&item).await.0, "pending");
}

#[tokio::test]
async fn it_s3_proxied_part_plan_is_pinned_for_the_life_of_the_upload() {
    let rig = proxied_rig(steady()).await;
    rig.stack
        .set_max_file_size(i64::try_from(CAP * 2).unwrap())
        .await;
    rig.proxied.store(false, Ordering::SeqCst);
    let (session, item) = rig.one("direct.bin", 100 * GIB).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["partSizeBytes"], 16 * MIB);
    assert_eq!(created.json()["checksumHeadersRequired"], false);
    let body = Bytes::from(pattern(usize::try_from(PART).unwrap()));

    rig.proxied.store(true, Ordering::SeqCst);
    let too_big = rig.put_bytes(&session, &item, "1", &body).await;
    expect_invalid_part(&too_big, "part_size_exceeds_proxy_buffer");
    assert_eq!(rig.provider_part_calls(), 0);
    assert_eq!(rig.proxy_snapshot().peak_buffers, 0);
    let presigned = rig.presign(&session, &item, &[1]).await;
    expect_invalid_part(&presigned, "server_proxied_part_upload");
    let listed = rig.parts(&session, &item).await;
    assert_eq!(
        listed.json()["partSizeBytes"],
        16 * MIB,
        "the persisted plan is never reinterpreted"
    );
    assert_eq!(listed.json()["partCount"], 6_400);

    rig.proxied.store(false, Ordering::SeqCst);
    let presigned = rig.presign(&session, &item, &[1]).await;
    assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
    assert_eq!(
        rig.parts(&session, &item).await.json()["partSizeBytes"],
        16 * MIB
    );

    let (session, item) = rig.one("small.bin", 20 * MIB).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    rig.proxied.store(true, Ordering::SeqCst);
    let fits = rig.put_bytes(&session, &item, "1", &body).await;
    assert_eq!(
        fits.status,
        StatusCode::OK,
        "an 8 MiB plan made in direct mode fits the proxy buffer: {}",
        fits.text()
    );

    let (session, item) = rig.one("proxied.bin", 20 * MIB).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    rig.proxied.store(false, Ordering::SeqCst);
    let presigned = rig.presign(&session, &item, &[1, 2, 3]).await;
    assert_eq!(
        presigned.status,
        StatusCode::OK,
        "relaxing the requirement lets an existing 8 MiB plan use presigned parts: {}",
        presigned.text()
    );
    let calls = rig.provider_part_calls();
    expect_invalid_part(
        &rig.put_bytes(&session, &item, "1", &body).await,
        "direct_part_upload",
    );
    assert_eq!(rig.provider_part_calls(), calls);
}

#[tokio::test]
async fn it_s3_proxied_slow_connection_recovers_by_retry_without_duplicates() {
    let rig = proxied_rig(limits(
        Duration::from_millis(150),
        Duration::from_millis(300),
    ))
    .await;
    let part = usize::try_from(PART).unwrap();
    let size = PART + MIB;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let slice = |number: usize| data.slice((number - 1) * part..(number * part).min(data.len()));
    let (session, item) = rig.one("slow.bin", size).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    let chunked = |number: &str, body: Body| {
        rig.request(&rig.alice, &session, &item, number)
            .header("transfer-encoding", "chunked")
            .raw_body(body)
    };

    let (sender, stream) = channel_body();
    sender.send(Ok(slice(1).slice(..MIB as usize))).unwrap();
    let stalled = rig.put(chunked("1", stream)).await;
    assert_eq!(
        stalled.status,
        StatusCode::REQUEST_TIMEOUT,
        "{}",
        stalled.text()
    );
    assert_eq!(stalled.error_code(), "TRANSFER_IDLE_TIMEOUT");
    drop(sender);
    let after_stall = rig.proxy_snapshot();
    assert_eq!(
        (after_stall.active_permits, after_stall.active_buffers),
        (0, 0),
        "a stalled client releases its permit and buffer"
    );
    assert_eq!(rig.provider_part_calls(), 0);
    assert_ne!(rig.part_row(&item, 1).await.0, "uploaded");

    let first = slice(1);
    let (sender, stream) = channel_body();
    let slow = rig.put(chunked("1", stream));
    let feed = async {
        let step = first.len().div_ceil(12);
        let mut at = 0;
        while at < first.len() {
            let end = (at + step).min(first.len());
            sender.send(Ok(first.slice(at..end))).unwrap();
            at = end;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(sender);
    };
    let (retried, ()) = tokio::join!(slow, feed);
    assert_eq!(
        retried.status,
        StatusCode::OK,
        "the retransmission recovers: {}",
        retried.text()
    );
    assert_eq!(rig.provider_part_calls(), 1);

    rig.faults
        .part
        .lock()
        .unwrap()
        .push_back(PartFault::Before(timeout));
    let second = slice(2);
    let failed = rig.put_bytes(&session, &item, "2", &second).await;
    assert_eq!(
        failed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        failed.text()
    );
    assert_ne!(rig.part_row(&item, 2).await.0, "uploaded");
    let again = rig.put_bytes(&session, &item, "2", &second).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    let repeated = rig.put_bytes(&session, &item, "1", &first).await;
    assert_eq!(repeated.status, StatusCode::OK, "{}", repeated.text());

    let part_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM s3_multipart_parts p
           JOIN s3_multipart_uploads u ON u.id = p.s3_multipart_upload_id
          WHERE u.transfer_session_file_id = ?1",
    )
    .bind(&item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(part_rows, 2, "retries never add part rows");
    assert_eq!(rig.uploaded_parts(&item).await, 2);
    assert_eq!(rig.used().await, 0, "no part is charged");

    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(stored, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());
    assert_eq!(
        (
            rig.files().await,
            rig.objects().await,
            rig.uploads_audited().await
        ),
        (1, 1, 1),
        "one file, one object, one audit event"
    );
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    let end = rig.proxy_snapshot();
    assert_eq!((end.active_permits, end.active_buffers), (0, 0));
    assert_eq!(
        end.peak_buffers, 1,
        "a slow stream never holds a second buffer"
    );
    assert_eq!(end.peak_capacity_bytes, PART);
}

#[tokio::test]
async fn it_s3_unconfigured_generic_provider_keeps_the_direct_path() {
    let rig = S3Rig::build(RigOptions {
        profile: Some(S3Profile::Generic),
        ..RigOptions::default()
    })
    .await;
    assert!(!rig.provider.caps().requires_checksum_headers);
    let size = 9 * MIB + 5;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("direct.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["checksumHeadersRequired"], false);
    assert_eq!(created.json()["partSizeBytes"], PART);
    let presigned = rig.presign(&session, &item, &[1, 2]).await;
    assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
    let body = Bytes::from(data.clone());
    expect_invalid_part(
        &rig.put_bytes(
            &session,
            &item,
            "1",
            &body.slice(..usize::try_from(PART).unwrap()),
        )
        .await,
        "direct_part_upload",
    );
    assert_eq!(rig.provider_part_calls(), 0);
    rig.upload_all(&session, &item, &data).await;
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(stored, size);
    assert_eq!(digest, Sha256::digest(&data).to_vec());
    assert_eq!(rig.provider_part_calls(), 0, "no byte traversed Palmr");
    assert_eq!(rig.proxy_snapshot().peak_buffers, 0);
}

#[tokio::test]
async fn it_s3_mode_transitions_keep_uploaded_parts_resumable() {
    let rig = proxied_rig(steady()).await;
    let size = 2 * PART + MIB;
    let data = Bytes::from(pattern(usize::try_from(size).unwrap()));
    let part = usize::try_from(PART).unwrap();
    let slice = |number: usize| data.slice((number - 1) * part..(number * part).min(data.len()));

    let (session, item) = rig.one("proxied-to-direct.bin", size).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    let stored = rig.put_bytes(&session, &item, "1", &slice(1)).await;
    assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
    let etag = stored.json()["etag"].as_str().unwrap().to_owned();

    rig.proxied.store(false, Ordering::SeqCst);
    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.json()["completedParts"][0]["etag"], etag.as_str());
    assert_eq!(listed.json()["missingPartNumbers"], json!([2, 3]));
    assert_eq!(listed.json()["partSizeBytes"], PART);
    assert_eq!(rig.part_row(&item, 1).await.0, "verified");
    let presigned = rig.presign(&session, &item, &[2, 3]).await;
    assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
    let mut reports = Vec::new();
    for entry in presigned.json()["parts"].as_array().unwrap() {
        let number = usize::try_from(entry["partNumber"].as_u64().unwrap()).unwrap();
        let bytes = slice(number);
        let etag = rig
            .put_part(entry["url"].as_str().unwrap(), bytes.to_vec())
            .await;
        reports.push(json!({ "partNumber": number, "etag": etag, "sizeBytes": bytes.len() }));
    }
    assert_eq!(
        rig.report(&session, &item, json!(reports)).await.status,
        StatusCode::NO_CONTENT
    );
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored_bytes, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(
        (stored_bytes, digest),
        (size, Sha256::digest(&data).to_vec())
    );

    rig.proxied.store(false, Ordering::SeqCst);
    let (session, item) = rig.one("direct-to-proxied.bin", size).await;
    assert_eq!(
        rig.create(&session, &item).await.status,
        StatusCode::CREATED
    );
    let presigned = rig.presign(&session, &item, &[1]).await;
    let first = slice(1);
    let direct_etag = rig
        .put_part(
            presigned.json()["parts"][0]["url"].as_str().unwrap(),
            first.to_vec(),
        )
        .await;
    assert_eq!(
        rig.report(
            &session,
            &item,
            json!([{ "partNumber": 1, "etag": direct_etag, "sizeBytes": first.len() }])
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    rig.proxied.store(true, Ordering::SeqCst);
    let listed = rig.parts(&session, &item).await;
    assert_eq!(
        listed.json()["completedParts"][0]["etag"],
        direct_etag.as_str()
    );
    assert_eq!(listed.json()["missingPartNumbers"], json!([2, 3]));
    for number in [2, 3] {
        let stored = rig
            .put_bytes(&session, &item, &number.to_string(), &slice(number))
            .await;
        assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
    }
    assert_eq!(
        rig.provider_part_calls(),
        3,
        "only the three proxied parts went through Palmr"
    );
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let (stored_bytes, digest) = rig.read_object(&rig.final_key(&item).await).await;
    assert_eq!(
        (stored_bytes, digest),
        (size, Sha256::digest(&data).to_vec())
    );
    assert_eq!(
        (
            rig.files().await,
            rig.objects().await,
            rig.uploads_audited().await
        ),
        (2, 2, 2)
    );
}
