use std::sync::atomic::Ordering;

use sha2::{Digest, Sha256};

use super::folders::HOST_WORK;
use super::s3_multipart::{timeout, upstream, CompleteFault, S3Rig};
use super::transfers::{request_in, sized, unsized_file};
use super::tus::pattern;
use super::*;
use crate::features::transfers::finalize::Point;
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{MultipartHandle, MultipartStorage as _};
use crate::storage::s3::profile::MIB;

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn data(size: u64) -> Vec<u8> {
    pattern(usize::try_from(size).unwrap())
}

fn invalid_part() -> StorageError {
    crate::storage::s3::classify_failure(
        crate::storage::s3::Operation::CompleteMultipartUpload,
        crate::storage::s3::Failure::Service {
            status: 400,
            code: Some("InvalidPart".to_owned()),
        },
    )
}

fn no_such_upload() -> StorageError {
    StorageError::NotFound
}

async fn tombstones(rig: &S3Rig) -> i64 {
    rig.stack
        .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned'")
        .await
}

#[tokio::test]
async fn it_s3mp_concurrent_creates_leave_one_upload() {
    let rig = S3Rig::new().await;
    let (session, item) = rig.one("race.bin", 20 * MIB).await;
    let (a, b, c, d, e, f) = tokio::join!(
        rig.create(&session, &item),
        rig.create(&session, &item),
        rig.create(&session, &item),
        rig.create(&session, &item),
        rig.create(&session, &item),
        rig.create(&session, &item),
    );
    let attempts = [a, b, c, d, e, f];
    let created = attempts
        .iter()
        .filter(|fetched| fetched.status == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "exactly one create wins");
    for loser in attempts
        .iter()
        .filter(|fetched| fetched.status != StatusCode::CREATED)
    {
        assert_eq!(loser.status, StatusCode::CONFLICT, "{}", loser.text());
        assert_eq!(loser.error_code(), "MULTIPART_ALREADY_CREATED");
    }
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        1
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts")
            .await,
        3
    );
    let key = rig.final_key(&item).await;
    assert_eq!(
        rig.open_uploads(&key).await,
        1,
        "every losing provider upload was aborted"
    );
    assert_eq!(
        rig.faults.aborts.load(Ordering::SeqCst),
        rig.faults.creates.load(Ordering::SeqCst) - 1
    );
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    let again = rig.create(&session, &item).await;
    assert_eq!(again.error_code(), "MULTIPART_ALREADY_CREATED");
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_concurrent_completes_commit_once() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("twice.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let (a, b, c, d, e) = tokio::join!(
        rig.complete(&session, &item),
        rig.complete(&session, &item),
        rig.complete(&session, &item),
        rig.complete(&session, &item),
        rig.complete(&session, &item),
    );
    let attempts = [a, b, c, d, e];
    let mut file_ids = std::collections::BTreeSet::new();
    for fetched in &attempts {
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        file_ids.insert(fetched.json()["fileId"].as_str().unwrap().to_owned());
    }
    assert_eq!(file_ids.len(), 1, "every caller sees the one file");
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 1);
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(rig.uploads_audited().await, 1);
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(i64::try_from(size).unwrap()))
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_abort_wins_before_completion() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);
    let (session, item) = rig.one("aborted.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let key = rig.final_key(&item).await;

    let aborted = rig.abort(&session, &item).await;
    assert_eq!(aborted.status, StatusCode::NO_CONTENT, "{}", aborted.text());
    assert_eq!(rig.item_row(&item).await.0, "canceled");
    assert_eq!(rig.multipart_state(&item).await.as_deref(), Some("aborted"));
    assert_eq!(
        rig.open_uploads(&key).await,
        0,
        "the provider upload is gone"
    );
    assert_eq!(rig.faults.aborts.load(Ordering::SeqCst), 1);
    assert_eq!(rig.stack.reservation(&session).await.0, "released");

    let completed = rig.complete(&session, &item).await;
    assert_eq!(
        completed.status,
        StatusCode::NOT_FOUND,
        "{}",
        completed.text()
    );
    assert_eq!(completed.error_code(), "MULTIPART_NOT_FOUND");
    let presigned = rig.presign(&session, &item, &[1]).await;
    assert_eq!(presigned.error_code(), "MULTIPART_NOT_FOUND");
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 0);

    let twice = rig.abort(&session, &item).await;
    assert_eq!(
        twice.status,
        StatusCode::NO_CONTENT,
        "aborting twice is idempotent"
    );
    assert_eq!(rig.multipart_state(&item).await.as_deref(), Some("aborted"));
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_abort_during_completion_leaves_no_file() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);
    let (session, item) = rig.one("midair.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let key = rig.final_key(&item).await;

    let gate = rig.hooks.gate_at(Point::BeforePlace);
    let (completed, aborted) = tokio::join!(rig.complete(&session, &item), async {
        gate.wait_until_entered().await;
        let aborted = rig.abort(&session, &item).await;
        gate.open();
        aborted
    });
    assert_eq!(aborted.status, StatusCode::NO_CONTENT, "{}", aborted.text());
    assert_ne!(completed.status, StatusCode::OK, "{}", completed.text());
    assert!(
        matches!(
            completed.status,
            StatusCode::CONFLICT | StatusCode::NOT_FOUND
        ),
        "{}",
        completed.text()
    );
    assert_eq!((rig.files().await, rig.used().await), (0, 0));
    assert_eq!(rig.item_row(&item).await.0, "canceled");
    assert_eq!(rig.stack.reservation(&session).await.0, "released");
    assert!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await
            == 0,
        "no active object"
    );
    assert_eq!(rig.open_uploads(&key).await, 0);
    assert_eq!(rig.uploads_audited().await, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_completion_wins_before_abort() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);
    let (session, item) = rig.one("kept.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let key = rig.final_key(&item).await;

    let aborted = rig.abort(&session, &item).await;
    assert_eq!(aborted.status, StatusCode::CONFLICT, "{}", aborted.text());
    assert_eq!(aborted.error_code(), "MULTIPART_ALREADY_COMPLETED");
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(rig.read_object(&key).await, (size, sha256(&bytes)));
    assert_eq!(rig.faults.aborts.load(Ordering::SeqCst), 0);
    assert_eq!(rig.item_row(&item).await.0, "completed");
    for fetched in [
        rig.presign(&session, &item, &[1]).await,
        rig.report(
            &session,
            &item,
            json!([{ "partNumber": 1, "etag": "\"a\"", "sizeBytes": 8 * MIB }]),
        )
        .await,
        rig.parts(&session, &item).await,
    ] {
        assert_eq!(fetched.status, StatusCode::CONFLICT, "{}", fetched.text());
        assert_eq!(fetched.error_code(), "MULTIPART_ALREADY_COMPLETED");
    }
}

