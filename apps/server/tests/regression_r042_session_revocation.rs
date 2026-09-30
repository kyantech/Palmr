pub mod support;

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context, Result};
use base64ct::{Base64UrlUnpadded, Encoding};
use hmac::{Hmac, KeyInit, Mac};
use palmr_server::lifecycle::Drain;
use palmr_server::Clock;
use reqwest::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use url::Url;
use uuid::Uuid;

use support::TestApplication;

const PASSWORD: &str = "correct horse battery staple";
const REPLACEMENT: &str = "another correct horse staple";
const SESSION_COOKIE: &str = "palmr_session";
const CSRF_COOKIE: &str = "palmr_csrf";
const CSRF_HEADER: &str = "X-Palmr-CSRF";
const TOTP_STEP: u64 = 30;
const ME: &str = "/api/v1/auth/me";
const SESSIONS: &str = "/api/v1/sessions";
const DEVICES: &str = "/api/v1/auth/trusted-devices";
const PASSWORD_ROUTE: &str = "/api/v1/profile/password";
const ENROLL: &str = "/api/v1/auth/2fa/enroll";
const VERIFY: &str = "/api/v1/auth/2fa/enroll/verify";
const DISABLE: &str = "/api/v1/auth/2fa/disable";
const REGENERATE: &str = "/api/v1/auth/2fa/backup-codes/regenerate";
const RESET: &str = "/api/v1/auth/password/reset";
const VERIFY_EMAIL: &str = "/api/v1/auth/email/verify";
const LOGIN: &str = "/api/v1/auth/login";
const LOGIN_TOTP: &str = "/api/v1/auth/login/totp";
const LOGOUT: &str = "/api/v1/auth/logout";
const PASSWORD_CHANGED: &str = "password_changed";
const PASSWORD_RESET: &str = "password_reset";
const POLICY_CHANGED: &str = "policy_changed";
const USER_REQUEST: &str = "user_request";
const LOGGED_OUT: &str = "logout";
const ROLE_CHANGED: &str = "role_changed";
const DEACTIVATED: &str = "deactivated";
const ADMIN_REQUEST: &str = "admin_request";
const ADMIN_USERS: &str = "/api/v1/admin/users";

#[derive(Debug, Clone, Copy)]
enum Fate {
    Kept,
    Rotated,
    Revoked(&'static str),
}

#[derive(Debug, Clone)]
struct Creds {
    session: String,
    csrf: String,
}

struct Reply {
    status: StatusCode,
    cookies: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .rev()
            .find(|(cookie, _)| cookie == name)
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).with_context(|| format!("parse body {:?}", self.body))
    }

    fn creds(&self) -> Result<Creds> {
        let session = self
            .cookie(SESSION_COOKIE)
            .filter(|value| !value.is_empty());
        let csrf = self.cookie(CSRF_COOKIE).filter(|value| !value.is_empty());
        match (session, csrf) {
            (Some(session), Some(csrf)) => Ok(Creds {
                session: session.to_owned(),
                csrf: csrf.to_owned(),
            }),
            _ => Err(anyhow!(
                "reply carries no session cookie pair: {}",
                self.body
            )),
        }
    }

    fn expect(self, status: StatusCode) -> Result<Self> {
        ensure!(
            self.status == status,
            "expected {status}, got {} with body {}",
            self.status,
            self.body
        );
        Ok(self)
    }

    fn expect_success(self) -> Result<Self> {
        ensure!(
            self.status.is_success(),
            "expected success, got {} with body {}",
            self.status,
            self.body
        );
        Ok(self)
    }

    fn error_code(&self) -> Result<String> {
        Ok(self.json()?["error"]["code"]
            .as_str()
            .context("error envelope has no code")?
            .to_owned())
    }

    fn assert_cleared(&self) -> Result<()> {
        expect_eq(
            self.cookie(SESSION_COOKIE),
            Some(""),
            "session cookie cleared",
        )?;
        expect_eq(self.cookie(CSRF_COOKIE), Some(""), "csrf cookie cleared")
    }

    fn assert_no_session_cookie(&self) -> Result<()> {
        expect_eq(
            self.cookie(SESSION_COOKIE),
            None,
            "no session cookie emitted",
        )?;
        expect_eq(self.cookie(CSRF_COOKIE), None, "no csrf cookie emitted")
    }
}

fn expect_eq<T: PartialEq + Debug>(actual: T, expected: T, what: &str) -> Result<()> {
    ensure!(
        actual == expected,
        "{what}: expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest_of_cookie(cookie: &str) -> Result<String> {
    let bytes = Base64UrlUnpadded::decode_vec(cookie)
        .map_err(|error| anyhow!("decode cookie token: {error}"))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

fn base32_decode(text: &str) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for character in text.bytes().filter(|byte| *byte != b'=') {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'a'..=b'z' => character - b'a',
            b'2'..=b'7' => character - b'2' + 26,
            _ => return Err(anyhow!("invalid base32 character")),
        };
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push(u8::try_from((buffer >> bits) & 0xff)?);
        }
    }
    Ok(output)
}

fn totp_code(secret: &[u8], step: u64) -> Result<String> {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret)
        .map_err(|error| anyhow!("totp key: {error}"))?;
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    Ok(format!("{:06}", binary % 1_000_000))
}

struct Http {
    client: Client,
    base: Url,
    origin: String,
}

