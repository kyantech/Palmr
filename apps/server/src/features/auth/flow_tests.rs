use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use axum::Extension;
use http::header::{CONTENT_TYPE, COOKIE, ORIGIN, RETRY_AFTER, SET_COOKIE};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use tempfile::TempDir;
use time::macros::datetime;
use time::OffsetDateTime;
use tower::ServiceExt;

use super::lockout::{self, LockoutPolicy};
use super::routes::{LOGIN_ROUTE, LOGOUT_ROUTE, ME_ROUTE};
use super::AuthService;
use crate::app::auth_class::{AbsentSession, AuthClass};
use crate::app::health::Health;
use crate::app::lifecycle::{setup_locale, Readiness};
use crate::app::openapi::ApiDocs;
use crate::app::router::{
    application_routes, with_middleware, HttpEdge, RateLimitClass, RouteError, RoutePolicy, Routes,
    Transport,
};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};
use crate::domain::clock::TestClock;
use crate::domain::email::Email;
use crate::domain::locale::LocaleCode;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::domain::username::Username;
use crate::features::audit;
use crate::features::audit::service::AuditDrain;
use crate::features::auth::sessions::SessionService;
use crate::features::settings::SettingsService;
use crate::features::setup::SetupService;
use crate::features::users::model::{NewUser, NormalizedIdentifier, QuotaOverride, UserId};
use crate::features::users::repo as users;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::crypto::password::{hash_password, verify_password, PasswordVerification};
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::csrf::{CsrfGuard, CSRF_HEADER};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const BASE_URL: &str = "https://files.example.test";
const LOGIN: &str = "/api/v1/auth/login";
const LOGOUT: &str = "/api/v1/auth/logout";
const ME: &str = "/api/v1/auth/me";
const BODY_CAP: usize = 64 * 1024;
const PASSWORD: &str = "correct horse battery staple";
const WRONG: &str = "incorrect horse battery staple";
const START: OffsetDateTime = datetime!(2026-09-25 12:00 UTC);

type AuditSummary<'a> = (&'a str, &'a str, Option<&'a str>, &'a str, Option<&'a str>);

struct Stack {
    pools: DbPools,
    clock: TestClock,
    settings: SettingsService,
    sessions: SessionService,
    auth: AuthService,
    drain: AuditDrain,
    service: BoxedService,
}

#[derive(Clone)]
struct BoxedService(tower::util::BoxCloneSyncService<Request, Response, Infallible>);

