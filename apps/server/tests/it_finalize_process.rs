#![cfg(feature = "e2e-fixture")]

pub mod support;

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Method, StatusCode};
use tempfile::TempDir;

use support::client::Http;
use support::tus::{generated_digest, send_range, wait_until, TusWorld, Upload, MIB};
use support::ServerProcess;

async fn world(
    process: &ServerProcess,
    data_dir: &Path,
    previous: Option<&TusWorld>,
) -> Result<TusWorld> {
    let base = url::Url::parse(&format!("http://127.0.0.1:{}/", process.port()))?;
    let http = Http::new(base.clone())?;
    let creds = match previous {
        Some(previous) => previous.creds.clone(),
        None => http.setup_admin().await?,
    };
    Ok(TusWorld::new(base, data_dir.to_path_buf(), http, creds))
}

fn marks(marker: &Path) -> usize {
    support::read_text(marker).map_or(0, |text| text.lines().count())
}

async fn paused_process(
    root: &TempDir,
    data_dir: &Path,
    point: &str,
) -> Result<(ServerProcess, TusWorld, PathBuf)> {
    let marker = root.path().join("paused.marker");
    let mut process = ServerProcess::spawn_with(
        data_dir,
        &[
            ("PALMR_E2E_FINALIZE_PAUSE_AT", point.as_ref()),
            ("PALMR_E2E_FINALIZE_PAUSE_FILE", marker.as_os_str()),
        ],
    )?;
    process.wait_ready().await?;
    let first = world(&process, data_dir, None).await?;
    Ok((process, first, marker))
}

async fn stream_patch(world: &TusWorld, upload: &Upload, size: u64) -> Result<()> {
    let mut patch = world.open(upload, 0, None).await?;
    send_range(&mut patch, size, 0, size).await?;
    patch.finish().await?;
    let response = patch.response().await?;
    assert_eq!(response.status, 204, "{}", response.body);
    assert_eq!(response.headers["upload-offset"], size.to_string());
    Ok(())
}

async fn wait_marks(marker: &Path, expected: usize, point: &str) -> Result<()> {
    wait_until("the finalizer to pause", || async {
        Ok(marks(marker) >= expected)
    })
    .await?;
    for line in support::read_text(marker)?.lines() {
        assert_eq!(line, point);
    }
    Ok(())
}

