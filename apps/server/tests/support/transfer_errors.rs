use std::ops::Deref;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use super::client::{Http, Reply};
use super::tus::{b64, RawResponse, TusWorld, SESSIONS, TUS};
use super::TestApplication;

pub struct World {
    pub app: TestApplication,
    pub tus: TusWorld,
}

impl Deref for World {
    type Target = TusWorld;

    fn deref(&self) -> &TusWorld {
        &self.tus
    }
}

pub struct Failure {
    pub status: u16,
    pub code: String,
    pub header_id: String,
    pub envelope_id: String,
}

pub struct Upload {
    pub id: String,
    pub session: String,
    pub item: String,
}

impl World {
    pub async fn start(name: &str) -> Result<Self> {
        let app = TestApplication::start(name).await?;
        let http = Http::new(app.url("/")?)?;
        let creds = http.setup_admin().await?;
        let tus = TusWorld::new(app.url("/")?, app.data_dir().to_path_buf(), http, creds);
        Ok(Self { app, tus })
    }

    pub async fn restarted(self) -> Result<Self> {
        let Self { app, tus } = self;
        let (_drain, app) = app.restart(Duration::from_secs(10)).await?;
        let http = Http::new(app.url("/")?)?;
        let tus = TusWorld::new(app.url("/")?, tus.data_dir, http, tus.creds);
        Ok(Self { app, tus })
    }

    pub async fn plan_files(&self, files: &[(&str, Option<u64>)]) -> Result<(String, Vec<String>)> {
        let declared: Vec<Value> = files
            .iter()
            .enumerate()
            .map(|(index, (name, size))| match size {
                Some(size) => {
                    json!({ "clientId": format!("c{index}"), "name": name, "sizeBytes": size })
                }
                None => json!({ "clientId": format!("c{index}"), "name": name }),
            })
            .collect();
        let created = self
            .http
            .send(
                Method::POST,
                SESSIONS,
                Some(&self.creds),
                Some(json!({
                    "target": { "kind": "my_files", "folderId": null },
                    "files": declared,
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let session = created["id"].as_str().context("session id")?.to_owned();
        let items = created["files"]
            .as_array()
            .context("files")?
            .iter()
            .map(|file| Ok(file["itemId"].as_str().context("item id")?.to_owned()))
            .collect::<Result<Vec<_>>>()?;
        Ok((session, items))
    }

    pub async fn create_upload(
        &self,
        name: &str,
        session: &str,
        item: &str,
        length: Option<u64>,
    ) -> Result<Upload> {
        let metadata = format!(
            "filename {},transferSessionId {},itemId {}",
            b64(name),
            b64(session),
            b64(item)
        );
        let length_header = length.map(|length| length.to_string());
        let mut headers: Vec<(&str, &str)> =
            vec![("tus-resumable", "1.0.0"), ("upload-metadata", &metadata)];
        match &length_header {
            Some(length) => headers.push(("upload-length", length)),
            None => headers.push(("upload-defer-length", "1")),
        }
        let created = self
            .http
            .send_with_headers(Method::POST, TUS, Some(&self.creds), None, &headers)
            .await?
            .expect(StatusCode::CREATED)?;
        let location = created
            .headers
            .get("location")
            .context("location")?
            .to_str()?;
        Ok(Upload {
            id: location.rsplit('/').next().context("id")?.to_owned(),
            session: session.to_owned(),
            item: item.to_owned(),
        })
    }

    pub async fn planned_upload(&self, name: &str, size: u64) -> Result<Upload> {
        let (session, items) = self.plan_files(&[(name, Some(size))]).await?;
        self.create_upload(name, &session, &items[0], Some(size))
            .await
    }

    pub async fn patch(&self, upload: &Upload, offset: u64, body: Vec<u8>) -> Result<Reply> {
        let offset = offset.to_string();
        self.http
            .send_bytes(
                Method::PATCH,
                &format!("{TUS}/{}", upload.id),
                &self.creds,
                &[
                    ("tus-resumable", "1.0.0"),
                    ("upload-offset", &offset),
                    ("content-type", "application/offset+octet-stream"),
                ],
                body,
            )
            .await
    }

    pub async fn streamed_patch(
        &self,
        upload: &Upload,
        offset: u64,
        body: &[u8],
    ) -> Result<RawResponse> {
        let mut patch = self
            .open(
                &super::tus::Upload {
                    id: upload.id.clone(),
                },
                offset,
                None,
            )
            .await?;
        patch.frame(body).await?;
        patch.response().await
    }

    pub async fn complete(&self, session: &str) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                &format!("{SESSIONS}/{session}/complete"),
                Some(&self.creds),
                None,
            )
            .await
    }

    pub async fn retry(&self, session: &str, item: &str) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                &format!("{SESSIONS}/{session}/files/{item}/retry"),
                Some(&self.creds),
                None,
            )
            .await
    }