impl Stack {
    async fn start(root: &Path, clock: &TestClock) -> Self {
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_BASE_URL",
            BASE_URL,
        )]))
        .unwrap()
        .config;
        let pools = DbPools::open(root, 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(root).unwrap();
        let settings = SettingsService::load_with_setup_locale(
            &pools,
            Arc::new(clock.clone()),
            &instance_key,
            setup_locale(&config),
        )
        .await
        .unwrap();
        let sessions = SessionService::new(
            pools.clone(),
            Arc::new(clock.clone()),
            settings.handle(),
            settings.keys(),
            &config.base_url,
        );
        let (audit, drain) = audit::channel(
            audit::AUDIT_CHANNEL_CAPACITY,
            pools.clone(),
            Arc::new(clock.clone()),
        );
        let auth = AuthService::new(
            pools.clone(),
            Arc::new(clock.clone()),
            settings.handle(),
            sessions.clone(),
            audit,
        )
        .unwrap();
        let setup = SetupService::new(
            pools.clone(),
            Arc::new(clock.clone()),
            settings.clone(),
            sessions.clone(),
        );
        let assembled = application_routes().build().unwrap();
        let docs = ApiDocs::new(assembled.openapi, &config.base_url).unwrap();
        let router = assembled
            .router
            .with_state(AppState::new(
                Arc::new(clock.clone()),
                Health::new(Readiness::new()),
                docs,
                settings.handle(),
                StorageRuntime::for_test(),
            ))
            .layer(Extension(auth.clone()))
            .layer(Extension(setup))
            .layer(Extension(sessions.clone()));
        let edge = HttpEdge::new(
            Arc::new(clock.clone()),
            TrustedProxies::new(&config.trust_proxy),
            SecurityHeaders::new(&config),
            CsrfGuard::new(&config.base_url),
        );
        let service = BoxedService(tower::util::BoxCloneSyncService::new(with_middleware(
            router, &edge,
        )));
        Self {
            pools,
            clock: clock.clone(),
            settings,
            sessions,
            auth,
            drain,
            service,
        }
    }

    async fn send(&self, request: Request) -> Fetched {
        let response = self.service.0.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, BODY_CAP)
            .collect()
            .await
            .unwrap()
            .to_bytes();
        Fetched {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }

    async fn login(&self, identifier: &str, password: &str, host: u8) -> Fetched {
        self.login_with(identifier, password, host, None).await
    }

    async fn login_with(
        &self,
        identifier: &str,
        password: &str,
        host: u8,
        cookie: Option<&str>,
    ) -> Fetched {
        let body = json!({ "identifier": identifier, "password": password });
        self.post_json(LOGIN, &body.to_string(), host, cookie).await
    }

    async fn post_json(&self, path: &str, body: &str, host: u8, cookie: Option<&str>) -> Fetched {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, BASE_URL);
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, cookie);
        }
        self.send(with_peer(
            builder.body(Body::from(body.to_owned())).unwrap(),
            host,
        ))
        .await
    }

    async fn signed_in(&self, identifier: &str, host: u8) -> Credentials {
        let fetched = self.login(identifier, PASSWORD, host).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        Credentials::from(&fetched)
    }

    async fn logout(&self, request: LogoutRequest<'_>, host: u8) -> Fetched {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(LOGOUT)
            .header(ORIGIN, request.origin.unwrap_or(BASE_URL));
        let mut cookies = Vec::new();
        if let Some(session) = request.session {
            cookies.push(format!("palmr_session={session}"));
        }
        if let Some(csrf) = request.csrf_cookie {
            cookies.push(format!("palmr_csrf={csrf}"));
        }
        if !cookies.is_empty() {
            builder = builder.header(COOKIE, cookies.join("; "));
        }
        if let Some(header) = request.csrf_header {
            builder = builder.header(CSRF_HEADER, header);
        }
        self.send(with_peer(builder.body(Body::empty()).unwrap(), host))
            .await
    }

    async fn get(&self, path: &str, session: Option<&str>, host: u8) -> Fetched {
        let mut builder = Request::builder().method(Method::GET).uri(path);
        if let Some(session) = session {
            builder = builder.header(COOKIE, format!("palmr_session={session}"));
        }
        self.send(with_peer(builder.body(Body::empty()).unwrap(), host))
            .await
    }

    async fn user(&self, spec: UserSpec<'_>) -> UserId {
        let new = NewUser {
            email: Email::parse(spec.email).unwrap(),
            username: Username::parse(spec.username).unwrap(),
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            password_hash: spec.hash.cloned(),
            must_change_password: spec.must_change_password,
            role: Role::User,
            is_active: spec.active,
            quota: QuotaOverride::Inherit,
            created_by: None,
        };
        self.pools
            .write_tx(&self.clock, "auth.test_user", async |tx| {
                let user = users::insert(tx, &self.clock, &new).await?;
                if let Some(locale) = spec.locale {
                    users::insert_preferences(tx, &self.clock, user.id, locale).await?;
                }
                Ok::<_, crate::features::users::error::UserError>(user.id)
            })
            .await
            .unwrap()
    }

    async fn execute(&self, sql: &str) {
        self.pools
            .write_tx(&self.clock, "auth.test_execute", async |tx| {
                sqlx::query(sql).execute(tx.executor()).await?;
                Ok::<(), crate::infra::db::DbError>(())
            })
            .await
            .unwrap();
    }

    async fn setting(&self, key: &str, value_type: &str, value_json: &str) {
        self.execute(&format!(
            "INSERT INTO app_settings (key, group_name, value_type, value_json, is_secret, updated_at)
             VALUES ('{key}', 'security', '{value_type}', '{value_json}', 0, '2026-09-25T12:00:00.000Z')
             ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json"
        ))
        .await;
        self.settings.reload().await.unwrap();
    }

    async fn scalar_i64(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn lock_row(&self, user: UserId) -> (i64, Option<String>, i64) {
        sqlx::query_as(
            "SELECT failed_count, locked_until, lock_count FROM account_lockouts WHERE user_id = ?1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn attempt_results(&self) -> Vec<(String, Option<String>, String)> {
        sqlx::query_as(
            "SELECT identifier_normalized, user_id, result FROM login_attempts ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn flush_audit(&mut self) -> usize {
        self.drain
            .flush_batch(audit::AUDIT_BATCH_MAX)
            .await
            .unwrap()
    }

    async fn audit_actions(&self) -> Vec<(String, String, Option<String>, String, Option<String>)> {
        sqlx::query_as(
            "SELECT action, actor_type, actor_label, result, error_code
               FROM audit_events WHERE action LIKE 'LOGIN%' OR action = 'LOGOUT' ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn stop(self) {
        drop(self.service);
        drop(self.auth);
        drop(self.sessions);
        drop(self.settings);
        drop(self.drain);
        let _shutdown = self.pools.shutdown().await;
    }
}

struct UserSpec<'a> {
    username: &'a str,
    email: &'a str,
    hash: Option<&'a Secret<String>>,
    active: bool,
    must_change_password: bool,
    locale: Option<LocaleCode>,
}

impl<'a> UserSpec<'a> {
    const fn local(username: &'a str, email: &'a str, hash: &'a Secret<String>) -> Self {
        Self {
            username,
            email,
            hash: Some(hash),
            active: true,
            must_change_password: false,
            locale: None,
        }
    }
}

#[derive(Default)]
struct LogoutRequest<'a> {
    session: Option<&'a str>,
    csrf_cookie: Option<&'a str>,
    csrf_header: Option<&'a str>,
    origin: Option<&'a str>,
}

impl<'a> LogoutRequest<'a> {
    fn with(credentials: &'a Credentials) -> Self {
        Self {
            session: Some(&credentials.session),
            csrf_cookie: Some(&credentials.csrf),
            csrf_header: Some(&credentials.csrf),
            origin: None,
        }
    }
}

struct Credentials {
    session: String,
    csrf: String,
}

impl From<&Fetched> for Credentials {
    fn from(fetched: &Fetched) -> Self {
        Self {
            session: fetched.cookie("palmr_session"),
            csrf: fetched.cookie("palmr_csrf"),
        }
    }
}

struct Fetched {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Fetched {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    fn text(&self) -> String {
        String::from_utf8(self.body.to_vec()).unwrap()
    }

    fn error_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap().to_owned()
    }

    fn error_without_request_id(&self) -> Value {
        let mut error = self.json()["error"].clone();
        error.as_object_mut().unwrap().remove("requestId");
        error
    }

    fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }

    fn cookie(&self, name: &str) -> String {
        let prefix = format!("{name}=");
        let matching: Vec<String> = self
            .set_cookies()
            .into_iter()
            .filter(|value| value.starts_with(&prefix))
            .collect();
        assert_eq!(matching.len(), 1, "{matching:?}");
        matching[0][prefix.len()..]
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    fn retry_after(&self) -> Option<u64> {
        self.headers
            .get(RETRY_AFTER)
            .map(|value| value.to_str().unwrap().parse().unwrap())
    }
}

fn with_peer(mut request: Request, host: u8) -> Request {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((
            [198, 51, 100, host],
            40_000,
        ))));
    request
}

