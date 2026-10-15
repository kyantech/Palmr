#![cfg(feature = "e2e-fixture")]

pub mod support;

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use support::client::{Creds, Db, Http};
use support::minio::MinioServer;
use support::tus::{generated, generated_digest, wait_until, MIB, SESSIONS};
use support::ServerProcess;

const PART: u64 = 8 * MIB;
const LONG_AGO: &str = "2026-01-01T00:00:00.000Z";

struct World {
    http: Arc<Http>,
    db: Db,
    creds: Creds,
}

fn storage_env(minio: &MinioServer, bucket: &str) -> Vec<(String, OsString)> {
    [
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_PROFILE", "minio"),
        ("PALMR_S3_ENDPOINT", minio.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", bucket),
        ("PALMR_S3_ACCESS_KEY", minio.access_key()),
        ("PALMR_S3_SECRET_KEY", minio.secret_key()),
        ("PALMR_S3_FORCE_PATH_STYLE", "true"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), OsString::from(value)))
    .collect()
}

fn borrowed(env: &[(String, OsString)]) -> Vec<(&str, &std::ffi::OsStr)> {
    env.iter()
        .map(|(name, value)| (name.as_str(), value.as_os_str()))
        .collect()
}

async fn world(
    process: &ServerProcess,
    data_dir: &Path,
    previous: Option<&World>,
) -> Result<World> {
    let base = url::Url::parse(&format!("http://127.0.0.1:{}/", process.port()))?;
    let http = Http::new(base)?;
    let creds = match previous {
        Some(previous) => previous.creds.clone(),
        None => http.setup_admin().await?,
    };
    Ok(World {
        http: Arc::new(http),
        db: Db::new(data_dir),
        creds,
    })
}

