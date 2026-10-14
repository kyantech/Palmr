use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use super::finalize::Rig;
use super::folders::{Member, HOST_A, HOST_B};
use super::transfer_reaper::{context, job_count, pass, start_upload, tus_marker, Jobs, DAY, HOUR};
use super::tus::{
    assert_head_error, assert_tus_error, channel_body, pattern, wait_for_offset, Fault, TusReq, TUS,
};
use super::tus_patch::{framed_patch, patch};
use super::*;
use crate::domain::clock::Clock;
use crate::features::transfers::finalize::Point;
use crate::features::transfers::model::SESSION_TTL;
use crate::features::transfers::reaper::{self, ReaperContext, UploadOutcome};
use crate::infra::jobs::JobKind;

fn head<'a>(member: &'a Member, id: &str) -> TusReq<'a> {
    TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), member)
}

#[tokio::test]
async fn it_expired_upload_is_gone_and_a_foreign_resource_stays_not_found() {
    let rig = Rig::manual().await;
    let bob = rig.stack.member("bob", HOST_B).await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    let id = &upload.id;
    assert_eq!(
        rig.stack.tus_send(head(&rig.alice, id)).await.status,
        StatusCode::OK
    );

    rig.stack.clock.advance(DAY);
    let before_reaper = rig.stack.tus_send(head(&rig.alice, id)).await;
    assert_head_error(&before_reaper, StatusCode::GONE);

    assert_eq!(pass(&rig).await.uploads_expired, 1);
    let gone = rig.stack.tus_send(head(&rig.alice, id)).await;
    assert_head_error(&gone, StatusCode::GONE);
    let patched = rig
        .stack
        .tus_send(patch(&rig.alice, id, 100).bytes(&pattern(10)))
        .await;
    assert_eq!(patched.status, StatusCode::GONE, "{}", patched.text());
    let deleted = rig
        .stack
        .tus_send(TusReq::new(
            Method::DELETE,
            &format!("{TUS}/{id}"),
            &rig.alice,
        ))
        .await;
    assert_eq!(deleted.status, StatusCode::GONE, "{}", deleted.text());

    let foreign = rig.stack.tus_send(head(&bob, id)).await;
    assert_head_error(&foreign, StatusCode::NOT_FOUND);
    let foreign_patch = rig
        .stack
        .tus_send(patch(&bob, id, 100).bytes(&pattern(10)))
        .await;
    assert_tus_error(&foreign_patch, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(
        rig.stack
            .reservation(&upload.planned.session)
            .await
            .3
            .as_deref(),
        Some("expired")
    );
    assert_eq!(rig.stack.used_bytes(bob.id).await, 0);
}

#[tokio::test]
async fn it_patch_renewal_after_discovery_is_not_expired() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 1_000, 100).await;
    let id = upload.id.as_str();
    let reaper_context = context(&rig);

    rig.stack.clock.advance(DAY - Duration::from_secs(60));
    let renewed = rig
        .stack
        .tus_send(patch(&rig.alice, &upload.id, 100).bytes(&pattern(100)))
        .await;
    assert_eq!(renewed.status, StatusCode::NO_CONTENT, "{}", renewed.text());
    rig.stack.clock.advance(Duration::from_secs(120));

    let outcome = reaper::expire_upload(&reaper_context, id).await.unwrap();
    assert_eq!(
        outcome,
        UploadOutcome::NotDue,
        "the renewal after discovery wins under the write transaction"
    );
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "in_progress");
    assert_eq!(
        rig.stack.item_state(&upload.planned.item).await,
        "uploading"
    );
    assert_eq!(rig.stack.counts().await.held, 1_000);
    assert!(rig.stack.staged_bytes(&upload.id).is_some());
    assert_eq!(
        super::tus_patch::head_offset(&rig.stack, &rig.alice, &upload.id).await,
        200
    );

    let active = pass(&rig).await;
    assert_eq!(active.uploads_expired, 0);
    assert_eq!(active.staging_removed, 0);
}