fn digest(raw: &str) -> String {
    Token::decode(raw).unwrap().digest().as_str().to_owned()
}

fn password_hash() -> Secret<String> {
    hash_password(PASSWORD.as_bytes()).unwrap()
}

async fn session_state(stack: &Stack, raw: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, revoked_reason FROM sessions WHERE token_hash = ?1")
        .bind(digest(raw))
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap()
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-048")]
#[tokio::test]
async fn regression_R048_login_response_non_enumerating() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let inert = stack
        .user(UserSpec {
            active: false,
            ..UserSpec::local("inert", "inert@example.test", &hash)
        })
        .await;
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    let locked = stack
        .user(UserSpec::local("locked", "locked@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "INSERT INTO account_lockouts (user_id, failed_count, first_failed_at, last_failed_at,
                                           locked_until, lock_count, updated_at)
             VALUES ('{locked}', 5, '2026-09-25T11:59:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:10:00.000Z', 1, '2026-09-25T12:00:00.000Z')"
        ))
        .await;

    let cases = [
        ("unknown identifier", "ghost@example.test", PASSWORD),
        ("known identifier, wrong password", "ada", WRONG),
        (
            "inactive account, correct password",
            "inert@example.test",
            PASSWORD,
        ),
        ("inactive account, wrong password", "inert", WRONG),
        ("SSO-only account without a local password", "sso", PASSWORD),
        (
            "locked account, wrong password",
            "locked@example.test",
            WRONG,
        ),
    ];
    let mut observed = Vec::new();
    for (index, (case, identifier, password)) in cases.iter().enumerate() {
        let before = stack.auth.verifications_performed();
        let fetched = stack
            .login(identifier, password, 10 + u8::try_from(index).unwrap())
            .await;
        assert_eq!(
            stack.auth.verifications_performed(),
            before + 1,
            "{case} must run exactly one Argon2id verification"
        );
        assert_eq!(fetched.status, StatusCode::UNAUTHORIZED, "{case}");
        assert_eq!(fetched.error_code(), "AUTH_INVALID_CREDENTIALS", "{case}");
        assert!(fetched.set_cookies().is_empty(), "{case}");
        assert!(fetched.retry_after().is_none(), "{case}");
        observed.push(fetched.error_without_request_id());
    }
    assert!(
        observed.windows(2).all(|pair| pair[0] == pair[1]),
        "{observed:?}"
    );
    assert_eq!(
        observed[0],
        json!({
            "code": "AUTH_INVALID_CREDENTIALS",
            "message": "The credentials are invalid",
            "details": {},
        })
    );

    let before = stack.auth.verifications_performed();
    let proven = stack.login("LOCKED", PASSWORD, 30).await;
    assert_eq!(stack.auth.verifications_performed(), before + 1);
    assert_eq!(proven.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(proven.error_code(), "AUTH_LOCKED");
    assert_eq!(proven.retry_after(), Some(600));
    assert!(proven.set_cookies().is_empty());

    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM sessions").await, 0);
    let ghost = NormalizedIdentifier::from_input("ghost@example.test");
    assert_eq!(
        stack.attempt_results().await,
        vec![
            (
                ghost.as_str().to_owned(),
                None,
                "unknown_identifier".to_owned()
            ),
            (
                "ada".to_owned(),
                Some(ada.to_string()),
                "bad_credentials".to_owned()
            ),
            (
                "inert@example.test".to_owned(),
                Some(inert.to_string()),
                "inactive".to_owned()
            ),
            (
                "inert".to_owned(),
                Some(inert.to_string()),
                "bad_credentials".to_owned()
            ),
            (
                "sso".to_owned(),
                Some(sso.to_string()),
                "bad_credentials".to_owned()
            ),
            (
                "locked@example.test".to_owned(),
                Some(locked.to_string()),
                "locked_out".to_owned()
            ),
            (
                "locked".to_owned(),
                Some(locked.to_string()),
                "locked_out".to_owned()
            ),
        ]
    );
    assert_eq!(
        stack.lock_row(locked).await,
        (5, Some("2026-09-25T12:10:00.000Z".to_owned()), 1)
    );
    stack.stop().await;
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-049")]
#[tokio::test]
async fn regression_R049_credential_surface_rate_limits() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let inventory = application_routes().build().unwrap().inventory;
    let login = inventory.get(&Method::POST, LOGIN).unwrap().policy();
    assert_eq!(login.rate_limit(), RateLimitClass::AuthLogin);
    assert_eq!(login.auth(), AuthClass::Public);

    for _ in 0..10 {
        let fetched = stack.post_json(LOGIN, "{}", 60, None).await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let throttled = stack.login("ada", PASSWORD, 60).await;
    assert_eq!(throttled.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(throttled.error_code(), "RATE_LIMITED");
    assert_eq!(
        throttled.json()["error"]["details"]["scope"],
        "rl.auth.login"
    );
    assert!(throttled.retry_after().is_some());
    assert!(throttled.set_cookies().is_empty());
    assert_eq!(stack.auth.verifications_performed(), 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts")
            .await,
        0
    );
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM sessions").await, 0);

    let other_client = stack.login("ada", PASSWORD, 61).await;
    assert_eq!(other_client.status, StatusCode::OK);
    assert_eq!(stack.auth.verifications_performed(), 1);

    stack.clock.advance(Duration::from_secs(60));
    let refilled = stack.login("ada", PASSWORD, 60).await;
    assert_eq!(refilled.status, StatusCode::OK);
    stack.stop().await;
}

#[tokio::test]
async fn it_login_email_or_username_case_insensitive() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec {
            locale: Some("pt-BR".parse().unwrap()),
            ..UserSpec::local("Ada.Lovelace", "Ada.Lovelace@Example.test", &hash)
        })
        .await;

    for (index, identifier) in [
        "ada.lovelace@example.test",
        "  ADA.LOVELACE@EXAMPLE.TEST ",
        "ada.lovelace",
        "ADA.LOVELACE",
        "\u{ff21}\u{ff44}\u{ff41}.Lovelace",
    ]
    .into_iter()
    .enumerate()
    {
        let fetched = stack
            .login(identifier, PASSWORD, 10 + u8::try_from(index).unwrap())
            .await;
        assert_eq!(
            fetched.status,
            StatusCode::OK,
            "{identifier}: {}",
            fetched.text()
        );
        assert_eq!(
            fetched.json(),
            json!({
                "user": {
                    "id": ada.to_string(),
                    "firstName": "Ada",
                    "lastName": "Lovelace",
                    "username": "Ada.Lovelace",
                    "email": "Ada.Lovelace@Example.test",
                    "role": "user",
                    "isActive": true,
                    "avatarUrl": null,
                    "locale": "pt-BR",
                },
                "mustChangePassword": false,
                "mfaEnrollmentRequired": false,
            }),
            "{identifier}"
        );
    }
    let recorded: Vec<String> = stack
        .attempt_results()
        .await
        .into_iter()
        .map(|(identifier, _, result)| {
            assert_eq!(result, "success");
            identifier
        })
        .collect();
    assert_eq!(
        recorded,
        [
            "ada.lovelace@example.test",
            "  ADA.LOVELACE@EXAMPLE.TEST ",
            "ada.lovelace",
            "ADA.LOVELACE",
            "\u{ff21}\u{ff44}\u{ff41}.Lovelace",
        ]
        .map(|input| NormalizedIdentifier::from_input(input).as_str().to_owned())
    );

    let other_hash = hash_password(WRONG.as_bytes()).unwrap();
    let grace = stack
        .user(UserSpec::local(
            "ada.lovelace@example.test",
            "grace@example.test",
            &other_hash,
        ))
        .await;
    let by_email = stack.login("ada.lovelace@example.test", PASSWORD, 20).await;
    assert_eq!(by_email.json()["user"]["id"], ada.to_string());
    let shadowed = stack.login("ada.lovelace@example.test", WRONG, 21).await;
    assert_eq!(shadowed.error_code(), "AUTH_INVALID_CREDENTIALS");
    let grace_by_email = stack.login("GRACE@example.test", WRONG, 22).await;
    assert_eq!(grace_by_email.json()["user"]["id"], grace.to_string());
    stack.stop().await;
}

#[tokio::test]
async fn it_lockout_durable_and_admin_clearable() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.setting("max_login_attempts", "integer", "3").await;
    stack.setting("login_lockout_minutes", "integer", "7").await;
    assert_eq!(
        LockoutPolicy::from_settings(&stack.settings.current()).max_attempts(),
        3
    );
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    for attempt in 1..=3_u8 {
        let fetched = stack.login("ada", WRONG, attempt).await;
        assert_eq!(fetched.error_code(), "AUTH_INVALID_CREDENTIALS");
        clock.advance(Duration::from_secs(1));
    }
    let crossed_at = START + Duration::from_secs(2);
    let locked_until = Timestamp::try_from(crossed_at + Duration::from_secs(7 * 60))
        .unwrap()
        .to_string();
    assert_eq!(
        stack.lock_row(ada).await,
        (3, Some(locked_until.clone()), 1)
    );

    let repeated = stack.login("ada", WRONG, 4).await;
    assert_eq!(repeated.error_code(), "AUTH_INVALID_CREDENTIALS");
    assert_eq!(
        stack.lock_row(ada).await,
        (3, Some(locked_until.clone()), 1)
    );
    let proven = stack.login("ada", PASSWORD, 5).await;
    assert_eq!(proven.error_code(), "AUTH_LOCKED");
    assert_eq!(proven.retry_after(), Some(7 * 60 - 1));
    stack.stop().await;

    let restarted = Stack::start(root.path(), &clock).await;
    let after_restart = restarted.login("ada", PASSWORD, 5).await;
    assert_eq!(after_restart.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(after_restart.error_code(), "AUTH_LOCKED");
    assert_eq!(after_restart.retry_after(), Some(7 * 60 - 1));
    assert_eq!(restarted.lock_row(ada).await, (3, Some(locked_until), 1));

    clock.advance(Duration::from_secs(7 * 60 - 1));
    let expired = restarted.login("ada", PASSWORD, 6).await;
    assert_eq!(expired.status, StatusCode::OK, "{}", expired.text());
    assert_eq!(restarted.lock_row(ada).await, (0, None, 1));

    for attempt in 0..3_u8 {
        restarted.login("ada", WRONG, 7 + attempt).await;
    }
    assert_eq!(restarted.lock_row(ada).await.2, 2);
    clock.advance(Duration::from_secs(7 * 60));
    restarted.login("ada", WRONG, 10).await;
    let (failed, until, lock_count) = restarted.lock_row(ada).await;
    assert_eq!((failed, until.is_none(), lock_count), (1, true, 2));

    for attempt in 0..2_u8 {
        restarted.login("ada", WRONG, 11 + attempt).await;
    }
    assert_eq!(restarted.lock_row(ada).await.2, 3);
    let admin = restarted
        .user(UserSpec::local("root", "root@example.test", &hash))
        .await;
    let cleared = restarted
        .pools
        .write_tx(&clock, "auth.test_clear", async |tx| {
            let now = Timestamp::try_from(clock_now(&clock)).unwrap();
            lockout::clear(tx, ada, admin, now).await
        })
        .await
        .unwrap();
    assert!(cleared);
    let (failed, until, lock_count) = restarted.lock_row(ada).await;
    assert_eq!((failed, until, lock_count), (0, None, 3));
    let cleared_by: Option<String> =
        sqlx::query_scalar("SELECT cleared_by FROM account_lockouts WHERE user_id = ?1")
            .bind(ada.to_string())
            .fetch_one(restarted.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(cleared_by, Some(admin.to_string()));
    assert_eq!(
        restarted.login("ada", PASSWORD, 13).await.status,
        StatusCode::OK
    );
    restarted.stop().await;
}

fn clock_now(clock: &TestClock) -> OffsetDateTime {
    crate::domain::clock::Clock::now(clock)
}

#[tokio::test]
async fn it_lockout_concurrent_failures_counted_once() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let attempts = (0..8_u8).map(|host| stack.login("ada", WRONG, 100 + host));
    for fetched in futures_util::future::join_all(attempts).await {
        assert_eq!(fetched.error_code(), "AUTH_INVALID_CREDENTIALS");
    }
    let results = stack.attempt_results().await;
    let counted = results
        .iter()
        .filter(|(_, _, result)| result == "bad_credentials")
        .count();
    let refused = results
        .iter()
        .filter(|(_, _, result)| result == "locked_out")
        .count();
    assert_eq!((counted, refused), (5, 3));
    let (failed, until, lock_count) = stack.lock_row(ada).await;
    assert_eq!((failed, until.is_some(), lock_count), (5, true, 1));
    stack.stop().await;
}

#[tokio::test]
async fn it_logout_idempotent() {
    let root = TempDir::new().unwrap();
    let mut stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let first = stack.signed_in("ada", 10).await;

    let signed_out = stack.logout(LogoutRequest::with(&first), 11).await;
    assert_eq!(
        signed_out.status,
        StatusCode::NO_CONTENT,
        "{}",
        signed_out.text()
    );
    assert!(signed_out.body.is_empty());
    let cleared = signed_out.set_cookies();
    assert_eq!(cleared.len(), 2, "{cleared:?}");
    assert!(cleared
        .iter()
        .any(|value| value.starts_with("palmr_session=;")
            && value.contains("Max-Age=0")
            && value.contains("HttpOnly")));
    assert!(cleared
        .iter()
        .any(|value| value.starts_with("palmr_csrf=;") && value.contains("Max-Age=0")));
    assert!(cleared.iter().all(|value| !value.contains("palmr_device")));
    assert_eq!(
        session_state(&stack, &first.session).await,
        ("revoked".to_owned(), Some("logout".to_owned()))
    );
    assert_eq!(
        stack.get(ME, Some(&first.session), 12).await.status,
        StatusCode::UNAUTHORIZED
    );

    let again = stack.logout(LogoutRequest::default(), 13).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT, "{}", again.text());
    assert_eq!(again.set_cookies().len(), 2);

    let stale = stack.logout(LogoutRequest::with(&first), 14).await;
    assert_eq!(stale.status, StatusCode::NO_CONTENT, "{}", stale.text());
    assert_eq!(stale.set_cookies().len(), 2);

    let second = stack.signed_in("ada", 15).await;
    let session_id: String = sqlx::query_scalar("SELECT id FROM sessions WHERE token_hash = ?1")
        .bind(digest(&second.session))
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    let unprotected = stack
        .send(with_peer(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/v1/sessions/{session_id}"))
                .header(ORIGIN, BASE_URL)
                .header(COOKIE, format!("palmr_session={}", second.session))
                .body(Body::empty())
                .unwrap(),
            16,
        ))
        .await;
    assert_eq!(unprotected.status, StatusCode::FORBIDDEN);
    assert_eq!(unprotected.error_code(), "CSRF_TOKEN_MISSING");
    let anonymous_mutation = stack
        .send(with_peer(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/v1/sessions/{session_id}"))
                .header(ORIGIN, BASE_URL)
                .body(Body::empty())
                .unwrap(),
            17,
        ))
        .await;
    assert_eq!(anonymous_mutation.error_code(), "CSRF_TOKEN_MISSING");
    assert_eq!(session_state(&stack, &second.session).await.0, "active");

    stack.flush_audit().await;
    let logouts: Vec<_> = stack
        .audit_actions()
        .await
        .into_iter()
        .filter(|(action, ..)| action == "LOGOUT")
        .collect();
    assert_eq!(
        logouts,
        vec![(
            "LOGOUT".to_owned(),
            "user".to_owned(),
            Some("ada".to_owned()),
            "success".to_owned(),
            None
        )]
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_logout_with_presented_session_requires_csrf() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("temp", "temp@example.test", &hash)
        })
        .await;
    let ada = stack.signed_in("ada", 10).await;

    let missing = stack
        .logout(
            LogoutRequest {
                session: Some(&ada.session),
                ..LogoutRequest::default()
            },
            11,
        )
        .await;
    assert_eq!(missing.status, StatusCode::FORBIDDEN);
    assert_eq!(missing.error_code(), "CSRF_TOKEN_MISSING");
    assert!(missing.set_cookies().is_empty());

    let foreign = Token::mint().unwrap().encode();
    let foreign = foreign.expose_secret();
    let unbound = stack
        .logout(
            LogoutRequest {
                session: Some(&ada.session),
                csrf_cookie: Some(foreign),
                csrf_header: Some(foreign),
                origin: None,
            },
            12,
        )
        .await;
    assert_eq!(unbound.status, StatusCode::FORBIDDEN);
    assert_eq!(unbound.error_code(), "CSRF_TOKEN_INVALID");

    let cross_origin = stack
        .logout(
            LogoutRequest {
                origin: Some("https://evil.example"),
                ..LogoutRequest::with(&ada)
            },
            13,
        )
        .await;
    assert_eq!(cross_origin.error_code(), "ORIGIN_NOT_ALLOWED");
    let anonymous_cross_origin = stack
        .logout(
            LogoutRequest {
                origin: Some("https://evil.example"),
                ..LogoutRequest::default()
            },
            14,
        )
        .await;
    assert_eq!(anonymous_cross_origin.error_code(), "ORIGIN_NOT_ALLOWED");
    assert_eq!(session_state(&stack, &ada.session).await.0, "active");
    assert_eq!(
        stack.get(ME, Some(&ada.session), 15).await.status,
        StatusCode::OK
    );

    let restricted = stack.signed_in("temp", 16).await;
    assert_eq!(
        stack
            .get("/api/v1/sessions", Some(&restricted.session), 17)
            .await
            .error_code(),
        "AUTH_PASSWORD_CHANGE_REQUIRED"
    );
    let restricted_out = stack.logout(LogoutRequest::with(&restricted), 18).await;
    assert_eq!(restricted_out.status, StatusCode::NO_CONTENT);
    assert_eq!(
        session_state(&stack, &restricted.session).await.0,
        "revoked"
    );
    stack.stop().await;
}

