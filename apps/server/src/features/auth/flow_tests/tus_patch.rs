use std::io::Write as _;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use base64ct::{Base64, Encoding};
use sha2::{Digest, Sha256};

use super::folders::{Member, HOST_A};
use super::tus::{
    assert_tus_error, channel_body, frames, header, pattern, small_limits, stream_of,
    wait_for_offset, Planned, TestStaging, TusReq, TUS,
};
use super::tus_stream::{stack_with_limits, upload_of};
use super::*;
use crate::features::transfers::TusLimits;

pub(super) fn patch<'a>(member: &'a Member, id: &str, offset: u64) -> TusReq<'a> {
    TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), member)
        .header("upload-offset", &offset.to_string())
}

pub(super) fn framed_patch<'a>(
    member: &'a Member,
    id: &str,
    offset: u64,
    body: Body,
) -> TusReq<'a> {
    patch(member, id, offset)
        .raw_body(body)
        .header("content-type", super::tus::OCTET)
}

pub(super) fn checksum_of(data: &[u8]) -> String {
    format!("sha256 {}", Base64::encode_string(&Sha256::digest(data)))
}

pub(super) async fn stack_small(root: &Path) -> (Stack, Arc<TestStaging>) {
    stack_with_limits(root, small_limits()).await
}

pub(super) async fn upload_for(
    stack: &Stack,
    member: &Member,
    name: &str,
    length: Option<u64>,
) -> (Planned, String) {
    let planned = stack.planned(member, name, length).await;
    let id = stack.tus_created(member, &planned, length).await;
    (planned, id)
}

pub(super) async fn head_offset(stack: &Stack, member: &Member, id: &str) -> u64 {
    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), member))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    header(&head, "upload-offset").parse().unwrap()
}

fn blob_path(stack: &Stack, id: &str) -> std::path::PathBuf {
    stack
        .data_dir
        .join("uploads")
        .join(id.replace('-', ""))
        .join("blob")
}

fn blob_len(stack: &Stack, id: &str) -> u64 {
    std::fs::metadata(blob_path(stack, id)).unwrap().len()
}

fn assert_blob_equals(path: &std::path::Path, expected: &[u8]) {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).unwrap();
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut at = 0_usize;
    loop {
        let read = file.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        assert!(
            expected[at..].starts_with(&chunk[..read]),
            "the staged bytes diverge at {at}"
        );
        at += read;
    }
    assert_eq!(at, expected.len(), "the staged length");
}

async fn assert_quiet_row(stack: &Stack, id: &str, offset: i64) {
    let row = stack.tus_row(id).await;
    assert_eq!(row.offset, offset);
    assert!(row.locked_by.is_none(), "the lease is released");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.counts().await.objects, 0);
}

#[tokio::test]
async fn it_tus_patch_appends_and_reports_the_durable_offset() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(20_000)).await;
    let held = stack.reservation(&planned.session).await;
    let data = pattern(20_000);

    let first = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..12_000]))
        .await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.text());
    assert!(first.body.is_empty(), "a 204 has no body");
    assert_eq!(header(&first, "upload-offset"), "12000");
    assert_eq!(header(&first, "tus-resumable"), "1.0.0");
    assert_eq!(header(&first, "cache-control"), "no-store");
    assert!(first.headers.contains_key("x-request-id"));
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 12_000);
    assert_eq!(row.state, "in_progress");
    assert!(row.locked_by.is_none());
    let expires: crate::domain::time::Timestamp = row.expires_at.parse().unwrap();
    assert_eq!(
        header(&first, "upload-expires"),
        crate::features::transfers::tus::headers::imf_fixdate(expires),
        "the header is the persisted expiry"
    );
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..12_000]);
    assert_eq!(head_offset(&stack, &alice, &id).await, 12_000);

    let second = stack
        .tus_send(patch(&alice, &id, 12_000).bytes(&data[12_000..]))
        .await;
    assert_eq!(second.status, StatusCode::NO_CONTENT, "{}", second.text());
    assert_eq!(header(&second, "upload-offset"), "20000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);

    let empty = stack.tus_send(patch(&alice, &id, 20_000).bytes(&[])).await;
    assert_eq!(empty.status, StatusCode::NO_CONTENT);
    assert_eq!(header(&empty, "upload-offset"), "20000");

    assert_eq!(stack.item_state(&planned.item).await, "uploading");
    assert_eq!(stack.session_row(&planned.session).await.0, "uploading");
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert_quiet_row(&stack, &id, 20_000).await;
    assert_eq!(stack.used_bytes(alice.id).await, 0);
    assert!(stack.tus.upload_lock_entries() == 0);
}

