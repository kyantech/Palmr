use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context, Result};
use base64ct::{Base64UrlUnpadded, Encoding};
use reqwest::header::{HeaderMap, CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use url::Url;
use uuid::Uuid;

pub const PASSWORD: &str = "correct horse battery staple";
const SESSION_COOKIE: &str = "palmr_session";
const CSRF_COOKIE: &str = "palmr_csrf";
const CSRF_HEADER: &str = "X-Palmr-CSRF";

#[derive(Debug, Clone)]
pub struct Creds {
    pub session: String,
    pub csrf: String,
}

pub struct Reply {
    pub status: StatusCode,
    pub body: String,
    pub headers: HeaderMap,
    cookies: Vec<(String, String)>,
}

impl Reply {
    pub fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).with_context(|| format!("parse body {:?}", self.body))
    }

    pub fn error_code(&self) -> Result<String> {
        Ok(self.json()?["error"]["code"]
            .as_str()
            .with_context(|| format!("error envelope has no code: {}", self.body))?
            .to_owned())
    }

    pub fn creds(&self) -> Result<Creds> {
        let find = |name: &str| {
            self.cookies
                .iter()
                .rev()
                .find(|(cookie, value)| cookie == name && !value.is_empty())
                .map(|(_, value)| value.clone())
        };
        match (find(SESSION_COOKIE), find(CSRF_COOKIE)) {
            (Some(session), Some(csrf)) => Ok(Creds { session, csrf }),
            _ => Err(anyhow!(
                "reply carries no session cookie pair: {}",
                self.body
            )),
        }
    }

    pub fn expect(self, status: StatusCode) -> Result<Self> {
        ensure!(
            self.status == status,
            "expected {status}, got {} with body {}",
            self.status,
            self.body
        );
        Ok(self)
    }
}

pub struct Http {
    client: Client,
    base: Url,
    origin: String,
}

impl Http {
    pub fn new(base: Url) -> Result<Self> {
        Ok(Self {
            client: Client::builder().build().context("build HTTP client")?,
            origin: base.origin().ascii_serialization(),
            base,
        })
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        creds: Option<&Creds>,
        body: Option<Value>,
    ) -> Result<Reply> {
        self.send_with_headers(method, path, creds, body, &[]).await
    }