#[tokio::test]
async fn it_s3mp_provider_failures_are_classified_and_completion_recovers() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 3;
    let bytes = data(size);

    // a timeout before the provider answered: retryable, resumes with a second completion
    let (session, item) = rig.one("timeout.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    rig.faults
        .complete
        .lock()
        .unwrap()
        .push_back(CompleteFault::Before(timeout));
    let failed = rig.complete(&session, &item).await;
    assert_eq!(
        failed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        failed.text()
    );
    assert_eq!(failed.error_code(), "STORAGE_UNAVAILABLE");
    let (state, stage, _) = rig.item_row(&item).await;
    assert_eq!((state.as_str(), stage.as_str()), ("finalizing", "placing"));
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("completing")
    );
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    let retried = rig.complete(&session, &item).await;
    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text());
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 2);
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));

    // a lost response after the provider completed: adopted, never completed twice
    let (session, item) = rig.one("lost.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let before = rig.faults.completes.load(Ordering::SeqCst);
    rig.faults
        .complete
        .lock()
        .unwrap()
        .push_back(CompleteFault::After(upstream));
    let lost = rig.complete(&session, &item).await;
    assert_eq!(
        lost.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        lost.text()
    );
    assert_eq!(rig.files().await, 1);
    let adopted = rig.complete(&session, &item).await;
    assert_eq!(adopted.status, StatusCode::OK, "{}", adopted.text());
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), before + 1);
    assert_eq!((rig.files().await, rig.objects().await), (2, 2));
    assert_eq!(rig.used().await, i64::try_from(2 * size).unwrap());

    // NoSuchUpload while the object exists: the completion already happened
    let (session, item) = rig.one("vanished-upload.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    rig.faults
        .complete
        .lock()
        .unwrap()
        .push_back(CompleteFault::After(no_such_upload));
    let adopted = rig.complete(&session, &item).await;
    assert_eq!(adopted.status, StatusCode::OK, "{}", adopted.text());
    assert_eq!((rig.files().await, rig.objects().await), (3, 3));

    // an InvalidPart answer leaves the upload open and the item uploading
    let (session, item) = rig.one("invalid.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    rig.faults
        .complete
        .lock()
        .unwrap()
        .push_back(CompleteFault::Before(invalid_part));
    let refused = rig.complete(&session, &item).await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text()
    );
    assert_eq!(refused.error_code(), "MULTIPART_PART_INVALID");
    let (state, stage, _) = rig.item_row(&item).await;
    assert_eq!((state.as_str(), stage.as_str()), ("uploading", "none"));
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("in_progress")
    );
    assert_eq!(rig.files().await, 3);
    let recovered = rig.complete(&session, &item).await;
    assert_eq!(recovered.status, StatusCode::OK, "{}", recovered.text());
    assert_eq!(rig.files().await, 4);

    // NoSuchUpload with no object: the upload was aborted elsewhere
    let (session, item) = rig.one("gone.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    rig.faults
        .complete
        .lock()
        .unwrap()
        .push_back(CompleteFault::Before(no_such_upload));
    let lost = rig.complete(&session, &item).await;
    assert_eq!(lost.status, StatusCode::NOT_FOUND, "{}", lost.text());
    assert_eq!(lost.error_code(), "MULTIPART_NOT_FOUND");
    let (state, _, code) = rig.item_row(&item).await;
    assert_eq!(
        (state.as_str(), code.as_deref()),
        ("failed", Some("MULTIPART_NOT_FOUND"))
    );
    assert_eq!(rig.files().await, 4);
    assert_eq!(
        tombstones(&rig).await,
        1,
        "the recorded key is tombstoned defensively"
    );
    assert_eq!(rig.stack.reservation(&session).await.1, 0);
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("abandoned")
    );
    assert_eq!(rig.uploads_audited().await, 4);

    // provider failures while listing change nothing
    let (session, item) = rig.one("listing.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    rig.faults.list_parts.lock().unwrap().push_back(upstream);
    let down = rig.complete(&session, &item).await;
    assert_eq!(
        down.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        down.text()
    );
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("in_progress")
    );
    rig.faults
        .list_parts
        .lock()
        .unwrap()
        .push_back(no_such_upload);
    let missing = rig.parts(&session, &item).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), "MULTIPART_NOT_FOUND");
    let finished = rig.complete(&session, &item).await;
    assert_eq!(finished.status, StatusCode::OK, "{}", finished.text());
    assert_eq!(rig.files().await, 5);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_short_non_final_part_is_refused_before_the_provider_completes() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);
    let (session, item) = rig.one("short.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let presigned = rig.presign(&session, &item, &[1, 2, 3]).await;
    let urls: Vec<String> = presigned.json()["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| part["url"].as_str().unwrap().to_owned())
        .collect();
    rig.put_part(&urls[0], bytes[..6 * MIB as usize].to_vec())
        .await;
    rig.put_part(
        &urls[1],
        bytes[8 * MIB as usize..16 * MIB as usize].to_vec(),
    )
    .await;
    rig.put_part(&urls[2], bytes[16 * MIB as usize..].to_vec())
        .await;
    let refused = rig.complete(&session, &item).await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text()
    );
    assert_eq!(refused.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 0);
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    let report = rig
        .report(
            &session,
            &item,
            json!([{ "partNumber": 1, "etag": "\"abc\"", "sizeBytes": 6 * MIB }]),
        )
        .await;
    assert_eq!(report.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(report.error_code(), "MULTIPART_PART_INVALID");
    for bad in [
        json!([{ "partNumber": 0, "etag": "\"a\"", "sizeBytes": 8 * MIB }]),
        json!([{ "partNumber": 4, "etag": "\"a\"", "sizeBytes": 8 * MIB }]),
        json!([{ "partNumber": 1, "etag": "", "sizeBytes": 8 * MIB }]),
        json!([{ "partNumber": 1, "etag": "bad\nvalue", "sizeBytes": 8 * MIB }]),
        json!([
            { "partNumber": 1, "etag": "\"a\"", "sizeBytes": 8 * MIB },
            { "partNumber": 1, "etag": "\"b\"", "sizeBytes": 8 * MIB }
        ]),
    ] {
        let fetched = rig.report(&session, &item, bad.clone()).await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
        assert_eq!(fetched.error_code(), "MULTIPART_PART_INVALID", "{bad}");
    }
    let empty = rig.report(&session, &item, json!([])).await;
    assert_eq!(empty.error_code(), "VALIDATION_ERROR");

    rig.upload_parts(&session, &item, &bytes, &[1], true).await;
    let done = rig.complete(&session, &item).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    let key = rig.final_key(&item).await;
    assert_eq!(rig.read_object(&key).await, (size, sha256(&bytes)));
}

#[tokio::test]
async fn it_s3mp_checksum_required_profile_is_deferred_not_presigned() {
    let rig = S3Rig::new().await;
    rig.proxied.store(true, Ordering::SeqCst);
    let (session, item) = rig.one("proxied.bin", 20 * MIB).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["checksumHeadersRequired"], true);
    assert_eq!(created.json()["partSizeBytes"], 8 * MIB);
    assert_eq!(created.json()["partCount"], 3);
    let signs = rig.faults.signs.load(Ordering::SeqCst);
    let presigned = rig.presign(&session, &item, &[1]).await;
    assert_eq!(
        presigned.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        presigned.text()
    );
    assert_eq!(presigned.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(
        presigned.json()["error"]["details"]["reason"],
        "server_proxied_part_upload"
    );
    assert_eq!(
        rig.faults.signs.load(Ordering::SeqCst),
        signs,
        "no incompatible URL is signed"
    );
    assert!(!presigned.text().contains("X-Amz"));

    rig.proxied.store(false, Ordering::SeqCst);
    let (session, item) = rig
        .session(&[unsized_file("c1", "stream.bin")])
        .await
        .pipe();
    let unknown = rig.create(&session, &item).await;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown.text()
    );
    assert_eq!(unknown.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(
        unknown.json()["error"]["details"]["reason"],
        "unknown_length_requires_server_mediated_upload"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        1
    );
}

trait Single {
    fn pipe(self) -> (String, String);
}

impl Single for (String, Vec<String>) {
    fn pipe(self) -> (String, String) {
        (self.0, self.1.into_iter().next().unwrap())
    }
}

#[tokio::test]
async fn it_s3mp_lists_every_provider_page() {
    let rig = S3Rig::new().await;
    let parts = 1_025_u64;
    let size = parts * 8 * MIB;
    let (session, item) = rig.one("paged.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["partCount"], parts);
    let (key, upload_id): (String, String) =
        sqlx::query_as("SELECT object_key, s3_upload_id FROM s3_multipart_uploads")
            .fetch_one(rig.stack.pools.reader().executor())
            .await
            .unwrap();
    let handle = MultipartHandle::new(ObjectKey::parse(&key).unwrap(), upload_id);
    for number in 1..=u32::try_from(parts).unwrap() {
        rig.provider
            .upload_part_stream(
                &handle,
                number,
                16,
                Box::pin(std::io::Cursor::new(vec![7_u8; 16])),
            )
            .await
            .unwrap();
    }
    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let body = listed.json();
    assert_eq!(
        body["completedParts"].as_array().unwrap().len(),
        1_025,
        "both provider pages are read"
    );
    assert_eq!(body["completedParts"][1_024]["partNumber"], 1_025);
    assert_eq!(body["missingPartNumbers"], json!([]));
    assert_eq!(body["uploadedBytes"], 1_025 * 16);
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts WHERE state = 'verified'")
            .await,
        1_025
    );
    let refused = rig.complete(&session, &item).await;
    assert_eq!(refused.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 0);
    let aborted = rig.abort(&session, &item).await;
    assert_eq!(aborted.status, StatusCode::NO_CONTENT);
    assert_eq!(rig.stack.reservation(&session).await.0, "released");
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_completion_rechecks_account_folder_and_expiry() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);

    let (session, item) = rig.one("inactive.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    let (completed, ()) = tokio::join!(rig.complete(&session, &item), async {
        gate.wait_until_entered().await;
        rig.stack
            .execute(&format!(
                "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z' WHERE id = '{}'",
                rig.alice.id
            ))
            .await;
        gate.open();
    });
    assert_eq!(
        completed.status,
        StatusCode::FORBIDDEN,
        "{}",
        completed.text()
    );
    assert_eq!(completed.error_code(), "AUTH_ACCOUNT_INACTIVE");
    assert_eq!(rig.item_row(&item).await.0, "failed");
    assert_eq!((rig.files().await, rig.used().await), (0, 0));
    assert_eq!(tombstones(&rig).await, 1, "the placed object is tombstoned");
    rig.stack
        .execute(&format!(
            "UPDATE users SET is_active = 1, deactivated_at = NULL WHERE id = '{}'",
            rig.alice.id
        ))
        .await;

    let folder = rig.stack.make(&rig.alice, "target", None).await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let opened = rig
        .stack
        .open(
            &rig.alice,
            &request_in(&folder_id, &[sized("c1", "inside.bin", size)]),
        )
        .await;
    assert_eq!(opened.status, StatusCode::CREATED, "{}", opened.text());
    let session = opened.json()["id"].as_str().unwrap().to_owned();
    let item = rig.stack.item_ids(&session).await.remove(0);
    rig.open(&session, &item, &bytes).await;
    let removed = rig
        .stack
        .api(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}"),
            &rig.alice,
            None,
        )
        .await;
    assert!(removed.status.is_success(), "{}", removed.text());
    let key = rig.final_key(&item).await;
    let gone = rig.complete(&session, &item).await;
    assert!(
        matches!(
            (gone.status, gone.error_code().as_str()),
            (StatusCode::CONFLICT, "FOLDER_DELETING") | (StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND")
        ),
        "{}",
        gone.text()
    );
    assert_eq!(rig.files().await, 0);
    assert_eq!(
        rig.faults.completes.load(Ordering::SeqCst),
        1,
        "only the first item reached the provider"
    );
    assert_eq!(
        rig.open_uploads(&key).await,
        0,
        "the rejected upload was aborted"
    );
    assert_eq!(rig.multipart_state(&item).await.as_deref(), Some("aborted"));

    let (session, item) = rig.one("expired.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED);
    rig.stack
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-09-25T11:00:00.000Z' WHERE id = '{session}'"
        ))
        .await;
    for fetched in [
        rig.presign(&session, &item, &[1]).await,
        rig.parts(&session, &item).await,
        rig.complete(&session, &item).await,
    ] {
        assert_eq!(fetched.status, StatusCode::GONE, "{}", fetched.text());
        assert_eq!(fetched.error_code(), "TRANSFER_SESSION_EXPIRED");
    }
    rig.assert_no_overlap();
}

