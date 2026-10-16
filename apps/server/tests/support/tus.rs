use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use base64ct::{Base64, Encoding};
use palmr_stress::gen::{GeneratedStream, Seed};
use palmr_stress::manifest::Manifest;
use reqwest::{Method, StatusCode};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use super::client::{Creds, Db, Http};

pub const SESSIONS: &str = "/api/v1/transfers/sessions";
pub const TUS: &str = "/api/v1/uploads/tus";
pub const MIB: u64 = 1024 * 1024;
pub const SEED: &str = "0000000000000000000000000000000000000000000000000000000000000001";

pub struct TusWorld {
    pub http: Http,
    pub db: Db,
    pub creds: Creds,
    pub base: Url,
    pub data_dir: PathBuf,
}

pub struct Upload {
    pub id: String,
}

pub struct RawPatch {
    pub stream: TcpStream,
}

pub struct RawResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

pub fn seed() -> Result<Seed> {
    Seed::from_hex(SEED).context("seed")
}

pub fn b64(text: &str) -> String {
    Base64::encode_string(text.as_bytes())
}

pub fn digest_header(digest: &[u8]) -> String {
    format!("sha256 {}", Base64::encode_string(digest))
}

impl TusWorld {
    pub fn new(base: Url, data_dir: PathBuf, http: Http, creds: Creds) -> Self {
        let db = Db::new(&data_dir);
        Self {
            http,
            db,
            creds,
            base,
            data_dir,
        }
    }