#[tokio::test]
async fn it_leased_upload_is_never_expired_under_its_writer() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 30_000, 100).await;
    let reaper_context = context(&rig);
    let id = upload.id.as_str();
    let (sender, body) = channel_body();

    let writer = rig
        .stack
        .tus_send(framed_patch(&rig.alice, &upload.id, 100, body));
    let driver = async {
        sender
            .send(Ok(Bytes::copy_from_slice(&pattern(2_000))))
            .unwrap();
        wait_for_offset(&rig.stack, &upload.id, 100).await;
        loop {
            if rig.stack.tus_row(&upload.id).await.locked_by.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        rig.stack
            .execute(&format!(
                "UPDATE tus_uploads SET expires_at = '2026-09-25T12:00:01.000Z' WHERE id = '{}'",
                upload.id
            ))
            .await;
        rig.stack.clock.advance(Duration::from_secs(5));
        let leased = reaper::expire_upload(&reaper_context, id).await.unwrap();
        assert_eq!(
            leased,
            UploadOutcome::Leased,
            "an active PATCH owns the upload"
        );
        assert_eq!(pass(&rig).await.uploads_expired, 0);
        sender
            .send(Ok(Bytes::copy_from_slice(&pattern(1_000))))
            .unwrap();
        drop(sender);
    };
    let (response, ()) = futures_util::future::join(writer, driver).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "{}",
        response.text()
    );
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "in_progress");
}

#[tokio::test]
async fn it_expired_upload_with_a_dead_lease_is_reaped_but_a_live_lease_defers_cleanup() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 1_000, 100).await;
    rig.stack.clock.advance(DAY);
    rig.stack
        .execute(&format!(
            "UPDATE tus_uploads SET locked_by = 'dead-instance:patch', lock_expires_at = '2026-09-25T12:00:30.000Z' WHERE id = '{}'",
            upload.id
        ))
        .await;
    let report = pass(&rig).await;
    assert_eq!(
        report.uploads_expired, 1,
        "a lapsed lease protects nothing: {report:?}"
    );
    assert_eq!(report.staging_removed, 1);

    let second = start_upload(&rig, "b.bin", 1_000, 100).await;
    rig.stack
        .execute(&format!(
            "UPDATE tus_uploads SET state = 'terminated', locked_by = 'writer:patch', lock_expires_at = '{}' WHERE id = '{}'",
            crate::domain::time::Timestamp::try_from(rig.stack.clock.now() + HOUR).unwrap(),
            second.id
        ))
        .await;
    let deferred = pass(&rig).await;
    assert_eq!(
        deferred.staging_removed, 0,
        "a live writer's staging is left alone"
    );
    assert!(rig.stack.staged_bytes(&second.id).is_some());
    rig.stack.clock.advance(2 * HOUR);
    let later = pass(&rig).await;
    assert_eq!(later.staging_removed, 1, "{later:?}");
    assert!(rig.stack.staged_bytes(&second.id).is_none());
}

#[tokio::test]
async fn it_finalizer_wins_the_race_and_the_reaper_cannot_delete_final_bytes() {
    let rig = Rig::new().await;
    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    let data = pattern(300);
    let pushed = rig.push("won.bin", &data).await;
    gate.wait_until_entered().await;
    let item = pushed.planned.item.clone();
    let id = pushed.upload.as_str();
    let reaper_context = context(&rig);

    rig.stack.clock.advance(DAY + HOUR);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let outcome = reaper::expire_upload(&reaper_context, id).await.unwrap();
    assert!(
        matches!(
            outcome,
            UploadOutcome::Leased | UploadOutcome::AwaitingFinalization
        ),
        "{outcome:?}"
    );
    assert_eq!(pass(&rig).await.uploads_expired, 0);

    gate.open();
    rig.settle().await;
    let files = rig.file_rows().await;
    assert_eq!(files.len(), 1, "the file is visible");
    assert_eq!(rig.object_state(&item).await.unwrap().0, "active");
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 300);
    let (state, _, committed, _) = rig.stack.reservation(&pushed.planned.session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(300)));
    assert_eq!(rig.audit_uploads().await, 1);

    assert_eq!(
        reaper::expire_upload(&reaper_context, id).await.unwrap(),
        UploadOutcome::Gone,
        "a stale candidate of a completed upload does nothing"
    );
    rig.stack.clock.advance(SESSION_TTL);
    let later = pass(&rig).await;
    assert_eq!(later.errors, 0, "{later:?}");
    assert!(
        rig.final_path(&item).await.exists(),
        "final bytes are never deleted"
    );
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(rig.object_state(&item).await.unwrap().0, "active");
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 300);
    assert!(rig.stack.staged_bytes(&pushed.upload).is_none());
    assert_eq!(
        rig.stack.session_row(&pushed.planned.session).await.0,
        "completed"
    );
}

