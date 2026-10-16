#![allow(
    dead_code,
    reason = "each download test binary uses a different subset of these helpers"
)]

use std::path::Path;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::header::{HeaderMap, COOKIE};
use reqwest::redirect::Policy;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;
use uuid::Uuid;

use super::client::{Creds, Db, Http, EPOCH};
use super::tus::{generated, send_range, TusWorld, MIB, SESSIONS};

pub struct Fetch {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Fetch {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn require(&self, name: &str) -> Result<&str> {
        self.header(name)
            .with_context(|| format!("the response has no {name} header"))
    }

    pub fn error_code(&self) -> Result<String> {
        let envelope: Value = serde_json::from_slice(&self.body)
            .with_context(|| format!("not an error envelope: {:?}", self.text()))?;
        Ok(envelope["error"]["code"]
            .as_str()
            .context("the envelope has no code")?
            .to_owned())
    }

    pub fn envelope(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).context("parse the error envelope")
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn sha256(&self) -> Vec<u8> {
        Sha256::digest(&self.body).to_vec()
    }
}

#[derive(Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    base: Url,
}

impl Fetcher {
    pub fn new(base: &Url) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(120))
                .build()?,
            base: base.clone(),
        })
    }

    pub fn request(
        &self,
        method: Method,
        path: &str,
        creds: Option<&Creds>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::RequestBuilder> {
        let mut request = self
            .client
            .request(method, self.base.join(path.trim_start_matches('/'))?);
        if let Some(creds) = creds {
            request = request.header(
                COOKIE,
                format!("palmr_session={}; palmr_csrf={}", creds.session, creds.csrf),
            );
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        Ok(request)
    }

    pub async fn get(
        &self,
        path: &str,
        creds: Option<&Creds>,
        headers: &[(&str, &str)],
    ) -> Result<Fetch> {
        let response = self
            .request(Method::GET, path, creds, headers)?
            .send()
            .await
            .context("send the download request")?;
        collect(response).await
    }

    pub async fn open(
        &self,
        path: &str,
        creds: Option<&Creds>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        self.request(Method::GET, path, creds, headers)?
            .send()
            .await
            .context("send the download request")
    }
}

pub async fn collect(response: reqwest::Response) -> Result<Fetch> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.bytes().await.context("read the body")?.to_vec();
    Ok(Fetch {
        status,
        headers,
        body,
    })
}

pub fn expected_bytes(size: u64, from: u64, to: u64) -> Result<Vec<u8>> {
    let mut stream = generated(from, size)?;
    let mut bytes = vec![0_u8; usize::try_from(to - from)?];
    let mut filled = 0;
    while filled < bytes.len() {
        let read = stream
            .read(&mut bytes[filled..])
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(read > 0, "the generator ended early");
        filled += read;
    }
    Ok(bytes)
}

pub fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

pub fn content_path(file_id: &str) -> String {
    format!("/api/v1/files/{file_id}/content")
}

pub fn preview_path(file_id: &str) -> String {
    format!("/api/v1/files/{file_id}/preview")
}

pub async fn upload_local(world: &TusWorld, name: &str, size: u64) -> Result<String> {
    let upload = world.upload(name, size).await?;
    let mut patch = world.open(&upload, 0, None).await?;
    send_range(&mut patch, size, 0, size).await?;
    patch.finish().await?;
    let response = patch.response().await?;
    ensure!(response.status == 204, "{}", response.body);
    world.wait_finalized(&upload).await?;
    world
        .db
        .scalar_string(&format!(
            "SELECT t.resulting_file_id FROM transfer_session_files t
               JOIN tus_uploads u ON u.transfer_session_file_id = t.id
              WHERE u.id = '{}'",
            upload.id
        ))
        .await
}

pub struct Seeded {
    pub file_id: String,
    pub object_id: String,
    pub path: std::path::PathBuf,
}