fn bytes_of(size: u64, from: u64, to: u64) -> Result<Vec<u8>> {
    let mut stream = generated(from, size)?;
    let mut part = vec![0_u8; usize::try_from(to - from)?];
    let mut filled = 0;
    while filled < part.len() {
        let read = stream
            .read(&mut part[filled..])
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        anyhow::ensure!(read > 0, "the generator ended early");
        filled += read;
    }
    Ok(part)
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn marks(marker: &Path) -> usize {
    support::read_text(marker).map_or(0, |text| text.lines().count())
}

impl World {
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<support::client::Reply> {
        self.http.send(method, path, Some(&self.creds), body).await
    }

    async fn plan(&self, name: &str, size: u64) -> Result<(String, String)> {
        let session = self
            .call(
                Method::POST,
                SESSIONS,
                Some(json!({
                    "target": { "kind": "my_files", "folderId": null },
                    "files": [{ "clientId": "c1", "name": name, "sizeBytes": size }],
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let session = session["id"].as_str().context("session id")?.to_owned();
        let item = self
            .db
            .scalar_string(&format!(
                "SELECT id FROM transfer_session_files WHERE transfer_session_id = '{session}'"
            ))
            .await?;
        Ok((session, item))
    }

    fn route(session: &str, item: &str, tail: &str) -> String {
        format!("{SESSIONS}/{session}/files/{item}/s3/multipart{tail}")
    }

    async fn create(&self, session: &str, item: &str) -> Result<()> {
        self.call(Method::POST, &Self::route(session, item, ""), None)
            .await?
            .expect(StatusCode::CREATED)?;
        Ok(())
    }

    async fn put_parts(&self, session: &str, item: &str, size: u64, numbers: &[u64]) -> Result<()> {
        let presigned = self
            .call(
                Method::POST,
                &Self::route(session, item, "/presign"),
                Some(json!({ "partNumbers": numbers })),
            )
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        let client = reqwest::Client::new();
        let mut reports = Vec::new();
        for entry in presigned["parts"].as_array().context("parts")? {
            let number = entry["partNumber"].as_u64().context("part number")?;
            let from = (number - 1) * PART;
            let to = (number * PART).min(size);
            let response = client
                .put(entry["url"].as_str().context("url")?)
                .body(bytes_of(size, from, to)?)
                .send()
                .await
                .context("the presigned PUT")?;
            anyhow::ensure!(response.status().is_success(), "{}", response.status());
            let etag = response
                .headers()
                .get("etag")
                .context("the provider ETag")?
                .to_str()?
                .to_owned();
            reports.push(json!({ "partNumber": number, "etag": etag, "sizeBytes": to - from }));
        }
        self.call(
            Method::POST,
            &Self::route(session, item, "/parts"),
            Some(json!({ "parts": reports })),
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
        Ok(())
    }

    async fn listed(&self, session: &str, item: &str) -> Result<Value> {
        self.call(Method::GET, &Self::route(session, item, "/parts"), None)
            .await?
            .expect(StatusCode::OK)?
            .json()
    }

    async fn multipart_state(&self) -> Result<String> {
        self.db
            .scalar_string("SELECT state FROM s3_multipart_uploads")
            .await
    }
}

async fn reaper_jobs_idle(world: &World) -> Result<bool> {
    Ok(world
        .db
        .scalar_i64(
            "SELECT COUNT(*) FROM jobs WHERE kind IN ('s3.abort_abandoned_multipart', 's3.reconcile_multipart')
                AND state IN ('pending', 'claimed') AND run_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .await?
        == 0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_s3_reaper_kill_before_provider_abort_recovers_once() -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("reap").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let marker = root.path().join("paused.marker");
    let size = 20 * MIB + 321;

    let plain = storage_env(&minio, &bucket);
    let mut paused = plain.clone();
    paused.push((
        "PALMR_E2E_S3_REAPER_PAUSE_AT".to_owned(),
        "before_provider_abort".into(),
    ));
    paused.push((
        "PALMR_E2E_S3_REAPER_PAUSE_FILE".to_owned(),
        marker.as_os_str().to_owned(),
    ));

    let mut first_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    first_boot.wait_ready().await?;
    let first = world(&first_boot, &data_dir, None).await?;
    let (session, item) = first.plan("expires.bin", size).await?;
    first.create(&session, &item).await?;
    first.put_parts(&session, &item, size, &[1, 2, 3]).await?;
    assert_eq!(first.multipart_state().await?, "in_progress");
    assert_eq!(
        minio.incomplete_uploads(&bucket).await?,
        1,
        "the independent oracle sees the live upload"
    );
    let identity = first
        .db
        .scalar_string(
            "SELECT bucket || '|' || object_key || '|' || s3_upload_id FROM s3_multipart_uploads",
        )
        .await?;
    let status = first_boot.kill_hard()?;
    assert_eq!(status.signal(), Some(9));

    first
        .db
        .execute(&format!(
            "UPDATE s3_multipart_uploads SET expires_at = '{LONG_AGO}'"
        ))
        .await?;

    let mut second_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&paused))?;
    second_boot.wait_ready().await?;
    let second = world(&second_boot, &data_dir, Some(&first)).await?;
    wait_until("the reaper to pause before the provider abort", || async {
        Ok(marks(&marker) >= 1)
    })
    .await?;
    for line in support::read_text(&marker)?.lines() {
        assert_eq!(line, "before_provider_abort");
    }
    assert_eq!(
        second.multipart_state().await?,
        "abandoned",
        "the terminal state is durable"
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state FROM transfer_session_files")
            .await?,
        "expired"
    );
    assert_eq!(
        minio.incomplete_uploads(&bucket).await?,
        1,
        "the provider abort has not happened yet"
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT bucket || '|' || object_key || '|' || s3_upload_id FROM s3_multipart_uploads")
            .await?,
        identity,
        "the provider identity is preserved"
    );
    let status = second_boot.kill_hard()?;
    assert_eq!(status.signal(), Some(9));

    let mut third_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    third_boot.wait_ready().await?;
    let third = world(&third_boot, &data_dir, Some(&first)).await?;
    wait_until("the restarted reaper to abort the upload", || async {
        Ok(third.multipart_state().await? == "aborted")
    })
    .await?;
    assert_eq!(
        minio.incomplete_uploads(&bucket).await?,
        0,
        "the provider upload is gone"
    );
    assert_eq!(
        third
            .db
            .scalar_string(
                "SELECT state || ':' || COALESCE(release_reason, '') FROM quota_reservations"
            )
            .await?,
        "released:expired",
        "one settlement"
    );
    assert_eq!(
        third
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        0
    );
    assert_eq!(third.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);
    assert_eq!(
        third
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        0
    );
    assert!(
        third
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'TRANSFER_EXPIRED'")
            .await?
            <= 1,
        "no duplicate logical audit event"
    );
    wait_until("the reaper jobs to settle", || async {
        reaper_jobs_idle(&third).await
    })
    .await?;
    assert!(third_boot.stop().await?.success());

    let mut fourth_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    fourth_boot.wait_ready().await?;
    let fourth = world(&fourth_boot, &data_dir, Some(&first)).await?;
    wait_until("the reaper jobs to settle again", || async {
        reaper_jobs_idle(&fourth).await
    })
    .await?;
    assert_eq!(fourth.multipart_state().await?, "aborted");
    assert_eq!(
        fourth
            .db
            .scalar_string(
                "SELECT state || ':' || COALESCE(release_reason, '') FROM quota_reservations"
            )
            .await?,
        "released:expired"
    );
    assert_eq!(
        fourth
            .db
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await?,
        1
    );
    assert_eq!(minio.incomplete_uploads(&bucket).await?, 0);
    fourth_boot.stop().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R083_restart_preserves_multipart_state_e2e_fixture() -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("r083").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 20 * MIB + 321;
    let expected = generated_digest(size)?;
    let plain = storage_env(&minio, &bucket);

    let first_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    let mut first_boot = first_boot;
    first_boot.wait_ready().await?;
    let first = world(&first_boot, &data_dir, None).await?;
    let (session, item) = first.plan("restart.bin", size).await?;
    first.create(&session, &item).await?;
    first.put_parts(&session, &item, size, &[1]).await?;
    let identity = first
        .db
        .scalar_string("SELECT id || '|' || s3_upload_id FROM s3_multipart_uploads")
        .await?;
    assert_eq!(minio.incomplete_uploads(&bucket).await?, 1);
    first_boot.kill_hard()?;

    let mut second_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    second_boot.wait_ready().await?;
    let second = world(&second_boot, &data_dir, Some(&first)).await?;
    wait_until("startup reconciliation to finish", || async {
        reaper_jobs_idle(&second).await
    })
    .await?;
    assert_eq!(second.multipart_state().await?, "in_progress");
    assert_eq!(
        minio.incomplete_uploads(&bucket).await?,
        1,
        "the restart aborted nothing"
    );
    let listed = second.listed(&session, &item).await?;
    assert_eq!(listed["completedParts"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["missingPartNumbers"], json!([2, 3]));

    second.put_parts(&session, &item, size, &[2, 3]).await?;
    let completed = second
        .call(
            Method::POST,
            &World::route(&session, &item, "/complete"),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert!(completed["fileId"].is_string(), "{completed}");
    assert_eq!(
        second
            .db
            .scalar_string("SELECT id || '|' || s3_upload_id FROM s3_multipart_uploads")
            .await?,
        identity,
        "the same provider upload finished"
    );
    let key = second
        .db
        .scalar_string("SELECT final_object_key FROM transfer_session_files")
        .await?;
    let object = minio.read_object(&bucket, &key).await?;
    assert_eq!(object.len() as u64, size);
    assert_eq!(sha256(&object), expected, "the exact generated bytes");
    second_boot.stop().await?;
    Ok(())
}