#[tokio::test]
async fn it_expiry_wins_before_the_intent_and_the_finalizer_cannot_commit() {
    let rig = Rig::new().await;
    let gate = rig.hooks.gate_at(Point::StagingSynced);
    let pushed = rig.push("early.bin", &pattern(200)).await;
    gate.wait_until_entered().await;
    let item = pushed.planned.item.clone();

    rig.stack
        .clock
        .advance(SESSION_TTL + Duration::from_secs(1));
    let report = pass(&rig).await;
    assert_eq!(report.sessions_expired, 1, "{report:?}");
    assert_eq!(rig.item_row(&item).await.0, "expired");

    gate.open();
    rig.settle().await;
    assert_eq!(rig.item_row(&item).await.0, "expired", "no resurrection");
    assert!(rig.file_rows().await.is_empty());
    assert!(
        rig.object_state(&item).await.is_none(),
        "no object was ever placed"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        0
    );
    let (state, _, _, reason) = rig.stack.reservation(&pushed.planned.session).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("released", Some("expired"))
    );
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 0);
    assert_eq!(job_count(&rig, JobKind::StorageDeleteBlob).await, 0);
}

#[tokio::test]
async fn it_complete_but_unclaimed_upload_is_not_reaped_and_is_redriven() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "full.bin", 400, 400).await;
    assert_eq!(
        rig.stack.item_state(&upload.planned.item).await,
        "uploading"
    );

    rig.stack.clock.advance(DAY + HOUR);
    let report = pass(&rig).await;
    assert_eq!(report.uploads_expired, 0, "{report:?}");
    assert_eq!(report.uploads_skipped, 1);
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "in_progress");
    assert!(rig.stack.staged_bytes(&upload.id).is_some());
    assert_eq!(rig.stack.counts().await.held, 400);

    let finalizer = rig.restart().await;
    finalizer.idle().await;
    assert_eq!(
        rig.file_rows().await.len(),
        1,
        "the delayed finalization still commits"
    );
    assert_eq!(rig.stack.used_bytes(rig.alice.id).await, 400);
    let settled = pass(&rig).await;
    assert_eq!(settled.errors, 0);
    assert_eq!(rig.file_rows().await.len(), 1);
}

#[tokio::test]
async fn it_unclaimed_complete_upload_still_ends_with_its_session() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "full.bin", 400, 400).await;
    rig.stack.clock.advance(SESSION_TTL);
    let report = pass(&rig).await;
    assert_eq!(report.sessions_expired, 1, "{report:?}");
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "expired");
    assert!(rig.file_rows().await.is_empty());
}

#[tokio::test]
async fn it_staging_removal_failures_stay_retryable_and_are_never_marked_early() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    rig.stack.clock.advance(DAY);

    rig.staging.fail_remove.store(true, Ordering::SeqCst);
    let denied = pass(&rig).await;
    assert_eq!(denied.uploads_expired, 1);
    assert_eq!(denied.staging_failed, 1, "{denied:?}");
    assert_eq!(denied.errors, 0);
    let (state, removed) = tus_marker(&rig, &upload.id).await;
    assert_eq!((state.as_str(), removed), ("expired", None));
    assert!(rig.stack.staged_bytes(&upload.id).is_some());
    assert_eq!(
        rig.stack.counts().await.held,
        0,
        "quota is released before any byte is removed"
    );
    rig.staging.fail_remove.store(false, Ordering::SeqCst);

    *rig.staging.remove_fault.lock().unwrap() = Some((1, Fault::Io));
    let io = pass(&rig).await;
    assert_eq!(io.staging_failed, 1, "{io:?}");
    assert!(tus_marker(&rig, &upload.id).await.1.is_none());

    let directory = rig.root.join("uploads").join(upload.id.replace('-', ""));
    std::fs::write(directory.join("stray.tmp"), b"left behind").unwrap();
    let partial = pass(&rig).await;
    assert_eq!(
        partial.staging_failed, 1,
        "a directory that cannot be emptied: {partial:?}"
    );
    assert!(tus_marker(&rig, &upload.id).await.1.is_none());
    assert!(
        !directory.join("blob").exists(),
        "the blob was already unlinked"
    );

    std::fs::remove_file(directory.join("stray.tmp")).unwrap();
    let recovered = pass(&rig).await;
    assert_eq!(
        (recovered.staging_removed, recovered.staging_failed),
        (1, 0)
    );
    assert!(tus_marker(&rig, &upload.id).await.1.is_some());
    assert!(!directory.exists());
    assert_eq!(pass(&rig).await, reaper::ReapReport::default());
}