#[tokio::test]
async fn it_tus_patch_offset_mismatch_409() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(10_000)).await;
    let data = pattern(10_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..3_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
    let held = stack.reservation(&planned.session).await;
    let used = stack.used_bytes(alice.id).await;

    for stale in [0_u64, 1_000, 5_000, 9_999_999] {
        let rejected = stack
            .tus_send(patch(&alice, &id, stale).bytes(&data[..2_000]))
            .await;
        assert_tus_error(&rejected, StatusCode::CONFLICT, "UPLOAD_OFFSET_MISMATCH");
        assert_eq!(header(&rejected, "upload-offset"), "3000", "{stale}");
        let error = &rejected.json()["error"];
        assert_eq!(error["details"]["expectedOffset"], 3_000);
        assert_eq!(error["requestId"], header(&rejected, "x-request-id"));
        assert_eq!(
            blob_len(&stack, &id),
            3_000,
            "no byte of a stale request is accepted"
        );
        assert_quiet_row(&stack, &id, 3_000).await;
    }
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert_eq!(stack.used_bytes(alice.id).await, used);

    let retried = stack
        .tus_send(patch(&alice, &id, 3_000).bytes(&data[3_000..]))
        .await;
    assert_eq!(retried.status, StatusCode::NO_CONTENT, "{}", retried.text());
    assert_eq!(header(&retried, "upload-offset"), "10000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
}

#[tokio::test]
async fn it_tus_patch_rejects_malformed_requests_before_accepting_bytes() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(5_000)).await;
    let data = pattern(1_000);
    let base = |offset: &str| {
        TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), &alice)
            .header("upload-offset", offset)
            .bytes(&data)
    };

    let missing = stack
        .tus_send(TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), &alice).bytes(&data))
        .await;
    assert_tus_error(&missing, StatusCode::BAD_REQUEST, "UPLOAD_METADATA_INVALID");
    assert_eq!(missing.json()["error"]["details"]["key"], "Upload-Offset");
    for bad in [
        "-1",
        "+1",
        "01",
        "1.5",
        "abc",
        "",
        " 0",
        "99999999999999999999999",
    ] {
        let response = stack.tus_send(base(bad)).await;
        assert_tus_error(
            &response,
            StatusCode::BAD_REQUEST,
            "UPLOAD_METADATA_INVALID",
        );
    }
    let twice = stack.tus_send(base("0").header("upload-offset", "0")).await;
    assert_tus_error(&twice, StatusCode::BAD_REQUEST, "UPLOAD_METADATA_INVALID");

    let no_version = stack.tus_send(base("0").no_version()).await;
    assert_tus_error(
        &no_version,
        StatusCode::PRECONDITION_FAILED,
        "TUS_VERSION_UNSUPPORTED",
    );
    assert_eq!(header(&no_version, "tus-version"), "1.0.0");

    for content_type in ["application/json", "text/plain", "application/octet-stream"] {
        let response = stack
            .tus_send(
                TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), &alice)
                    .header("upload-offset", "0")
                    .header("content-length", "1000")
                    .header("content-type", content_type)
                    .raw_body(Body::from(data.clone())),
            )
            .await;
        assert_tus_error(
            &response,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
        );
    }
    let untyped = stack
        .tus_send(
            TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), &alice).header("upload-offset", "0"),
        )
        .await;
    assert_tus_error(
        &untyped,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );

    let no_csrf = stack.tus_send(base("0").no_csrf()).await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    assert_eq!(header(&no_csrf, "tus-resumable"), "1.0.0");

    let concat = stack
        .tus_send(base("0").header("upload-concat", "partial"))
        .await;
    assert_tus_error(
        &concat,
        StatusCode::NOT_IMPLEMENTED,
        "TUS_EXTENSION_UNSUPPORTED",
    );
    let defer = stack
        .tus_send(base("0").header("upload-defer-length", "1"))
        .await;
    assert_tus_error(&defer, StatusCode::BAD_REQUEST, "UPLOAD_METADATA_INVALID");

    for checksum in [
        "sha256",
        "sha256 ",
        "md5 1B2M2Y8AsgTpgAmY7PhCfg==",
        "SHA256 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
        "sha256 !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
        "sha256 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU",
        "sha256 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU==",
        "sha256 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSu==",
        "sha256  47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
        "sha256 47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU=",
    ] {
        let response = stack
            .tus_send(base("0").header("upload-checksum", checksum))
            .await;
        assert_tus_error(
            &response,
            StatusCode::BAD_REQUEST,
            "UPLOAD_METADATA_INVALID",
        );
        assert_eq!(
            response.json()["error"]["details"]["key"],
            "Upload-Checksum",
            "{checksum}"
        );
    }

    let foreign = stack
        .tus_send(
            TusReq::new(Method::PATCH, &format!("{TUS}/{id}"), &bob)
                .header("upload-offset", "0")
                .bytes(&data),
        )
        .await;
    assert_tus_error(&foreign, StatusCode::NOT_FOUND, "NOT_FOUND");
    let unknown = stack
        .tus_send(
            TusReq::new(
                Method::PATCH,
                &format!("{TUS}/0192f3a1-0000-7000-8000-00000000abcd"),
                &alice,
            )
            .header("upload-offset", "0")
            .bytes(&data),
        )
        .await;
    assert_tus_error(&unknown, StatusCode::NOT_FOUND, "NOT_FOUND");
    let garbage = stack
        .tus_send(
            TusReq::new(Method::PATCH, &format!("{TUS}/not-an-id"), &alice)
                .header("upload-offset", "0")
                .bytes(&data),
        )
        .await;
    assert_tus_error(&garbage, StatusCode::NOT_FOUND, "NOT_FOUND");
    let anonymous = stack
        .tus_send(
            TusReq::anonymous(Method::PATCH, &format!("{TUS}/{id}"))
                .header("upload-offset", "0")
                .bytes(&data),
        )
        .await;
    assert_eq!(
        anonymous.status,
        StatusCode::FORBIDDEN,
        "the request gate answers before identity does"
    );
    assert_eq!(header(&anonymous, "tus-resumable"), "1.0.0");

    assert_eq!(blob_len(&stack, &id), 0, "no rejected request wrote a byte");
    assert_quiet_row(&stack, &id, 0).await;
    let accepted = stack.tus_send(base("0")).await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn it_tus_min_reconciliation_truncates_torn_tail() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(40_000)).await;
    let data = pattern(40_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..8_192]))
        .await;
    assert_eq!(header(&accepted, "upload-offset"), "8192");

    let torn: Vec<u8> = (0..6_000_u32).map(|n| 0xEE ^ (n % 7) as u8).collect();
    std::fs::OpenOptions::new()
        .append(true)
        .open(blob_path(&stack, &id))
        .unwrap()
        .write_all(&torn)
        .unwrap();
    assert_eq!(blob_len(&stack, &id), 8_192 + 6_000);
    assert_eq!(stack.tus_row(&id).await.offset, 8_192);

    assert_eq!(
        head_offset(&stack, &alice, &id).await,
        8_192,
        "HEAD never reveals the unproven tail"
    );
    assert_eq!(
        blob_len(&stack, &id),
        8_192 + 6_000,
        "HEAD stays read-only; the PATCH owns the truncation"
    );

    let stale = stack
        .tus_send(patch(&alice, &id, 14_192).bytes(&data[14_192..16_000]))
        .await;
    assert_tus_error(&stale, StatusCode::CONFLICT, "UPLOAD_OFFSET_MISMATCH");
    assert_eq!(header(&stale, "upload-offset"), "8192");
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        &data[..8_192],
        "the tail is gone before any new byte"
    );

    let resumed = stack
        .tus_send(patch(&alice, &id, 8_192).bytes(&data[8_192..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(header(&resumed, "upload-offset"), "40000");
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        data,
        "no unverified byte survives"
    );
    assert_quiet_row(&stack, &id, 40_000).await;
}

#[tokio::test]
async fn it_tus_min_reconciliation_repairs_phantom_offset() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(40_000)).await;
    let data = pattern(40_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..8_192]))
        .await;
    assert_eq!(header(&accepted, "upload-offset"), "8192");

    stack
        .execute(&format!(
            "UPDATE tus_uploads SET upload_offset = 14000 WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(stack.tus_row(&id).await.offset, 14_000);
    assert_eq!(
        head_offset(&stack, &alice, &id).await,
        8_192,
        "the phantom offset is never reported"
    );
    assert_eq!(
        stack.tus_row(&id).await.offset,
        14_000,
        "HEAD does not write"
    );

    let phantom = stack
        .tus_send(patch(&alice, &id, 14_000).bytes(&data[14_000..16_000]))
        .await;
    assert_tus_error(&phantom, StatusCode::CONFLICT, "UPLOAD_OFFSET_MISMATCH");
    assert_eq!(header(&phantom, "upload-offset"), "8192");
    assert_eq!(
        stack.tus_row(&id).await.offset,
        8_192,
        "the downward correction is persisted before any byte"
    );
    assert_eq!(blob_len(&stack, &id), 8_192, "no hole was written");

    let resumed = stack
        .tus_send(patch(&alice, &id, 8_192).bytes(&data[8_192..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        data,
        "no zero-filled hole and no shifted byte"
    );
    assert_quiet_row(&stack, &id, 40_000).await;
}

#[tokio::test]
async fn it_tus_patch_reconciles_a_missing_staging_file_to_zero() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(9_000)).await;
    let data = pattern(9_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..4_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
    std::fs::remove_dir_all(blob_path(&stack, &id).parent().unwrap()).unwrap();
    assert_eq!(head_offset(&stack, &alice, &id).await, 0);

    let stale = stack
        .tus_send(patch(&alice, &id, 4_000).bytes(&data[4_000..]))
        .await;
    assert_tus_error(&stale, StatusCode::CONFLICT, "UPLOAD_OFFSET_MISMATCH");
    assert_eq!(header(&stale, "upload-offset"), "0");
    assert_eq!(stack.tus_row(&id).await.offset, 0);
    let resumed = stack.tus_send(patch(&alice, &id, 0).bytes(&data)).await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
}

#[tokio::test]
async fn it_tus_patch_checksum_accepts_a_matching_digest() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(30_000)).await;
    let data = pattern(30_000);

    let first = stack
        .tus_send(
            patch(&alice, &id, 0)
                .header("upload-checksum", &checksum_of(&data[..12_000]))
                .bytes(&data[..12_000]),
        )
        .await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.text());
    assert_eq!(header(&first, "upload-offset"), "12000");
    let second = stack
        .tus_send(
            patch(&alice, &id, 12_000)
                .header("upload-checksum", &checksum_of(&data[12_000..]))
                .bytes(&data[12_000..]),
        )
        .await;
    assert_eq!(second.status, StatusCode::NO_CONTENT, "{}", second.text());
    assert_eq!(header(&second, "upload-offset"), "30000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);

    let empty = stack
        .tus_send(
            patch(&alice, &id, 30_000)
                .header("upload-checksum", &checksum_of(&[]))
                .bytes(&[]),
        )
        .await;
    assert_eq!(empty.status, StatusCode::NO_CONTENT);
    let (algorithm, digest): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT checksum_algo, client_checksum FROM tus_uploads WHERE id = ?1")
            .bind(&id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        (algorithm, digest),
        (None, None),
        "a per-PATCH digest is never stored as a whole-file digest"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await,
        0
    );
}

