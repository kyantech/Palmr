use futures_util::future::join_all;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::s3_multipart::{ListMode, Listing, S3Rig};
use super::s3_proxied::{proxied_rig, steady};
use super::s3_reapers::{
    aborted, data, enqueue_raw, instant, jobs_of, live_upload, multipart_id, row, Reaper, ABORT,
    GRACE, MILLI, RECONCILE,
};
use super::transfer_reaper::{DAY, HOUR};
use super::transfers::sized;
use super::*;
use crate::features::transfers::s3::reapers::{Coverage, Point, Strategy};
use crate::infra::jobs::JobKind;
use crate::storage::key::{BrandingKind, KeyNamespace, ObjectKey};
use crate::storage::provider::{MultipartHandle, MultipartStorage as _, PutHint};
use crate::storage::s3::profile::MIB;

const DAY_PLUS: Duration = Duration::from_secs(24 * 60 * 60);

async fn orphan(rig: &S3Rig) -> (MultipartHandle, OffsetDateTime) {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = rig
        .provider
        .create_multipart(&key, PutHint::default())
        .await
        .unwrap();
    let listed = rig.provider.list_multipart_uploads("", None).await.unwrap();
    let initiated = listed
        .uploads
        .iter()
        .find(|pending| pending.handle == handle)
        .expect("the provider lists the orphan")
        .initiated_at;
    (handle, initiated)
}

async fn orphan_jobs(rig: &S3Rig) -> i64 {
    rig.stack
        .scalar_i64(
            "SELECT COUNT(*) FROM jobs WHERE kind = 's3.abort_abandoned_multipart'
                AND json_extract(payload_json, '$.mode') = 'orphan'",
        )
        .await
}

async fn reconcile(rig: &S3Rig, reaper: &Reaper, tag: &str) -> u64 {
    enqueue_raw(
        rig,
        RECONCILE,
        json!({"mode": "sweep"}),
        &format!("t:{tag}"),
    )
    .await;
    reaper.run_all(RECONCILE).await
}

fn coverage(reaper: &Reaper) -> Coverage {
    reaper.context.scan_status().last().unwrap().coverage
}

async fn provider_has(rig: &S3Rig, handle: &MultipartHandle) -> bool {
    rig.provider
        .list_multipart_uploads(handle.key().as_str(), None)
        .await
        .unwrap()
        .uploads
        .iter()
        .any(|pending| pending.handle == *handle)
}

async fn renew_in_db(rig: &S3Rig, item: &str) {
    let expires = crate::domain::time::Timestamp::try_from(
        clock_now(&rig.stack.clock) + Duration::from_secs(24 * 60 * 60),
    )
    .unwrap();
    rig.stack
        .execute(&format!(
            "UPDATE s3_multipart_uploads SET expires_at = '{expires}'
              WHERE transfer_session_file_id = '{item}'"
        ))
        .await;
}

fn set_listing(rig: &S3Rig, listing: Listing) {
    *rig.faults.listing.lock().unwrap() = listing;
}

#[tokio::test]
async fn it_minio_enumerates_multipart_uploads_only_by_bucket_root_or_exact_key() {
    let rig = S3Rig::new().await;
    let mut handles = Vec::new();
    for _ in 0..3 {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        handles.push(
            rig.provider
                .create_multipart(&key, PutHint::default())
                .await
                .unwrap(),
        );
    }
    let branding = ObjectKey::allocate(KeyNamespace::Branding(BrandingKind::Logo));
    rig.provider
        .create_multipart(&branding, PutHint::default())
        .await
        .unwrap();
    let first = handles[0].key().as_str().to_owned();
    let counts = |prefix: String| {
        let provider = &rig.provider;
        async move {
            provider
                .list_multipart_uploads(&prefix, None)
                .await
                .unwrap()
                .uploads
                .len()
        }
    };
    assert_eq!(
        counts(String::new()).await,
        4,
        "the bucket root lists every upload"
    );
    assert_eq!(
        counts(first.clone()).await,
        1,
        "an exact key lists its upload"
    );
    for prefix in [
        "objects/",
        "objects",
        &first[..11],
        &first[..14],
        "branding/",
    ] {
        assert_eq!(
            counts(prefix.to_owned()).await,
            0,
            "the pinned MinIO build answers a namespace prefix with nothing: {prefix}"
        );
    }
}