fn with_body<'a>(call: super::profile::Call<'a>, body: &Option<Value>) -> super::profile::Call<'a> {
    match body {
        Some(body) => call.json(body),
        None => call,
    }
}

#[tokio::test]
async fn it_s3mp_mutations_require_authentication_and_csrf() {
    let rig = S3Rig::new().await;
    let size = 9 * MIB;
    let (session, item) = rig.one("guarded.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let bodies: [(&str, Option<Value>); 5] = [
        ("", None),
        ("/presign", Some(json!({ "partNumbers": [1] }))),
        (
            "/parts",
            Some(json!({ "parts": [{ "partNumber": 1, "etag": "\"a\"", "sizeBytes": 8 * MIB }] })),
        ),
        ("/complete", None),
        ("/abort", None),
    ];
    for (tail, body) in &bodies {
        let path = S3Rig::path(&session, &item, tail);
        let mut anonymous = super::profile::Call::new(Method::POST, &path, &rig.alice.creds);
        anonymous.session = None;
        let fetched = rig.stack.call(with_body(anonymous, body), HOST_WORK).await;
        assert_eq!(
            fetched.status,
            StatusCode::UNAUTHORIZED,
            "{tail}: {}",
            fetched.text()
        );

        let mut forged = super::profile::Call::new(Method::POST, &path, &rig.alice.creds);
        forged.csrf_header = None;
        let fetched = rig.stack.call(with_body(forged, body), HOST_WORK).await;
        assert_eq!(
            fetched.status,
            StatusCode::FORBIDDEN,
            "{tail}: {}",
            fetched.text()
        );

        let mut foreign = super::profile::Call::new(Method::POST, &path, &rig.alice.creds);
        foreign.origin = Some("https://evil.example.test");
        let fetched = rig.stack.call(with_body(foreign, body), HOST_WORK).await;
        assert_eq!(
            fetched.status,
            StatusCode::FORBIDDEN,
            "{tail}: {}",
            fetched.text()
        );
    }
    let read = rig
        .stack
        .call(
            super::profile::Call::new(
                Method::GET,
                &S3Rig::path(&session, &item, "/parts"),
                &rig.alice.creds,
            ),
            HOST_WORK,
        )
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 0);
    assert_eq!(rig.faults.aborts.load(Ordering::SeqCst), 0);
    assert_eq!(rig.multipart_state(&item).await.as_deref(), Some("created"));
}

