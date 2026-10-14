pub mod support;

use std::os::unix::process::ExitStatusExt;

use anyhow::{ensure, Result};
use tempfile::TempDir;

use support::client::Http;
use support::tus::{digest_header, generated_digest, send_range, wait_until, TusWorld, MIB};
use support::ServerProcess;

async fn world(
    process: &ServerProcess,
    data_dir: &std::path::Path,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_tus_real_process_crash_recovers_without_a_lease_wait() -> Result<()> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 24 * MIB;
    let expected = generated_digest(size)?;
    let header = digest_header(&expected);

    let mut process = ServerProcess::spawn(&data_dir)?;
    process.wait_ready().await?;
    let first = world(&process, &data_dir, None).await?;
    let unchecked = first.upload("unchecked.bin", size).await?;
    let checked = first.upload("checked.bin", size).await?;
    let mut plain = first.open(&unchecked, 0, None).await?;
    let mut verified = first.open(&checked, 0, Some(&header)).await?;
    send_range(&mut plain, size, 0, 12 * MIB).await?;
    send_range(&mut verified, size, 0, 12 * MIB).await?;
    wait_until("a durable checkpoint of the unchecked PATCH", || async {
        Ok(first.offset(&unchecked).await? >= 8 * MIB)
    })
    .await?;
    wait_until(
        "unverified bytes of the checksummed PATCH on disk",
        || async { Ok(std::fs::metadata(first.blob(&checked))?.len() >= 8 * MIB) },
    )
    .await?;
    let durable = first.offset(&unchecked).await?;
    assert_eq!(first.offset(&checked).await?, 0);
    assert!(first.locked(&unchecked).await? && first.locked(&checked).await?);

    let status = process.kill_hard()?;
    assert_eq!(
        status.signal(),
        Some(9),
        "the process died of SIGKILL, not a graceful exit"
    );
    drop((plain, verified));
    assert!(
        first.locked(&unchecked).await? && first.locked(&checked).await?,
        "the crash left both leases persisted"
    );

    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    assert!(
        !second.locked(&unchecked).await? && !second.locked(&checked).await?,
        "startup released the orphan leases"
    );
    assert_eq!(
        second.offset(&unchecked).await?,
        durable,
        "startup rewrites no offset"
    );
    assert_eq!(second.offset(&checked).await?, 0);
    ensure!(
        std::fs::metadata(second.blob(&unchecked))?.len() >= durable,
        "startup deleted nothing"
    );
    assert!(std::fs::metadata(second.blob(&checked))?.len() >= 8 * MIB);

    let resume_at = second.head(&unchecked).await?;
    assert_eq!(resume_at, durable, "HEAD reports the last durable offset");
    assert_eq!(
        second.head(&checked).await?,
        0,
        "the whole checksummed body is retransmitted"
    );

    let mut resumed = second.open(&unchecked, resume_at, None).await?;
    send_range(&mut resumed, size, resume_at, size).await?;
    resumed.finish().await?;
    let response = resumed.response().await?;
    assert_eq!(
        response.status, 204,
        "no lease wait after the crash: {}",
        response.body
    );
    let mut retry = second.open(&checked, 0, Some(&header)).await?;
    send_range(&mut retry, size, 0, size).await?;
    retry.finish().await?;
    let response = retry.response().await?;
    assert_eq!(response.status, 204, "{}", response.body);

    for upload in [&unchecked, &checked] {
        assert_eq!(second.offset(upload).await?, size);
        second.wait_finalized(upload).await?;
        let (staged, digest) = second.blob_digest(upload).await?;
        assert_eq!((staged, digest), (size, expected.clone()));
    }
    assert_eq!(second.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 2);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        2,
        "both completed uploads were finalized exactly once"
    );
    let exit = restarted.stop().await?;
    assert!(exit.success());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_tus_restart_after_downtime_expires_abandoned_uploads_without_a_client_request(
) -> Result<()> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 8 * MIB;

    let mut process = ServerProcess::spawn(&data_dir)?;
    process.wait_ready().await?;
    let first = world(&process, &data_dir, None).await?;
    let abandoned = first.upload("abandoned.bin", size).await?;
    let live = first.upload("live.bin", size).await?;
    let mut writer = first.open(&abandoned, 0, None).await?;
    send_range(&mut writer, size, 0, 2 * MIB).await?;
    wait_until("a durable checkpoint of the abandoned upload", || async {
        Ok(first.offset(&abandoned).await? >= 2 * MIB)
    })
    .await?;
    drop(writer);
    wait_until("the dropped request to release its lease", || async {
        Ok(!first.locked(&abandoned).await?)
    })
    .await?;
    let session = first
        .db
        .scalar_string(&format!(
            "SELECT t.transfer_session_id FROM transfer_session_files t
               JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id WHERE tu.id = '{}'",
            abandoned.id
        ))
        .await?;
    let live_session = first
        .db
        .scalar_string(&format!(
            "SELECT t.transfer_session_id FROM transfer_session_files t
               JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id WHERE tu.id = '{}'",
            live.id
        ))
        .await?;
    assert!(first.blob(&abandoned).exists());
    let status = process.stop().await?;
    assert!(status.success());

    first
        .db
        .execute(&format!(
            "UPDATE tus_uploads SET expires_at = '2020-01-01T00:00:00.000Z' WHERE id = '{}'",
            abandoned.id
        ))
        .await?;
    first
        .db
        .execute(
            "UPDATE jobs SET run_at = '2020-01-01T00:00:00.000Z' WHERE kind = 'tus.expire_stale' AND state = 'pending'",
        )
        .await?;
    let held_before = first
        .db
        .scalar_i64("SELECT COUNT(*) FROM quota_reservations WHERE state = 'held'")
        .await?;
    assert_eq!(held_before, 2);

    let mut restarted = ServerProcess::spawn(&data_dir)?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    wait_until("the reaper to expire the abandoned upload", || async {
        Ok(second
            .db
            .scalar_string(&format!(
                "SELECT state FROM tus_uploads WHERE id = '{}'",
                abandoned.id
            ))
            .await?
            == "expired")
    })
    .await?;
    wait_until("the abandoned staging to be removed", || async {
        Ok(!second.blob(&abandoned).exists())
    })
    .await?;
    assert_eq!(
        second
            .db
            .scalar_string(&format!(
                "SELECT state || '/' || release_reason FROM quota_reservations WHERE transfer_session_id = '{session}'"
            ))
            .await?,
        "released/expired"
    );
    assert_eq!(
        second
            .db
            .scalar_string(&format!(
                "SELECT state FROM quota_reservations WHERE transfer_session_id = '{live_session}'"
            ))
            .await?,
        "held",
        "the live upload's hold is untouched"
    );
    assert_eq!(
        second
            .db
            .scalar_string(&format!(
                "SELECT state FROM tus_uploads WHERE id = '{}'",
                live.id
            ))
            .await?,
        "created"
    );
    assert_eq!(second.head(&live).await?, 0);
    let exit = restarted.stop().await?;
    assert!(exit.success());
    Ok(())
}