#[tokio::test]
async fn it_reconcile_sweep_age_guard() {
    let rig = S3Rig::new().await;
    let reaper = Reaper::new(&rig);
    let mut orphans = Vec::new();
    for _ in 0..3 {
        orphans.push(orphan(&rig).await);
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let (first, first_at) = orphans[0].clone();

    rig.stack.clock.set(first_at + DAY_PLUS - MILLI);
    reconcile(&rig, &reaper, "minus").await;
    assert_eq!(orphan_jobs(&rig).await, 0, "24 h - 1 ms is too young");
    reaper.drain().await;
    assert!(provider_has(&rig, &first).await);

    rig.stack.clock.set(first_at + DAY_PLUS);
    reconcile(&rig, &reaper, "exact").await;
    let due = orphans
        .iter()
        .filter(|(_, at)| first_at + DAY_PLUS >= *at + DAY_PLUS)
        .count();
    assert_eq!(
        orphan_jobs(&rig).await,
        i64::try_from(due).unwrap(),
        "exactly the uploads that are at least 24 h old are queued"
    );
    assert!(due >= 1);
    reaper.drain().await;
    assert!(!provider_has(&rig, &first).await, "age = 24 h");

    let (last, last_at) = orphans[2].clone();
    rig.stack.clock.set(last_at + DAY_PLUS + MILLI);
    reconcile(&rig, &reaper, "plus").await;
    reaper.drain().await;
    assert!(!provider_has(&rig, &last).await, "age = 24 h + 1 ms");
    for (handle, _) in &orphans {
        assert!(!provider_has(&rig, handle).await);
    }
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_orphan_discovery_after_create_then_db_crash() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, known, _) = live_upload(&rig, "known.bin", size).await;
    let known_row = row(&rig, &known).await;
    let (lost, initiated) = orphan(&rig).await;
    let reaper = Reaper::new(&rig);
    rig.stack.clock.set(initiated + DAY_PLUS);
    renew_in_db(&rig, &known).await;

    reconcile(&rig, &reaper, "one").await;
    assert_eq!(
        orphan_jobs(&rig).await,
        1,
        "only the unknown upload is queued"
    );
    reconcile(&rig, &reaper, "two").await;
    assert_eq!(
        orphan_jobs(&rig).await,
        1,
        "the dedup key is the provider identity"
    );
    assert_eq!(coverage(&reaper), Coverage::Complete(Strategy::BucketRoot));

    reaper.drain().await;
    assert!(!provider_has(&rig, &lost).await);
    assert!(aborted(&rig).contains(&(lost.key().as_str().to_owned(), lost.upload_id().to_owned())));
    assert!(
        !aborted(&rig).contains(&(known_row.key.clone(), known_row.upload_id.clone())),
        "the known upload is never aborted by the orphan sweep"
    );
    assert_eq!(row(&rig, &known).await.state, "in_progress");
    assert_eq!(rig.open_uploads(&known_row.key).await, 1);
    assert_eq!(
        (rig.files().await, rig.objects().await),
        (0, 0),
        "nothing is adopted"
    );

    reconcile(&rig, &reaper, "three").await;
    assert_eq!(
        orphan_jobs(&rig).await,
        1,
        "no hot loop after the upload is gone"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_prefix_aware_provider_is_scanned_through_the_objects_prefix() {
    let rig = S3Rig::new().await;
    set_listing(
        &rig,
        Listing {
            mode: ListMode::PrefixAware,
            page_size: Some(2),
            ..Listing::default()
        },
    );
    let reaper = Reaper::new(&rig);
    let mut found = Vec::new();
    for _ in 0..5 {
        found.push(orphan(&rig).await);
    }
    let newest = found.iter().map(|(_, at)| *at).max().unwrap();
    rig.stack.clock.set(newest + DAY_PLUS);
    reconcile(&rig, &reaper, "aware").await;
    assert_eq!(
        coverage(&reaper),
        Coverage::Complete(Strategy::ObjectsPrefix)
    );
    assert_eq!(
        orphan_jobs(&rig).await,
        5,
        "three pages of at most two uploads"
    );
    reaper.drain().await;
    for (handle, _) in &found {
        assert!(!provider_has(&rig, handle).await);
    }
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_exact_key_only_listing_degrades_without_a_false_zero() {
    let rig = S3Rig::new().await;
    set_listing(
        &rig,
        Listing {
            mode: ListMode::ExactOnly,
            ..Listing::default()
        },
    );
    let size = 20 * MIB + 9;
    let (_, known, _) = live_upload(&rig, "known.bin", size).await;
    let (lost, initiated) = orphan(&rig).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.set(initiated + DAY_PLUS * 2);
    reaper.drain().await;

    assert_eq!(coverage(&reaper), Coverage::ExactKeyOnly);
    assert_eq!(orphan_jobs(&rig).await, 0, "nothing is claimed cleaned");
    assert!(
        provider_has(&rig, &lost).await,
        "no destructive fallback scan"
    );
    let known_row = row(&rig, &known).await;
    assert_eq!(
        known_row.state, "aborted",
        "the database-driven abort is unaffected"
    );
    assert!(
        aborted(&rig).contains(&(known_row.key.clone(), known_row.upload_id.clone())),
        "known rows are aborted by their stored identity"
    );
    assert!(!aborted(&rig).contains(&(lost.key().as_str().to_owned(), lost.upload_id().to_owned())));
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_listing_that_the_provider_refuses_is_reported_as_unsupported() {
    let rig = S3Rig::new().await;
    set_listing(
        &rig,
        Listing {
            mode: ListMode::NotImplemented,
            ..Listing::default()
        },
    );
    let reaper = Reaper::new(&rig);
    let executed = reconcile(&rig, &reaper, "refused").await;
    assert_eq!(executed, 1);
    assert_eq!(coverage(&reaper), Coverage::Unsupported);
    assert_eq!(
        jobs_of(&rig, RECONCILE, "succeeded").await,
        1,
        "not a failure"
    );
    assert_eq!(jobs_of(&rig, RECONCILE, "dead").await, 0);
}

#[tokio::test]
async fn it_incomplete_listing_does_not_imply_absence() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let mut live = Vec::new();
    for name in ["a.bin", "b.bin"] {
        live.push(live_upload(&rig, name, size).await);
    }
    let mut orphans = Vec::new();
    for _ in 0..3 {
        orphans.push(orphan(&rig).await);
    }
    set_listing(
        &rig,
        Listing {
            mode: ListMode::PrefixAware,
            page_size: Some(1),
            fail_page: Some(3),
            ..Listing::default()
        },
    );
    let reaper = Reaper::new(&rig);
    let newest = orphans.iter().map(|(_, at)| *at).max().unwrap();
    rig.stack.clock.set(newest + DAY_PLUS);
    let mut keep_alive = Vec::new();
    for (_, item, _) in &live {
        renew_in_db(&rig, item).await;
        keep_alive.push(row(&rig, item).await);
    }

    reconcile(&rig, &reaper, "partial").await;
    assert_eq!(coverage(&reaper), Coverage::Incomplete("listing_failed"));
    for ((_, item, _), before) in live.iter().zip(&keep_alive) {
        let after = row(&rig, item).await;
        assert_eq!(after.state, "in_progress", "an unlisted row is not absent");
        assert_eq!(after.upload_id, before.upload_id);
        assert_eq!(rig.item_row(item).await.0, "uploading");
    }
    assert_eq!(
        rig.stack.counts().await.held,
        2 * i64::try_from(size).unwrap()
    );
    assert_eq!(
        jobs_of(&rig, RECONCILE, "succeeded").await,
        1,
        "a failed enumeration is retried by the next run, not by the job backoff"
    );

    set_listing(&rig, Listing::default());
    reconcile(&rig, &reaper, "complete").await;
    assert_eq!(coverage(&reaper), Coverage::Complete(Strategy::BucketRoot));
    reaper.drain().await;
    for (handle, _) in &orphans {
        assert!(!provider_has(&rig, handle).await);
    }
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_listing_that_repeats_its_cursor_terminates() {
    let rig = S3Rig::new().await;
    set_listing(
        &rig,
        Listing {
            mode: ListMode::PrefixAware,
            page_size: Some(1),
            repeat_cursor: true,
            ..Listing::default()
        },
    );
    orphan(&rig).await;
    let reaper = Reaper::new(&rig);
    let executed = reconcile(&rig, &reaper, "loop").await;
    assert_eq!(executed, 1);
    assert_eq!(coverage(&reaper), Coverage::Incomplete("repeated_cursor"));
    assert!(
        rig.faults
            .list_uploads_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 6,
        "the scan stops at the repeated cursor"
    );
}

#[tokio::test]
async fn it_real_minio_listing_pages_past_one_thousand_uploads() {
    let rig = S3Rig::new().await;
    let reaper = Reaper::new(&rig);
    let total = 1_005_usize;
    let mut handles = Vec::with_capacity(total);
    for chunk in (0..total).collect::<Vec<_>>().chunks(32) {
        let created = join_all(chunk.iter().map(|_| async {
            let key = ObjectKey::allocate(KeyNamespace::Objects);
            rig.provider
                .create_multipart(&key, PutHint::default())
                .await
                .unwrap()
        }))
        .await;
        handles.extend(created);
    }
    let mut newest = None;
    let mut cursor = None;
    let mut sizes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = rig
            .provider
            .list_multipart_uploads("", cursor)
            .await
            .unwrap();
        sizes.push(page.uploads.len());
        for pending in &page.uploads {
            newest = newest.max(Some(pending.initiated_at));
            assert!(
                seen.insert((
                    pending.handle.key().as_str().to_owned(),
                    pending.handle.upload_id().to_owned()
                )),
                "a page repeated an upload"
            );
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen.len(),
        total,
        "every upload is listed exactly once: pages {sizes:?}"
    );

    rig.stack.clock.set(newest.unwrap() + DAY_PLUS);
    reconcile(&rig, &reaper, "thousand").await;
    assert_eq!(orphan_jobs(&rig).await, i64::try_from(total).unwrap());
    reaper.drain().await;
    let left = rig
        .provider
        .list_multipart_uploads("", None)
        .await
        .unwrap()
        .uploads
        .len();
    assert_eq!(left, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_orphan_job_payloads_are_validated_against_server_state() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, known, _) = live_upload(&rig, "known.bin", size).await;
    let known_row = row(&rig, &known).await;
    let (young, young_at) = orphan(&rig).await;
    let (old, old_at) = orphan(&rig).await;
    let reaper = Reaper::new(&rig);
    let bucket = rig.bucket.clone();
    let at = |instant: OffsetDateTime| {
        crate::domain::time::Timestamp::try_from(instant)
            .unwrap()
            .to_string()
    };
    let cases = [
        (
            "known",
            json!({"mode": "orphan", "bucket": bucket, "key": known_row.key,
            "uploadId": known_row.upload_id, "initiatedAt": at(old_at - DAY_PLUS * 3)}),
        ),
        (
            "foreign_bucket",
            json!({"mode": "orphan", "bucket": "elsewhere", "key": old.key().as_str(),
            "uploadId": old.upload_id(), "initiatedAt": at(old_at - DAY_PLUS * 3)}),
        ),
        (
            "branding_key",
            json!({"mode": "orphan", "bucket": bucket, "key": "branding/logo/0123456789abcdef0123456789abcdef",
            "uploadId": "x", "initiatedAt": at(old_at - DAY_PLUS * 3)}),
        ),
        (
            "garbage_key",
            json!({"mode": "orphan", "bucket": bucket, "key": "../../etc/passwd",
            "uploadId": "x", "initiatedAt": at(old_at - DAY_PLUS * 3)}),
        ),
        (
            "control_upload_id",
            json!({"mode": "orphan", "bucket": bucket, "key": old.key().as_str(),
            "uploadId": "a\nb", "initiatedAt": at(old_at - DAY_PLUS * 3)}),
        ),
        (
            "unknown_field",
            json!({"mode": "orphan", "bucket": bucket, "key": old.key().as_str(),
            "uploadId": old.upload_id(), "initiatedAt": at(old_at), "extra": 1}),
        ),
        ("unknown_mode", json!({"mode": "delete_everything"})),
    ];
    for (name, payload) in cases {
        enqueue_raw(&rig, ABORT, payload, &format!("t:{name}")).await;
    }
    reaper.run_all(ABORT).await;
    assert_eq!(row(&rig, &known).await.state, "in_progress");
    assert!(
        provider_has(&rig, &old).await,
        "no hostile payload reached the provider"
    );
    assert_eq!(rig.open_uploads(&known_row.key).await, 1);
    assert_eq!(
        jobs_of(&rig, ABORT, "dead").await,
        6,
        "malformed work is rejected, not retried"
    );
    assert_eq!(
        jobs_of(&rig, ABORT, "succeeded").await,
        1,
        "the known identity is skipped"
    );

    enqueue_raw(
        &rig,
        ABORT,
        json!({"mode": "orphan", "bucket": bucket, "key": young.key().as_str(),
            "uploadId": young.upload_id(), "initiatedAt": at(young_at)}),
        "t:young",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert!(
        provider_has(&rig, &young).await,
        "a younger upload is never aborted"
    );
    rig.stack.clock.set(young_at + DAY_PLUS);
    enqueue_raw(
        &rig,
        ABORT,
        json!({"mode": "orphan", "bucket": bucket, "key": young.key().as_str(),
            "uploadId": young.upload_id(), "initiatedAt": at(young_at)}),
        "t:old",
    )
    .await;
    reaper.run_all(ABORT).await;
    assert!(!provider_has(&rig, &young).await);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_reconcile_marks_a_vanished_upload_lost_and_fails_the_item() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (session, item, _) = live_upload(&rig, "vanished.bin", size).await;
    let before = row(&rig, &item).await;
    let handle = MultipartHandle::new(
        ObjectKey::parse(&before.key).unwrap(),
        before.upload_id.clone(),
    );
    rig.provider.abort_multipart(&handle).await.unwrap();
    let reaper = Reaper::new(&rig);
    reconcile(&rig, &reaper, "lost").await;

    assert_eq!(row(&rig, &item).await.state, "aborted");
    assert_eq!(
        rig.item_row(&item).await,
        (
            "failed".to_owned(),
            "none".to_owned(),
            Some("MULTIPART_NOT_FOUND".to_owned())
        )
    );
    assert_eq!(
        rig.stack.counts().await.held,
        0,
        "the share is released once"
    );
    assert_eq!(rig.stack.session_row(&session).await.0, "failed");
    let retried = rig.stack.retry_item(&rig.alice, &session, &item).await;
    assert_eq!(retried.status, StatusCode::CONFLICT, "{}", retried.text());
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::NOT_FOUND);
    reconcile(&rig, &reaper, "lost-again").await;
    assert_eq!(rig.stack.counts().await.held, 0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_reconcile_keeps_a_live_upload_the_listing_omits() {
    let rig = S3Rig::new().await;
    set_listing(
        &rig,
        Listing {
            mode: ListMode::ExactOnly,
            ..Listing::default()
        },
    );
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "omitted.bin", size).await;
    let reaper = Reaper::new(&rig);
    reconcile(&rig, &reaper, "omitted").await;
    assert_eq!(row(&rig, &item).await.state, "in_progress");
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    assert_eq!(rig.stack.counts().await.held, i64::try_from(size).unwrap());
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_reconcile_repairs_the_part_cache_without_extending_expiry() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("cache.bin", size).await;
    rig.create(&session, &item).await;
    rig.upload_parts(&session, &item, &bytes, &[1, 2, 3], false)
        .await;
    let before = row(&rig, &item).await;
    let reaper = Reaper::new(&rig);
    rig.stack.clock.advance(HOUR * 5);
    reconcile(&rig, &reaper, "cache").await;

    let verified: i64 = rig
        .stack
        .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts WHERE state = 'verified'")
        .await;
    assert_eq!(verified, 3, "ListParts is the truth");
    assert_eq!(
        row(&rig, &item).await.expires_at,
        before.expires_at,
        "reconciliation is not client activity"
    );
    assert!(
        rig.stack
            .scalar_i64(
                "SELECT COUNT(*) FROM s3_multipart_uploads WHERE last_reconciled_at IS NOT NULL"
            )
            .await
            == 1
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_ttl_sweep_skips_an_upload_with_a_proxied_part_in_flight() {
    let rig = proxied_rig(steady()).await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("proxy.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let key = rig.final_key(&item).await;
    let reaper = Reaper::new(&rig);
    let gate = super::s3_multipart::PartGate::new();
    *rig.faults.part_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let part = bytes::Bytes::copy_from_slice(&bytes[..8 * 1024 * 1024]);

    let upload = rig.put_bytes(&session, &item, "1", &part);
    let sweep = async {
        gate.entered.acquire().await.unwrap().forget();
        rig.stack.clock.advance(DAY + HOUR);
        enqueue_raw(&rig, ABORT, json!({"mode": "sweep"}), "t:proxy").await;
        reaper.run_all(ABORT).await;
        assert_eq!(
            row(&rig, &item).await.state,
            "created",
            "a live proxied part is protected"
        );
        assert!(aborted(&rig).is_empty());
        gate.release.add_permits(1);
    };
    let (stored, ()) = tokio::join!(upload, sweep);
    assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert_eq!(row(&rig, &item).await.state, "in_progress");
    let renewed = instant(&row(&rig, &item).await.expires_at);
    assert!(
        renewed > clock_now(&rig.stack.clock),
        "recording the part renewed the upload"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_backlog_drains_through_continuations() {
    let rig = S3Rig::new().await;
    let total = 230_usize;
    let mut items = Vec::new();
    for index in 0..total {
        let (_, mut ids) = rig
            .session(&[sized("c1", &format!("f{index}.bin"), 6 * MIB)])
            .await;
        items.push(ids.remove(0));
    }
    let mut script = String::new();
    for (index, item) in items.iter().enumerate() {
        let key = rig.final_key(item).await;
        script.push_str(&format!(
            "INSERT INTO s3_multipart_uploads (id, transfer_session_file_id, s3_upload_id, bucket,
                 object_key, owner_user_id, total_size_bytes, part_size_bytes, part_count, state,
                 created_at, updated_at, expires_at)
             VALUES ('{}', '{item}', 'gone-{index}', '{}', '{key}', '{}', {}, 5242880, 2,
                     'abandoned', '2026-09-24T00:00:00.000Z', '2026-09-24T00:00:00.000Z',
                     '2026-09-24T00:00:00.000Z');",
            rig.stack.fresh_id(),
            rig.bucket,
            rig.alice.id,
            6 * MIB
        ));
    }
    rig.stack.execute(&script).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(HOUR);
    reaper.drain().await;

    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads WHERE state = 'aborted'")
            .await,
        i64::try_from(total).unwrap(),
        "230 rows drain through three bounded batches without a manual restart"
    );
    assert_eq!(aborted(&rig).len(), total);
    assert!(jobs_of(&rig, ABORT, "succeeded").await >= 3);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_job_recurrence_and_dedup() {
    let rig = S3Rig::new().await;
    let reaper = Reaper::new(&rig);
    let first = reaper.startup(&rig).await;
    let second = reaper.startup(&rig).await;
    assert_eq!(first, 4, "two chain heads and two startup runs");
    assert_eq!(second, 2, "a second start adds only its own startup runs");
    let heads = |kind: JobKind| {
        let rig = &rig;
        async move {
            rig.stack
                .scalar_i64(&format!(
                    "SELECT COUNT(*) FROM jobs WHERE kind = '{}' AND state = 'pending'
                        AND json_extract(payload_json, '$.mode') = 'sweep'
                        AND dedup_key NOT LIKE '%:startup:%'",
                    kind.as_str()
                ))
                .await
        }
    };
    assert_eq!(heads(ABORT).await, 1);
    assert_eq!(heads(RECONCILE).await, 1);

    reaper.drain().await;
    assert_eq!(heads(ABORT).await, 1, "the run left exactly one successor");
    assert_eq!(heads(RECONCILE).await, 1);
    rig.stack.clock.advance(HOUR);
    reaper.drain().await;
    assert_eq!(heads(ABORT).await, 1, "no duplicate perpetual chain");
    rig.stack.clock.advance(HOUR * 6);
    reaper.drain().await;
    assert_eq!(heads(ABORT).await, 1);
    assert_eq!(heads(RECONCILE).await, 1);
    let next: String = sqlx::query_scalar(
        "SELECT dedup_key FROM jobs WHERE kind = 's3.reconcile_multipart' AND state = 'pending'
            AND dedup_key NOT LIKE '%:startup:%'",
    )
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(next.starts_with("s3.reconcile_multipart:"), "{next}");
}

#[tokio::test]
async fn it_successor_survives_a_failing_sweep() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "fails.bin", size).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    for _ in 0..40 {
        rig.faults
            .abort
            .lock()
            .unwrap()
            .push_back(super::s3_multipart::upstream);
    }
    reaper.drain().await;
    assert_eq!(row(&rig, &item).await.state, "abandoned");
    assert_eq!(
        rig.stack
            .scalar_i64(
                "SELECT COUNT(*) FROM jobs WHERE kind = 's3.abort_abandoned_multipart'
                    AND state = 'pending' AND json_extract(payload_json, '$.mode') = 'sweep'
                    AND dedup_key NOT LIKE '%:startup:%'"
            )
            .await,
        1,
        "the chain survives handler failure"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_shutdown_stops_the_sweep_between_rows() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "stop.bin", size).await;
    let stop = tokio_util::sync::CancellationToken::new();
    let reaper = Reaper::stoppable(&rig, stop.clone());
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    stop.cancel();
    reaper.drain().await;
    assert_eq!(
        row(&rig, &item).await.state,
        "in_progress",
        "no new work after the stop"
    );
    assert!(aborted(&rig).is_empty());
}

#[tokio::test]
async fn it_startup_issues_no_abort_for_live_multipart() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("live.bin", size).await;
    rig.create(&session, &item).await;
    rig.upload_parts(&session, &item, &bytes, &[1, 2], true)
        .await;
    let before = row(&rig, &item).await;
    let key = rig.final_key(&item).await;

    let reaper = Reaper::new(&rig);
    let report = crate::app::reconcile::ReconcileRegistry::production()
        .run(&crate::app::reconcile::ReconcileContext::new(
            rig.stack.pools.clone(),
            Arc::new(rig.stack.clock.clone()),
            crate::infra::db::InstanceId::generate(&rig.stack.clock),
        ))
        .await;
    assert_eq!(report.failed(), 0, "{report:?}");
    assert_eq!(
        rig.faults.aborts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "startup itself aborts nothing"
    );
    reaper.drain().await;

    let after = row(&rig, &item).await;
    assert_eq!(after.state, "in_progress");
    assert_eq!(
        (after.upload_id, after.key, after.expires_at),
        (before.upload_id, before.key, before.expires_at),
        "identity and expiry are untouched"
    );
    let own: Vec<_> = aborted(&rig)
        .into_iter()
        .filter(|(aborted_key, _)| *aborted_key == key)
        .collect();
    assert!(
        own.is_empty(),
        "no AbortMultipartUpload for the live upload"
    );
    assert_eq!(rig.open_uploads(&key).await, 1);
    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.json()["completedParts"].as_array().unwrap().len(), 2);
    assert_eq!(listed.json()["missingPartNumbers"], json!([3]));
    rig.assert_no_overlap();
}

#[tokio::test]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R083_restart_preserves_multipart_state() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("restart.bin", size).await;
    rig.create(&session, &item).await;
    rig.upload_parts(&session, &item, &bytes, &[1], true).await;
    let key = rig.final_key(&item).await;
    let before = row(&rig, &item).await;

    for round in 0..2 {
        let reaper = Reaper::new(&rig);
        reaper.startup(&rig).await;
        rig.stack.clock.advance(HOUR * 3);
        reaper.drain().await;
        assert_eq!(rig.open_uploads(&key).await, 1, "restart {round}");
        let listed = rig.parts(&session, &item).await;
        assert_eq!(
            listed.json()["completedParts"].as_array().unwrap().len(),
            1,
            "the part uploaded before the restart is still there"
        );
    }
    assert_eq!(
        aborted(&rig)
            .iter()
            .filter(|(aborted_key, _)| *aborted_key == key)
            .count(),
        0
    );
    rig.upload_parts(&session, &item, &bytes, &[2, 3], true)
        .await;
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(row(&rig, &item).await.upload_id, before.upload_id);
    assert_eq!(
        rig.read_object(&key).await,
        (size, Sha256::digest(&bytes).to_vec())
    );
    rig.stack.flush_audit().await;
    assert_eq!(rig.uploads_audited().await, 1);
    rig.assert_no_overlap();
}

#[tokio::test]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R083_restart_preserves_proxied_multipart_state() {
    let mut rig = proxied_rig(steady()).await;
    let size = 20 * MIB + 9;
    let bytes = data(size);
    let (session, item) = rig.one("proxied.bin", size).await;
    rig.create(&session, &item).await;
    let first = bytes::Bytes::copy_from_slice(&bytes[..8 * 1024 * 1024]);
    let stored = rig.put_bytes(&session, &item, "1", &first).await;
    assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
    let key = rig.final_key(&item).await;

    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(HOUR * 5);
    reaper.drain().await;
    assert_eq!(rig.open_uploads(&key).await, 1);
    assert!(aborted(&rig)
        .iter()
        .all(|(aborted_key, _)| *aborted_key != key));
    let listed = rig.parts(&session, &item).await;
    assert_eq!(listed.json()["completedParts"].as_array().unwrap().len(), 1);

    for (number, range) in [
        ("2", 8 * 1024 * 1024..16 * 1024 * 1024),
        ("3", 16 * 1024 * 1024..bytes.len()),
    ] {
        let part = bytes::Bytes::copy_from_slice(&bytes[range]);
        let stored = rig.put_bytes(&session, &item, number, &part).await;
        assert_eq!(stored.status, StatusCode::OK, "{}", stored.text());
    }
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(
        rig.read_object(&key).await,
        (size, Sha256::digest(&bytes).to_vec())
    );
    rig.stack.flush_audit().await;
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_repeated_job_delivery_is_idempotent() {
    let mut rig = S3Rig::new().await;
    let size = 20 * MIB + 9;
    let (_, item, _) = live_upload(&rig, "twice.bin", size).await;
    let reaper = Reaper::new(&rig);
    reaper.startup(&rig).await;
    rig.stack.clock.advance(DAY);
    reaper.drain().await;
    assert_eq!(aborted(&rig).len(), 1);
    let id = multipart_id(&rig, &item).await;
    for tag in ["a", "b", "c"] {
        enqueue_raw(
            &rig,
            ABORT,
            json!({"mode": "multipart", "multipartId": id}),
            &format!("t:{tag}"),
        )
        .await;
    }
    assert_eq!(reaper.run_all(ABORT).await, 3);
    assert_eq!(
        aborted(&rig).len(),
        1,
        "a terminal row is never aborted again"
    );
    rig.stack.flush_audit().await;
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'TRANSFER_EXPIRED'")
            .await,
        1
    );
    assert_eq!(GRACE.as_secs(), 120);
    let _ = (Point::ExpireDiscovered, &ABORT);
}