async fn counts(world: &TusWorld) -> Result<(i64, i64, i64, i64)> {
    Ok((
        world.db.scalar_i64("SELECT COUNT(*) FROM files").await?,
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await?,
        world
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
            .await?,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_finalize_kill_before_commit_recovers_exactly_once() -> Result<()> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 20 * MIB + 321;
    let expected = generated_digest(size)?;

    let (process, first, marker) = paused_process(&root, &data_dir, "before_commit").await?;
    let upload = first.upload("crash.bin", size).await?;
    stream_patch(&first, &upload, size).await?;
    wait_marks(&marker, 1, "before_commit").await?;

    assert_eq!(first.item_state(&upload).await?, "finalizing");
    assert_eq!(
        first
            .db
            .scalar_string("SELECT finalize_stage FROM transfer_session_files")
            .await?,
        "placing"
    );
    let key = first.final_path(&upload).await?;
    assert!(key.exists(), "the blob is placed at its recorded key");
    let placed = std::fs::metadata(&key)?;
    let before = (
        placed.dev(),
        placed.ino(),
        placed.len(),
        placed.mtime_nsec(),
    );
    assert_eq!(first.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        0
    );
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT SUM(used_bytes) FROM users")
            .await?,
        0
    );
    let identity = first
        .db
        .scalar_string(
            "SELECT final_object_id || '|' || final_object_key FROM transfer_session_files",
        )
        .await?;

    let status = process.kill_hard()?;
    assert_eq!(status.signal(), Some(9), "the process died of SIGKILL");

    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    second.wait_finalized(&upload).await?;

    assert_eq!(counts(&second).await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(
        second
            .db
            .scalar_string(
                "SELECT final_object_id || '|' || final_object_key FROM transfer_session_files"
            )
            .await?,
        identity,
        "the recorded identity is reused"
    );
    let after = std::fs::metadata(&key)?;
    assert_eq!(
        (after.dev(), after.ino(), after.len(), after.mtime_nsec()),
        before,
        "the placed object was not replaced or rewritten"
    );
    assert_eq!(second.blob_digest(&upload).await?, (size, expected.clone()));
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state || ':' || committed_bytes FROM quota_reservations")
            .await?,
        format!("committed:{size}")
    );
    wait_until("the staging directory to be removed", || async {
        Ok(!data_dir
            .join("uploads")
            .join(upload.id.replace('-', ""))
            .exists())
    })
    .await
    .context("no indefinite orphan")?;
    let file_id = second.db.scalar_string("SELECT id FROM files").await?;
    second
        .http
        .send(
            Method::GET,
            &format!("/api/v1/files/{file_id}"),
            Some(&second.creds),
            None,
        )
        .await?
        .expect(StatusCode::OK)?;

    let exit = restarted.stop().await?;
    assert!(exit.success());
    let mut again = ServerProcess::spawn(&data_dir)?;
    again.wait_ready().await?;
    let third = world(&again, &data_dir, Some(&first)).await?;
    assert_eq!(counts(&third).await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(
        third.db.scalar_string("SELECT id FROM files").await?,
        file_id
    );
    again.stop().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_finalize_cancel_then_kill_leaves_a_tombstone_the_jobs_erase() -> Result<()>
{
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 6 * MIB + 5;

    let (process, first, marker) = paused_process(&root, &data_dir, "before_commit").await?;
    let upload = first.upload("crash.bin", size).await?;
    stream_patch(&first, &upload, size).await?;
    wait_marks(&marker, 1, "before_commit").await?;
    let session = first
        .db
        .scalar_string("SELECT transfer_session_id FROM transfer_session_files")
        .await?;
    let key = first.final_path(&upload).await?;
    assert!(key.exists());
    first
        .http
        .send(
            Method::DELETE,
            &format!("/api/v1/transfers/sessions/{session}"),
            Some(&first.creds),
            None,
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    assert_eq!(
        first
            .db
            .scalar_string("SELECT state FROM storage_objects")
            .await?,
        "tombstoned"
    );
    assert!(key.exists(), "the bytes wait for the deletion lifecycle");
    process.kill_hard()?;

    let past = "2020-01-01T00:00:00.000Z";
    first
        .db
        .execute(&format!(
            "UPDATE file_deletion_queue SET not_before = '{past}';
             UPDATE jobs SET run_at = '{past}' WHERE kind = 'storage.delete_blob'"
        ))
        .await?;

    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    wait_until("the delete job to remove the placed blob", || async {
        Ok(!key.exists())
    })
    .await
    .context("eventual physical cleanup")?;
    wait_until("the object to be recorded deleted", || async {
        Ok(second
            .db
            .scalar_string("SELECT state FROM storage_objects")
            .await?
            == "deleted")
    })
    .await?;
    assert_eq!(second.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT SUM(used_bytes) FROM users")
            .await?,
        0
    );
    assert_eq!(second.item_state(&upload).await?, "canceled");
    restarted.stop().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_finalize_crash_before_detached_task_claim() -> Result<()> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 18 * MIB + 77;
    let expected = generated_digest(size)?;

    let (process, first, marker) = paused_process(&root, &data_dir, "dispatched").await?;
    let upload = first.upload("unclaimed.bin", size).await?;
    stream_patch(&first, &upload, size).await?;
    wait_marks(&marker, 1, "dispatched").await?;

    assert_eq!(
        first.offset(&upload).await?,
        size,
        "every byte was acknowledged"
    );
    assert_eq!(
        first
            .db
            .scalar_string("SELECT state FROM tus_uploads")
            .await?,
        "in_progress"
    );
    assert_eq!(first.item_state(&upload).await?, "uploading");
    assert_eq!(
        first
            .db
            .scalar_string("SELECT finalize_stage FROM transfer_session_files")
            .await?,
        "none",
        "the finalization claim was never taken"
    );
    assert_eq!(first.blob_digest(&upload).await?, (size, expected.clone()));
    assert_eq!(counts(&first).await?, (0, 0, 0, 0));

    let status = process.kill_hard()?;
    assert_eq!(status.signal(), Some(9));

    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    second.wait_finalized(&upload).await?;

    assert_eq!(counts(&second).await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(second.blob_digest(&upload).await?, (size, expected));
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state || ':' || committed_bytes FROM quota_reservations")
            .await?,
        format!("committed:{size}"),
        "the reservation settled once"
    );
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM quota_reservations")
            .await?,
        1
    );
    let file_id = second.db.scalar_string("SELECT id FROM files").await?;
    assert!(restarted.stop().await?.success());

    let mut again = ServerProcess::spawn(&data_dir)?;
    again.wait_ready().await?;
    let third = world(&again, &data_dir, Some(&first)).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(counts(&third).await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(
        third.db.scalar_string("SELECT id FROM files").await?,
        file_id
    );
    again.stop().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_finalize_crash_before_claim_recovers_every_dispatch_source() -> Result<()> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let (patched_size, body_size) = (7 * MIB + 11, 5 * MIB + 3);

    let (process, first, marker) = paused_process(&root, &data_dir, "dispatched").await?;
    let patched = first.upload("patched.bin", patched_size).await?;
    stream_patch(&first, &patched, patched_size).await?;
    let with_body = first.create_with_body("with-body.bin", body_size).await?;
    let empty = first.upload("empty.bin", 0).await?;
    wait_marks(&marker, 3, "dispatched").await?;
    assert_eq!(counts(&first).await?, (0, 0, 0, 0), "nothing was finalized");

    process.kill_hard()?;
    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    for upload in [&patched, &with_body, &empty] {
        second.wait_finalized(upload).await?;
    }
    let total = i64::try_from(patched_size + body_size)?;
    assert_eq!(counts(&second).await?, (3, 3, total, 3));
    assert_eq!(
        second.blob_digest(&patched).await?,
        (patched_size, generated_digest(patched_size)?)
    );
    assert_eq!(
        second.blob_digest(&with_body).await?,
        (body_size, generated_digest(body_size)?)
    );
    assert_eq!(second.blob_digest(&empty).await?.0, 0);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM quota_reservations WHERE state = 'committed'")
            .await?,
        3
    );
    assert!(restarted.stop().await?.success());
    let mut again = ServerProcess::spawn(&data_dir)?;
    again.wait_ready().await?;
    let third = world(&again, &data_dir, Some(&first)).await?;
    assert_eq!(counts(&third).await?, (3, 3, total, 3));
    again.stop().await?;
    Ok(())
}