    pub async fn session(&self, session: &str) -> Result<Value> {
        self.http
            .get(&format!("{SESSIONS}/{session}"), Some(&self.creds))
            .await?
            .expect(StatusCode::OK)?
            .json()
    }

    pub async fn item_view(&self, session: &str, item: &str) -> Result<Value> {
        let view = self.session(session).await?;
        view["files"]
            .as_array()
            .context("files")?
            .iter()
            .find(|file| file["itemId"] == item)
            .cloned()
            .context("the item is listed")
    }

    pub async fn wait_item(&self, item: &str, state: &str) -> Result<()> {
        super::tus::wait_until(&format!("item {item} to be {state}"), || async {
            Ok(self
                .db
                .scalar_string(&format!(
                    "SELECT state FROM transfer_session_files WHERE id = '{item}'"
                ))
                .await?
                == state)
        })
        .await
    }

    pub async fn db_error(
        &self,
        table: &str,
        id: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        let code = self
            .db
            .scalar_string(&format!(
                "SELECT COALESCE(error_code, '') FROM {table} WHERE id = '{id}'"
            ))
            .await?;
        let request = self
            .db
            .scalar_string(&format!(
                "SELECT COALESCE(error_request_id, '') FROM {table} WHERE id = '{id}'"
            ))
            .await?;
        Ok((
            (!code.is_empty()).then_some(code),
            (!request.is_empty()).then_some(request),
        ))
    }
}

pub fn failure_of(response: &RawResponse) -> Result<Failure> {
    let envelope: Value =
        serde_json::from_str(&response.body).with_context(|| format!("parse {}", response.body))?;
    Ok(Failure {
        status: response.status,
        code: envelope["error"]["code"]
            .as_str()
            .with_context(|| format!("error code in {}", response.body))?
            .to_owned(),
        envelope_id: envelope["error"]["requestId"]
            .as_str()
            .context("error requestId")?
            .to_owned(),
        header_id: response
            .headers
            .get("x-request-id")
            .context("the response carries X-Request-Id")?
            .clone(),
    })
}

pub fn failure(reply: &Reply) -> Result<Failure> {
    let envelope = reply.json()?;
    let header_id = reply
        .headers
        .get("x-request-id")
        .context("the response carries X-Request-Id")?
        .to_str()?
        .to_owned();
    Ok(Failure {
        status: reply.status.as_u16(),
        code: envelope["error"]["code"]
            .as_str()
            .with_context(|| format!("error code in {} {}", reply.status, reply.body))?
            .to_owned(),
        envelope_id: envelope["error"]["requestId"]
            .as_str()
            .with_context(|| format!("error requestId in {}", reply.body))?
            .to_owned(),
        header_id,
    })
}

pub fn blob_of(world: &World, upload: &Upload) -> std::path::PathBuf {
    world
        .data_dir
        .join("uploads")
        .join(upload.id.replace('-', ""))
        .join("blob")
}

impl Failure {
    pub fn expect(&self, status: u16, code: &str) -> Result<()> {
        ensure!(
            self.status == status && self.code == code,
            "expected {status} {code}, got {} {}",
            self.status,
            self.code
        );
        ensure!(
            self.header_id == self.envelope_id,
            "X-Request-Id {} differs from error.requestId {}",
            self.header_id,
            self.envelope_id
        );
        Ok(())
    }
}

pub fn error_of(view: &Value) -> (Option<String>, Option<String>) {
    let error = &view["error"];
    (
        error["code"].as_str().map(str::to_owned),
        error["requestId"].as_str().map(str::to_owned),
    )
}