#[tokio::test]
async fn it_missing_staging_pieces_count_as_removed() {
    let rig = Rig::manual().await;
    let blobless = start_upload(&rig, "a.bin", 500, 100).await;
    let hintless = start_upload(&rig, "b.bin", 500, 100).await;
    let dirless = start_upload(&rig, "c.bin", 500, 100).await;
    let root = rig.root.join("uploads");
    std::fs::remove_file(root.join(blobless.id.replace('-', "")).join("blob")).unwrap();
    std::fs::remove_file(root.join(hintless.id.replace('-', "")).join("meta.json")).unwrap();
    std::fs::remove_dir_all(root.join(dirless.id.replace('-', ""))).unwrap();
    rig.stack.clock.advance(DAY);

    let report = pass(&rig).await;
    assert_eq!(report.uploads_expired, 3, "{report:?}");
    assert_eq!((report.staging_removed, report.staging_failed), (3, 0));
    for upload in [&blobless, &hintless, &dirless] {
        assert!(tus_marker(&rig, &upload.id).await.1.is_some());
    }
    assert!(rig.stack.staging_dirs().is_empty());
}

#[tokio::test]
async fn it_restart_after_the_terminal_commit_resumes_staging_cleanup() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let staging = super::tus::TestStaging::new(root.path(), &clock);
    let setup = || TusSetup {
        staging: Some(staging.clone()),
        limits: super::tus::small_limits(),
        ..TusSetup::default()
    };
    let stack = Stack::start_with_tus(root.path(), &clock, setup()).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(500)).await;
    let id = stack.tus_created(&alice, &planned, Some(500)).await;
    let sent = stack
        .tus_send(patch(&alice, &id, 0).bytes(&pattern(100)))
        .await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT);

    let canceled = stack.cancel_session(&alice, &planned.session).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.tus_row(&id).await.state, "terminated");
    assert!(
        stack.staged_bytes(&id).is_some(),
        "the crash window: terminal row, bytes still staged"
    );
    stack.stop().await;

    let restarted = Stack::start_with_tus(root.path(), &clock, setup()).await;
    assert!(restarted.staged_bytes(&id).is_some());
    let reaper_context = ReaperContext::new(
        restarted.pools.clone(),
        Arc::new(clock.clone()),
        restarted.settings.handle(),
        restarted.audit.clone(),
        Some(staging.clone()),
        CancellationToken::new(),
    );
    let report = reaper::run_pass(&reaper_context).await.unwrap();
    assert_eq!(
        (report.staging_removed, report.errors),
        (1, 0),
        "{report:?}"
    );
    assert!(restarted.staged_bytes(&id).is_none());
    assert!(restarted.staging_dirs().is_empty());
    let marked: Option<String> =
        sqlx::query_scalar("SELECT staging_removed_at FROM tus_uploads WHERE id = ?1")
            .bind(&id)
            .fetch_one(restarted.pools.reader().executor())
            .await
            .unwrap();
    assert!(marked.is_some());
    restarted.stop().await;
}