#[tokio::test]
async fn it_tus_patch_checksum_mismatch_460_keeps_the_previous_offset() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(100_000)).await;
    let held = stack.reservation(&planned.session).await;
    let data = pattern(100_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..5_000]))
        .await;
    assert_eq!(header(&accepted, "upload-offset"), "5000");

    let mut wrong = data[5_000..45_000].to_vec();
    wrong[39_999] ^= 0xFF;
    let claimed = checksum_of(&wrong);
    let (sender, body) = channel_body();
    let request =
        stack.tus_send(framed_patch(&alice, &id, 5_000, body).header("upload-checksum", &claimed));
    let driver = async {
        for piece in data[5_000..45_000].chunks(10_000) {
            sender.send(Ok(Bytes::copy_from_slice(piece))).unwrap();
        }
        let wait = async {
            while blob_len(&stack, &id) < 5_000 + 30_000 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait)
            .await
            .expect("the checkpoint windows were reached");
        assert!(
            staging.flushes.load(Ordering::SeqCst) >= 3,
            "several checkpoints were synced"
        );
        assert_eq!(
            stack.tus_row(&id).await.offset,
            5_000,
            "no checkpoint of an unverified chunk advances the offset"
        );
        assert_eq!(head_offset(&stack, &alice, &id).await, 5_000);
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(
        &response,
        StatusCode::from_u16(460).unwrap(),
        "UPLOAD_CHECKSUM_MISMATCH",
    );
    let error = &response.json()["error"];
    assert_eq!(error["details"]["algorithm"], "sha256");
    assert_eq!(error["details"]["offset"], 5_000);
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        &data[..5_000],
        "the rejected chunk is not reachable"
    );
    assert_quiet_row(&stack, &id, 5_000).await;
    assert_eq!(head_offset(&stack, &alice, &id).await, 5_000);
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert_eq!(stack.item_state(&planned.item).await, "uploading");

    let retry = stack
        .tus_send(
            patch(&alice, &id, 5_000)
                .header("upload-checksum", &checksum_of(&data[5_000..45_000]))
                .bytes(&data[5_000..45_000]),
        )
        .await;
    assert_eq!(retry.status, StatusCode::NO_CONTENT, "{}", retry.text());
    assert_eq!(header(&retry, "upload-offset"), "45000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..45_000]);

    let single = stack
        .tus_send(
            patch(&alice, &id, 45_000)
                .header("upload-checksum", &checksum_of(b"other"))
                .bytes(&data[45_000..45_001]),
        )
        .await;
    assert_tus_error(
        &single,
        StatusCode::from_u16(460).unwrap(),
        "UPLOAD_CHECKSUM_MISMATCH",
    );
    let empty = stack
        .tus_send(
            patch(&alice, &id, 45_000)
                .header("upload-checksum", &checksum_of(b"x"))
                .bytes(&[]),
        )
        .await;
    assert_tus_error(
        &empty,
        StatusCode::from_u16(460).unwrap(),
        "UPLOAD_CHECKSUM_MISMATCH",
    );
    assert_eq!(blob_len(&stack, &id), 45_000);
    assert_quiet_row(&stack, &id, 45_000).await;
}