    pub async fn send_with_headers(
        &self,
        method: Method,
        path: &str,
        creds: Option<&Creds>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<Reply> {
        let proof = creds.map(|creds| {
            (
                format!(
                    "{SESSION_COOKIE}={}; {CSRF_COOKIE}={}",
                    creds.session, creds.csrf
                ),
                creds.csrf.clone(),
            )
        });
        self.dispatch(method, path, proof, body, headers).await
    }

    pub async fn send_with_anonymous_csrf(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Reply> {
        let token = csrf_token();
        self.dispatch(
            method,
            path,
            Some((format!("{CSRF_COOKIE}={token}"), token)),
            body,
            &[],
        )
        .await
    }

    async fn dispatch(
        &self,
        method: Method,
        path: &str,
        proof: Option<(String, String)>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Result<Reply> {
        let url = self.base.join(path.trim_start_matches('/'))?;
        let mut request = self
            .client
            .request(method, url)
            .header(ORIGIN, &self.origin);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some((cookie, csrf)) = proof {
            request = request.header(COOKIE, cookie).header(CSRF_HEADER, csrf);
        }
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let response = request.send().await.context("send request")?;
        let status = response.status();
        let headers = response.headers().clone();
        let cookies = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        let body = response.text().await.context("read response body")?;
        Ok(Reply {
            status,
            body,
            headers,
            cookies,
        })
    }

    pub async fn get(&self, path: &str, creds: Option<&Creds>) -> Result<Reply> {
        self.send(Method::GET, path, creds, None).await
    }

    pub async fn setup_admin(&self) -> Result<Creds> {
        self.send(
            Method::POST,
            "/api/v1/setup",
            None,
            Some(json!({
                "appName": "Palmr",
                "firstName": "Ada",
                "lastName": "Lovelace",
                "username": "ada",
                "email": "ada@example.test",
                "password": PASSWORD,
                "locale": "en-US",
            })),
        )
        .await?
        .expect(StatusCode::CREATED)?
        .creds()
    }

    pub async fn invite_and_accept(&self, admin: &Creds, username: &str) -> Result<Creds> {
        let invite = self
            .send(
                Method::POST,
                "/api/v1/admin/invites",
                Some(admin),
                Some(json!({
                    "email": format!("{username}@example.test"),
                    "role": "user",
                    "sendEmail": false,
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let url = invite["inviteUrl"].as_str().context("invite url")?;
        let token = url.rsplit('/').next().context("invite token")?;
        self.send(
            Method::POST,
            &format!("/api/v1/public/invites/{token}/accept"),
            None,
            Some(json!({
                "firstName": "Regular",
                "lastName": username,
                "username": username,
                "password": PASSWORD,
                "locale": "en-US",
            })),
        )
        .await?
        .expect(StatusCode::CREATED)?
        .creds()
    }
}

pub struct Db {
    path: PathBuf,
}

impl Db {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self {
            path: data_dir.join("palmr.db"),
        }
    }

    pub async fn writer(&self) -> Result<SqliteConnection> {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.path)
                .busy_timeout(Duration::from_secs(10)),
        )
        .await
        .context("open write connection")
    }

    pub async fn execute(&self, sql: &str) -> Result<()> {
        let mut connection = self.writer().await?;
        sqlx::query(sql)
            .execute(&mut connection)
            .await
            .with_context(|| format!("execute {sql}"))?;
        Ok(())
    }

    pub async fn scalar_string(&self, sql: &str) -> Result<String> {
        let mut connection = self.writer().await?;
        sqlx::query_scalar(sql)
            .fetch_one(&mut connection)
            .await
            .with_context(|| format!("query {sql}"))
    }

    pub async fn scalar_i64(&self, sql: &str) -> Result<i64> {
        let mut connection = self.writer().await?;
        sqlx::query_scalar(sql)
            .fetch_one(&mut connection)
            .await
            .with_context(|| format!("query {sql}"))
    }
}

pub const EPOCH: &str = "2026-01-01T00:00:00.000Z";
pub const FAR_FUTURE: &str = "2030-01-01T00:00:00.000Z";

pub struct SessionSpec<'a> {
    pub user_id: &'a str,
    pub state: &'a str,
    pub last_seen_at: &'a str,
    pub idle_expires_at: &'a str,
    pub absolute_expires_at: &'a str,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub auth_method: &'a str,
}

impl<'a> SessionSpec<'a> {
    pub fn active(user_id: &'a str) -> Self {
        Self {
            user_id,
            state: "active",
            last_seen_at: EPOCH,
            idle_expires_at: FAR_FUTURE,
            absolute_expires_at: FAR_FUTURE,
            ip: None,
            user_agent: None,
            auth_method: "password",
        }
    }
}

pub fn v7(counter: u64) -> String {
    format!("0192f3a1-0000-7000-8000-{counter:012x}")
}

pub fn csrf_token() -> String {
    random_token().0
}

fn random_token() -> (String, String) {
    let bytes = [
        Uuid::now_v7().as_bytes().as_slice(),
        Uuid::now_v7().as_bytes().as_slice(),
    ]
    .concat();
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (Base64UrlUnpadded::encode_string(&bytes), digest)
}

impl Db {
    pub async fn insert_session(&self, spec: &SessionSpec<'_>) -> Result<(String, Creds)> {
        let id = Uuid::now_v7().to_string();
        let (session, session_hash) = random_token();
        let (csrf, csrf_hash) = random_token();
        let revoked = spec.state == "revoked";
        let mfa = spec.state == "mfa_pending";
        let (mfa_hash, mfa_expires) = if mfa {
            (Some(random_token().1), Some(FAR_FUTURE))
        } else {
            (None, None)
        };
        let mut connection = self.writer().await?;
        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, csrf_token_hash, state, auth_method,
                mfa_token_hash, mfa_expires_at, created_at, last_seen_at, last_auth_at,
                idle_expires_at, absolute_expires_at, revoked_at, revoked_reason, ip, user_agent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12, ?13, ?14, ?15, ?16)",
        )
        .bind(&id)
        .bind(spec.user_id)
        .bind(session_hash)
        .bind(csrf_hash)
        .bind(spec.state)
        .bind(spec.auth_method)
        .bind(mfa_hash)
        .bind(mfa_expires)
        .bind(EPOCH)
        .bind(spec.last_seen_at)
        .bind(spec.idle_expires_at)
        .bind(spec.absolute_expires_at)
        .bind(revoked.then_some(EPOCH))
        .bind(revoked.then_some("admin_request"))
        .bind(spec.ip)
        .bind(spec.user_agent)
        .execute(&mut connection)
        .await
        .context("insert session")?;
        Ok((id, Creds { session, csrf }))
    }

    pub async fn insert_user(&self, id: &str, username: &str, role: &str) -> Result<()> {
        self.execute_bound(
            "INSERT INTO users (id, email, email_normalized, username, username_normalized,
                role, created_at, updated_at)
             VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?5, ?5)",
            &[
                id,
                &format!("{username}@example.test"),
                username,
                role,
                EPOCH,
            ],
        )
        .await
    }

    pub async fn execute_bound(&self, sql: &str, binds: &[&str]) -> Result<()> {
        let mut connection = self.writer().await?;
        let mut query = sqlx::query(sql);
        for bind in binds {
            query = query.bind(*bind);
        }
        query
            .execute(&mut connection)
            .await
            .with_context(|| format!("execute {sql}"))?;
        Ok(())
    }
}