    pub async fn plan(&self, name: &str, length: u64) -> Result<(String, String)> {
        let session = self
            .http
            .send(
                Method::POST,
                SESSIONS,
                Some(&self.creds),
                Some(json!({
                    "target": { "kind": "my_files", "folderId": null },
                    "files": [{ "clientId": "c1", "name": name, "sizeBytes": length }],
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

    fn metadata(name: &str, session: &str, item: &str) -> String {
        format!(
            "filename {},transferSessionId {},itemId {}",
            b64(name),
            b64(session),
            b64(item)
        )
    }

    pub async fn upload(&self, name: &str, length: u64) -> Result<Upload> {
        let (session, item) = self.plan(name, length).await?;
        let metadata = Self::metadata(name, &session, &item);
        let created = self
            .http
            .send_with_headers(
                Method::POST,
                TUS,
                Some(&self.creds),
                None,
                &[
                    ("tus-resumable", "1.0.0"),
                    ("upload-length", &length.to_string()),
                    ("upload-metadata", &metadata),
                ],
            )
            .await?
            .expect(StatusCode::CREATED)?;
        let location = created
            .headers
            .get("location")
            .context("location")?
            .to_str()?;
        Ok(Upload {
            id: location.rsplit('/').next().context("id")?.to_owned(),
        })
    }

    pub async fn create_with_body(&self, name: &str, length: u64) -> Result<Upload> {
        let (session, item) = self.plan(name, length).await?;
        let base = self.base.clone();
        let host = base.host_str().context("host")?;
        let port = base.port().context("port")?;
        let mut stream = TcpStream::connect((host, port)).await?;
        let head = format!(
            "POST {TUS} HTTP/1.1\r\nHost: {host}:{port}\r\nOrigin: {origin}\r\n\
             Cookie: palmr_session={session}; palmr_csrf={csrf}\r\nX-Palmr-CSRF: {csrf}\r\n\
             Tus-Resumable: 1.0.0\r\nUpload-Length: {length}\r\nUpload-Metadata: {metadata}\r\n\
             Content-Type: application/offset+octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
            origin = base.origin().ascii_serialization(),
            session = self.creds.session,
            csrf = self.creds.csrf,
            metadata = Self::metadata(name, &session, &item),
        );
        stream.write_all(head.as_bytes()).await?;
        let mut request = RawPatch { stream };
        send_range(&mut request, length, 0, length).await?;
        request.finish().await?;
        let response = request.response().await?;
        ensure!(response.status == 201, "{}", response.body);
        ensure!(
            response.headers.get("upload-offset").map(String::as_str) == Some(&*length.to_string()),
            "the creation body was not fully acknowledged"
        );
        let location = response.headers.get("location").context("location")?;
        Ok(Upload {
            id: location.rsplit('/').next().context("id")?.to_owned(),
        })
    }

    pub async fn offset(&self, upload: &Upload) -> Result<u64> {
        let value = self
            .db
            .scalar_i64(&format!(
                "SELECT upload_offset FROM tus_uploads WHERE id = '{}'",
                upload.id
            ))
            .await?;
        Ok(u64::try_from(value)?)
    }

    pub async fn head(&self, upload: &Upload) -> Result<u64> {
        let reply = self
            .http
            .send_with_headers(
                Method::HEAD,
                &format!("{TUS}/{}", upload.id),
                Some(&self.creds),
                None,
                &[("tus-resumable", "1.0.0")],
            )
            .await?
            .expect(StatusCode::OK)?;
        Ok(reply
            .headers
            .get("upload-offset")
            .context("upload-offset")?
            .to_str()?
            .parse()?)
    }

    pub async fn locked(&self, upload: &Upload) -> Result<bool> {
        Ok(self
            .db
            .scalar_i64(&format!(
                "SELECT locked_by IS NOT NULL FROM tus_uploads WHERE id = '{}'",
                upload.id
            ))
            .await?
            == 1)
    }

    pub fn blob(&self, upload: &Upload) -> PathBuf {
        self.data_dir
            .join("uploads")
            .join(upload.id.replace('-', ""))
            .join("blob")
    }

    pub async fn open(
        &self,
        upload: &Upload,
        offset: u64,
        checksum: Option<&str>,
    ) -> Result<RawPatch> {
        let base = self.base.clone();
        let host = base.host_str().context("host")?;
        let port = base.port().context("port")?;
        let mut stream = TcpStream::connect((host, port)).await?;
        let mut head = format!(
            "PATCH {TUS}/{id} HTTP/1.1\r\nHost: {host}:{port}\r\nOrigin: {origin}\r\n\
             Cookie: palmr_session={session}; palmr_csrf={csrf}\r\nX-Palmr-CSRF: {csrf}\r\n\
             Tus-Resumable: 1.0.0\r\nUpload-Offset: {offset}\r\n\
             Content-Type: application/offset+octet-stream\r\nTransfer-Encoding: chunked\r\n",
            id = upload.id,
            origin = base.origin().ascii_serialization(),
            session = self.creds.session,
            csrf = self.creds.csrf,
        );
        if let Some(checksum) = checksum {
            head.push_str(&format!("Upload-Checksum: {checksum}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await?;
        Ok(RawPatch { stream })
    }

    pub async fn final_path(&self, upload: &Upload) -> Result<PathBuf> {
        let key = self
            .db
            .scalar_string(&format!(
                "SELECT t.final_object_key FROM transfer_session_files t
                   JOIN tus_uploads u ON u.transfer_session_file_id = t.id
                  WHERE u.id = '{}'",
                upload.id
            ))
            .await?;
        Ok(self.data_dir.join("storage").join(key))
    }

    pub async fn item_state(&self, upload: &Upload) -> Result<String> {
        self.db
            .scalar_string(&format!(
                "SELECT t.state FROM transfer_session_files t
                   JOIN tus_uploads u ON u.transfer_session_file_id = t.id
                  WHERE u.id = '{}'",
                upload.id
            ))
            .await
    }

    pub async fn wait_finalized(&self, upload: &Upload) -> Result<()> {
        wait_until("the upload to be finalized", || async {
            Ok(self.item_state(upload).await? == "completed")
        })
        .await
    }

    pub async fn blob_digest(&self, upload: &Upload) -> Result<(u64, Vec<u8>)> {
        let mut file = match std::fs::File::open(self.blob(upload)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.wait_finalized(upload).await?;
                std::fs::File::open(self.final_path(upload).await?)?
            }
            Err(error) => return Err(error.into()),
        };
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; MIB as usize];
        let mut total = 0_u64;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                return Ok((total, hasher.finalize().to_vec()));
            }
            hasher.update(&buffer[..read]);
            total += read as u64;
        }
    }
}

impl RawPatch {
    pub async fn frame(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.stream
            .write_all(format!("{:x}\r\n", data.len()).as_bytes())
            .await?;
        self.stream.write_all(data).await?;
        self.stream.write_all(b"\r\n").await?;
        Ok(())
    }

    pub async fn finish(&mut self) -> Result<()> {
        self.stream.write_all(b"0\r\n\r\n").await?;
        Ok(())
    }

    pub async fn response(&mut self) -> Result<RawResponse> {
        let mut raw = Vec::new();
        let mut byte = [0_u8; 1];
        while !raw.ends_with(b"\r\n\r\n") {
            let read = tokio::time::timeout(Duration::from_secs(30), self.stream.read(&mut byte))
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
        let headers: BTreeMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let length: usize = headers
            .get("content-length")
            .map_or(Ok(0), |value| value.parse())?;
        let mut body = vec![0_u8; length];
        self.stream.read_exact(&mut body).await?;
        Ok(RawResponse {
            status,
            headers,
            body: String::from_utf8(body)?,
        })
    }
}

pub fn generated(offset: u64, size: u64) -> Result<GeneratedStream> {
    GeneratedStream::at_offset(&seed()?, size, offset).map_err(|error| anyhow::anyhow!("{error}"))
}

pub fn generated_digest(size: u64) -> Result<Vec<u8>> {
    let mut whole = Sha256::new();
    let mut reference = generated(0, size)?;
    let mut buffer = vec![0_u8; MIB as usize];
    loop {
        let read = reference
            .read(&mut buffer)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        if read == 0 {
            return Ok(whole.finalize().to_vec());
        }
        whole.update(&buffer[..read]);
    }
}

pub async fn send_range(patch: &mut RawPatch, size: u64, from: u64, to: u64) -> Result<()> {
    let mut stream = generated(from, size)?;
    let mut buffer = vec![0_u8; MIB as usize];
    let mut remaining = to - from;
    while remaining > 0 {
        let want = usize::try_from(remaining.min(MIB))?;
        let read = stream
            .read(&mut buffer[..want])
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(read > 0, "the generator ended early");
        patch.frame(&buffer[..read]).await?;
        remaining -= read as u64;
    }
    Ok(())
}

pub async fn wait_until<F, Fut>(what: &str, mut check: F) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    for _ in 0..3_000 {
        if check().await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bail!("timed out waiting for {what}")
}

pub fn expected_digest(size: u64) -> Result<Vec<u8>> {
    let manifest = Manifest::load_default().map_err(|error| anyhow::anyhow!("{error}"))?;
    let entry = manifest
        .entries
        .iter()
        .find(|entry| entry.size == size && entry.seed.to_hex() == SEED)
        .context("the manifest has no entry of this size")?;
    Ok(entry.sha256.as_bytes().to_vec())
}
