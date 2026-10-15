#![cfg(feature = "e2e-fixture")]

pub mod support;

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use support::client::{Creds, Db, Http};
use support::minio::MinioServer;
use support::tus::{generated, wait_until, MIB, SESSIONS};
use support::ServerProcess;

const PART: u64 = 8 * MIB;
const KEY: &str = "directory-tree-session-key-0001";

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
        ensure!(read > 0, "the generator ended early");
        filled += read;
    }
    Ok(part)
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

struct Planned {
    path: &'static str,
    size: u64,
    item: String,
}

const TREE: [(&str, u64); 4] = [
    ("Project/docs/img/b.bin", 3 * MIB + 5),
    ("Project/docs/a.bin", 2 * PART + 77),
    ("Project/c.bin", 1_024),
    ("top.bin", 2_048),
];

impl World {
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<support::client::Reply> {
        self.http.send(method, path, Some(&self.creds), body).await
    }

    fn body() -> Value {
        let files: Vec<Value> = TREE
            .iter()
            .enumerate()
            .map(|(index, (path, size))| {
                json!({
                    "clientId": format!("tree-{index}"),
                    "name": path.rsplit('/').next().unwrap_or(path),
                    "sizeBytes": size,
                    "relativePath": path,
                })
            })
            .collect();
        json!({ "target": { "kind": "my_files", "folderId": null }, "files": files })
    }

    async fn open(&self) -> Result<(support::client::Reply, String, Vec<Planned>)> {
        let reply = self
            .http
            .send_with_headers(
                Method::POST,
                SESSIONS,
                Some(&self.creds),
                Some(Self::body()),
                &[("Idempotency-Key", KEY)],
            )
            .await?
            .expect(StatusCode::CREATED)?;
        let session = reply.json()?["id"]
            .as_str()
            .context("session id")?
            .to_owned();
        let mut planned = Vec::new();
        for (index, (path, size)) in TREE.iter().enumerate() {
            let item = self
                .db
                .scalar_string(&format!(
                    "SELECT id FROM transfer_session_files
                      WHERE transfer_session_id = '{session}' AND ordinal = {index}"
                ))
                .await?;
            planned.push(Planned {
                path,
                size: *size,
                item,
            });
        }
        Ok((reply, session, planned))
    }

    fn route(session: &str, item: &str, tail: &str) -> String {
        format!("{SESSIONS}/{session}/files/{item}/s3/multipart{tail}")
    }

    async fn upload_parts(&self, session: &str, item: &Planned) -> Result<()> {
        let item_id = &item.item;
        let size = item.size;
        let created = self
            .call(Method::POST, &Self::route(session, item_id, ""), None)
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let count = created["partCount"].as_u64().context("part count")?;
        let numbers: Vec<u64> = (1..=count).collect();
        let presigned = self
            .call(
                Method::POST,
                &Self::route(session, item_id, "/presign"),
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
            ensure!(response.status().is_success(), "{}", response.status());
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
            &Self::route(session, item_id, "/parts"),
            Some(json!({ "parts": reports })),
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
        let listed = self
            .call(Method::GET, &Self::route(session, item_id, "/parts"), None)
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        ensure!(listed["missingPartNumbers"] == json!([]), "{listed}");
        ensure!(listed["uploadedBytes"] == size, "{listed}");
        Ok(())
    }

    async fn complete(&self, session: &str, item: &Planned) -> Result<support::client::Reply> {
        self.call(
            Method::POST,
            &Self::route(session, &item.item, "/complete"),
            None,
        )
        .await
    }

    async fn upload(&self, session: &str, item: &Planned) -> Result<()> {
        self.upload_parts(session, item).await?;
        let completed = self.complete(session, item).await?;
        ensure!(
            completed.status.is_success(),
            "{} {}",
            completed.status,
            completed.body
        );
        Ok(())
    }

    async fn folders(&self) -> Result<String> {
        self.db
            .scalar_string(
                "SELECT group_concat(id || '|' || name || '|' || ifnull(parent_id, '') || '|' || depth, ';')
                   FROM (SELECT * FROM folders ORDER BY depth, name)",
            )
            .await
    }

    async fn placed(&self) -> Result<String> {
        self.db
            .scalar_string(
                "SELECT group_concat(path, ';') FROM (
                    SELECT CASE
                             WHEN f.folder_id IS NULL THEN f.name
                             WHEN d.depth = 0 THEN d.name || '/' || f.name
                             WHEN d.depth = 1 THEN p.name || '/' || d.name || '/' || f.name
                             ELSE g.name || '/' || p.name || '/' || d.name || '/' || f.name
                           END AS path
                      FROM files f
                      LEFT JOIN folders d ON d.id = f.folder_id
                      LEFT JOIN folders p ON p.id = d.parent_id
                      LEFT JOIN folders g ON g.id = p.parent_id
                     ORDER BY path)",
            )
            .await
    }
}