#[tokio::test]
async fn it_s3mp_local_items_cannot_use_the_s3_routes() {
    let rig = super::finalize::Rig::new().await;
    let planned = rig.plan("local.bin", Some(1_000)).await;
    let path = |tail: &str| S3Rig::path(&planned.session, &planned.item, tail);
    let created = rig
        .stack
        .api(Method::POST, &path(""), &rig.alice, None)
        .await;
    assert_eq!(created.status, StatusCode::CONFLICT, "{}", created.text());
    assert_eq!(created.error_code(), "TRANSFER_SESSION_STATE_INVALID");
    for (method, tail, body) in [
        (
            Method::POST,
            "/presign",
            Some(json!({ "partNumbers": [1] })),
        ),
        (Method::GET, "/parts", None),
        (Method::POST, "/complete", None),
        (Method::POST, "/abort", None),
    ] {
        let fetched = rig
            .stack
            .api(method, &path(tail), &rig.alice, body.as_ref())
            .await;
        assert_eq!(
            fetched.status,
            StatusCode::NOT_FOUND,
            "{tail}: {}",
            fetched.text()
        );
        assert_eq!(fetched.error_code(), "MULTIPART_NOT_FOUND");
    }
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        0
    );
    assert_eq!(rig.item_row(&planned.item).await.0, "pending");
}