#[test]
fn unit_idempotent_sign_out_declared_only_by_logout() {
    let inventory = application_routes().build().unwrap().inventory;
    let signed_out: Vec<(String, String)> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.policy().absent_session() == AbsentSession::AlreadySignedOut)
        .map(|entry| (entry.method().to_string(), entry.path().to_owned()))
        .collect();
    assert_eq!(signed_out, vec![("POST".to_owned(), LOGOUT.to_owned())]);

    for (method, path, policy, auth, rate_limit) in [
        (
            Method::POST,
            LOGIN,
            LOGIN_ROUTE,
            AuthClass::Public,
            RateLimitClass::AuthLogin,
        ),
        (
            Method::POST,
            LOGOUT,
            LOGOUT_ROUTE,
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::GET,
            ME,
            ME_ROUTE,
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
    ] {
        let entry = inventory.get(&method, path).unwrap();
        assert_eq!(entry.policy(), policy);
        assert_eq!(entry.policy().auth(), auth);
        assert_eq!(entry.policy().rate_limit(), rate_limit);
    }
    assert_eq!(ME_ROUTE.absent_session(), AbsentSession::Reject);
    assert_eq!(LOGIN_ROUTE.absent_session(), AbsentSession::Reject);
}

#[utoipa::path(get, path = "/api/v1/probe/read", responses((status = 204)))]
async fn probe_read() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/probe/admin", responses((status = 204)))]
async fn probe_admin() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/probe/public", responses((status = 204)))]
async fn probe_public() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[test]
fn unit_idempotent_sign_out_refused_outside_authenticated_post() {
    let sign_out = |auth| {
        RoutePolicy::new(auth, RateLimitClass::Write, Transport::ControlPlane)
            .with_idempotent_sign_out()
    };
    let routes: Routes<()> = Routes::new()
        .route(
            RoutePolicy::new(
                AuthClass::Authenticated,
                RateLimitClass::Read,
                Transport::ControlPlane,
            )
            .with_idempotent_sign_out(),
            utoipa_axum::routes!(probe_read),
        )
        .route(
            sign_out(AuthClass::Admin),
            utoipa_axum::routes!(probe_admin),
        )
        .route(
            sign_out(AuthClass::Public),
            utoipa_axum::routes!(probe_public),
        );
    let Err(error) = routes.build() else {
        panic!("sign-out outside an authenticated POST must be refused");
    };
    let refused: Vec<String> = error
        .errors()
        .iter()
        .map(|error| match error {
            RouteError::SignOutOutsideAuthenticatedPost { method, path } => {
                format!("{method} {path}")
            }
            other => panic!("unexpected route error {other}"),
        })
        .collect();
    assert_eq!(
        refused,
        [
            "GET /api/v1/probe/read",
            "POST /api/v1/probe/admin",
            "POST /api/v1/probe/public"
        ]
    );
}