#[tokio::test]
async fn it_tus_patch_checksum_mismatch_after_a_timed_checkpoint_rolls_back() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.flush_bytes = u64::MAX;
    limits.flush_interval = std::time::Duration::from_millis(120);
    let (stack, staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(20_000)).await;
    let data = pattern(20_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..2_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);

    let claimed = checksum_of(b"not the body");
    let (sender, body) = channel_body();
    let request =
        stack.tus_send(framed_patch(&alice, &id, 2_000, body).header("upload-checksum", &claimed));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[2_000..3_000])))
            .unwrap();
        let before = staging.flushes.load(Ordering::SeqCst);
        while staging.flushes.load(Ordering::SeqCst) == before {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            stack.tus_row(&id).await.offset,
            2_000,
            "the timed checkpoint synced the bytes but not the offset"
        );
        sender
            .send(Ok(Bytes::copy_from_slice(&data[3_000..6_000])))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(request, driver).await;
    assert_tus_error(
        &response,
        StatusCode::from_u16(460).unwrap(),
        "UPLOAD_CHECKSUM_MISMATCH",
    );
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..2_000]);
    assert_quiet_row(&stack, &id, 2_000).await;
}

#[tokio::test]
async fn it_tus_patch_unverifiable_checksummed_chunks_are_rolled_back() {
    let root = TempDir::new().unwrap();
    let mut limits = small_limits();
    limits.idle_timeout = std::time::Duration::from_millis(400);
    let (stack, _staging) = stack_with_limits(root.path(), limits).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_planned, id) = upload_for(&stack, &alice, "a.bin", Some(50_000)).await;
    let data = pattern(50_000);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..3_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);
    let claimed = checksum_of(&data[3_000..20_000]);

    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[3_000..12_000])))
        .unwrap();
    let stalled = stack
        .tus_send(framed_patch(&alice, &id, 3_000, body).header("upload-checksum", &claimed))
        .await;
    assert_tus_error(
        &stalled,
        StatusCode::REQUEST_TIMEOUT,
        "TRANSFER_IDLE_TIMEOUT",
    );
    drop(sender);
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        &data[..3_000],
        "an unverifiable stalled chunk is discarded"
    );
    assert_quiet_row(&stack, &id, 3_000).await;

    let (sender, body) = channel_body();
    sender
        .send(Ok(Bytes::copy_from_slice(&data[3_000..9_000])))
        .unwrap();
    sender
        .send(Err(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )))
        .unwrap();
    drop(sender);
    let _ = stack
        .tus_send(framed_patch(&alice, &id, 3_000, body).header("upload-checksum", &claimed))
        .await;
    assert_eq!(stack.staged_bytes(&id).unwrap(), &data[..3_000]);
    assert_quiet_row(&stack, &id, 3_000).await;
    assert_eq!(head_offset(&stack, &alice, &id).await, 3_000);
}