#[tokio::test]
async fn it_completed_upload_staging_is_swept_after_a_failed_post_commit_cleanup() {
    let rig = Rig::new().await;
    rig.staging.fail_remove.store(true, Ordering::SeqCst);
    let pushed = rig.push_and_settle("done.bin", &pattern(200)).await;
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(
        tus_marker(&rig, &pushed.upload).await,
        ("completed".to_owned(), None)
    );
    assert_eq!(
        rig.stack.staging_dirs().len(),
        1,
        "the directory outlived the failed cleanup"
    );

    rig.staging.fail_remove.store(false, Ordering::SeqCst);
    let report = pass(&rig).await;
    assert_eq!(
        (report.staging_removed, report.staging_failed),
        (1, 0),
        "{report:?}"
    );
    assert!(tus_marker(&rig, &pushed.upload).await.1.is_some());
    assert!(rig.stack.staging_dirs().is_empty());
    assert_eq!(
        rig.file_rows().await.len(),
        1,
        "the finalized file is untouched"
    );
}

#[tokio::test]
async fn it_staging_removal_stays_inside_the_canonical_upload_directory() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "escape.bin", 500, 100).await;
    let outside = rig.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("precious.txt"), b"do not delete").unwrap();
    let decoy = rig.root.join("uploads").join("not-an-upload-id");
    std::fs::create_dir_all(&decoy).unwrap();
    std::fs::write(decoy.join("keep.txt"), b"keep").unwrap();

    let linked = start_upload(&rig, "linked.bin", 500, 100).await;
    let linked_dir = rig.root.join("uploads").join(linked.id.replace('-', ""));
    std::fs::remove_dir_all(&linked_dir).unwrap();
    std::os::unix::fs::symlink(&outside, &linked_dir).unwrap();

    rig.stack.clock.advance(DAY);
    let report = pass(&rig).await;
    assert_eq!(report.uploads_expired, 2, "{report:?}");
    assert!(tus_marker(&rig, &upload.id).await.1.is_some());
    assert!(
        tus_marker(&rig, &linked.id).await.1.is_none(),
        "a symlinked staging directory is never followed nor marked clean"
    );
    assert!(outside.join("precious.txt").exists());
    assert!(decoy.join("keep.txt").exists());
    assert!(!rig
        .root
        .join("uploads")
        .join(upload.id.replace('-', ""))
        .exists());
}

#[tokio::test]
async fn it_shutdown_leaves_durable_state_and_the_next_pass_resumes() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    rig.stack.clock.advance(DAY);

    let stop = CancellationToken::new();
    stop.cancel();
    let staging: Arc<dyn crate::storage::staging::StagingStorage> = rig.staging.clone();
    let stopped = ReaperContext::new(
        rig.stack.pools.clone(),
        Arc::new(rig.stack.clock.clone()),
        rig.stack.settings.handle(),
        rig.stack.audit.clone(),
        Some(staging),
        stop,
    );
    let idle = reaper::run_pass(&stopped).await.unwrap();
    assert_eq!(idle, reaper::ReapReport::default());
    assert_eq!(tus_marker(&rig, &upload.id).await.0, "in_progress");

    let resumed = pass(&rig).await;
    assert_eq!((resumed.uploads_expired, resumed.staging_removed), (1, 1));
}

#[tokio::test]
async fn it_expire_job_chain_survives_a_failed_run_and_never_duplicates() {
    let rig = Rig::manual().await;
    start_upload(&rig, "a.bin", 500, 100).await;
    rig.stack.clock.advance(DAY);
    let jobs = Jobs::new(&rig);
    reaper::ensure_scheduled(&rig.stack.pools, &rig.stack.clock)
        .await
        .unwrap();

    rig.staging.fail_remove.store(true, Ordering::SeqCst);
    let first = jobs.run_all(JobKind::TusExpireStale).await;
    assert!(first >= 1);
    let pending: i64 = rig
        .stack
        .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'tus.expire_stale' AND state = 'pending' AND run_at > '2026-09-26T12:00:00.000Z'")
        .await;
    assert_eq!(
        pending, 1,
        "the successor exists even though cleanup is still failing"
    );
    reaper::ensure_scheduled(&rig.stack.pools, &rig.stack.clock)
        .await
        .unwrap();
    assert_eq!(
        rig.stack
            .scalar_i64(
                "SELECT COUNT(*) FROM jobs WHERE kind = 'tus.expire_stale' AND state = 'pending'"
            )
            .await,
        1,
        "a restart finds the live chain and adds nothing"
    );
}