#[tokio::test]
async fn it_me_reports_restriction() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("temp", "temp@example.test", &hash)
        })
        .await;

    let normal = stack.signed_in("ada", 10).await;
    clock.advance(Duration::from_secs(90));
    let me = stack.get(ME, Some(&normal.session), 11).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.headers.get("cache-control").unwrap(), "no-store");
    let body = me.json();
    let session_id = body["session"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        body,
        json!({
            "user": {
                "id": ada.to_string(),
                "firstName": "Ada",
                "lastName": "Lovelace",
                "username": "ada",
                "email": "ada@example.test",
                "pendingEmail": null,
                "role": "user",
                "isActive": true,
                "avatarUrl": null,
                "locale": "en-US",
                "theme": "system",
                "accent": "default",
                "createdAt": "2026-09-25T12:00:00.000Z",
            },
            "session": {
                "id": session_id,
                "createdAt": "2026-09-25T12:00:00.000Z",
                "lastSeenAt": "2026-09-25T12:01:30.000Z",
                "expiresAt": "2026-10-02T12:01:30.000Z",
                "recentAuthUntil": "2026-09-25T12:05:00.000Z",
            },
            "restriction": null,
            "capabilities": {
                "canChangePassword": true,
                "hasLocalPassword": true,
                "twoFactorEnabled": false,
                "identityLinkCount": 0,
            },
        })
    );
    let last_auth: String = sqlx::query_scalar("SELECT last_auth_at FROM sessions WHERE id = ?1")
        .bind(&session_id)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(last_auth, "2026-09-25T12:00:00.000Z");

    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
        .bind(ada.to_string())
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    let text = me.text();
    for leaked in [
        normal.session.as_str(),
        normal.csrf.as_str(),
        digest(&normal.session).as_str(),
        digest(&normal.csrf).as_str(),
        stored.as_str(),
        "passwordHash",
        "tokenHash",
        "csrfTokenHash",
        "failedCount",
        "lockedUntil",
        "lastAuthAt",
        "$argon2",
    ] {
        assert!(!text.contains(leaked), "{leaked}");
    }

    let temp_login = stack.login("temp", PASSWORD, 12).await;
    assert_eq!(temp_login.json()["mustChangePassword"], true);
    assert_eq!(temp_login.json()["mfaEnrollmentRequired"], false);
    let temp = Credentials::from(&temp_login);
    let restricted = stack.get(ME, Some(&temp.session), 13).await;
    assert_eq!(restricted.status, StatusCode::OK);
    assert_eq!(restricted.json()["restriction"], "must_change_password");

    stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let enrolling_login = stack.login("ada", PASSWORD, 14).await;
    assert_eq!(enrolling_login.json()["mfaEnrollmentRequired"], true);
    assert_eq!(enrolling_login.json()["mustChangePassword"], false);
    let enrolling = Credentials::from(&enrolling_login);
    let enrolling_me = stack.get(ME, Some(&enrolling.session), 15).await;
    assert_eq!(
        enrolling_me.json()["restriction"],
        "mfa_enrollment_required"
    );
    assert_eq!(
        stack
            .get("/api/v1/sessions", Some(&enrolling.session), 16)
            .await
            .error_code(),
        "AUTH_2FA_ENROLLMENT_REQUIRED"
    );
    assert_eq!(
        stack.get(ME, Some(&temp.session), 17).await.json()["restriction"],
        "must_change_password"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_session_works_with_me() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let body = json!({
        "appName": "Nova Files",
        "firstName": "Ada",
        "lastName": "Lovelace",
        "username": "ada",
        "email": "ada@example.test",
        "password": PASSWORD,
        "locale": "pt-BR",
    });
    let created = stack
        .post_json("/api/v1/setup", &body.to_string(), 10, None)
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let credentials = Credentials::from(&created);

    let me = stack.get(ME, Some(&credentials.session), 11).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    let me = me.json();
    assert_eq!(me["user"]["id"], created.json()["user"]["id"]);
    assert_eq!(me["user"]["role"], "admin");
    assert_eq!(me["user"]["locale"], "pt-BR");
    assert_eq!(me["restriction"], Value::Null);
    assert_eq!(me["capabilities"]["hasLocalPassword"], true);

    let signed_out = stack.logout(LogoutRequest::with(&credentials), 12).await;
    assert_eq!(signed_out.status, StatusCode::NO_CONTENT);
    stack.stop().await;
}