#[tokio::test]
async fn it_tus_patch_known_length_boundary_is_enforced_before_and_during_the_stream() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(20_000);

    let (first_plan, first) = upload_for(&stack, &alice, "first.bin", Some(10_000)).await;
    let over = stack
        .tus_send(patch(&alice, &first, 0).bytes(&data[..10_001]))
        .await;
    assert_tus_error(&over, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(over.json()["error"]["details"]["reason"], "upload_length");
    assert_eq!(
        blob_len(&stack, &first),
        0,
        "declared overrun is refused up front"
    );
    assert_eq!(stack.item_state(&first_plan.item).await, "uploading");
    assert_quiet_row(&stack, &first, 0).await;
    let exact = stack
        .tus_send(patch(&alice, &first, 0).bytes(&data[..10_000]))
        .await;
    assert_eq!(exact.status, StatusCode::NO_CONTENT);
    assert_eq!(header(&exact, "upload-offset"), "10000");
    let beyond = stack
        .tus_send(patch(&alice, &first, 10_000).bytes(&data[..1]))
        .await;
    assert_tus_error(&beyond, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(blob_len(&stack, &first), 10_000);

    let (second_plan, second) = upload_for(&stack, &alice, "second.bin", Some(10_000)).await;
    let chunked = stack
        .tus_send(framed_patch(
            &alice,
            &second,
            0,
            stream_of(frames(&data[..15_000], &[6_000, 6_000, 3_000])),
        ))
        .await;
    assert_tus_error(&chunked, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    let row = stack.tus_row(&second).await;
    assert_eq!(row.offset, 10_000, "the prefix up to the boundary is kept");
    assert_eq!(blob_len(&stack, &second), 10_000);
    assert_eq!(
        stack.item_state(&second_plan.item).await,
        "failed",
        "a body beyond the declared length fails the item as in creation"
    );
    assert!(row.locked_by.is_none());
}

#[tokio::test]
async fn it_tus_patch_sets_a_deferred_length() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let data = pattern(30_000);

    let (_plan, id) = upload_for(&stack, &alice, "a.bin", None).await;
    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(header(&head, "upload-defer-length"), "1");
    let first = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..10_000]))
        .await;
    assert_eq!(header(&first, "upload-offset"), "10000");

    let below = stack
        .tus_send(
            patch(&alice, &id, 10_000)
                .header("upload-length", "9999")
                .bytes(&data[10_000..11_000]),
        )
        .await;
    assert_tus_error(
        &below,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
    assert_eq!(blob_len(&stack, &id), 10_000);
    let malformed = stack
        .tus_send(
            patch(&alice, &id, 10_000)
                .header("upload-length", "0x10")
                .bytes(&data[10_000..11_000]),
        )
        .await;
    assert_tus_error(
        &malformed,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    let still_deferred = stack.tus_row(&id).await;
    assert_eq!((still_deferred.defer, still_deferred.length), (1, None));

    let set = stack
        .tus_send(
            patch(&alice, &id, 10_000)
                .header("upload-length", "30000")
                .bytes(&data[10_000..20_000]),
        )
        .await;
    assert_eq!(set.status, StatusCode::NO_CONTENT, "{}", set.text());
    let fixed = stack.tus_row(&id).await;
    assert_eq!(
        (fixed.defer, fixed.length, fixed.offset),
        (0, Some(30_000), 20_000)
    );
    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(header(&head, "upload-length"), "30000");
    assert!(!head.headers.contains_key("upload-defer-length"));

    let conflicting = stack
        .tus_send(
            patch(&alice, &id, 20_000)
                .header("upload-length", "31000")
                .bytes(&data[20_000..21_000]),
        )
        .await;
    assert_tus_error(
        &conflicting,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
    assert_eq!(stack.tus_row(&id).await.length, Some(30_000));
    let same = stack
        .tus_send(
            patch(&alice, &id, 20_000)
                .header("upload-length", "30000")
                .bytes(&data[20_000..]),
        )
        .await;
    assert_eq!(same.status, StatusCode::NO_CONTENT, "{}", same.text());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
    let overrun = stack
        .tus_send(patch(&alice, &id, 30_000).bytes(&data[..1]))
        .await;
    assert_tus_error(&overrun, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");

    let (_plan, known) = upload_for(&stack, &alice, "known.bin", Some(5_000)).await;
    let wrong = stack
        .tus_send(
            patch(&alice, &known, 0)
                .header("upload-length", "5001")
                .bytes(&data[..100]),
        )
        .await;
    assert_tus_error(
        &wrong,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
    assert_eq!(blob_len(&stack, &known), 0);
}

#[tokio::test]
async fn it_tus_patch_enforces_the_maximum_for_a_deferred_length() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_max_file_size(5_000).await;
    let data = pattern(6_000);

    let (_plan, declared) = upload_for(&stack, &alice, "declared.bin", None).await;
    let above = stack
        .tus_send(
            patch(&alice, &declared, 0)
                .header("upload-length", "100000")
                .bytes(&data[..10]),
        )
        .await;
    assert_tus_error(&above, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(above.json()["error"]["details"]["maxBytes"], 5_000);
    assert_eq!(blob_len(&stack, &declared), 0);
    assert_eq!(stack.tus_row(&declared).await.defer, 1);

    let (_plan, sized) = upload_for(&stack, &alice, "sized.bin", None).await;
    let up_front = stack.tus_send(patch(&alice, &sized, 0).bytes(&data)).await;
    assert_tus_error(&up_front, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(blob_len(&stack, &sized), 0, "refused before any byte");

    let (plan, streamed) = upload_for(&stack, &alice, "streamed.bin", None).await;
    let over = stack
        .tus_send(framed_patch(
            &alice,
            &streamed,
            0,
            stream_of(frames(&data, &[3_000, 3_000])),
        ))
        .await;
    assert_tus_error(&over, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(stack.tus_row(&streamed).await.offset, 5_000);
    assert_eq!(stack.item_state(&plan.item).await, "failed");
}

#[tokio::test]
async fn it_tus_patch_runs_the_quota_check_for_deferred_lengths() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(6_000)).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", None).await;
    assert_eq!(stack.reservation(&planned.session).await.1, 0);
    let held = stack.reservation(&planned.session).await;
    let data = pattern(9_000);

    let first = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..4_000]))
        .await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.text());
    assert_eq!(stack.reservation(&planned.session).await, held);

    let over = stack
        .tus_send(framed_patch(
            &alice,
            &id,
            4_000,
            stream_of(frames(&data[4_000..], &[1_500, 3_500])),
        ))
        .await;
    assert_tus_error(&over, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    let row = stack.tus_row(&id).await;
    assert_eq!(row.offset, 6_000, "bytes stop exactly at the quota");
    assert_eq!(
        blob_len(&stack, &id),
        6_000,
        "the rejected slice is not written"
    );
    assert!(row.locked_by.is_none());
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
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    let again = stack
        .tus_send(patch(&alice, &id, 6_000).bytes(&data[6_000..6_001]))
        .await;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "a failed item accepts no further bytes"
    );
}