#[tokio::test]
async fn it_s3mp_commit_rechecks_quota_and_tombstones_the_placed_object() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let bytes = data(size);
    let (session, item) = rig.one("over.bin", size).await;
    rig.open(&session, &item, &bytes).await;
    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    let (completed, ()) = tokio::join!(rig.complete(&session, &item), async {
        gate.wait_until_entered().await;
        rig.stack.set_quota(rig.alice.id, Some(1_000)).await;
        gate.open();
    });
    assert_eq!(
        completed.status,
        StatusCode::INSUFFICIENT_STORAGE,
        "{}",
        completed.text()
    );
    assert_eq!(completed.error_code(), "QUOTA_EXCEEDED");
    assert_eq!((rig.files().await, rig.used().await), (0, 0));
    let tombstone: (String, i64) = sqlx::query_as("SELECT state, size_bytes FROM storage_objects")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        tombstone,
        ("tombstoned".to_owned(), i64::try_from(size).unwrap())
    );
    assert_eq!(
        rig.item_row(&item).await.2.as_deref(),
        Some("QUOTA_EXCEEDED")
    );
    assert_eq!(
        rig.stack.reservation(&session).await.1,
        0,
        "the share is released"
    );
    assert_eq!(rig.uploads_audited().await, 0);
    let again = rig.complete(&session, &item).await;
    assert_eq!(
        again.error_code(),
        "QUOTA_EXCEEDED",
        "a recorded failure repeats"
    );
    rig.assert_no_overlap();
}