pub async fn seed_local_file(
    db: &Db,
    data_dir: &Path,
    owner_id: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<Seeded> {
    let oid = Uuid::now_v7().simple().to_string();
    let key = format!("objects/{}/{}/{oid}", &oid[..2], &oid[2..4]);
    let path = data_dir.join("storage").join(&key);
    std::fs::create_dir_all(path.parent().context("object directory")?)?;
    std::fs::write(&path, bytes)?;
    let object_id = Uuid::now_v7().to_string();
    let file_id = Uuid::now_v7().to_string();
    let size = bytes.len().to_string();
    db.execute_bound(
        "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
            created_at, updated_at, finalized_at)
         VALUES (?1, ?2, 'local', ?3, 'active', 1, ?4, ?4, ?4)",
        &[&object_id, &key, &size, EPOCH],
    )
    .await?;
    let normalized = name.to_lowercase();
    db.execute_bound(
        "INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized,
            size_bytes, mime_type, created_at, updated_at)
         VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        &[
            &file_id,
            owner_id,
            &object_id,
            name,
            &normalized,
            &size,
            mime,
            EPOCH,
        ],
    )
    .await?;
    Ok(Seeded {
        file_id,
        object_id,
        path,
    })
}

pub async fn user_id(db: &Db, username: &str) -> Result<String> {
    db.scalar_string(&format!(
        "SELECT id FROM users WHERE username_normalized = '{username}'"
    ))
    .await
}

pub async fn upload_s3(
    http: &Http,
    db: &Db,
    creds: &Creds,
    name: &str,
    size: u64,
) -> Result<String> {
    let call = |method: Method, path: String, body: Option<Value>| async move {
        http.send(method, &path, Some(creds), body).await
    };
    let session = call(
        Method::POST,
        SESSIONS.to_owned(),
        Some(json!({
            "target": { "kind": "my_files", "folderId": null },
            "files": [{ "clientId": "c1", "name": name, "sizeBytes": size }],
        })),
    )
    .await?
    .expect(StatusCode::CREATED)?
    .json()?;
    let session = session["id"].as_str().context("session id")?.to_owned();
    let item = db
        .scalar_string(&format!(
            "SELECT id FROM transfer_session_files WHERE transfer_session_id = '{session}'
              ORDER BY created_at DESC LIMIT 1"
        ))
        .await?;
    let route = |tail: &str| format!("{SESSIONS}/{session}/files/{item}/s3/multipart{tail}");
    let created = call(Method::POST, route(""), None)
        .await?
        .expect(StatusCode::CREATED)?
        .json()?;
    let count = created["partCount"].as_u64().context("part count")?;
    let part_size = match &created["partSizeBytes"] {
        Value::Number(number) => number.as_u64().context("part size")?,
        Value::String(text) => text.parse()?,
        other => anyhow::bail!("unexpected part size {other}"),
    };
    let numbers: Vec<u64> = (1..=count).collect();
    let presigned = call(
        Method::POST,
        route("/presign"),
        Some(json!({ "partNumbers": numbers })),
    )
    .await?
    .expect(StatusCode::OK)?
    .json()?;
    let client = reqwest::Client::new();
    let mut reports = Vec::new();
    for entry in presigned["parts"].as_array().context("parts")? {
        let number = entry["partNumber"].as_u64().context("part number")?;
        let from = (number - 1) * part_size;
        let to = (number * part_size).min(size);
        let response = client
            .put(entry["url"].as_str().context("url")?)
            .body(expected_bytes(size, from, to)?)
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
    call(
        Method::POST,
        route("/parts"),
        Some(json!({ "parts": reports })),
    )
    .await?
    .expect(StatusCode::NO_CONTENT)?;
    let completed = call(Method::POST, route("/complete"), None)
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    Ok(completed["fileId"]
        .as_str()
        .context("the completed file id")?
        .to_owned())
}

pub const SMALL: u64 = MIB + 12_345;

pub struct Head {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub async fn read_head(stream: &mut TcpStream) -> Result<(Head, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await?;
        ensure!(read == 1, "the connection closed before a response head");
        raw.push(byte[0]);
        ensure!(raw.len() < 32_768, "an unbounded response head");
    }
    let text = String::from_utf8(raw)?;
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .context("status line")?
        .parse()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Ok((Head { status, headers }, Vec::new()))
}

pub async fn raw_get(base: &Url, file: &str, creds: &Creds, extra: &str) -> Result<TcpStream> {
    raw_get_path(base, &content_path(file), creds, extra).await
}

pub async fn raw_get_path(base: &Url, path: &str, creds: &Creds, extra: &str) -> Result<TcpStream> {
    let host = base.host_str().context("host")?;
    let port = base.port().context("port")?;
    let mut stream = TcpStream::connect((host, port)).await?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nCookie: palmr_session={}; palmr_csrf={}\r\n{extra}\r\n",
        creds.session,
        creds.csrf,
    );
    stream.write_all(request.as_bytes()).await?;
    Ok(stream)
}