#[tokio::test]
async fn it_tus_patch_resumes_a_creation_with_upload() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(30_000)).await;
    let held = stack.reservation(&planned.session).await;
    let data = pattern(30_000);

    let created = stack
        .tus_send(
            stack
                .create_req(&alice, &planned, Some(30_000))
                .raw_body(stream_of(frames(&data[..9_000], &[3_000, 6_000])))
                .header("content-type", super::tus::OCTET)
                .header("content-length", "9000"),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(header(&created, "upload-offset"), "9000");
    let id = super::tus::upload_id_of(&created);
    assert_eq!(head_offset(&stack, &alice, &id).await, 9_000);

    let resumed = stack
        .tus_send(patch(&alice, &id, 9_000).bytes(&data[9_000..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(header(&resumed, "upload-offset"), "30000");
    assert_eq!(head_offset(&stack, &alice, &id).await, 30_000);
    assert_eq!(
        stack.staged_bytes(&id).unwrap(),
        data,
        "no gap and no duplicated prefix"
    );
    assert_eq!(stack.reservation(&planned.session).await, held);
    assert_eq!(stack.tus_count().await, 1);
    assert_eq!(stack.staging_dirs().len(), 1);
    assert_quiet_row(&stack, &id, 30_000).await;
}

#[tokio::test]
async fn it_tus_patch_accepts_the_method_override() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_plan, id) = upload_for(&stack, &alice, "a.bin", Some(6_000)).await;
    let data = pattern(6_000);
    let overridden = |offset: u64| {
        TusReq::new(Method::POST, &format!("{TUS}/{id}"), &alice)
            .header("x-http-method-override", "PATCH")
            .header("upload-offset", &offset.to_string())
    };

    let first = stack.tus_send(overridden(0).bytes(&data[..2_500])).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.text());
    assert_eq!(header(&first, "upload-offset"), "2500");
    assert!(header(&first, "upload-expires").ends_with("GMT"));
    let stale = stack.tus_send(overridden(0).bytes(&data[..10])).await;
    assert_tus_error(&stale, StatusCode::CONFLICT, "UPLOAD_OFFSET_MISMATCH");
    let no_csrf = stack
        .tus_send(overridden(2_500).bytes(&data[2_500..]).no_csrf())
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    let checked = stack
        .tus_send(
            overridden(2_500)
                .header("upload-checksum", &checksum_of(&data[2_500..]))
                .bytes(&data[2_500..]),
        )
        .await;
    assert_eq!(checked.status, StatusCode::NO_CONTENT, "{}", checked.text());
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
}

