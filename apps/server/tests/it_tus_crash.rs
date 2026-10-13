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
        let (staged, digest) = second.blob_digest(upload)?;
        assert_eq!((staged, digest), (size, expected.clone()));
    }
    assert_eq!(second.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        0
    );
    let exit = restarted.stop().await?;
    assert!(exit.success());
    Ok(())
}