#[tokio::test]
async fn it_login_replaces_presented_session() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let bootstrap = stack.get("/api/v1/bootstrap", None, 10).await;
    let anonymous_csrf = bootstrap.cookie("palmr_csrf");
    let first_login = stack
        .login_with(
            "ada",
            PASSWORD,
            11,
            Some(&format!("palmr_csrf={anonymous_csrf}")),
        )
        .await;
    assert_eq!(first_login.status, StatusCode::OK);
    let cookies = first_login.set_cookies();
    assert_eq!(cookies.len(), 2, "{cookies:?}");
    assert!(cookies.iter().all(|value| !value.contains(", palmr_")));
    assert!(cookies
        .iter()
        .all(|value| !value.starts_with("palmr_device")));
    let first = Credentials::from(&first_login);
    assert_ne!(first.csrf, anonymous_csrf);

    let second_login = stack
        .login_with(
            "ada",
            PASSWORD,
            12,
            Some(&format!(
                "palmr_session={}; palmr_csrf={}",
                first.session, first.csrf
            )),
        )
        .await;
    assert_eq!(second_login.status, StatusCode::OK);
    let second = Credentials::from(&second_login);
    assert_ne!(second.session, first.session);
    assert_ne!(second.csrf, first.csrf);
    for secret in [&first.session, &first.csrf, &second.session, &second.csrf] {
        assert!(!second_login.text().contains(secret.as_str()));
    }
    assert_eq!(
        session_state(&stack, &first.session).await,
        ("revoked".to_owned(), Some("rotated".to_owned()))
    );
    assert_eq!(
        stack.get(ME, Some(&first.session), 13).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        stack.get(ME, Some(&second.session), 14).await.status,
        StatusCode::OK
    );
    let bound: String =
        sqlx::query_scalar("SELECT csrf_token_hash FROM sessions WHERE token_hash = ?1")
            .bind(digest(&second.session))
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(bound, digest(&second.csrf));
    stack.stop().await;
}