impl Http {
    fn new(base: Url) -> Result<Self> {
        Ok(Self {
            client: Client::builder().build().context("build HTTP client")?,
            origin: base.origin().ascii_serialization(),
            base,
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        session: Option<&Creds>,
        device: Option<&str>,
        body: Option<Value>,
    ) -> Result<Reply> {
        let url = self.base.join(path.trim_start_matches('/'))?;
        let mut request = self
            .client
            .request(method, url)
            .header(ORIGIN, &self.origin);
        let mut cookies = Vec::new();
        if let Some(creds) = session {
            cookies.push(format!("{SESSION_COOKIE}={}", creds.session));
            cookies.push(format!("{CSRF_COOKIE}={}", creds.csrf));
            request = request.header(CSRF_HEADER, &creds.csrf);
        }
        if let Some(device) = device {
            cookies.push(format!("palmr_device={device}"));
        }
        if !cookies.is_empty() {
            request = request.header(COOKIE, cookies.join("; "));
        }
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let response = request.send().await.context("send request")?;
        let status = response.status();
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
            cookies,
            body,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SessionRow {
    id: String,
    user_id: String,
    token_hash: String,
    csrf_token_hash: String,
    state: String,
    revoked_at: Option<String>,
    revoked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct DeviceRow {
    id: String,
    user_id: String,
    token_hash: String,
    revoked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct UserRow {
    id: String,
    password_hash: Option<String>,
    must_change_password: i64,
    totp_enabled: i64,
}

type EmailRow = (String, String, String, Option<String>, Option<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    sessions: BTreeMap<String, SessionRow>,
    devices: BTreeMap<String, DeviceRow>,
    users: BTreeMap<String, UserRow>,
    totp_secrets: Vec<(String, String, Option<i64>)>,
    backup_codes: Vec<(String, i64, i64)>,
    reset_tokens: Vec<(String, Option<String>, Option<String>)>,
    emails: Vec<EmailRow>,
    email_verifications: Vec<(String, Option<String>, Option<String>)>,
}

struct Db {
    path: PathBuf,
}

impl Db {
    async fn reader(&self) -> Result<SqliteConnection> {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.path)
                .read_only(true),
        )
        .await
        .context("open read connection")
    }

    async fn writer(&self) -> Result<SqliteConnection> {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.path)
                .busy_timeout(Duration::from_secs(10)),
        )
        .await
        .context("open write connection")
    }

    async fn execute(&self, sql: &str) -> Result<()> {
        let mut connection = self.writer().await?;
        sqlx::query(sql)
            .execute(&mut connection)
            .await
            .with_context(|| format!("execute {sql}"))?;
        Ok(())
    }

    async fn scalar_text(&self, sql: &str, bind: &str) -> Result<String> {
        let mut connection = self.reader().await?;
        sqlx::query_scalar(sql)
            .bind(bind)
            .fetch_one(&mut connection)
            .await
            .with_context(|| format!("query {sql}"))
    }

    async fn scalar_count(&self, sql: &str, bind: &str) -> Result<i64> {
        let mut connection = self.reader().await?;
        sqlx::query_scalar(sql)
            .bind(bind)
            .fetch_one(&mut connection)
            .await
            .with_context(|| format!("query {sql}"))
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let mut connection = self.reader().await?;
        let sessions: Vec<SessionRow> = sqlx::query_as(
            "SELECT id, user_id, token_hash, csrf_token_hash, state, revoked_at, revoked_reason
               FROM sessions ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        let devices: Vec<DeviceRow> = sqlx::query_as(
            "SELECT id, user_id, token_hash, revoked_at FROM trusted_devices ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        let users: Vec<UserRow> = sqlx::query_as(
            "SELECT id, password_hash, must_change_password, totp_enabled FROM users ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        let totp_secrets = sqlx::query_as(
            "SELECT user_id, state, last_used_step FROM totp_secrets ORDER BY user_id",
        )
        .fetch_all(&mut connection)
        .await?;
        let backup_codes = sqlx::query_as(
            "SELECT user_id, count(*), count(used_at) FROM totp_backup_codes
              GROUP BY user_id ORDER BY user_id",
        )
        .fetch_all(&mut connection)
        .await?;
        let reset_tokens = sqlx::query_as(
            "SELECT id, used_at, invalidated_at FROM password_reset_tokens ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        let emails = sqlx::query_as(
            "SELECT id, email, email_normalized, pending_email, pending_email_normalized
               FROM users ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        let email_verifications = sqlx::query_as(
            "SELECT id, consumed_at, invalidated_at FROM email_verifications ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await?;
        Ok(Snapshot {
            sessions: sessions
                .into_iter()
                .map(|row| (row.id.clone(), row))
                .collect(),
            devices: devices
                .into_iter()
                .map(|row| (row.id.clone(), row))
                .collect(),
            users: users.into_iter().map(|row| (row.id.clone(), row)).collect(),
            totp_secrets,
            backup_codes,
            reset_tokens,
            emails,
            email_verifications,
        })
    }
}

struct Live {
    creds: Creds,
    id: String,
}

struct Device {
    id: String,
    cookie: String,
}

struct Person {
    id: String,
    sessions: Vec<Live>,
    devices: Vec<Device>,
}

struct World {
    app: TestApplication,
    http: Http,
    db: Db,
    a: Person,
    b: Person,
    totp_secret: Option<Vec<u8>>,
}

impl World {
    async fn start(name: &str, with_totp: bool, a_sessions: usize) -> Result<Self> {
        let app = TestApplication::start(name).await?;
        let http = Http::new(app.url("/")?)?;
        let db = Db {
            path: app.data_dir().join("palmr.db"),
        };
        let mut world = Self {
            a: Person {
                id: String::new(),
                sessions: Vec::new(),
                devices: Vec::new(),
            },
            b: Person {
                id: String::new(),
                sessions: Vec::new(),
                devices: Vec::new(),
            },
            app,
            http,
            db,
            totp_secret: None,
        };

        let setup = world
            .http
            .send(
                Method::POST,
                "/api/v1/setup",
                None,
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
            .expect(StatusCode::CREATED)?;
        let first = setup.creds()?;
        world.a.id = world.user_id("ada").await?;

        let invite = world
            .http
            .send(
                Method::POST,
                "/api/v1/admin/invites",
                Some(&first),
                None,
                Some(json!({ "email": "bea@example.test", "role": "user", "sendEmail": false })),
            )
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        let invite_url = invite["inviteUrl"].as_str().context("invite url")?;
        let token = invite_url.rsplit('/').next().context("invite token")?;
        let accepted = world
            .http
            .send(
                Method::POST,
                &format!("/api/v1/public/invites/{token}/accept"),
                None,
                None,
                Some(json!({
                    "firstName": "Bea",
                    "lastName": "Baker",
                    "username": "bea",
                    "password": PASSWORD,
                    "locale": "en-US",
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?;
        world.b.id = world.user_id("bea").await?;
        let accepted = world.live(accepted.creds()?).await?;
        world.b.sessions.push(accepted);

        let mut first = first;
        if with_totp {
            let (enrollment, secret) = world.enroll(&first).await?;
            let verified = world.verify(&first, &enrollment, &secret).await?;
            first = verified.expect(StatusCode::OK)?.creds()?;
            world.totp_secret = Some(secret);
        }
        let first = world.live(first).await?;
        world.a.sessions.push(first);
        while world.a.sessions.len() < a_sessions {
            world.add_session().await?;
        }

        let second = world.login("bea").await?;
        world.b.sessions.push(second);

        for label in ["laptop", "phone"] {
            let device = world.seed_device(&world.a.id, label).await?;
            world.a.devices.push(device);
        }
        let device = world.seed_device(&world.b.id, "tablet").await?;
        world.b.devices.push(device);
        Ok(world)
    }

    async fn standard(name: &str, with_totp: bool) -> Result<Self> {
        Self::start(name, with_totp, 3).await
    }

    async fn finish(self) -> Result<()> {
        expect_eq(self.app.shutdown().await, Drain::Completed, "drain")
    }

    async fn user_id(&self, username: &str) -> Result<String> {
        self.db
            .scalar_text(
                "SELECT id FROM users WHERE username_normalized = ?1",
                username,
            )
            .await
    }

    async fn live(&self, creds: Creds) -> Result<Live> {
        let id = self
            .db
            .scalar_text(
                "SELECT id FROM sessions WHERE token_hash = ?1",
                &digest_of_cookie(&creds.session)?,
            )
            .await?;
        Ok(Live { creds, id })
    }

    async fn add_session(&mut self) -> Result<()> {
        let creds = self.login_creds("ada").await?;
        let live = self.live(creds).await?;
        self.a.sessions.push(live);
        Ok(())
    }

    async fn login(&self, username: &str) -> Result<Live> {
        let creds = self.login_creds(username).await?;
        self.live(creds).await
    }

    fn step(&self) -> Result<u64> {
        Ok(u64::try_from(self.app.clock().now().unix_timestamp())? / TOTP_STEP)
    }

    fn advance_step(&self) {
        self.app.clock().advance(Duration::from_secs(TOTP_STEP));
    }

    async fn login_creds(&self, username: &str) -> Result<Creds> {
        let totp = username == "ada" && self.totp_secret.is_some();
        if totp {
            self.advance_step();
        }
        let password_step = self
            .http
            .send(
                Method::POST,
                LOGIN,
                None,
                None,
                Some(json!({ "identifier": username, "password": PASSWORD })),
            )
            .await?;
        let Some(secret) = self.totp_secret.as_deref().filter(|_| totp) else {
            return password_step.expect(StatusCode::OK)?.creds();
        };
        let challenge = password_step.expect(StatusCode::UNAUTHORIZED)?.json()?;
        let mfa_token = challenge["error"]["details"]["mfaToken"]
            .as_str()
            .context("mfa token")?;
        self.http
            .send(
                Method::POST,
                LOGIN_TOTP,
                None,
                None,
                Some(json!({ "mfaToken": mfa_token, "code": totp_code(secret, self.step()?)? })),
            )
            .await?
            .expect(StatusCode::OK)?
            .creds()
    }

    async fn enroll(&self, creds: &Creds) -> Result<(String, Vec<u8>)> {
        let body = self
            .http
            .send(Method::POST, ENROLL, Some(creds), None, None)
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        Ok((
            body["enrollmentId"]
                .as_str()
                .context("enrollment id")?
                .to_owned(),
            base32_decode(body["secretBase32"].as_str().context("secret")?)?,
        ))
    }

    async fn verify(&self, creds: &Creds, enrollment: &str, secret: &[u8]) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                VERIFY,
                Some(creds),
                None,
                Some(
                    json!({ "enrollmentId": enrollment, "code": totp_code(secret, self.step()?)? }),
                ),
            )
            .await
    }

    async fn add_admin_actor(&self) -> Result<Creds> {
        self.http
            .send(
                Method::POST,
                ADMIN_USERS,
                Some(&self.a.sessions[0].creds),
                None,
                Some(json!({
                    "firstName": "Cy",
                    "lastName": "Actor",
                    "username": "cyrus",
                    "email": "cyrus@example.test",
                    "role": "admin",
                    "password": PASSWORD,
                    "requirePasswordChange": false,
                    "locale": "en-US",
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?;
        self.login_creds("cyrus").await
    }

    async fn lifecycle(&self, actor: &Creds, target: &str, action: &str) -> Result<Reply> {
        let path = format!("{ADMIN_USERS}/{target}/{action}");
        match action {
            "role" => {
                self.call_json(Method::PUT, &path, actor, json!({ "role": "user" }))
                    .await
            }
            _ => self.call(Method::POST, &path, actor).await,
        }
    }

    async fn seed_device(&self, user_id: &str, label: &str) -> Result<Device> {
        let raw: Vec<u8> = Sha256::digest(format!("r042-device-{user_id}-{label}").as_bytes())
            .iter()
            .copied()
            .collect();
        let id = Uuid::now_v7().to_string();
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO trusted_devices (id, user_id, token_hash, label, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2026-01-01'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2027-01-01'))",
        )
        .bind(&id)
        .bind(user_id)
        .bind(hex(&Sha256::digest(&raw)))
        .bind(label)
        .execute(&mut connection)
        .await
        .context("seed trusted device")?;
        Ok(Device {
            id,
            cookie: Base64UrlUnpadded::encode_string(&raw),
        })
    }

    async fn seed_reset_token(&self, user_id: &str) -> Result<String> {
        let raw: Vec<u8> = Sha256::digest(format!("r042-reset-{user_id}").as_bytes())
            .iter()
            .copied()
            .collect();
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO password_reset_tokens (id, user_id, token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3,
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2026-01-01'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2027-01-01'))",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(user_id)
        .bind(hex(&Sha256::digest(&raw)))
        .execute(&mut connection)
        .await
        .context("seed reset token")?;
        Ok(Base64UrlUnpadded::encode_string(&raw))
    }

    async fn seed_email_change(&self, user_id: &str, email: &str) -> Result<String> {
        let raw: Vec<u8> = Sha256::digest(format!("r042-email-{user_id}").as_bytes())
            .iter()
            .copied()
            .collect();
        let normalized = email.to_lowercase();
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "UPDATE users SET pending_email = ?2, pending_email_normalized = ?3 WHERE id = ?1",
        )
        .bind(user_id)
        .bind(email)
        .bind(&normalized)
        .execute(&mut connection)
        .await
        .context("seed pending e-mail")?;
        sqlx::query(
            "INSERT INTO email_verifications
                 (id, user_id, purpose, email, email_normalized, token_hash, created_at,
                  expires_at, requested_by)
             VALUES (?1, ?2, 'email_change', ?3, ?4, ?5,
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2026-01-01'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', '2027-01-01'), ?2)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(user_id)
        .bind(email)
        .bind(&normalized)
        .bind(hex(&Sha256::digest(&raw)))
        .execute(&mut connection)
        .await
        .context("seed e-mail verification")?;
        Ok(Base64UrlUnpadded::encode_string(&raw))
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        self.db.snapshot().await
    }

    async fn audit_count(&self, action: &str) -> Result<i64> {
        self.db
            .scalar_count(
                "SELECT count(*) FROM audit_events WHERE action = ?1 AND result = 'success'",
                action,
            )
            .await
    }

    async fn inject_audit_failure(&self, action: &str) -> Result<()> {
        self.db
            .execute(&format!(
                "CREATE TRIGGER r042_injected_audit_failure BEFORE INSERT ON audit_events
                 WHEN NEW.action = '{action}'
                 BEGIN SELECT RAISE(ABORT, 'injected r042 audit failure'); END"
            ))
            .await
    }

    async fn remove_audit_failure(&self) -> Result<()> {
        self.db
            .execute("DROP TRIGGER r042_injected_audit_failure")
            .await
    }

    async fn me(&self, creds: &Creds) -> Result<StatusCode> {
        Ok(self
            .http
            .send(Method::GET, ME, Some(creds), None, None)
            .await?
            .status)
    }

    async fn call(&self, method: Method, path: &str, creds: &Creds) -> Result<Reply> {
        self.http.send(method, path, Some(creds), None, None).await
    }

    async fn call_json(
        &self,
        method: Method,
        path: &str,
        creds: &Creds,
        body: Value,
    ) -> Result<Reply> {
        self.http
            .send(method, path, Some(creds), None, Some(body))
            .await
    }

    async fn assert_injected_failure(&self, reply: Reply) -> Result<()> {
        expect_eq(
            reply.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "injected failure status",
        )?;
        expect_eq(
            reply.error_code()?,
            "INTERNAL_ERROR".to_owned(),
            "injected failure code",
        )
    }

    async fn assert_rolled_back(
        &self,
        trigger: &str,
        before: &Snapshot,
        audit_action: &str,
        audit_before: i64,
    ) -> Result<()> {
        let after = self.snapshot().await?;
        ensure!(
            &after == before,
            "{trigger}: a failed in-transaction audit write left partial state\nbefore: {before:#?}\nafter: {after:#?}"
        );
        expect_eq(
            self.audit_count(audit_action).await?,
            audit_before,
            &format!("{trigger}: no success audit row after rollback"),
        )?;
        let every_session_still_kept = vec![Fate::Kept; self.a.sessions.len()];
        self.probe(trigger, &every_session_still_kept, None).await
    }

    async fn settle(
        &self,
        trigger: &str,
        before: &Snapshot,
        sessions: &[Fate],
        devices: &[bool],
        rotated: Option<&Creds>,
    ) -> Result<Snapshot> {
        let after = self.snapshot().await?;
        self.check_rows(trigger, before, &after, sessions, devices)?;
        self.probe(trigger, sessions, rotated).await?;
        Ok(after)
    }

    fn check_rows(
        &self,
        trigger: &str,
        before: &Snapshot,
        after: &Snapshot,
        sessions: &[Fate],
        devices: &[bool],
    ) -> Result<()> {
        expect_eq(
            sessions.len(),
            self.a.sessions.len(),
            "one fate per session",
        )?;
        expect_eq(devices.len(), self.a.devices.len(), "one fate per device")?;
        expect_eq(
            after.sessions.keys().collect::<Vec<_>>(),
            before.sessions.keys().collect::<Vec<_>>(),
            &format!("{trigger}: no session row created or deleted"),
        )?;
        expect_eq(
            after.devices.keys().collect::<Vec<_>>(),
            before.devices.keys().collect::<Vec<_>>(),
            &format!("{trigger}: no trusted-device row created or deleted"),
        )?;
        for (live, fate) in self.a.sessions.iter().zip(sessions) {
            let old = before.sessions.get(&live.id).context("session before")?;
            let new = after.sessions.get(&live.id).context("session after")?;
            let label = format!("{trigger}: session {} ({fate:?})", live.id);
            expect_eq(
                old.state.as_str(),
                "active",
                &format!("{label} precondition"),
            )?;
            expect_eq(&new.user_id, &self.a.id, &format!("{label} owner"))?;
            match fate {
                Fate::Kept => expect_eq(new, old, &format!("{label} row"))?,
                Fate::Rotated => {
                    expect_eq(new.state.as_str(), "active", &format!("{label} state"))?;
                    expect_eq(&new.revoked_at, &None, &format!("{label} revoked_at"))?;
                    expect_eq(
                        &new.revoked_reason,
                        &None,
                        &format!("{label} revoked_reason"),
                    )?;
                    ensure!(
                        new.token_hash != old.token_hash,
                        "{label}: token hash must change"
                    );
                    ensure!(
                        new.csrf_token_hash != old.csrf_token_hash,
                        "{label}: csrf binding must change"
                    );
                }
                Fate::Revoked(reason) => {
                    expect_eq(new.state.as_str(), "revoked", &format!("{label} state"))?;
                    expect_eq(
                        new.revoked_reason.as_deref(),
                        Some(*reason),
                        &format!("{label} revoked_reason"),
                    )?;
                    ensure!(new.revoked_at.is_some(), "{label}: revoked_at must be set");
                    expect_eq(
                        &new.token_hash,
                        &old.token_hash,
                        &format!("{label} token hash"),
                    )?;
                    expect_eq(
                        &new.csrf_token_hash,
                        &old.csrf_token_hash,
                        &format!("{label} csrf hash"),
                    )?;
                }
            }
        }
        for (device, revoked) in self.a.devices.iter().zip(devices) {
            let old = before.devices.get(&device.id).context("device before")?;
            let new = after.devices.get(&device.id).context("device after")?;
            let label = format!("{trigger}: trusted device {}", device.id);
            expect_eq(&old.revoked_at, &None, &format!("{label} precondition"))?;
            expect_eq(
                &new.token_hash,
                &old.token_hash,
                &format!("{label} token hash"),
            )?;
            if *revoked {
                ensure!(new.revoked_at.is_some(), "{label}: must be revoked");
            } else {
                expect_eq(new, old, &format!("{label} row"))?;
            }
        }
        for live in &self.b.sessions {
            expect_eq(
                after.sessions.get(&live.id),
                before.sessions.get(&live.id),
                &format!("{trigger}: other user's session {}", live.id),
            )?;
        }
        for device in &self.b.devices {
            expect_eq(
                after.devices.get(&device.id),
                before.devices.get(&device.id),
                &format!("{trigger}: other user's trusted device {}", device.id),
            )?;
        }
        expect_eq(
            after.users.get(&self.b.id),
            before.users.get(&self.b.id),
            &format!("{trigger}: other user's account row"),
        )?;
        for row in after.sessions.values().filter(|row| row.state == "revoked") {
            ensure!(
                row.revoked_reason.is_some() && row.revoked_at.is_some(),
                "{trigger}: revoked session {} lacks reason or timestamp",
                row.id
            );
        }
        Ok(())
    }

    async fn probe(&self, trigger: &str, sessions: &[Fate], rotated: Option<&Creds>) -> Result<()> {
        for (live, fate) in self.a.sessions.iter().zip(sessions) {
            let expected = if matches!(fate, Fate::Kept) {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            };
            expect_eq(
                self.me(&live.creds).await?,
                expected,
                &format!("{trigger}: next request of session {} ({fate:?})", live.id),
            )?;
        }
        if let Some(creds) = rotated {
            expect_eq(
                self.me(creds).await?,
                StatusCode::OK,
                &format!("{trigger}: rotated credentials"),
            )?;
        }
        for live in &self.b.sessions {
            expect_eq(
                self.me(&live.creds).await?,
                StatusCode::OK,
                &format!("{trigger}: other user's session {}", live.id),
            )?;
        }
        Ok(())
    }

    async fn login_with_device_skips_second_factor(&self, device: &Device) -> Result<()> {
        let skipped = self
            .http
            .send(
                Method::POST,
                LOGIN,
                None,
                Some(&device.cookie),
                Some(json!({ "identifier": "ada", "password": PASSWORD })),
            )
            .await?;
        expect_eq(
            skipped.status,
            StatusCode::OK,
            "trusted device still skips the second factor",
        )?;
        let challenged = self
            .http
            .send(
                Method::POST,
                LOGIN,
                None,
                None,
                Some(json!({ "identifier": "ada", "password": PASSWORD })),
            )
            .await?;
        expect_eq(
            challenged.status,
            StatusCode::UNAUTHORIZED,
            "control login without device",
        )?;
        expect_eq(
            challenged.error_code()?,
            "AUTH_2FA_REQUIRED".to_owned(),
            "control challenge",
        )
    }
}

const CURRENT: usize = 2;
const ALL_KEPT: [Fate; 3] = [Fate::Kept, Fate::Kept, Fate::Kept];
const NO_DEVICE_REVOKED: [bool; 2] = [false, false];
const EVERY_DEVICE_REVOKED: [bool; 2] = [true, true];

async fn password_change() -> Result<()> {
    let world = World::standard("r042_password_change", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let body = json!({ "currentPassword": PASSWORD, "newPassword": REPLACEMENT });
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("PASSWORD_CHANGED").await?;

    world.inject_audit_failure("PASSWORD_CHANGED").await?;
    let failed = world
        .call_json(Method::POST, PASSWORD_ROUTE, &current, body.clone())
        .await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back("password change", &before, "PASSWORD_CHANGED", audit_before)
        .await?;
    world.remove_audit_failure().await?;

    let reply = world
        .call_json(Method::POST, PASSWORD_ROUTE, &current, body)
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    let rotated = reply.creds()?;
    ensure!(
        rotated.session != current.session,
        "session cookie must rotate"
    );
    let fates = [
        Fate::Revoked(PASSWORD_CHANGED),
        Fate::Revoked(PASSWORD_CHANGED),
        Fate::Rotated,
    ];
    world
        .settle(
            "password change",
            &before,
            &fates,
            &EVERY_DEVICE_REVOKED,
            Some(&rotated),
        )
        .await?;
    expect_eq(
        world.audit_count("PASSWORD_CHANGED").await?,
        audit_before + 1,
        "password change audit",
    )?;
    world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada", "password": REPLACEMENT })),
        )
        .await?
        .expect(StatusCode::OK)?;
    world.finish().await
}

async fn forced_password_change() -> Result<()> {
    let mut world = World::start("r042_forced_password_change", false, 2).await?;
    world
        .db
        .execute(&format!(
            "UPDATE users SET must_change_password = 1 WHERE id = '{}'",
            world.a.id
        ))
        .await?;
    world.add_session().await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    expect_eq(
        before.users[&world.a.id].must_change_password,
        1,
        "forced flag precondition",
    )?;

    let reply = world
        .call_json(
            Method::POST,
            PASSWORD_ROUTE,
            &current,
            json!({ "newPassword": REPLACEMENT }),
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    let rotated = reply.creds()?;
    let fates = [
        Fate::Revoked(PASSWORD_CHANGED),
        Fate::Revoked(PASSWORD_CHANGED),
        Fate::Rotated,
    ];
    let after = world
        .settle(
            "forced password change",
            &before,
            &fates,
            &EVERY_DEVICE_REVOKED,
            Some(&rotated),
        )
        .await?;
    expect_eq(
        after.users[&world.a.id].must_change_password,
        0,
        "forced flag cleared",
    )?;
    ensure!(
        after.users[&world.a.id].password_hash != before.users[&world.a.id].password_hash,
        "password hash must change"
    );
    expect_eq(
        world.audit_count("PASSWORD_CHANGED").await?,
        1,
        "forced change audit",
    )?;
    world.finish().await
}

async fn password_reset() -> Result<()> {
    let world = World::standard("r042_password_reset", false).await?;
    let token = world.seed_reset_token(&world.a.id).await?;
    let body = json!({ "token": token, "newPassword": REPLACEMENT });
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("PASSWORD_RESET_COMPLETED").await?;

    world
        .inject_audit_failure("PASSWORD_RESET_COMPLETED")
        .await?;
    let failed = world
        .http
        .send(Method::POST, RESET, None, None, Some(body.clone()))
        .await?;
    ensure!(
        failed.status.is_server_error(),
        "injected failure must surface as a server error, got {}",
        failed.status
    );
    world
        .assert_rolled_back(
            "password reset",
            &before,
            "PASSWORD_RESET_COMPLETED",
            audit_before,
        )
        .await?;
    world.remove_audit_failure().await?;

    let reply = world
        .http
        .send(Method::POST, RESET, None, None, Some(body))
        .await?
        .expect_success()?;
    reply.assert_no_session_cookie()?;
    let fates = [Fate::Revoked(PASSWORD_RESET); 3];
    let after = world
        .settle(
            "password reset",
            &before,
            &fates,
            &EVERY_DEVICE_REVOKED,
            None,
        )
        .await?;
    ensure!(
        after.users[&world.a.id].password_hash != before.users[&world.a.id].password_hash,
        "password hash must change"
    );
    expect_eq(
        world.audit_count("PASSWORD_RESET_COMPLETED").await?,
        audit_before + 1,
        "password reset audit",
    )?;
    world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada", "password": REPLACEMENT })),
        )
        .await?
        .expect(StatusCode::OK)?;
    world.finish().await
}

async fn totp_enable() -> Result<()> {
    let world = World::standard("r042_totp_enable", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let (enrollment, secret) = world.enroll(&current).await?;
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("TWO_FACTOR_ENABLED").await?;

    world.inject_audit_failure("TWO_FACTOR_ENABLED").await?;
    let failed = world.verify(&current, &enrollment, &secret).await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back("TOTP enable", &before, "TWO_FACTOR_ENABLED", audit_before)
        .await?;
    world.remove_audit_failure().await?;

    let reply = world
        .verify(&current, &enrollment, &secret)
        .await?
        .expect(StatusCode::OK)?;
    let rotated = reply.creds()?;
    ensure!(
        rotated.session != current.session,
        "session cookie must rotate"
    );
    let fates = [
        Fate::Revoked(POLICY_CHANGED),
        Fate::Revoked(POLICY_CHANGED),
        Fate::Rotated,
    ];
    let after = world
        .settle(
            "TOTP enable",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            Some(&rotated),
        )
        .await?;
    expect_eq(
        after.users[&world.a.id].totp_enabled,
        1,
        "totp enabled flag",
    )?;
    expect_eq(
        world.audit_count("TWO_FACTOR_ENABLED").await?,
        audit_before + 1,
        "TOTP enable audit",
    )?;
    world
        .login_with_device_skips_second_factor(&world.a.devices[0])
        .await?;
    world.finish().await
}

async fn totp_disable() -> Result<()> {
    let world = World::standard("r042_totp_disable", true).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("TWO_FACTOR_DISABLED").await?;
    expect_eq(
        before.users[&world.a.id].totp_enabled,
        1,
        "totp precondition",
    )?;

    world.inject_audit_failure("TWO_FACTOR_DISABLED").await?;
    let failed = world.call(Method::POST, DISABLE, &current).await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back("TOTP disable", &before, "TWO_FACTOR_DISABLED", audit_before)
        .await?;
    world.remove_audit_failure().await?;

    let reply = world
        .call(Method::POST, DISABLE, &current)
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    reply.assert_cleared()?;
    let fates = [Fate::Revoked(POLICY_CHANGED); 3];
    let after = world
        .settle("TOTP disable", &before, &fates, &EVERY_DEVICE_REVOKED, None)
        .await?;
    expect_eq(
        after.users[&world.a.id].totp_enabled,
        0,
        "totp disabled flag",
    )?;
    ensure!(
        after
            .totp_secrets
            .iter()
            .all(|(user, _, _)| user != &world.a.id),
        "totp secret must be deleted"
    );
    expect_eq(
        world.audit_count("TWO_FACTOR_DISABLED").await?,
        audit_before + 1,
        "TOTP disable audit",
    )?;
    world.finish().await
}

async fn backup_codes_regenerated() -> Result<()> {
    let world = World::standard("r042_backup_codes", true).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world
        .audit_count("TWO_FACTOR_BACKUP_CODES_REGENERATED")
        .await?;

    let reply = world
        .call(Method::POST, REGENERATE, &current)
        .await?
        .expect(StatusCode::OK)?;
    reply.assert_no_session_cookie()?;
    expect_eq(
        reply.json()?["backupCodes"].as_array().map(Vec::len),
        Some(10),
        "regenerated code count",
    )?;
    world
        .settle(
            "backup-code regeneration",
            &before,
            &ALL_KEPT,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world
            .audit_count("TWO_FACTOR_BACKUP_CODES_REGENERATED")
            .await?,
        audit_before + 1,
        "backup-code regeneration audit",
    )?;
    world
        .login_with_device_skips_second_factor(&world.a.devices[1])
        .await?;
    world.finish().await
}

async fn revoke_one_session() -> Result<()> {
    let world = World::standard("r042_revoke_one", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let target = world.a.sessions[1].id.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("SESSION_REVOKED").await?;

    let reply = world
        .call(Method::DELETE, &format!("{SESSIONS}/{target}"), &current)
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    reply.assert_no_session_cookie()?;
    let fates = [Fate::Kept, Fate::Revoked(USER_REQUEST), Fate::Kept];
    world
        .settle(
            "revoke one session",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("SESSION_REVOKED").await?,
        audit_before + 1,
        "session revoke audit",
    )?;
    world.finish().await
}

async fn revoke_other_sessions() -> Result<()> {
    let world = World::standard("r042_revoke_others", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("ALL_SESSIONS_REVOKED").await?;

    let reply = world
        .call(Method::DELETE, SESSIONS, &current)
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    reply.assert_no_session_cookie()?;
    let fates = [
        Fate::Revoked(USER_REQUEST),
        Fate::Revoked(USER_REQUEST),
        Fate::Kept,
    ];
    world
        .settle(
            "revoke all other sessions",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("ALL_SESSIONS_REVOKED").await?,
        audit_before + 1,
        "revoke-others audit",
    )?;
    world.finish().await
}

async fn revoke_all_sessions() -> Result<()> {
    let world = World::standard("r042_revoke_all", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("ALL_SESSIONS_REVOKED").await?;

    let reply = world
        .call(
            Method::DELETE,
            &format!("{SESSIONS}?includeCurrent=true"),
            &current,
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    reply.assert_cleared()?;
    let fates = [Fate::Revoked(USER_REQUEST); 3];
    world
        .settle(
            "revoke all sessions including current",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("ALL_SESSIONS_REVOKED").await?,
        audit_before + 1,
        "revoke-all audit",
    )?;
    world.finish().await
}

async fn logout() -> Result<()> {
    let world = World::standard("r042_logout", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;

    world
        .call(Method::POST, LOGOUT, &current)
        .await?
        .expect_success()?
        .assert_cleared()?;
    let fates = [Fate::Kept, Fate::Kept, Fate::Revoked(LOGGED_OUT)];
    world
        .settle("logout", &before, &fates, &NO_DEVICE_REVOKED, None)
        .await?;
    let mut logged = 0;
    for _ in 0..100 {
        logged = world.audit_count("LOGOUT").await?;
        if logged == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    expect_eq(logged, 1, "logout audit (enqueued write path)")?;
    world.finish().await
}

async fn revoke_one_trusted_device() -> Result<()> {
    let world = World::standard("r042_device_one", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let target = world.a.devices[0].id.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("TRUSTED_DEVICE_REVOKED").await?;

    world
        .call(Method::DELETE, &format!("{DEVICES}/{target}"), &current)
        .await?
        .expect(StatusCode::NO_CONTENT)?
        .assert_no_session_cookie()?;
    world
        .settle(
            "revoke one trusted device",
            &before,
            &ALL_KEPT,
            &[true, false],
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("TRUSTED_DEVICE_REVOKED").await?,
        audit_before + 1,
        "trusted-device revoke audit",
    )?;
    world.finish().await
}

async fn revoke_all_trusted_devices() -> Result<()> {
    let world = World::standard("r042_device_all", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("TRUSTED_DEVICE_REVOKED").await?;

    world
        .call(Method::DELETE, DEVICES, &current)
        .await?
        .expect(StatusCode::NO_CONTENT)?
        .assert_no_session_cookie()?;
    world
        .settle(
            "revoke all trusted devices",
            &before,
            &ALL_KEPT,
            &EVERY_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("TRUSTED_DEVICE_REVOKED").await?,
        audit_before + 1,
        "trusted-device revoke-all audit",
    )?;
    world.finish().await
}

async fn role_demotion() -> Result<()> {
    let world = World::standard("r042_role_demotion", false).await?;
    let actor = world.add_admin_actor().await?;
    let target = world.a.id.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("USER_ROLE_CHANGED").await?;

    world.inject_audit_failure("USER_ROLE_CHANGED").await?;
    let failed = world.lifecycle(&actor, &target, "role").await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back("role demotion", &before, "USER_ROLE_CHANGED", audit_before)
        .await?;
    world.remove_audit_failure().await?;

    world
        .lifecycle(&actor, &target, "role")
        .await?
        .expect(StatusCode::OK)?
        .assert_no_session_cookie()?;
    let fates = [Fate::Revoked(ROLE_CHANGED); 3];
    let after = world
        .settle("role demotion", &before, &fates, &NO_DEVICE_REVOKED, None)
        .await?;
    expect_eq(
        world.audit_count("USER_ROLE_CHANGED").await?,
        audit_before + 1,
        "role demotion audit",
    )?;
    expect_eq(
        world
            .db
            .scalar_text("SELECT role FROM users WHERE id = ?1", &target)
            .await?
            .as_str(),
        "user",
        "demoted role",
    )?;
    expect_eq(
        after.users.get(&world.a.id).map(|row| &row.password_hash),
        before.users.get(&world.a.id).map(|row| &row.password_hash),
        "a role change keeps the credential",
    )?;
    world.finish().await
}

async fn role_promotion() -> Result<()> {
    let world = World::standard("r042_role_promotion", false).await?;
    let actor = world.add_admin_actor().await?;
    let target = world.a.id.clone();
    world
        .db
        .execute(&format!(
            "UPDATE users SET role = 'user' WHERE id = '{target}'"
        ))
        .await?;
    let before = world.snapshot().await?;

    let promote = world
        .call_json(
            Method::PUT,
            &format!("{ADMIN_USERS}/{target}/role"),
            &actor,
            json!({ "role": "admin" }),
        )
        .await?
        .expect(StatusCode::OK)?;
    promote.assert_no_session_cookie()?;
    let fates = [Fate::Revoked(ROLE_CHANGED); 3];
    world
        .settle("role promotion", &before, &fates, &NO_DEVICE_REVOKED, None)
        .await?;
    expect_eq(
        world.audit_count("USER_ROLE_CHANGED").await?,
        1,
        "role promotion audit",
    )?;
    let fresh = world.login_creds("ada").await?;
    world
        .call(Method::GET, ADMIN_USERS, &fresh)
        .await?
        .expect(StatusCode::OK)?;
    world.finish().await
}

async fn user_deactivation() -> Result<()> {
    let world = World::standard("r042_user_deactivation", false).await?;
    let actor = world.add_admin_actor().await?;
    let target = world.a.id.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("USER_DEACTIVATED").await?;

    world.inject_audit_failure("USER_DEACTIVATED").await?;
    let failed = world.lifecycle(&actor, &target, "deactivate").await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back("deactivation", &before, "USER_DEACTIVATED", audit_before)
        .await?;
    world.remove_audit_failure().await?;

    world
        .lifecycle(&actor, &target, "deactivate")
        .await?
        .expect(StatusCode::OK)?
        .assert_no_session_cookie()?;
    let fates = [Fate::Revoked(DEACTIVATED); 3];
    let after = world
        .settle("deactivation", &before, &fates, &EVERY_DEVICE_REVOKED, None)
        .await?;
    expect_eq(
        world.audit_count("USER_DEACTIVATED").await?,
        audit_before + 1,
        "deactivation audit",
    )?;
    let refused = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada", "password": PASSWORD })),
        )
        .await?;
    expect_eq(
        refused.status,
        StatusCode::UNAUTHORIZED,
        "an inactive account cannot sign in",
    )?;

    world
        .lifecycle(&actor, &target, "activate")
        .await?
        .expect(StatusCode::OK)?
        .assert_no_session_cookie()?;
    let reactivated = world.snapshot().await?;
    expect_eq(
        &reactivated.sessions,
        &after.sessions,
        "activation restores no session",
    )?;
    expect_eq(
        &reactivated.devices,
        &after.devices,
        "activation restores no trusted device",
    )?;
    world.probe("activation", &fates, None).await?;
    world.login_creds("ada").await?;
    world.finish().await
}

async fn admin_password_reset() -> Result<()> {
    let world = World::standard("r042_admin_password_reset", true).await?;
    let actor = world.add_admin_actor().await?;
    let target = world.a.id.clone();
    world.seed_reset_token(&target).await?;
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("USER_PASSWORD_RESET_BY_ADMIN").await?;
    let path = format!("{ADMIN_USERS}/{target}/password-reset");

    world
        .inject_audit_failure("USER_PASSWORD_RESET_BY_ADMIN")
        .await?;
    let failed = world.call(Method::POST, &path, &actor).await?;
    ensure!(
        !failed.body.contains("temporaryPassword"),
        "a rolled-back reset must not return a temporary password: {}",
        failed.body
    );
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back(
            "admin password reset",
            &before,
            "USER_PASSWORD_RESET_BY_ADMIN",
            audit_before,
        )
        .await?;
    world.remove_audit_failure().await?;

    let reset = world
        .call(Method::POST, &path, &actor)
        .await?
        .expect(StatusCode::OK)?;
    reset.assert_no_session_cookie()?;
    let body = reset.json()?;
    let temporary = body["temporaryPassword"]
        .as_str()
        .context("temporary password")?
        .to_owned();
    expect_eq(
        body["mustChangePassword"].as_bool(),
        Some(true),
        "forced flag",
    )?;
    let fates = [Fate::Revoked(PASSWORD_RESET); 3];
    let after = world
        .settle(
            "admin password reset",
            &before,
            &fates,
            &EVERY_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("USER_PASSWORD_RESET_BY_ADMIN").await?,
        audit_before + 1,
        "admin password reset audit",
    )?;
    let (old, new) = (&before.users[&target], &after.users[&target]);
    ensure!(
        new.password_hash != old.password_hash,
        "the credential must change"
    );
    expect_eq(
        new.must_change_password,
        1,
        "reset forces a password change",
    )?;
    expect_eq(new.totp_enabled, old.totp_enabled, "TOTP flag is kept")?;
    expect_eq(
        &after.totp_secrets,
        &before.totp_secrets,
        "TOTP secret is kept",
    )?;
    expect_eq(
        &after.backup_codes,
        &before.backup_codes,
        "backup codes are kept",
    )?;
    expect_eq(
        after
            .reset_tokens
            .iter()
            .filter(|(_, used, invalidated)| used.is_none() && invalidated.is_none())
            .count(),
        0,
        "no outstanding reset link survives",
    )?;
    expect_eq(
        world.me(&actor).await?,
        StatusCode::OK,
        "the acting Admin keeps their session",
    )?;

    let old_password = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada", "password": PASSWORD })),
        )
        .await?;
    expect_eq(
        old_password.error_code()?,
        "AUTH_INVALID_CREDENTIALS".to_owned(),
        "the old password no longer works",
    )?;
    let challenged = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada", "password": temporary })),
        )
        .await?;
    expect_eq(
        challenged.error_code()?,
        "AUTH_2FA_REQUIRED".to_owned(),
        "the temporary password is accepted and TOTP is still required",
    )?;
    world.finish().await
}

async fn admin_revoke_all_sessions() -> Result<()> {
    let world = World::standard("r042_admin_revoke_all", true).await?;
    let actor = world.add_admin_actor().await?;
    let target = world.a.id.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("ALL_SESSIONS_REVOKED").await?;
    let path = format!("{ADMIN_USERS}/{target}/sessions");

    world.inject_audit_failure("ALL_SESSIONS_REVOKED").await?;
    let failed = world.call(Method::DELETE, &path, &actor).await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back(
            "admin revoke all sessions",
            &before,
            "ALL_SESSIONS_REVOKED",
            audit_before,
        )
        .await?;
    world.remove_audit_failure().await?;

    world
        .call(Method::DELETE, &path, &actor)
        .await?
        .expect(StatusCode::NO_CONTENT)?
        .assert_no_session_cookie()?;
    let fates = [Fate::Revoked(ADMIN_REQUEST); 3];
    let after = world
        .settle(
            "admin revoke all sessions",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("ALL_SESSIONS_REVOKED").await?,
        audit_before + 1,
        "admin revoke-all audit",
    )?;
    expect_eq(
        &after.users,
        &before.users,
        "revoking sessions changes no account row",
    )?;
    expect_eq(&after.totp_secrets, &before.totp_secrets, "TOTP is kept")?;
    world
        .login_with_device_skips_second_factor(&world.a.devices[0])
        .await?;
    expect_eq(
        world.me(&actor).await?,
        StatusCode::OK,
        "the acting Admin keeps their session",
    )?;
    world.finish().await
}

async fn admin_revoke_own_sessions() -> Result<()> {
    let world = World::standard("r042_admin_revoke_own", false).await?;
    let current = world.a.sessions[CURRENT].creds.clone();
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("ALL_SESSIONS_REVOKED").await?;

    let reply = world
        .call(
            Method::DELETE,
            &format!("{ADMIN_USERS}/{}/sessions", world.a.id),
            &current,
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    reply.assert_cleared()?;
    let fates = [Fate::Revoked(ADMIN_REQUEST); 3];
    world
        .settle(
            "admin revokes their own sessions",
            &before,
            &fates,
            &NO_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("ALL_SESSIONS_REVOKED").await?,
        audit_before + 1,
        "self revoke-all audit",
    )?;
    world.finish().await
}

async fn email_change_verified() -> Result<()> {
    let world = World::standard("r042_email_change_verified", true).await?;
    let target = world.a.id.clone();
    let token = world
        .seed_email_change(&target, "Ada.Next@Example.test")
        .await?;
    let before = world.snapshot().await?;
    let audit_before = world.audit_count("USER_EMAIL_CHANGE_CONFIRMED").await?;
    let verify = |token: String| {
        world.http.send(
            Method::POST,
            VERIFY_EMAIL,
            None,
            None,
            Some(json!({ "token": token })),
        )
    };

    world
        .inject_audit_failure("USER_EMAIL_CHANGE_CONFIRMED")
        .await?;
    let failed = verify(token.clone()).await?;
    world.assert_injected_failure(failed).await?;
    world
        .assert_rolled_back(
            "verified e-mail change",
            &before,
            "USER_EMAIL_CHANGE_CONFIRMED",
            audit_before,
        )
        .await?;
    world.remove_audit_failure().await?;

    let confirmed = verify(token.clone())
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    confirmed.assert_no_session_cookie()?;
    let fates = [Fate::Revoked(ADMIN_REQUEST); 3];
    let after = world
        .settle(
            "verified e-mail change",
            &before,
            &fates,
            &EVERY_DEVICE_REVOKED,
            None,
        )
        .await?;
    expect_eq(
        world.audit_count("USER_EMAIL_CHANGE_CONFIRMED").await?,
        audit_before + 1,
        "verified e-mail change audit",
    )?;
    let (old, new) = (&before.users[&target], &after.users[&target]);
    expect_eq(
        new.password_hash.as_ref(),
        old.password_hash.as_ref(),
        "the credential is kept",
    )?;
    expect_eq(
        new.must_change_password,
        old.must_change_password,
        "forced flag",
    )?;
    expect_eq(new.totp_enabled, old.totp_enabled, "TOTP flag is kept")?;
    expect_eq(
        &after.totp_secrets,
        &before.totp_secrets,
        "TOTP secret is kept",
    )?;
    expect_eq(
        &after.backup_codes,
        &before.backup_codes,
        "backup codes are kept",
    )?;
    let identity = after
        .emails
        .iter()
        .find(|(id, ..)| id == &target)
        .context("target identity row")?;
    expect_eq(
        (&identity.1, &identity.2, &identity.3, &identity.4),
        (
            &"Ada.Next@Example.test".to_owned(),
            &"ada.next@example.test".to_owned(),
            &None,
            &None,
        ),
        "the pending address is promoted",
    )?;
    expect_eq(
        after
            .email_verifications
            .iter()
            .filter(|(_, consumed, invalidated)| consumed.is_none() && invalidated.is_none())
            .count(),
        0,
        "no live verification survives",
    )?;

    let old_identity = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ada@example.test", "password": PASSWORD })),
        )
        .await?;
    expect_eq(
        old_identity.error_code()?,
        "AUTH_INVALID_CREDENTIALS".to_owned(),
        "the old address no longer signs in",
    )?;
    let challenged = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            None,
            Some(json!({ "identifier": "ADA.NEXT@example.test", "password": PASSWORD })),
        )
        .await?;
    expect_eq(
        challenged.error_code()?,
        "AUTH_2FA_REQUIRED".to_owned(),
        "the new address signs in and TOTP is still required",
    )?;
    let replay = verify(token).await?;
    expect_eq(
        replay.error_code()?,
        "EMAIL_VERIFICATION_TOKEN_INVALID".to_owned(),
        "the token is single-use",
    )?;
    world.finish().await
}

type Scenario = Pin<Box<dyn Future<Output = Result<()>>>>;

#[allow(non_snake_case, reason = "the accepted regression identifier is R-042")]
#[tokio::test(flavor = "multi_thread")]
async fn regression_R042_session_revocation_on_security_change() -> Result<()> {
    let scenarios: Vec<(&str, Scenario)> = vec![
        ("password_change", Box::pin(password_change())),
        ("forced_password_change", Box::pin(forced_password_change())),
        ("password_reset", Box::pin(password_reset())),
        ("totp_enable", Box::pin(totp_enable())),
        ("totp_disable", Box::pin(totp_disable())),
        (
            "backup_codes_regenerated",
            Box::pin(backup_codes_regenerated()),
        ),
        ("revoke_one_session", Box::pin(revoke_one_session())),
        ("revoke_other_sessions", Box::pin(revoke_other_sessions())),
        ("revoke_all_sessions", Box::pin(revoke_all_sessions())),
        ("logout", Box::pin(logout())),
        (
            "revoke_one_trusted_device",
            Box::pin(revoke_one_trusted_device()),
        ),
        (
            "revoke_all_trusted_devices",
            Box::pin(revoke_all_trusted_devices()),
        ),
        ("role_demotion", Box::pin(role_demotion())),
        ("role_promotion", Box::pin(role_promotion())),
        ("user_deactivation", Box::pin(user_deactivation())),
        ("admin_password_reset", Box::pin(admin_password_reset())),
        (
            "admin_revoke_all_sessions",
            Box::pin(admin_revoke_all_sessions()),
        ),
        (
            "admin_revoke_own_sessions",
            Box::pin(admin_revoke_own_sessions()),
        ),
        ("email_change_verified", Box::pin(email_change_verified())),
    ];
    let (names, futures): (Vec<_>, Vec<_>) = scenarios.into_iter().unzip();
    let results = futures_util::future::join_all(futures).await;
    let failures: Vec<String> = names
        .iter()
        .zip(results)
        .filter_map(|(name, result)| result.err().map(|error| format!("{name}: {error:#}")))
        .collect();
    ensure!(
        failures.is_empty(),
        "revocation matrix violations:\n{}",
        failures.join("\n\n")
    );
    Ok(())
}
