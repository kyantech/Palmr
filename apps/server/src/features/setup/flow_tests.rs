use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use axum::Extension;
use http::header::{CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tower::ServiceExt;

use super::routes::{SETUP_ROUTE, SETUP_STATUS_ROUTE};
use super::SetupService;
use crate::app::auth_class::AuthClass;
use crate::app::health::Health;
use crate::app::lifecycle::{setup_locale, Readiness};
use crate::app::openapi::ApiDocs;
use crate::app::router::{application_routes, with_middleware, HttpEdge, RateLimitClass};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};
use crate::domain::clock::TestClock;
use crate::domain::email::Email;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::username::Username;
use crate::features::auth::sessions::{SessionError, SessionService};
use crate::features::settings::SettingsService;
use crate::features::users::model::{NewUser, QuotaOverride};
use crate::features::users::repo as users;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::crypto::password::{verify_password, PasswordVerification};
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::csrf::CsrfGuard;
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const BASE_URL: &str = "https://files.example.test";
const SETUP_PATH: &str = "/api/v1/setup";
const STATUS_PATH: &str = "/api/v1/setup/status";
const BODY_CAP: usize = 64 * 1024;
const PASSWORD: &str = "correct horse battery staple";

struct Stack {
    pools: DbPools,
    clock: TestClock,
    settings: SettingsService,
    sessions: SessionService,
    service: BoxedService,
}

#[derive(Clone)]
struct BoxedService(tower::util::BoxCloneSyncService<Request, Response, Infallible>);

impl Stack {
    async fn start(root: &Path, default_language: Option<&str>) -> Self {
        let mut vars = vec![("PALMR_BASE_URL", BASE_URL)];
        if let Some(language) = default_language {
            vars.push(("PALMR_DEFAULT_LANGUAGE", language));
        }
        let config = OperatorConfig::load(&EnvironmentSource::from_vars(vars))
            .unwrap()
            .config;
        let pools = DbPools::open(root, 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(root).unwrap();
        let clock = TestClock::new(datetime!(2026-09-25 12:00 UTC));
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
            clock,
            settings,
            sessions,
            service,
        }
    }

    async fn send(&self, request: Request) -> Fetched {
        send(self.service.clone(), request).await
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Fetched {
        let mut builder = Request::builder().method(Method::GET).uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, cookie);
        }
        self.send(with_peer(builder.body(Body::empty()).unwrap(), 10))
            .await
    }

    async fn setup(&self, body: &Value) -> Fetched {
        self.send(setup_request(body, 20)).await
    }

    async fn scalar_i64(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn setting_json(&self, key: &str) -> Option<String> {
        sqlx::query_scalar("SELECT value_json FROM app_settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn durable_setup_rows(&self) -> [i64; 5] {
        [
            self.scalar_i64("SELECT COUNT(*) FROM users").await,
            self.scalar_i64("SELECT COUNT(*) FROM user_preferences")
                .await,
            self.scalar_i64("SELECT COUNT(*) FROM app_settings").await,
            self.scalar_i64("SELECT COUNT(*) FROM audit_events").await,
            self.scalar_i64("SELECT COUNT(*) FROM sessions").await,
        ]
    }

    async fn execute(&self, sql: &str) {
        self.pools
            .write_tx(&self.clock, "setup.test_execute", async |tx| {
                sqlx::query(sql).execute(tx.executor()).await?;
                Ok::<(), crate::infra::db::DbError>(())
            })
            .await
            .unwrap();
    }

    async fn stop(self) {
        drop(self.service);
        drop(self.sessions);
        drop(self.settings);
        let _shutdown = self.pools.shutdown().await;
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
}

async fn send(service: BoxedService, request: Request) -> Fetched {
    let response = service.0.oneshot(request).await.unwrap();
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

fn with_peer(mut request: Request, host: u8) -> Request {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((
            [198, 51, 100, host],
            40_000,
        ))));
    request
}

fn setup_request(body: &Value, host: u8) -> Request {
    let request = Request::builder()
        .method(Method::POST)
        .uri(SETUP_PATH)
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, BASE_URL)
        .body(Body::from(body.to_string()))
        .unwrap();
    with_peer(request, host)
}