#[tokio::test]
async fn it_login_request_contract() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;

    let malformed = stack
        .post_json(LOGIN, "{\"identifier\": \"ada\",", 10, None)
        .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed.error_code(), "INVALID_JSON");

    for (index, (body, fields)) in [
        (json!({ "identifier": "ada" }), json!(["password"])),
        (json!({ "password": PASSWORD }), json!(["identifier"])),
        (
            json!({ "identifier": "   ", "password": PASSWORD }),
            json!(["identifier"]),
        ),
        (
            json!({ "identifier": "ada", "password": "" }),
            json!(["password"]),
        ),
        (
            json!({ "identifier": "a".repeat(255), "password": PASSWORD }),
            json!(["identifier"]),
        ),
        (
            json!({ "identifier": "ada", "password": PASSWORD, "type": "username" }),
            json!(["body"]),
        ),
        (
            json!({ "identifier": 7, "password": PASSWORD }),
            json!(["identifier"]),
        ),
        (json!([]), json!(["body"])),
    ]
    .into_iter()
    .enumerate()
    {
        let fetched = stack
            .post_json(
                LOGIN,
                &body.to_string(),
                20 + u8::try_from(index).unwrap(),
                None,
            )
            .await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(fetched.error_code(), "VALIDATION_ERROR", "{body}");
        assert_eq!(
            fetched.json()["error"]["details"]["fields"],
            fields,
            "{body}"
        );
        assert!(fetched.json()["error"]["details"].get("field").is_none());
    }

    let form = stack
        .send(with_peer(
            Request::builder()
                .method(Method::POST)
                .uri(LOGIN)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(ORIGIN, BASE_URL)
                .body(Body::from("identifier=ada&password=x"))
                .unwrap(),
            40,
        ))
        .await;
    assert_eq!(form.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let cross_origin = stack
        .send(with_peer(
            Request::builder()
                .method(Method::POST)
                .uri(LOGIN)
                .header(CONTENT_TYPE, "application/json")
                .header(ORIGIN, "https://evil.example")
                .body(Body::from(
                    json!({ "identifier": "ada", "password": PASSWORD }).to_string(),
                ))
                .unwrap(),
            41,
        ))
        .await;
    assert_eq!(cross_origin.error_code(), "ORIGIN_NOT_ALLOWED");
    assert_eq!(stack.auth.verifications_performed(), 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts")
            .await,
        0
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_login_upgrades_stale_password_hash() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let params = Params::new(12_288, 1, 1, Some(32)).unwrap();
    let salt = SaltString::encode_b64(&[7_u8; 16]).unwrap();
    let stale = Secret::new(
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(PASSWORD.as_bytes(), &salt)
            .unwrap()
            .to_string(),
    );
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &stale))
        .await;
    let stored = async || -> String {
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
            .bind(ada.to_string())
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap()
    };

    assert_eq!(
        stack.login("ada", WRONG, 10).await.error_code(),
        "AUTH_INVALID_CREDENTIALS"
    );
    assert_eq!(stored().await, *stale.expose_secret());

    assert_eq!(
        stack.login("ada", PASSWORD, 11).await.status,
        StatusCode::OK
    );
    let upgraded = stored().await;
    assert!(
        upgraded.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "{upgraded}"
    );
    assert_eq!(
        verify_password(PASSWORD.as_bytes(), &upgraded).unwrap(),
        PasswordVerification::Verified {
            needs_rehash: false
        }
    );
    assert_eq!(
        stack.login("ada", PASSWORD, 12).await.status,
        StatusCode::OK
    );
    assert_eq!(stored().await, upgraded);
    stack.stop().await;
}