fn expected_digest(size: u64) -> Result<Vec<u8>> {
    Ok(sha256(&bytes_of(size, 0, size)?))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_directory_tree_s3_minio_finalization_and_restart() -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("tree").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let env = storage_env(&minio, &bucket);

    let mut process = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    process.wait_ready().await?;
    let first = world(&process, &data_dir, None).await?;
    first
        .call(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Project", "parentId": null })),
        )
        .await?
        .expect(StatusCode::CREATED)?;
    let project = first
        .db
        .scalar_string("SELECT id FROM folders WHERE name = 'Project'")
        .await?;

    let (created, session, planned) = first.open().await?;
    let skeleton = first.folders().await?;
    assert_eq!(
        first.db.scalar_i64("SELECT COUNT(*) FROM folders").await?,
        3,
        "Project (reused), docs and img: {skeleton}"
    );
    ensure!(
        skeleton.contains(&project),
        "the pre-existing folder keeps its identity"
    );
    assert_eq!(
        first
            .db
            .scalar_string(
                "SELECT group_concat(relative_path || '#' || display_name, ';')
                   FROM (SELECT * FROM transfer_session_files ORDER BY ordinal)"
            )
            .await?,
        "Project/docs/img#b.bin;Project/docs#a.bin;Project#c.bin;#top.bin"
    );
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT COUNT(*) FROM quota_reservations")
            .await?,
        1
    );
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await?,
        0,
        "admission allocated no multipart upload"
    );
    let replay = first
        .http
        .send_with_headers(
            Method::POST,
            SESSIONS,
            Some(&first.creds),
            Some(World::body()),
            &[("Idempotency-Key", KEY)],
        )
        .await?
        .expect(StatusCode::CREATED)?;
    assert_eq!(
        replay.body, created.body,
        "the replay is the original session"
    );
    assert_eq!(first.folders().await?, skeleton);
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions")
            .await?,
        1
    );

    first.upload(&session, &planned[0]).await?;
    let img = first
        .db
        .scalar_string("SELECT id FROM folders WHERE name = 'img'")
        .await?;
    assert_eq!(
        first
            .db
            .scalar_string("SELECT folder_id FROM files")
            .await?,
        img,
        "the nested item lands in its own folder"
    );
    let key = first
        .db
        .scalar_string(&format!(
            "SELECT final_object_key FROM transfer_session_files WHERE id = '{}'",
            planned[0].item
        ))
        .await?;
    assert!(
        key.starts_with("objects/") && !key.contains("Project") && !key.contains("b.bin"),
        "{key}"
    );
    let object = minio.read_object(&bucket, &key).await?;
    assert_eq!(
        object.len() as u64,
        planned[0].size,
        "HeadObject-authoritative size"
    );
    assert_eq!(sha256(&object), expected_digest(planned[0].size)?);
    assert_eq!(
        first
            .db
            .scalar_i64("SELECT size_bytes FROM storage_objects")
            .await?,
        i64::try_from(planned[0].size)?
    );

    let identities = first
        .db
        .scalar_string(
            "SELECT group_concat(id || ':' || final_object_id || ':' || final_object_key, ';')
               FROM (SELECT * FROM transfer_session_files ORDER BY ordinal)",
        )
        .await?;
    let status = process.kill_hard()?;
    assert_eq!(status.signal(), Some(9), "the process died of SIGKILL");

    let mut restarted = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    assert_eq!(
        second.folders().await?,
        skeleton,
        "the restart rebuilt nothing"
    );
    assert_eq!(
        second
            .db
            .scalar_string(
                "SELECT group_concat(id || ':' || final_object_id || ':' || final_object_key, ';')
                   FROM (SELECT * FROM transfer_session_files ORDER BY ordinal)"
            )
            .await?,
        identities,
        "the same item identities after the restart"
    );
    for item in &planned[1..] {
        second.upload(&session, item).await?;
    }

    assert_eq!(
        second.placed().await?,
        "Project/c.bin;Project/docs/a.bin;Project/docs/img/b.bin;top.bin"
    );
    assert_eq!(
        second.folders().await?,
        skeleton,
        "finalization created no folder"
    );
    assert_eq!(second.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 4);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await?,
        4,
        "one object per file"
    );
    let total: u64 = TREE.iter().map(|(_, size)| size).sum();
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        i64::try_from(total)?
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state || ':' || committed_bytes FROM quota_reservations")
            .await?,
        format!("committed:{total}")
    );
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state FROM transfer_sessions")
            .await?,
        "completed"
    );
    for item in &planned {
        let key = second
            .db
            .scalar_string(&format!(
                "SELECT final_object_key FROM transfer_session_files WHERE id = '{}'",
                item.item
            ))
            .await?;
        assert!(
            key.starts_with("objects/") && !key.contains(item.path),
            "{key}"
        );
        let object = minio.read_object(&bucket, &key).await?;
        assert_eq!(object.len() as u64, item.size, "{}", item.path);
        assert_eq!(
            sha256(&object),
            expected_digest(item.size)?,
            "{}",
            item.path
        );
        assert_eq!(
            second
                .db
                .scalar_string(&format!(
                    "SELECT state FROM transfer_session_files WHERE id = '{}'",
                    item.item
                ))
                .await?,
            "completed"
        );
    }
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
            .await?,
        4
    );
    assert!(restarted.stop().await?.success());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_e2e_fixture_directory_tree_s3_failed_item_does_not_strand_siblings() -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("strand").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let env = storage_env(&minio, &bucket);

    let mut process = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    process.wait_ready().await?;
    let first = world(&process, &data_dir, None).await?;
    let (_, session, planned) = first.open().await?;
    let skeleton = first.folders().await?;

    first.upload(&session, &planned[2]).await?;
    first.upload_parts(&session, &planned[0]).await?;
    let img = first
        .db
        .scalar_string("SELECT id FROM folders WHERE name = 'img'")
        .await?;
    first
        .call(
            Method::PATCH,
            &format!("/api/v1/folders/{img}"),
            Some(json!({ "name": "img-renamed" })),
        )
        .await?
        .expect(StatusCode::OK)?;
    let rejected = first.complete(&session, &planned[0]).await?;
    ensure!(!rejected.status.is_success(), "{}", rejected.body);
    wait_until("the renamed destination to fail the item", || async {
        Ok(first
            .db
            .scalar_string(&format!(
                "SELECT state || ':' || ifnull(error_code, '') FROM transfer_session_files WHERE id = '{}'",
                planned[0].item
            ))
            .await?
            == "failed:FOLDER_NOT_FOUND")
    })
    .await?;
    assert_eq!(
        first
            .db
            .scalar_string("SELECT state FROM transfer_sessions")
            .await?,
        "uploading",
        "two siblings are still pending"
    );
    let failed_key = first
        .db
        .scalar_string(&format!(
            "SELECT final_object_key FROM transfer_session_files WHERE id = '{}'",
            planned[0].item
        ))
        .await?;
    assert!(
        !minio.object_exists(&bucket, &failed_key).await,
        "the rejected item never reached its final key"
    );

    let status = process.kill_hard()?;
    assert_eq!(status.signal(), Some(9), "the process died of SIGKILL");
    let mut restarted = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    restarted.wait_ready().await?;
    let second = world(&restarted, &data_dir, Some(&first)).await?;
    second.upload(&session, &planned[1]).await?;
    second.upload(&session, &planned[3]).await?;

    assert_eq!(
        second
            .db
            .scalar_string("SELECT state FROM transfer_sessions")
            .await?,
        "failed",
        "nothing is live and one item failed"
    );
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions")
            .await?,
        1
    );
    assert_eq!(second.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 3);
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await?,
        3
    );
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
            .await?,
        3
    );
    let landed = planned[1].size + planned[2].size + planned[3].size;
    assert_eq!(
        second
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        i64::try_from(landed)?,
        "no double charge"
    );
    let renamed_skeleton = skeleton.replace("|img|", "|img-renamed|");
    assert_eq!(
        second.folders().await?,
        renamed_skeleton,
        "no tree was recreated"
    );
    assert_eq!(
        second.placed().await?,
        "Project/c.bin;Project/docs/a.bin;top.bin"
    );

    let closed = second
        .call(
            Method::POST,
            &format!("{SESSIONS}/{session}/complete"),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(closed["state"], "completed");
    assert_eq!(
        second
            .db
            .scalar_string("SELECT state || ':' || committed_bytes FROM quota_reservations")
            .await?,
        format!("committed:{landed}")
    );
    assert!(restarted.stop().await?.success());
    Ok(())
}
