pub mod support;

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use base64ct::{Base64, Encoding};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use support::checksum_gate::ChecksumGate;
use support::client::{Creds, Db, Http};
use support::minio::MinioServer;
use support::tus::{generated, generated_digest, wait_until, MIB, SESSIONS};
use support::ServerProcess;

const PART: u64 = 8 * MIB;
const CEILING: u64 = 83_047_219_200;

struct World {
    base: url::Url,
    http: Arc<Http>,
    db: Db,
    creds: Creds,
}

fn storage_env(minio: &MinioServer, gate: &ChecksumGate, bucket: &str) -> Vec<(String, OsString)> {
    let endpoint = gate.endpoint();
    [
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_PROFILE", "generic"),
        ("PALMR_S3_REQUIRE_PART_CHECKSUMS", "true"),
        ("PALMR_S3_ENDPOINT", endpoint.as_str()),
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

fn checksum(bytes: &[u8]) -> String {
    Base64::encode_string(&Sha256::digest(bytes))
}

struct Raw {
    status: u16,
    body: String,
}

enum Framing<'a> {
    Length(&'a [u8]),
    Declared(usize, &'a [u8]),
    Chunked(&'a [u8], usize),
}

impl World {
    async fn open(
        process: &ServerProcess,
        data_dir: &Path,
        previous: Option<&World>,
    ) -> Result<Self> {
        let base = url::Url::parse(&format!("http://127.0.0.1:{}/", process.port()))?;
        let http = Http::new(base.clone())?;
        let creds = match previous {
            Some(previous) => previous.creds.clone(),
            None => http.setup_admin().await?,
        };
        Ok(Self {
            base,
            http: Arc::new(http),
            db: Db::new(data_dir),
            creds,
        })
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<support::client::Reply> {
        self.http.send(method, path, Some(&self.creds), body).await
    }

    fn route(session: &str, item: &str, tail: &str) -> String {
        format!("{SESSIONS}/{session}/files/{item}/s3/multipart{tail}")
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

    async fn put(&self, session: &str, item: &str, part: u64, framing: Framing<'_>) -> Result<Raw> {
        let host = self.base.host_str().context("host")?;
        let port = self.base.port().context("port")?;
        let mut stream = TcpStream::connect((host, port)).await?;
        let (length_header, body_chunks): (String, Vec<Vec<u8>>) = match framing {
            Framing::Length(body) => (
                format!("Content-Length: {}\r\n", body.len()),
                vec![body.to_vec()],
            ),
            Framing::Declared(declared, body) => (
                format!("Content-Length: {declared}\r\n"),
                vec![body.to_vec()],
            ),
            Framing::Chunked(body, frame) => (
                "Transfer-Encoding: chunked\r\n".to_owned(),
                body.chunks(frame).map(<[u8]>::to_vec).collect(),
            ),
        };
        let chunked = length_header.starts_with("Transfer-Encoding");
        let head = format!(
            "PUT {path} HTTP/1.1\r\nHost: {host}:{port}\r\nOrigin: {origin}\r\n\
             Cookie: palmr_session={session_cookie}; palmr_csrf={csrf}\r\nX-Palmr-CSRF: {csrf}\r\n\
             Content-Type: application/octet-stream\r\n{length_header}\r\n",
            path = Self::route(session, item, &format!("/parts/{part}")),
            origin = self.base.origin().ascii_serialization(),
            session_cookie = self.creds.session,
            csrf = self.creds.csrf,
        );
        stream.write_all(head.as_bytes()).await?;
        let mut sent = Ok(());
        for chunk in &body_chunks {
            sent = write_body(&mut stream, chunk, chunked).await;
            if sent.is_err() {
                break;
            }
        }
        if sent.is_ok() && chunked {
            sent = stream.write_all(b"0\r\n\r\n").await;
        }
        if let Err(error) = sent {
            ensure!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ),
                "writing the body failed: {error}"
            );
        }
        read_response(&mut stream).await
    }

    async fn abandon(&self, session: &str, item: &str, part: u64, sent: &[u8]) -> Result<()> {
        let host = self.base.host_str().context("host")?;
        let port = self.base.port().context("port")?;
        let mut stream = TcpStream::connect((host, port)).await?;
        let head = format!(
            "PUT {path} HTTP/1.1\r\nHost: {host}:{port}\r\nOrigin: {origin}\r\n\
             Cookie: palmr_session={session_cookie}; palmr_csrf={csrf}\r\nX-Palmr-CSRF: {csrf}\r\n\
             Content-Type: application/octet-stream\r\nContent-Length: {PART}\r\n\r\n",
            path = Self::route(session, item, &format!("/parts/{part}")),
            origin = self.base.origin().ascii_serialization(),
            session_cookie = self.creds.session,
            csrf = self.creds.csrf,
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(sent).await?;
        stream.shutdown().await?;
        drop(stream);
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

    async fn ready(&self) -> Result<Value> {
        let reply = reqwest::get(self.base.join("health/ready")?).await?;
        ensure!(reply.status().is_success(), "{}", reply.status());
        Ok(serde_json::from_str(&reply.text().await?)?)
    }
}

async fn write_body(stream: &mut TcpStream, chunk: &[u8], chunked: bool) -> std::io::Result<()> {
    if chunked {
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(chunk).await?;
        stream.write_all(b"\r\n").await
    } else {
        stream.write_all(chunk).await
    }
}

async fn read_response(stream: &mut TcpStream) -> Result<Raw> {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        let read = tokio::time::timeout(Duration::from_secs(60), stream.read(&mut byte))
            .await
            .context("the response head did not arrive")??;
        ensure!(read == 1, "the connection closed before a response");
        raw.push(byte[0]);
        ensure!(raw.len() < 16_384, "an unbounded response head");
    }
    let head = String::from_utf8(raw)?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .context("status line")?
        .parse()?;
    let length: usize = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map_or(Ok(0), |(_, value)| value.trim().parse())?;
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await?;
    Ok(Raw {
        status,
        body: String::from_utf8(body)?,
    })
}

fn expect_part_invalid(raw: &Raw, reason: &str) -> Result<()> {
    ensure!(raw.status == 422, "{} {}", raw.status, raw.body);
    let body: Value = serde_json::from_str(&raw.body)?;
    ensure!(body["error"]["code"] == "MULTIPART_PART_INVALID", "{body}");
    ensure!(body["error"]["details"]["reason"] == reason, "{body}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_s3_proxied_process_uploads_through_a_checksum_demanding_provider() -> Result<()> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("proxied").await?;
    let gate = ChecksumGate::start(minio.endpoint()).await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let size = 2 * PART + 4 * MIB + 321;
    let expected = generated_digest(size)?;
    let env = storage_env(&minio, &gate, &bucket);

    let mut process = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    process.wait_ready().await?;
    let world = World::open(&process, &data_dir, None).await?;

    let ready = world.ready().await?;
    assert_eq!(ready["dataPlane"], "proxied", "{ready}");
    assert_eq!(ready["proxyCeilingBytes"], CEILING);
    assert!(ready.get("storage").is_some(), "{ready}");
    let logs = process.log_text()?;
    let warnings: Vec<Value> = logs
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["data_plane"] == "proxied")
        .collect();
    assert_eq!(warnings.len(), 1, "{logs}");
    assert_eq!(warnings[0]["level"], "WARN");
    assert_eq!(warnings[0]["max_object_bytes"], CEILING);
    assert_eq!(warnings[0]["checksum_requirement"], "operator");
    for secret in [minio.secret_key(), minio.access_key(), "X-Amz-Signature"] {
        assert!(!logs.contains(secret), "{secret} leaked into the logs");
    }

    let baseline = gate.forwarded_checksums().len();
    assert_eq!(
        baseline, 3,
        "the startup self-test sent its three probe parts with checksums"
    );
    let (session, item) = world.plan("proxied.bin", size).await?;
    let created = world
        .call(Method::POST, &World::route(&session, &item, ""), None)
        .await?
        .expect(StatusCode::CREATED)?
        .json()?;
    assert_eq!(created["checksumHeadersRequired"], true);
    assert_eq!(created["partSizeBytes"], PART);
    assert_eq!(created["partCount"], 3);
    let presign = world
        .call(
            Method::POST,
            &World::route(&session, &item, "/presign"),
            Some(json!({ "partNumbers": [1] })),
        )
        .await?;
    assert_eq!(
        presign.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        presign.body
    );

    let first = bytes_of(size, 0, PART)?;
    let second = bytes_of(size, PART, 2 * PART)?;
    let third = bytes_of(size, 2 * PART, size)?;

    let wrong = world
        .put(
            &session,
            &item,
            1,
            Framing::Declared(1_000, &first[..1_000]),
        )
        .await?;
    expect_part_invalid(&wrong, "content_length_mismatch")?;
    let short_chunked = world
        .put(
            &session,
            &item,
            1,
            Framing::Chunked(&first[..first.len() - 1], 1 << 20),
        )
        .await?;
    expect_part_invalid(&short_chunked, "body_too_short")?;
    let mut overlong = first.clone();
    overlong.push(0);
    let long_chunked = world
        .put(&session, &item, 1, Framing::Chunked(&overlong, 1 << 20))
        .await?;
    expect_part_invalid(&long_chunked, "body_too_long")?;
    assert_eq!(
        gate.forwarded_checksums().len(),
        baseline,
        "no malformed part reached the provider"
    );

    for round in 0..10 {
        world
            .abandon(&session, &item, 3, &third[..1_000 + round])
            .await?;
    }
    wait_until("abandoned requests to finish", || async {
        Ok(world
            .db
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts WHERE state = 'uploaded'")
            .await?
            == 0)
    })
    .await?;
    assert_eq!(
        gate.forwarded_checksums().len(),
        baseline,
        "a client that disconnects mid-part sends nothing to the provider"
    );

    let stored = world
        .put(&session, &item, 1, Framing::Length(&first))
        .await?;
    assert_eq!(stored.status, 200, "{}", stored.body);
    let view: Value = serde_json::from_str(&stored.body)?;
    assert_eq!(view["partNumber"], 1);
    assert_eq!(view["sizeBytes"], PART);
    let chunked = world
        .put(&session, &item, 2, Framing::Chunked(&second, 1 << 20))
        .await?;
    assert_eq!(chunked.status, 200, "{}", chunked.body);
    assert_eq!(
        gate.forwarded_checksums()[baseline..],
        [checksum(&first), checksum(&second)],
        "the provider received the SHA-256 of the exact bytes of each part"
    );

    process.stop().await?;
    let mut restarted = ServerProcess::spawn_with(&data_dir, &borrowed(&env))?;
    restarted.wait_ready().await?;
    let world = World::open(&restarted, &data_dir, Some(&world)).await?;
    let after_restart = gate.forwarded_checksums().len();
    let ready = world.ready().await?;
    assert_eq!(ready["dataPlane"], "proxied", "the mode survives a restart");

    let listed = world
        .call(Method::GET, &World::route(&session, &item, "/parts"), None)
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(listed["missingPartNumbers"], json!([3]), "{listed}");
    assert_eq!(
        listed["partSizeBytes"], PART,
        "the part plan is not rewritten"
    );
    assert_eq!(listed["partCount"], 3);
    let stored = world
        .put(&session, &item, 3, Framing::Length(&third))
        .await?;
    assert_eq!(stored.status, 200, "{}", stored.body);

    let completed = world
        .call(
            Method::POST,
            &World::route(&session, &item, "/complete"),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(completed["sizeBytes"], size);
    let key = world
        .db
        .scalar_string("SELECT final_object_key FROM transfer_session_files")
        .await?;
    let object = minio.read_object(&bucket, &key).await?;
    assert_eq!(object.len() as u64, size);
    assert_eq!(Sha256::digest(&object).to_vec(), expected);
    assert_eq!(world.counts().await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(gate.forwarded_checksums().len(), after_restart + 1);
    assert_eq!(gate.forwarded_checksums()[after_restart], checksum(&third));
    assert!(
        gate.rejected() >= 1,
        "the startup probe's browser-shaped part was refused, which is why the mode is proxied"
    );

    let rejected_at_admission = world
        .call(
            Method::POST,
            SESSIONS,
            Some(json!({
                "target": { "kind": "my_files", "folderId": null },
                "files": [{ "clientId": "c2", "name": "huge.bin", "sizeBytes": CEILING + 1 }],
            })),
        )
        .await?;
    assert_eq!(rejected_at_admission.status, StatusCode::PAYLOAD_TOO_LARGE);
    let body = rejected_at_admission.json()?;
    assert_eq!(body["error"]["details"]["reason"], "proxy_part_count");
    assert_eq!(body["error"]["details"]["providerMaxObjectBytes"], CEILING);

    restarted.stop().await?;
    Ok(())
}