#[tokio::test]
async fn it_tus_patch_streams_every_buffer_boundary_without_collecting() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = stack_with_limits(root.path(), TusLimits::production(262_144)).await;
    let alice = stack.member("alice", HOST_A).await;
    let (_plan, id) = upload_for(&stack, &alice, "a.bin", None).await;
    let sizes: [usize; 9] = [
        0,
        1,
        255 * 1024,
        256 * 1024,
        257 * 1024,
        8 * 1024 * 1024 - 1,
        1,
        8 * 1024 * 1024,
        1,
    ];
    let total: usize = sizes.iter().sum();
    let data = pattern(total);
    let mut offset = 0_usize;
    for size in sizes {
        let end = offset + size;
        let response = stack
            .tus_send(patch(&alice, &id, offset as u64).bytes(&data[offset..end]))
            .await;
        assert_eq!(
            response.status,
            StatusCode::NO_CONTENT,
            "{size}: {}",
            response.text()
        );
        assert_eq!(
            header(&response, "upload-offset"),
            end.to_string(),
            "{size}"
        );
        assert_eq!(stack.tus_row(&id).await.offset, end as i64, "{size}");
        offset = end;
    }
    assert_blob_equals(&blob_path(&stack, &id), &data);
    assert!(
        staging.max_chunk.load(Ordering::SeqCst) <= 262_144,
        "no write exceeds the buffer: {}",
        staging.max_chunk.load(Ordering::SeqCst)
    );
    assert_quiet_row(&stack, &id, total as i64).await;
}

#[tokio::test]
async fn it_tus_patch_survives_a_storage_write_failure_with_only_the_flushed_prefix() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(20_000)).await;
    let data = pattern(20_000);
    staging.fail_write_after.store(-1, Ordering::SeqCst);
    let accepted = stack
        .tus_send(patch(&alice, &id, 0).bytes(&data[..2_000]))
        .await;
    assert_eq!(accepted.status, StatusCode::NO_CONTENT);

    let second = stack.planned(&alice, "b.bin", Some(20_000)).await;
    staging.fail_write_after.store(4_096, Ordering::SeqCst);
    let id_two = stack.tus_created(&alice, &second, Some(20_000)).await;
    let failed = stack
        .tus_send(patch(&alice, &id_two, 0).bytes(&data[..12_000]))
        .await;
    assert_tus_error(
        &failed,
        StatusCode::INSUFFICIENT_STORAGE,
        "STORAGE_WRITE_FAILED",
    );
    let row = stack.tus_row(&id_two).await;
    assert_eq!(
        row.offset, 4_096,
        "only the bytes that reached disk are acknowledged"
    );
    assert_eq!(blob_len(&stack, &id_two), 4_096);
    assert!(row.locked_by.is_none());
    assert_eq!(
        stack.item_state(&second.item).await,
        "uploading",
        "a storage failure is retryable"
    );
    staging.fail_write_after.store(-1, Ordering::SeqCst);
    let resumed = stack
        .tus_send(patch(&alice, &id_two, 4_096).bytes(&data[4_096..]))
        .await;
    assert_eq!(resumed.status, StatusCode::NO_CONTENT, "{}", resumed.text());
    assert_eq!(stack.staged_bytes(&id_two).unwrap(), data);
    assert_eq!(stack.item_state(&planned.item).await, "uploading");
}