fn admin(username: &str, locale: &str) -> Value {
    json!({
        "appName": "Nova Files",
        "firstName": "Ada",
        "lastName": "Lovelace",
        "username": username,
        "email": format!("{username}@example.test"),
        "password": PASSWORD,
        "locale": locale,
    })
}

fn digest(raw: &str) -> String {
    Token::decode(raw).unwrap().digest().as_str().to_owned()
}

async fn insert_user(stack: &Stack, suffix: &str) {
    let new = NewUser {
        email: Email::parse(&format!("{suffix}@example.test")).unwrap(),
        username: Username::parse(suffix).unwrap(),
        first_name: String::new(),
        last_name: String::new(),
        password_hash: None,
        must_change_password: false,
        role: Role::User,
        is_active: true,
        quota: QuotaOverride::Inherit,
        created_by: None,
    };
    stack
        .pools
        .write_tx(&stack.clock, "setup.test_user", async |tx| {
            users::insert(tx, &stack.clock, &new).await
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn it_setup_status_reports_setup_state_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;

    let fetched = stack.get(STATUS_PATH, None).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(
        fetched.json(),
        json!({ "setupCompleted": false, "passwordMinLength": 8 })
    );
    assert!(fetched.set_cookies().is_empty());
    let polled = stack.get(STATUS_PATH, None).await;
    assert_eq!(polled.json()["setupCompleted"], false);
    assert_eq!(stack.durable_setup_rows().await, [0; 5]);

    stack
        .execute(
            "INSERT INTO app_settings (key, group_name, value_type, value_json, is_secret,
                                       updated_at)
             VALUES ('password_min_length', 'security', 'integer', '12', 0,
                     '2026-09-25T12:00:00.000Z')",
        )
        .await;
    stack.settings.reload().await.unwrap();
    assert_eq!(
        stack.get(STATUS_PATH, None).await.json(),
        json!({ "setupCompleted": false, "passwordMinLength": 12 })
    );

    let mut body = admin("ada", "en-US");
    body["password"] = json!("aaaaaaaaaaaa");
    assert_eq!(stack.setup(&body).await.status, StatusCode::CREATED);
    let done = stack.get(STATUS_PATH, None).await;
    assert_eq!(done.status, StatusCode::OK);
    assert_eq!(done.json(), json!({ "setupCompleted": true }));
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_seeds_instance_identity() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;

    let anonymous = stack.get("/api/v1/bootstrap", None).await;
    let anonymous_csrf = anonymous.cookie("palmr_csrf");
    let request = Request::builder()
        .method(Method::POST)
        .uri(SETUP_PATH)
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, BASE_URL)
        .header(COOKIE, format!("palmr_csrf={anonymous_csrf}"))
        .body(Body::from(admin("Ada.Admin", "pt-BR").to_string()))
        .unwrap();
    let created = stack.send(with_peer(request, 30)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    let body = created.json();
    let user_id = body["user"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        body,
        json!({
            "user": {
                "id": user_id,
                "username": "Ada.Admin",
                "email": "Ada.Admin@example.test",
                "role": "admin",
                "isActive": true,
            },
            "mustChangePassword": false,
        })
    );

    let set_cookies = created.set_cookies();
    assert_eq!(set_cookies.len(), 2, "{set_cookies:?}");
    assert!(set_cookies.iter().all(|value| !value.contains(", palmr_")));
    let session_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("palmr_session="))
        .unwrap();
    assert!(session_cookie.contains("; HttpOnly"));
    assert!(session_cookie.contains("; Secure"));
    let csrf_cookie = set_cookies
        .iter()
        .find(|value| value.starts_with("palmr_csrf="))
        .unwrap();
    assert!(!csrf_cookie.contains("HttpOnly"));
    assert!(csrf_cookie.contains("; Secure"));
    let session_token = created.cookie("palmr_session");
    let csrf_token = created.cookie("palmr_csrf");
    assert_ne!(csrf_token, anonymous_csrf);
    for secret in [&session_token, &csrf_token, &anonymous_csrf] {
        assert!(!created.text().contains(secret.as_str()));
    }
    for leaked in [
        "sessionToken",
        "csrfToken",
        "passwordHash",
        "setupToken",
        PASSWORD,
    ] {
        assert!(!created.text().contains(leaked), "{leaked}");
    }

    let (role, is_active, must_change, hash, email_normalized, username_normalized): (
        String,
        bool,
        bool,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT role, is_active, must_change_password, password_hash, email_normalized,
                username_normalized
         FROM users WHERE id = ?1",
    )
    .bind(&user_id)
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(role, "admin");
    assert!(is_active);
    assert!(!must_change);
    assert!(hash.starts_with("$argon2id$"));
    assert_eq!(
        verify_password(PASSWORD.as_bytes(), &hash).unwrap(),
        PasswordVerification::Verified {
            needs_rehash: false
        }
    );
    assert_eq!(email_normalized, "ada.admin@example.test");
    assert_eq!(username_normalized, "ada.admin");

    let locale: String =
        sqlx::query_scalar("SELECT locale FROM user_preferences WHERE user_id = ?1")
            .bind(&user_id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(locale, "pt-BR");
    for (key, expected) in [
        ("app_name", "\"Nova Files\""),
        ("app_description", "\"Self-hosted file transfer\""),
        ("default_locale", "\"pt-BR\""),
        ("setup_completed", "true"),
    ] {
        assert_eq!(
            stack.setting_json(key).await.as_deref(),
            Some(expected),
            "{key}"
        );
    }
    let updated_by: Vec<Option<String>> =
        sqlx::query_scalar("SELECT updated_by FROM app_settings ORDER BY key")
            .fetch_all(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(updated_by, vec![Some(user_id.clone()); 4]);

    let (auth_method, state, token_hash, csrf_hash): (String, String, String, String) =
        sqlx::query_as(
            "SELECT auth_method, state, token_hash, csrf_token_hash
             FROM sessions WHERE user_id = ?1",
        )
        .bind(&user_id)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(auth_method, "password");
    assert_eq!(state, "active");
    assert_eq!(token_hash, digest(&session_token));
    assert_eq!(csrf_hash, digest(&csrf_token));
    assert_ne!(csrf_hash, digest(&anonymous_csrf));

    let session = Secret::new(session_token.clone());
    let principal = stack
        .sessions
        .authenticate_bound(
            &session,
            Some(&Token::decode(&csrf_token).unwrap().digest()),
        )
        .await
        .unwrap();
    assert_eq!(principal.user_id.to_string(), user_id);
    assert_eq!(principal.role, Role::Admin);
    assert!(matches!(
        stack
            .sessions
            .authenticate_bound(
                &session,
                Some(&Token::decode(&anonymous_csrf).unwrap().digest()),
            )
            .await,
        Err(SessionError::CsrfInvalid)
    ));
    let listed = stack
        .get(
            "/api/v1/sessions",
            Some(&format!(
                "palmr_session={session_token}; palmr_csrf={csrf_token}"
            )),
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.json()["items"][0]["isCurrent"], true);

    let (action, actor_type, actor_id, target_type, target_id, result, metadata): (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT action, actor_type, actor_user_id, target_type, target_id, result, metadata_json
         FROM audit_events",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(action, "SETUP_COMPLETED");
    assert_eq!(actor_type, "user");
    assert_eq!(actor_id.as_deref(), Some(user_id.as_str()));
    assert_eq!(target_type.as_deref(), Some("user"));
    assert_eq!(target_id.as_deref(), Some(user_id.as_str()));
    assert_eq!(result, "success");
    assert_eq!(metadata, "{}");
    let audit_dump: String = sqlx::query_scalar(
        "SELECT group_concat(coalesce(actor_label, '') || coalesce(target_label, '') ||
                             coalesce(metadata_json, '') || coalesce(user_agent, ''), '|')
         FROM audit_events",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    for secret in [
        PASSWORD,
        &hash,
        &session_token,
        &csrf_token,
        &token_hash,
        &csrf_hash,
    ] {
        assert!(!audit_dump.contains(secret), "{audit_dump}");
    }

    let bootstrap = stack.get("/api/v1/bootstrap", None).await.json();
    assert_eq!(bootstrap["setupCompleted"], true);
    assert_eq!(bootstrap["appName"], "Nova Files");
    assert_eq!(bootstrap["appDescription"], "Self-hosted file transfer");
    assert_eq!(bootstrap["defaultLocale"], "pt-BR");
    let manifest = stack.get("/manifest.webmanifest", None).await;
    assert_eq!(manifest.status, StatusCode::OK);
    assert_eq!(manifest.json()["name"], "Nova Files");
    stack.stop().await;
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-069")]
async fn regression_R069_setup_atomic_and_idempotent() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;

    let unauthenticated = stack.get("/api/v1/sessions", None).await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthenticated.error_code(), "AUTH_REQUIRED");

    for (table, step) in [
        ("users", "first Admin insert"),
        ("audit_events", "SETUP_COMPLETED audit"),
        ("sessions", "session mint"),
    ] {
        stack
            .execute(&format!(
                "CREATE TRIGGER interrupt_setup BEFORE INSERT ON {table}
                 BEGIN SELECT RAISE(ABORT, 'interrupted setup'); END"
            ))
            .await;
        let interrupted = stack.setup(&admin("ada", "en-US")).await;
        assert_eq!(
            interrupted.status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{step}"
        );
        assert_eq!(interrupted.error_code(), "INTERNAL_ERROR", "{step}");
        assert!(interrupted.set_cookies().is_empty(), "{step}");
        assert!(!interrupted.text().contains("interrupted setup"), "{step}");
        assert_eq!(stack.durable_setup_rows().await, [0; 5], "{step}");
        assert!(!stack.settings.current().setup_completed(), "{step}");
        assert_eq!(
            stack.get(STATUS_PATH, None).await.json()["setupCompleted"],
            false,
            "{step}"
        );
        stack.execute("DROP TRIGGER interrupt_setup").await;
    }

    let stale = Stack::start(root.path(), None).await;
    let (first, second) = tokio::join!(
        send(
            stack.service.clone(),
            setup_request(&admin("ada", "en-US"), 41)
        ),
        send(
            stack.service.clone(),
            setup_request(&admin("grace", "de-DE"), 42)
        ),
    );
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    let (winner, loser) = if first.status == StatusCode::CREATED {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(loser.error_code(), "SETUP_ALREADY_COMPLETED");
    assert!(loser.set_cookies().is_empty());
    assert_eq!(winner.set_cookies().len(), 2);
    let winner_id = winner.json()["user"]["id"].as_str().unwrap().to_owned();

    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM users WHERE role = 'admin' AND is_active = 1")
            .await,
        1
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM user_preferences")
            .await,
        1
    );
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM sessions").await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'SETUP_COMPLETED'")
            .await,
        1
    );
    let default_locale = stack.setting_json("default_locale").await.unwrap();
    let preference: String = sqlx::query_scalar("SELECT locale FROM user_preferences")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(default_locale, format!("\"{preference}\""));
    let session_owner: String = sqlx::query_scalar("SELECT user_id FROM sessions")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(session_owner, winner_id);

    assert!(!stale.settings.current().setup_completed());
    let raced = stale.setup(&admin("mallory", "en-US")).await;
    assert_eq!(raced.status, StatusCode::CONFLICT);
    assert_eq!(raced.error_code(), "SETUP_ALREADY_COMPLETED");
    assert!(raced.set_cookies().is_empty());
    assert_eq!(stale.scalar_i64("SELECT COUNT(*) FROM users").await, 1);
    stale.stop().await;

    let again = stack.setup(&admin("mallory", "en-US")).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.error_code(), "SETUP_ALREADY_COMPLETED");
    assert!(again.set_cookies().is_empty());
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_closed_forever() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;
    let created = stack.setup(&admin("ada", "en-US")).await;
    assert_eq!(created.status, StatusCode::CREATED);

    stack.execute("DELETE FROM sessions").await;
    stack.execute("DELETE FROM users").await;
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 0);
    stack.settings.reload().await.unwrap();

    let status = stack.get(STATUS_PATH, None).await;
    assert_eq!(status.json(), json!({ "setupCompleted": true }));
    let reopened = stack.setup(&admin("mallory", "en-US")).await;
    assert_eq!(reopened.status, StatusCode::CONFLICT);
    assert_eq!(reopened.error_code(), "SETUP_ALREADY_COMPLETED");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 0);
    let anonymous_admin_surface = stack.get("/api/v1/sessions", None).await;
    assert_eq!(anonymous_admin_surface.status, StatusCode::UNAUTHORIZED);
    stack.stop().await;

    let restarted = Stack::start(root.path(), Some("de-DE")).await;
    assert!(restarted.settings.current().setup_completed());
    assert_eq!(
        restarted.get(STATUS_PATH, None).await.json(),
        json!({ "setupCompleted": true })
    );
    assert_eq!(
        restarted.get("/api/v1/bootstrap", None).await.json()["setupCompleted"],
        true
    );
    let after_restart = restarted.setup(&admin("mallory", "en-US")).await;
    assert_eq!(after_restart.status, StatusCode::CONFLICT);
    assert_eq!(after_restart.error_code(), "SETUP_ALREADY_COMPLETED");
    assert_eq!(restarted.scalar_i64("SELECT COUNT(*) FROM users").await, 0);
    restarted.stop().await;
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-031")]
async fn regression_R031_user_count_never_authorizes_setup() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;
    insert_user(&stack, "preexisting").await;
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 1);
    assert_eq!(
        stack.get(STATUS_PATH, None).await.json()["setupCompleted"],
        false
    );

    let created = stack.setup(&admin("ada", "en-US")).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM users").await, 2);

    let taken_root = TempDir::new().unwrap();
    let taken = Stack::start(taken_root.path(), None).await;
    insert_user(&taken, "ada").await;
    let email_conflict = taken.setup(&admin("ada", "en-US")).await;
    assert_eq!(email_conflict.status, StatusCode::CONFLICT);
    assert_eq!(email_conflict.error_code(), "USER_EMAIL_TAKEN");
    let mut other_email = admin("ADA", "en-US");
    other_email["email"] = json!("someone-else@example.test");
    let username_conflict = taken.setup(&other_email).await;
    assert_eq!(username_conflict.status, StatusCode::CONFLICT);
    assert_eq!(username_conflict.error_code(), "USER_USERNAME_TAKEN");
    assert!(!username_conflict.text().to_lowercase().contains("unique"));
    assert!(taken.setting_json("setup_completed").await.is_none());
    assert_eq!(
        taken.scalar_i64("SELECT COUNT(*) FROM audit_events").await,
        0
    );
    taken.stop().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_rejects_invalid_requests_and_stays_open() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;

    let long = "x".repeat(101);
    let cases: Vec<(Value, Value)> = vec![
        (
            json!({ "appName": "Palmr" }),
            json!([
                "firstName",
                "lastName",
                "username",
                "email",
                "password",
                "locale"
            ]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["email"] = json!(5);
                body["password"] = Value::Null;
                body
            },
            json!(["email", "password"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["username"] = json!("x".repeat(65));
                body["email"] = json!("not-an-email");
                body["locale"] = json!("xx-XX");
                body["appName"] = json!("");
                body
            },
            json!(["appName", "username", "email", "locale"]),
        ),
        (json!(["not", "an", "object"]), json!(["body"])),
        (
            {
                let mut body = admin("ada", "en-US");
                body["appDescription"] = json!("Custom");
                body
            },
            json!(["body"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["appName"] = json!("   ");
                body
            },
            json!(["appName"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["appName"] = json!(long);
                body
            },
            json!(["appName"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["firstName"] = json!("");
                body
            },
            json!(["firstName"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["lastName"] = json!("Love\u{0007}lace");
                body
            },
            json!(["lastName"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["username"] = json!("x".repeat(65));
                body
            },
            json!(["username"]),
        ),
        (
            {
                let mut body = admin("ada", "en-US");
                body["email"] = json!("not-an-email");
                body
            },
            json!(["email"]),
        ),
        (admin("ada", "xx-XX"), json!(["locale"])),
        (admin("ada", "pt-br"), json!(["locale"])),
    ];
    for (host, (body, fields)) in (100..).zip(cases) {
        let rejected = stack.send(setup_request(&body, host)).await;
        assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let error = rejected.json();
        assert_eq!(error["error"]["code"], "VALIDATION_ERROR", "{body}");
        assert_eq!(
            error["error"]["details"],
            json!({ "fields": fields }),
            "{body}"
        );
        assert!(error["error"]["requestId"].is_string());
    }

    for (host, raw) in (150..).zip(["", "{\"appName\":", "{'appName':'Palmr'}", "nul"]) {
        let malformed = Request::builder()
            .method(Method::POST)
            .uri(SETUP_PATH)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(raw))
            .unwrap();
        let malformed = stack.send(with_peer(malformed, host)).await;
        assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{raw:?}");
        let error = malformed.json();
        assert_eq!(error["error"]["code"], "INVALID_JSON", "{raw:?}");
        assert_eq!(error["error"]["details"], json!({}), "{raw:?}");
        assert!(error["error"]["requestId"].is_string());
        assert!(!malformed.text().contains("serde"), "{raw:?}");
    }
    let wrong_type = Request::builder()
        .method(Method::POST)
        .uri(SETUP_PATH)
        .header(CONTENT_TYPE, "text/plain")
        .body(Body::from("{\"appName\":"))
        .unwrap();
    let wrong_type = stack.send(with_peer(wrong_type, 160)).await;
    assert_eq!(wrong_type.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(wrong_type.error_code(), "UNSUPPORTED_MEDIA_TYPE");

    let mut short = admin("ada", "en-US");
    short["password"] = json!("seven77");
    let rejected = stack.send(setup_request(&short, 51)).await;
    assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
    let error = rejected.json();
    assert_eq!(error["error"]["code"], "PASSWORD_POLICY_VIOLATION");
    assert_eq!(error["error"]["details"]["minLength"], 8);
    assert!(!rejected.text().contains("seven77"));

    assert_eq!(stack.durable_setup_rows().await, [0; 5]);
    assert_eq!(
        stack.get(STATUS_PATH, None).await.json()["setupCompleted"],
        false
    );

    let mut no_classes = admin("ada", "en-US");
    no_classes["password"] = json!("aaaaaaaa");
    let created = stack.send(setup_request(&no_classes, 52)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_request_policy_without_double_submit() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), None).await;
    let body = admin("ada", "en-US").to_string();

    let cross_origin = Request::builder()
        .method(Method::POST)
        .uri(SETUP_PATH)
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, "https://attacker.example")
        .body(Body::from(body.clone()))
        .unwrap();
    let rejected = stack.send(with_peer(cross_origin, 60)).await;
    assert_eq!(rejected.status, StatusCode::FORBIDDEN);
    assert_eq!(rejected.error_code(), "ORIGIN_NOT_ALLOWED");

    for content_type in [
        "application/x-www-form-urlencoded",
        "text/plain",
        "multipart/form-data; boundary=x",
    ] {
        let request = Request::builder()
            .method(Method::POST)
            .uri(SETUP_PATH)
            .header(CONTENT_TYPE, content_type)
            .header(ORIGIN, BASE_URL)
            .body(Body::from(body.clone()))
            .unwrap();
        let rejected = stack.send(with_peer(request, 61)).await;
        assert_eq!(
            rejected.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type}"
        );
        assert_eq!(rejected.error_code(), "UNSUPPORTED_MEDIA_TYPE");
    }
    assert_eq!(stack.durable_setup_rows().await, [0; 5]);

    let without_csrf = Request::builder()
        .method(Method::POST)
        .uri(SETUP_PATH)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .header(ORIGIN, BASE_URL)
        .body(Body::from(body))
        .unwrap();
    let created = stack.send(with_peer(without_csrf, 62)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    let inventory = application_routes().build().unwrap().inventory;
    let post = inventory.get(&Method::POST, SETUP_PATH).unwrap().policy();
    assert_eq!(post.auth(), AuthClass::Setup);
    assert_eq!(post.rate_limit(), RateLimitClass::AuthLogin);
    let status = inventory.get(&Method::GET, STATUS_PATH).unwrap().policy();
    assert_eq!(status.auth(), AuthClass::Public);
    assert_eq!(status.rate_limit(), RateLimitClass::PublicRead);
    assert_eq!(SETUP_ROUTE.auth(), AuthClass::Setup);
    assert_eq!(SETUP_STATUS_ROUTE.auth(), AuthClass::Public);
    stack.stop().await;
}

#[tokio::test]
async fn it_setup_default_language_only_suggests_before_setup() {
    let root = TempDir::new().unwrap();
    let suggested = Stack::start(root.path(), Some("de-DE")).await;
    let bootstrap = suggested.get("/api/v1/bootstrap", None).await.json();
    assert_eq!(bootstrap["setupCompleted"], false);
    assert_eq!(bootstrap["defaultLocale"], "de-DE");
    assert!(suggested.setting_json("default_locale").await.is_none());
    suggested.stop().await;

    let ignored = Stack::start(root.path(), Some("xx-XX")).await;
    assert_eq!(
        ignored.get("/api/v1/bootstrap", None).await.json()["defaultLocale"],
        "en-US"
    );
    let created = ignored.setup(&admin("ada", "pt-BR")).await;
    assert_eq!(created.status, StatusCode::CREATED);
    ignored.stop().await;

    for language in ["fr-FR", "en-US"] {
        let restarted = Stack::start(root.path(), Some(language)).await;
        assert_eq!(
            restarted.settings.current().default_locale().as_str(),
            "pt-BR"
        );
        assert_eq!(
            restarted.get("/api/v1/bootstrap", None).await.json()["defaultLocale"],
            "pt-BR",
            "{language}"
        );
        assert_eq!(
            restarted.setting_json("default_locale").await.as_deref(),
            Some("\"pt-BR\""),
            "{language}"
        );
        restarted.stop().await;
    }
}

#[test]
fn unit_setup_request_fields_match_openapi_schema() {
    use utoipa::PartialSchema;

    use super::model::SetupRequest;
    use crate::infra::http::json::JsonRequest;

    let schema = serde_json::to_value(SetupRequest::schema()).unwrap();
    let mut properties: Vec<String> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    properties.sort_unstable();
    let mut required: Vec<String> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field.as_str().unwrap().to_owned())
        .collect();
    required.sort_unstable();
    let mut declared: Vec<String> = SetupRequest::FIELDS
        .iter()
        .map(|field| field.name().to_owned())
        .collect();
    declared.sort_unstable();
    let mut declared_required: Vec<String> = SetupRequest::FIELDS
        .iter()
        .filter(|field| field.is_required())
        .map(|field| field.name().to_owned())
        .collect();
    declared_required.sort_unstable();
    assert_eq!(properties, declared);
    assert_eq!(required, declared_required);
}