#[tokio::test]
async fn it_login_second_factor_account_fails_closed() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{ada}'"
        ))
        .await;

    let refused = stack.login("ada", PASSWORD, 10).await;
    assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(refused.set_cookies().is_empty());
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM sessions").await, 0);
    assert_eq!(
        stack.login("ada", WRONG, 11).await.error_code(),
        "AUTH_INVALID_CREDENTIALS"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_login_audit_and_attempts_never_store_secrets() {
    let root = TempDir::new().unwrap();
    let mut stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    stack.login("Ghost@Example.test", PASSWORD, 10).await;
    for host in 0..5_u8 {
        stack.login("ada", WRONG, 20 + host).await;
    }
    stack.login("ada", PASSWORD, 30).await;
    stack.execute("DELETE FROM account_lockouts").await;
    let credentials = stack.signed_in("ada", 31).await;
    stack.logout(LogoutRequest::with(&credentials), 32).await;
    stack.flush_audit().await;

    let actions = stack.audit_actions().await;
    let summary: Vec<AuditSummary<'_>> = actions
        .iter()
        .map(|(action, actor, label, result, code)| {
            (
                action.as_str(),
                actor.as_str(),
                label.as_deref(),
                result.as_str(),
                code.as_deref(),
            )
        })
        .collect();
    let invalid = Some("AUTH_INVALID_CREDENTIALS");
    let failed = ("LOGIN_FAILED", "user", Some("ada"), "failure", invalid);
    assert_eq!(
        summary,
        vec![
            (
                "LOGIN_FAILED",
                "anonymous",
                Some("Ghost@Example.test"),
                "failure",
                invalid
            ),
            failed,
            failed,
            failed,
            failed,
            failed,
            (
                "LOGIN_LOCKED_OUT",
                "user",
                Some("ada"),
                "denied",
                Some("AUTH_LOCKED")
            ),
            (
                "LOGIN_FAILED",
                "user",
                Some("ada"),
                "denied",
                Some("AUTH_LOCKED")
            ),
            ("LOGIN_SUCCEEDED", "user", Some("ada"), "success", None),
            ("LOGOUT", "user", Some("ada"), "success", None),
        ]
    );

    let dump: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(action, '') || '|' || COALESCE(actor_label, '') || '|' ||
                COALESCE(target_label, '') || '|' || COALESCE(error_code, '') || '|' ||
                COALESCE(user_agent, '') || '|' || COALESCE(metadata_json, '')
           FROM audit_events
         UNION ALL
         SELECT identifier_normalized || '|' || result || '|' || COALESCE(user_agent, '') || '|' ||
                COALESCE(request_id, '')
           FROM login_attempts",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(!dump.is_empty());
    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users LIMIT 1")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    for row in &dump {
        for leaked in [
            PASSWORD,
            WRONG,
            stored.as_str(),
            "$argon2",
            credentials.session.as_str(),
            credentials.csrf.as_str(),
            digest(&credentials.session).as_str(),
            digest(&credentials.csrf).as_str(),
        ] {
            assert!(!row.contains(leaked), "{row}");
        }
    }
    stack.stop().await;
}
