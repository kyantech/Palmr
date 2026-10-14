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

    async fn upload(&self, session: &str, item: &str, size: u64) -> Result<()> {
        let created = self
            .call(Method::POST, &Self::route(session, item, ""), None)
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let count = created["partCount"].as_u64().context("part count")?;
        let numbers: Vec<u64> = (1..=count).collect();
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
        let listed = self
            .call(Method::GET, &Self::route(session, item, "/parts"), None)
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        anyhow::ensure!(listed["missingPartNumbers"] == json!([]), "{listed}");
        anyhow::ensure!(listed["uploadedBytes"] == size, "{listed}");
        Ok(())
    }

    async fn counts(&self) -> Result<(i64, i64, i64, i64)> {
        Ok((
            self.db.scalar_i64("SELECT COUNT(*) FROM files").await?,
            self.db
                .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
                .await?,
            self.db
                .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
                .await?,
            self.db
                .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
                .await?,
        ))
    }
}

fn marks(marker: &Path) -> usize {
    support::read_text(marker).map_or(0, |text| text.lines().count())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_s3_multipart_kill_after_provider_complete_recovers_exactly_once(
) -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("e2e").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let marker = root.path().join("paused.marker");
    let size = 20 * MIB + 321;
    let expected = generated_digest(size)?;

    let mut env = storage_env(&minio, &bucket);
    let plain = env.clone();
    env.push((
        "PALMR_E2E_FINALIZE_PAUSE_AT".to_owned(),
        "before_commit".into(),
    ));
    env.push((
        "PALMR_E2E_FINALIZE_PAUSE_FILE".to_owned(),
        marker.as_os_str().to_owned(),
    ));
    let mut process = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    process.wait_ready().await?;
    let first = world(&process, &data_dir, None).await?;
    let (session, item) = first.plan("crash.bin", size).await?;
    first.upload(&session, &item, size).await?;

    let completing = {
        let http = Arc::clone(&first.http);
        let creds = first.creds.clone();
        let path = World::route(&session, &item, "/complete");
        tokio::spawn(async move { http.send(Method::POST, &path, Some(&creds), None).await })
    };
    wait_until("the finalizer to pause", || async {
        Ok(marks(&marker) >= 1)
    })
    .await?;
    for line in support::read_text(&marker)?.lines() {
        assert_eq!(line, "before_commit");
    }

    let key = first
        .db
        .scalar_string("SELECT final_object_key FROM transfer_session_files")
        .await?;
    let identity = first
        .db
        .scalar_string(
            "SELECT final_object_id || '|' || final_object_key FROM transfer_session_files",
        )
        .await?;
    assert_eq!(
        first
            .db
            .scalar_string("SELECT state FROM transfer_session_files")
            .await?,
        "finalizing"
    );
    assert_eq!(
        first
            .db
            .scalar_string("SELECT finalize_stage FROM transfer_session_files")
            .await?,
        "placing"
    );
    assert_eq!(
        first
            .db
            .scalar_string("SELECT state FROM s3_multipart_uploads")
            .await?,
        "completing"
    );
    assert_eq!(
        first.counts().await?,
        (0, 0, 0, 0),
        "nothing is committed yet"
    );
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        0
    );
    assert!(
        minio.object_exists(&bucket, &key).await,
        "the provider holds the object"
    );
    let placed = minio.read_object(&bucket, &key).await?;
    assert_eq!(placed.len() as u64, size);
    assert_eq!(sha256(&placed), expected);

    let status = process.kill_hard()?;
    assert_eq!(status.signal(), Some(9), "the process died of SIGKILL");
    completing.abort();

    let mut restarted = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    wait_until("the placed object to be committed", || async {
        Ok(second
            .db
            .scalar_string("SELECT state FROM transfer_session_files")
            .await?
            == "completed")
    })
    .await?;

    assert_eq!(second.counts().await?, (1, 1, i64::try_from(size)?, 1));
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
    assert_eq!(
        second
            .db
            .scalar_string("SELECT id || '|' || object_key FROM storage_objects")
            .await?,
        identity
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state || ':' || committed_bytes FROM quota_reservations")
            .await?,
        format!("committed:{size}"),
        "one settlement"
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state FROM s3_multipart_uploads")
            .await?,
        "completed"
    );
    let object = minio.read_object(&bucket, &key).await?;
    assert_eq!(object.len() as u64, size, "the exact size");
    assert_eq!(sha256(&object), expected, "the exact generated bytes");
    let file_id = second.db.scalar_string("SELECT id FROM files").await?;
    second
        .call(Method::GET, &format!("/api/v1/files/{file_id}"), None)
        .await?
        .expect(StatusCode::OK)?;
    let again = second
        .call(
            Method::POST,
            &World::route(&session, &item, "/complete"),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(
        again["fileId"], file_id,
        "a browser retry gets the original file"
    );

    assert!(restarted.stop().await?.success());
    let mut third_boot = ServerProcess::spawn_with(&data_dir, &borrowed(&plain))?;
    third_boot.wait_ready().await?;
    let third = world(&third_boot, &data_dir, Some(&first)).await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(third.counts().await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(
        third.db.scalar_string("SELECT id FROM files").await?,
        file_id
    );
    assert_eq!(sha256(&minio.read_object(&bucket, &key).await?), expected);
    third_boot.stop().await?;
    Ok(())
}