#[tokio::test]
async fn it_tus_patch_is_incremental_over_the_harness_body() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let (planned, id) = upload_for(&stack, &alice, "a.bin", Some(30_000)).await;
    let data = pattern(30_000);
    let (sender, body) = channel_body();
    let request = stack.tus_send(framed_patch(&alice, &id, 0, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..20_000])))
            .unwrap();
        wait_for_offset(&stack, &id, 16_384).await;
        let mid = stack.tus_row(&id).await;
        assert!(mid.offset >= 16_384 && mid.offset <= 20_000);
        assert!(mid.locked_by.is_some(), "the lease is held while streaming");
        assert_eq!(upload_of(&stack, &planned.item).await, id);
        sender
            .send(Ok(Bytes::copy_from_slice(&data[20_000..])))
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
    assert_eq!(header(&response, "upload-offset"), "30000");
    assert_eq!(stack.staged_bytes(&id).unwrap(), data);
}

fn override_patch<'a>(member: &'a Member, id: &str, offset: u64) -> TusReq<'a> {
    TusReq::new(Method::POST, &format!("{TUS}/{id}"), member)
        .header("x-http-method-override", "PATCH")
        .header("upload-offset", &offset.to_string())
}

#[tokio::test]
async fn it_tus_method_override_patch_enforces_the_same_gates_as_patch() {
    let root = TempDir::new().unwrap();
    let (stack, staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_A).await;
    let (_plan, id) = upload_for(&stack, &alice, "a.bin", Some(30_000)).await;
    let data = pattern(30_000);

    let anonymous = stack
        .tus_send(
            TusReq::anonymous(Method::POST, &format!("{TUS}/{id}"))
                .header("x-http-method-override", "PATCH")
                .header("upload-offset", "0")
                .bytes(&data[..10]),
        )
        .await;
    assert!(matches!(
        anonymous.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let no_csrf = stack
        .tus_send(override_patch(&alice, &id, 0).bytes(&data[..10]).no_csrf())
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    let no_version = stack
        .tus_send(
            override_patch(&alice, &id, 0)
                .bytes(&data[..10])
                .no_version(),
        )
        .await;
    assert_tus_error(
        &no_version,
        StatusCode::PRECONDITION_FAILED,
        "TUS_VERSION_UNSUPPORTED",
    );
    let wrong_type = stack
        .tus_send(
            TusReq::new(Method::POST, &format!("{TUS}/{id}"), &alice)
                .header("x-http-method-override", "PATCH")
                .header("upload-offset", "0")
                .header("content-type", "application/json")
                .header("content-length", "10")
                .raw_body(Body::from(data[..10].to_vec())),
        )
        .await;
    assert_tus_error(
        &wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );
    let foreign = stack
        .tus_send(override_patch(&bob, &id, 0).bytes(&data[..10]))
        .await;
    assert_tus_error(&foreign, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(blob_len(&stack, &id), 0, "no rejected request wrote a byte");

    let (sender, body) = channel_body();
    let streaming = stack.tus_send(
        override_patch(&alice, &id, 0)
            .raw_body(body)
            .header("content-type", super::tus::OCTET),
    );
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&data[..20_000])))
            .unwrap();
        wait_for_offset(&stack, &id, 16_384).await;
        assert!(stack.tus_row(&id).await.locked_by.is_some());
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(streaming, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(header(&response, "upload-offset"), "20000");
    assert!(
        staging.max_chunk.load(Ordering::SeqCst) <= 4_096,
        "the overridden body is streamed through the bounded buffer"
    );

    let (planned, quota_id) = upload_for(&stack, &alice, "q.bin", None).await;
    stack.set_quota(alice.id, Some(31_000)).await;
    let over = stack
        .tus_send(
            TusReq::new(Method::POST, &format!("{TUS}/{quota_id}"), &alice)
                .header("x-http-method-override", "PATCH")
                .header("upload-offset", "0")
                .raw_body(stream_of(frames(&data[..3_000], &[1_500, 1_500])))
                .header("content-type", super::tus::OCTET),
        )
        .await;
    assert_tus_error(&over, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    assert_eq!(stack.tus_row(&quota_id).await.offset, 1_000);
    assert_eq!(stack.item_state(&planned.item).await, "failed");
}
