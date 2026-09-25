use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use http::header::{COOKIE, SET_COOKIE};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::Value;
use tempfile::TempDir;
use time::macros::datetime;
use tower::ServiceExt;
use utoipa_axum::routes;

use super::model::{AuthMethod, MintedSession, NewSession, RevokedReason, SessionRestriction};
use super::prune::{prune_step, PruneStep};
use super::SessionService;
use crate::app::auth_class::AuthClass;
use crate::app::health::Health;
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{
    with_middleware, BytePath, Deadline, HttpEdge, RateLimitClass, RequestBody, ResponseEncoding,
    RoutePolicy, Routes, Transport,
};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};
use crate::domain::clock::{Clock, TestClock};
use crate::domain::email::Email;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::domain::username::Username;
use crate::features::audit::model::ClientMetadata;
use crate::features::settings::model::AppSettings;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{NewUser, QuotaOverride, UserId};
use crate::features::users::repo as users;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::csrf::{CsrfGuard, RequestContent, CSRF_HEADER};
use crate::infra::http::extractors::{
    enforce_restriction, restriction_allows, Admin, AdminRecentAuth, Authenticated,
    AuthenticatedRecentAuth,
};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const RESPONSE_READ_CAP: usize = 64 * 1024;
const START: time::OffsetDateTime = datetime!(2026-09-25 12:00 UTC);

struct Harness {
    _root: TempDir,
    pools: DbPools,
    clock: TestClock,
    settings: SettingsHandle,
    config: OperatorConfig,
    service: SessionService,
}

impl Harness {
    async fn open() -> Self {
        Self::open_with_settings(AppSettings::defaults()).await
    }

    async fn open_with_settings(settings: AppSettings) -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(root.path()).unwrap();
        let keys = Arc::new(KeyRing::new(&instance_key));
        let clock = TestClock::new(START);
        let settings = SettingsHandle::new(settings);
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_BASE_URL",
            "https://files.example.test",
        )]))
        .unwrap()
        .config;
        let service = SessionService::new(
            pools.clone(),
            Arc::new(clock.clone()),
            settings.clone(),
            keys,
            &config.base_url,
        );
        Self {
            _root: root,
            pools,
            clock,
            settings,
            config,
            service,
        }
    }

    async fn user(&self, suffix: &str, role: Role, must_change_password: bool) -> UserId {
        let new = NewUser {
            email: Email::parse(&format!("{suffix}@example.test")).unwrap(),
            username: Username::parse(&format!("user-{suffix}")).unwrap(),
            first_name: String::new(),
            last_name: String::new(),
            password_hash: None,
            must_change_password,
            role,
            is_active: true,
            quota: QuotaOverride::Inherit,
            created_by: None,
        };
        self.pools
            .write_tx(&self.clock, "sessions.test_user", async |tx| {
                users::insert(tx, &self.clock, &new).await
            })
            .await
            .unwrap()
            .id
    }

    async fn mint(&self, user_id: UserId) -> super::model::MintedSession {
        self.service
            .mint(NewSession {
                user_id,
                auth_method: AuthMethod::Password,
                ip_address: Some("203.0.113.7".to_owned()),
                user_agent: Some("Palmr test agent".to_owned()),
            })
            .await
            .unwrap()
    }

    async fn timestamp(&self, id: super::model::SessionId, column: &str) -> String {
        let query = format!("SELECT {column} FROM sessions WHERE id = ?1");
        sqlx::query_scalar(&query)
            .bind(id.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn audit_rows(&self) -> Vec<(String, Option<String>, Option<String>, String)> {
        sqlx::query_as(
            "SELECT action, actor_label, target_id, metadata_json FROM audit_events ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn state(&self, id: super::model::SessionId) -> Option<String> {
        sqlx::query_scalar("SELECT state FROM sessions WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }
}

fn token_cookie(token: &Secret<String>) -> HeaderValue {
    HeaderValue::from_str(&format!("palmr_session={}", token.expose_secret())).unwrap()
}

fn session_cookies(session: &MintedSession) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "palmr_session={}; palmr_csrf={}",
        session.session_token.expose_secret(),
        session.csrf_token.expose_secret()
    ))
    .unwrap()
}

async fn response_json(response: Response) -> Value {
    let bytes = Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn it_session_idle_and_absolute_expiry() {
    let harness = Harness::open().await;
    let user = harness.user("expiry", Role::User, false).await;

    let idle = harness.mint(user).await;
    assert!(idle.idle_expires_at < idle.absolute_expires_at);
    let mut emitted = HeaderMap::new();
    harness.service.emit_cookies(&mut emitted, &idle).unwrap();
    assert_eq!(emitted.get_all(SET_COOKIE).iter().count(), 2);
    let raw_session = idle.session_token.expose_secret().clone();
    let raw_csrf = idle.csrf_token.expose_secret().clone();
    let (stored_session, stored_csrf): (String, String) =
        sqlx::query_as("SELECT token_hash, csrf_token_hash FROM sessions WHERE id = ?1")
            .bind(idle.id.to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_ne!(stored_session, raw_session);
    assert_ne!(stored_csrf, raw_csrf);
    assert_eq!(
        stored_session,
        Token::decode(&raw_session).unwrap().digest().as_str()
    );
    assert_eq!(
        stored_csrf,
        Token::decode(&raw_csrf).unwrap().digest().as_str()
    );

    harness.clock.advance(Duration::from_secs(7 * 86_400));
    assert!(matches!(
        harness.service.authenticate(&idle.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
    assert_eq!(harness.state(idle.id).await.as_deref(), Some("expired"));

    let sliding = harness.mint(user).await;
    let absolute = sliding.absolute_expires_at;
    for _ in 0..4 {
        harness.clock.advance(Duration::from_secs(6 * 86_400));
        harness
            .service
            .authenticate(&sliding.session_token)
            .await
            .unwrap();
    }
    harness.clock.advance(Duration::from_secs(5 * 86_400));
    harness
        .service
        .authenticate(&sliding.session_token)
        .await
        .unwrap();
    let rotated = harness.service.rotate(sliding.id).await.unwrap();
    assert_eq!(rotated.absolute_expires_at, absolute);
    assert!(matches!(
        harness.service.authenticate(&sliding.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
    harness
        .service
        .authenticate(&rotated.session_token)
        .await
        .unwrap();
    harness.clock.advance(Duration::from_secs(86_400));
    assert!(matches!(
        harness.service.authenticate(&rotated.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
    assert_eq!(
        harness.timestamp(sliding.id, "absolute_expires_at").await,
        absolute.to_string()
    );
}

#[tokio::test]
async fn it_session_last_seen_throttled() {
    let harness = Harness::open().await;
    let user = harness.user("touch", Role::User, false).await;
    let session = harness.mint(user).await;
    let original_seen = harness.timestamp(session.id, "last_seen_at").await;
    let original_auth = harness.timestamp(session.id, "last_auth_at").await;

    harness.clock.advance(Duration::from_secs(60));
    harness
        .service
        .authenticate(&session.session_token)
        .await
        .unwrap();
    assert_eq!(
        harness.timestamp(session.id, "last_seen_at").await,
        original_seen
    );

    harness.clock.advance(Duration::from_millis(1));
    harness
        .service
        .authenticate(&session.session_token)
        .await
        .unwrap();
    assert_ne!(
        harness.timestamp(session.id, "last_seen_at").await,
        original_seen
    );
    assert_eq!(
        harness.timestamp(session.id, "last_auth_at").await,
        original_auth
    );

    harness.clock.advance(Duration::from_secs(61));
    let concurrent = futures_util::future::join_all(
        (0..8).map(|_| harness.service.authenticate(&session.session_token)),
    )
    .await;
    assert!(concurrent.iter().all(Result::is_ok));
    assert_eq!(
        harness.timestamp(session.id, "last_seen_at").await,
        Timestamp::try_from(harness.clock.now())
            .unwrap()
            .to_string()
    );
}

#[tokio::test]
async fn it_session_request_metadata_and_transactional_revocation_primitives() {
    let harness = Harness::open().await;
    let user = harness.user("primitives", Role::User, false).await;
    let request = Request::builder()
        .header(
            http::header::USER_AGENT,
            format!("Palmr/{}", "x".repeat(600)),
        )
        .body(Body::empty())
        .unwrap();
    let new = harness
        .service
        .new_session(user, AuthMethod::Password, &request);
    assert_eq!(new.ip_address, None);
    assert!(new.user_agent.as_ref().unwrap().chars().count() > 512);
    let metadata_session = harness.service.mint(new).await.unwrap();
    let (ip, user_agent_chars): (Option<String>, i64) =
        sqlx::query_as("SELECT ip, length(user_agent) FROM sessions WHERE id = ?1")
            .bind(metadata_session.id.to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(ip, None);
    assert_eq!(user_agent_chars, 512);

    let current = harness.mint(user).await;
    let one = harness.mint(user).await;
    let two = harness.mint(user).await;
    harness
        .pools
        .write_tx(
            &harness.clock,
            "sessions.test_in_tx_revocation",
            async |tx| {
                assert!(
                    harness
                        .service
                        .revoke_one_in_tx(tx, user, one.id, RevokedReason::PasswordChanged)
                        .await?
                );
                assert_eq!(
                    harness
                        .service
                        .revoke_all_others_in_tx(tx, user, current.id, RevokedReason::RoleChanged,)
                        .await?,
                    2
                );
                Ok::<(), super::SessionError>(())
            },
        )
        .await
        .unwrap();
    harness
        .service
        .authenticate(&current.session_token)
        .await
        .unwrap();
    for revoked in [&one.session_token, &two.session_token] {
        assert!(matches!(
            harness.service.authenticate(revoked).await,
            Err(super::SessionError::AuthRequired)
        ));
    }
    harness
        .pools
        .write_tx(
            &harness.clock,
            "sessions.test_in_tx_revoke_all",
            async |tx| {
                assert_eq!(
                    harness
                        .service
                        .revoke_all_in_tx(tx, user, RevokedReason::Deactivated)
                        .await?,
                    1
                );
                Ok::<(), super::SessionError>(())
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        harness.service.authenticate(&current.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
}

#[tokio::test]
async fn it_mfa_pending_promotion_rotates_credentials_atomically() {
    let harness = Harness::open().await;
    let user = harness.user("pending", Role::User, false).await;
    let id = super::model::SessionId::generate(&harness.clock);
    let mfa_token = Token::mint().unwrap();
    let placeholder_csrf = Token::mint().unwrap();
    let now = Timestamp::try_from(harness.clock.now()).unwrap();
    let mfa_expires_at = Timestamp::try_from(now.get() + time::Duration::minutes(5)).unwrap();

    harness
        .pools
        .write_tx(&harness.clock, "sessions.test_pending", async |tx| {
            sqlx::query(
                "INSERT INTO sessions (
                    id, user_id, token_hash, csrf_token_hash, state, auth_method,
                    mfa_token_hash, mfa_expires_at, created_at, last_seen_at,
                    last_auth_at, idle_expires_at, absolute_expires_at
                 ) VALUES (?1, ?2, ?3, ?4, 'mfa_pending', 'password', ?3, ?5,
                           ?6, ?6, ?6, ?5, ?5)",
            )
            .bind(id.to_string())
            .bind(user.to_string())
            .bind(mfa_token.digest().as_str())
            .bind(placeholder_csrf.digest().as_str())
            .bind(mfa_expires_at.to_string())
            .bind(now.to_string())
            .execute(tx.executor())
            .await?;
            Ok::<(), super::SessionError>(())
        })
        .await
        .unwrap();

    let pending_credential = mfa_token.encode();
    assert!(matches!(
        harness.service.authenticate(&pending_credential).await,
        Err(super::SessionError::AuthRequired)
    ));

    let credentials = harness.service.prepare_credentials().unwrap();
    let promoted = harness
        .pools
        .write_tx(&harness.clock, "sessions.test_promote", async |tx| {
            harness
                .service
                .promote_pending_in_tx(
                    tx,
                    id,
                    &mfa_token.digest(),
                    AuthMethod::PasswordTotp,
                    &credentials,
                )
                .await
        })
        .await
        .unwrap();

    let principal = harness
        .service
        .authenticate(&promoted.session_token)
        .await
        .unwrap();
    assert_eq!(principal.auth_method, AuthMethod::PasswordTotp);
    assert!(matches!(
        harness.service.authenticate(&pending_credential).await,
        Err(super::SessionError::AuthRequired)
    ));
    let (state, mfa_hash, mfa_expiry): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT state, mfa_token_hash, mfa_expires_at FROM sessions WHERE id = ?1")
            .bind(id.to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(state, "active");
    assert_eq!(mfa_hash, None);
    assert_eq!(mfa_expiry, None);

    assert!(matches!(
        harness
            .pools
            .write_tx(&harness.clock, "sessions.test_repromote", async |tx| {
                harness
                    .service
                    .promote_pending_in_tx(
                        tx,
                        id,
                        &mfa_token.digest(),
                        AuthMethod::PasswordTotp,
                        &credentials,
                    )
                    .await
            })
            .await,
        Err(super::SessionError::AuthRequired)
    ));
}

#[utoipa::path(get, path = "/test/authenticated", responses((status = 204)))]
async fn authenticated(
    Authenticated(principal): Authenticated,
    axum::Extension(scope): axum::Extension<crate::infra::http::idempotency::IdempotencyScope>,
) -> StatusCode {
    assert_eq!(
        scope,
        crate::infra::http::idempotency::IdempotencyScope::user(principal.user_id)
    );
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/test/recent", responses((status = 204)))]
async fn recent(AuthenticatedRecentAuth(_): AuthenticatedRecentAuth) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/test/admin", responses((status = 204)))]
async fn admin(Admin(principal): Admin) -> StatusCode {
    assert_eq!(principal.role, Role::Admin);
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/test/admin-recent", responses((status = 204)))]
async fn admin_recent(AdminRecentAuth(principal): AdminRecentAuth) -> StatusCode {
    assert!(principal.recent_auth);
    StatusCode::NO_CONTENT
}

fn matrix_router(service: SessionService) -> axum::Router {
    let policy = |class| RoutePolicy::new(class, RateLimitClass::None, Transport::ControlPlane);
    Routes::<()>::new()
        .route(policy(AuthClass::Authenticated), routes!(authenticated))
        .route(policy(AuthClass::AuthenticatedRecentAuth), routes!(recent))
        .route(policy(AuthClass::Admin), routes!(admin))
        .route(policy(AuthClass::AdminRecentAuth), routes!(admin_recent))
        .build()
        .unwrap()
        .router
        .with_state(())
        .layer(axum::Extension(service))
}

async fn matrix_status(
    router: &axum::Router,
    path: &str,
    token: Option<&Secret<String>>,
) -> StatusCode {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    if let Some(token) = token {
        request.headers_mut().insert(COOKIE, token_cookie(token));
    }
    router.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn svc_auth_class_extractor_matrix() {
    let harness = Harness::open().await;
    let user = harness.user("matrix-user", Role::User, false).await;
    let admin_user = harness.user("matrix-admin", Role::Admin, false).await;
    let user_session = harness.mint(user).await;
    let admin_session = harness.mint(admin_user).await;
    let router = matrix_router(harness.service.clone());

    assert_eq!(
        matrix_status(&router, "/test/authenticated", None).await,
        401
    );
    assert_eq!(
        matrix_status(
            &router,
            "/test/authenticated",
            Some(&Secret::new("not-a-session".to_owned()))
        )
        .await,
        401
    );
    assert_eq!(
        matrix_status(
            &router,
            "/test/authenticated",
            Some(&user_session.session_token)
        )
        .await,
        204
    );
    assert_eq!(
        matrix_status(&router, "/test/recent", Some(&user_session.session_token)).await,
        204
    );
    assert_eq!(
        matrix_status(&router, "/test/admin", Some(&user_session.session_token)).await,
        403
    );
    assert_eq!(
        matrix_status(&router, "/test/admin", Some(&admin_session.session_token)).await,
        204
    );
    assert_eq!(
        matrix_status(
            &router,
            "/test/admin-recent",
            Some(&admin_session.session_token)
        )
        .await,
        204
    );

    harness.clock.advance(Duration::from_secs(5 * 60 + 1));
    assert_eq!(
        matrix_status(&router, "/test/recent", Some(&user_session.session_token)).await,
        403
    );
    assert_eq!(
        matrix_status(
            &router,
            "/test/authenticated",
            Some(&user_session.session_token)
        )
        .await,
        204
    );
    harness
        .service
        .mark_reauthenticated(user_session.id)
        .await
        .unwrap();
    assert_eq!(
        matrix_status(&router, "/test/recent", Some(&user_session.session_token)).await,
        204
    );

    harness
        .pools
        .write_tx(&harness.clock, "sessions.test_demote", async |tx| {
            sqlx::query("UPDATE users SET role = 'user' WHERE id = ?1")
                .bind(admin_user.to_string())
                .execute(tx.executor())
                .await?;
            Ok::<(), crate::infra::db::DbError>(())
        })
        .await
        .unwrap();
    assert_eq!(
        matrix_status(&router, "/test/admin", Some(&admin_session.session_token)).await,
        403
    );

    harness
        .pools
        .write_tx(&harness.clock, "sessions.test_deactivate", async |tx| {
            sqlx::query("UPDATE users SET is_active = 0, deactivated_at = ?2 WHERE id = ?1")
                .bind(user.to_string())
                .bind(crate::domain::time::Timestamp::try_from(harness.clock.now())?.to_string())
                .execute(tx.executor())
                .await?;
            Ok::<(), super::SessionError>(())
        })
        .await
        .unwrap();
    assert_eq!(
        matrix_status(
            &router,
            "/test/authenticated",
            Some(&user_session.session_token)
        )
        .await,
        401
    );
}

#[utoipa::path(get, path = "/api/v1/auth/me", responses((status = 204)))]
async fn stub_me(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 204)))]
async fn stub_logout(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/bootstrap", responses((status = 204)))]
async fn stub_bootstrap(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/settings/effective", responses((status = 204)))]
async fn stub_effective_settings(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/profile/password", responses((status = 204)))]
async fn stub_profile_password(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/auth/2fa", responses((status = 204)))]
async fn stub_2fa_status(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/auth/2fa/enroll", responses((status = 204)))]
async fn stub_2fa_enroll(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/auth/2fa/enroll/verify", responses((status = 204)))]
async fn stub_2fa_verify(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/files", responses((status = 204)))]
async fn stub_files(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/admin/users", responses((status = 204)))]
async fn stub_admin_users(Admin(_): Admin) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn allowlist_routes() -> Routes<AppState> {
    let policy = |class| RoutePolicy::new(class, RateLimitClass::None, Transport::ControlPlane);
    Routes::new()
        .route(policy(AuthClass::Authenticated), routes!(stub_me))
        .route(policy(AuthClass::Authenticated), routes!(stub_logout))
        .route(policy(AuthClass::Authenticated), routes!(stub_bootstrap))
        .route(
            policy(AuthClass::Authenticated),
            routes!(stub_effective_settings),
        )
        .route(
            policy(AuthClass::AuthenticatedRecentAuth),
            routes!(stub_profile_password),
        )
        .route(policy(AuthClass::Authenticated), routes!(stub_2fa_status))
        .route(
            policy(AuthClass::AuthenticatedRecentAuth),
            routes!(stub_2fa_enroll),
        )
        .route(
            policy(AuthClass::AuthenticatedRecentAuth),
            routes!(stub_2fa_verify),
        )
        .route(policy(AuthClass::Authenticated), routes!(stub_files))
        .route(policy(AuthClass::Admin), routes!(stub_admin_users))
}

const SHARED_ALLOWLIST: [(Method, &str); 4] = [
    (Method::GET, "/api/v1/auth/me"),
    (Method::POST, "/api/v1/auth/logout"),
    (Method::GET, "/api/v1/bootstrap"),
    (Method::GET, "/api/v1/settings/effective"),
];
const PASSWORD_ONLY: [(Method, &str); 1] = [(Method::POST, "/api/v1/profile/password")];
const TOTP_ONLY: [(Method, &str); 3] = [
    (Method::GET, "/api/v1/auth/2fa"),
    (Method::POST, "/api/v1/auth/2fa/enroll"),
    (Method::POST, "/api/v1/auth/2fa/enroll/verify"),
];

async fn restricted_code(
    service: impl tower::Service<Request, Response = Response, Error = std::convert::Infallible, Future: Send>
        + Clone,
    method: Method,
    path: &str,
    session: &MintedSession,
) -> (StatusCode, Option<String>) {
    let response = call_sessions(service, method, path, session).await;
    let status = response.status();
    if status == StatusCode::NO_CONTENT {
        return (status, None);
    }
    let code = response_json(response).await["error"]["code"]
        .as_str()
        .map(ToOwned::to_owned);
    (status, code)
}

fn concrete(path: &str, clock: &TestClock) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                super::model::SessionId::generate(clock).to_string()
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn svc_restricted_session_allowlist() {
    for (method, path) in SHARED_ALLOWLIST.iter().chain(&PASSWORD_ONLY) {
        assert!(restriction_allows(
            SessionRestriction::MustChangePassword,
            method,
            path
        ));
        assert!(enforce_restriction(SessionRestriction::MustChangePassword, method, path).is_ok());
    }
    for (method, path) in SHARED_ALLOWLIST.iter().chain(&TOTP_ONLY) {
        assert!(restriction_allows(
            SessionRestriction::MustEnrollTotp,
            method,
            path
        ));
    }
    for (method, path) in [
        (Method::GET, "/api/v1/files"),
        (Method::GET, "/api/v1/sessions"),
        (Method::DELETE, "/api/v1/sessions"),
        (Method::GET, "/api/v1/admin/users"),
        (Method::GET, "/api/v1/profile/password"),
        (Method::POST, "/api/v1/auth/me"),
        (Method::POST, "/api/v1/profile/password/extra"),
        (Method::POST, "/api/v1/auth/2fa/enroll/verify/extra"),
        (Method::GET, "/api/v1/auth/me/"),
    ] {
        assert!(!restriction_allows(
            SessionRestriction::MustChangePassword,
            &method,
            path
        ));
        assert!(!restriction_allows(
            SessionRestriction::MustEnrollTotp,
            &method,
            path
        ));
        assert!(
            enforce_restriction(SessionRestriction::MustChangePassword, &method, path).is_err()
        );
        assert!(enforce_restriction(SessionRestriction::MustEnrollTotp, &method, path).is_err());
    }

    let mut settings = AppSettings::defaults();
    settings.security.two_factor_required = true;
    let harness = Harness::open_with_settings(settings).await;
    let password_user = harness.user("restricted-password", Role::Admin, true).await;
    let totp_user = harness.user("restricted-totp", Role::Admin, false).await;
    let password_session = harness.mint(password_user).await;
    let totp_session = harness.mint(totp_user).await;
    for (token, restriction) in [
        (
            &password_session.session_token,
            SessionRestriction::MustChangePassword,
        ),
        (
            &totp_session.session_token,
            SessionRestriction::MustEnrollTotp,
        ),
    ] {
        assert_eq!(
            harness
                .service
                .authenticate(token)
                .await
                .unwrap()
                .restriction,
            restriction
        );
    }

    let stubs = app_service(&harness, allowlist_routes());
    for (method, path) in SHARED_ALLOWLIST.iter().chain(&PASSWORD_ONLY) {
        assert_eq!(
            restricted_code(stubs.clone(), method.clone(), path, &password_session).await,
            (StatusCode::NO_CONTENT, None),
            "{method} {path}"
        );
    }
    for (method, path) in SHARED_ALLOWLIST.iter().chain(&TOTP_ONLY) {
        assert_eq!(
            restricted_code(stubs.clone(), method.clone(), path, &totp_session).await,
            (StatusCode::NO_CONTENT, None),
            "{method} {path}"
        );
    }
    let password_denied = [
        (Method::GET, "/api/v1/files"),
        (Method::GET, "/api/v1/admin/users"),
    ];
    for (method, path) in password_denied.iter().chain(&TOTP_ONLY) {
        assert_eq!(
            restricted_code(stubs.clone(), method.clone(), path, &password_session).await,
            (
                StatusCode::FORBIDDEN,
                Some("AUTH_PASSWORD_CHANGE_REQUIRED".to_owned())
            ),
            "{method} {path}"
        );
    }
    for (method, path) in password_denied.iter().chain(&PASSWORD_ONLY) {
        assert_eq!(
            restricted_code(stubs.clone(), method.clone(), path, &totp_session).await,
            (
                StatusCode::FORBIDDEN,
                Some("AUTH_2FA_ENROLLMENT_REQUIRED".to_owned())
            ),
            "{method} {path}"
        );
    }

    let inventory = crate::app::router::application_routes()
        .build()
        .unwrap()
        .inventory;
    let protected: Vec<_> = inventory
        .entries()
        .iter()
        .filter(|entry| {
            !matches!(
                entry.policy().auth(),
                AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup
            )
        })
        .collect();
    assert!(!protected.is_empty());
    let application = app_service(&harness, crate::app::router::application_routes());
    for entry in protected {
        let path = concrete(entry.path(), &harness.clock);
        assert!(!restriction_allows(
            SessionRestriction::MustChangePassword,
            entry.method(),
            entry.path()
        ));
        for (token, code) in [
            (&password_session, "AUTH_PASSWORD_CHANGE_REQUIRED"),
            (&totp_session, "AUTH_2FA_ENROLLMENT_REQUIRED"),
        ] {
            assert_eq!(
                restricted_code(application.clone(), entry.method().clone(), &path, token).await,
                (StatusCode::FORBIDDEN, Some(code.to_owned())),
                "{} {}",
                entry.method(),
                entry.path()
            );
        }
    }
    assert_eq!(
        harness.state(password_session.id).await.as_deref(),
        Some("active")
    );
    assert_eq!(
        harness.state(totp_session.id).await.as_deref(),
        Some("active")
    );
}

fn sessions_service(
    harness: &Harness,
) -> impl tower::Service<Request, Response = Response, Error = std::convert::Infallible, Future: Send>
       + Clone {
    app_service(harness, super::routes::routes())
}

fn app_service(
    harness: &Harness,
    routes: Routes<AppState>,
) -> impl tower::Service<Request, Response = Response, Error = std::convert::Infallible, Future: Send>
       + Clone {
    let assembled = routes.build().unwrap();
    let docs = ApiDocs::new(assembled.openapi, &harness.config.base_url).unwrap();
    let state = AppState::new(
        Arc::new(harness.clock.clone()),
        Health::new(Readiness::new()),
        docs,
        harness.settings.clone(),
        StorageRuntime::for_test(),
    );
    let router = assembled
        .router
        .with_state(state)
        .layer(axum::Extension(harness.service.clone()));
    let edge = HttpEdge::new(
        Arc::new(harness.clock.clone()),
        TrustedProxies::new(&harness.config.trust_proxy),
        SecurityHeaders::new(&harness.config),
        CsrfGuard::new(&harness.config.base_url),
    );
    with_middleware(router, &edge)
}

async fn call_sessions(
    service: impl tower::Service<Request, Response = Response, Error = std::convert::Infallible, Future: Send>
        + Clone,
    method: Method,
    uri: &str,
    session: &MintedSession,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    request
        .headers_mut()
        .insert(COOKIE, session_cookies(session));
    request.headers_mut().insert(
        CSRF_HEADER,
        HeaderValue::from_str(session.csrf_token.expose_secret()).unwrap(),
    );
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:32100".parse::<SocketAddr>().unwrap(),
    ));
    service.oneshot(request).await.unwrap()
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

fn assert_session_pair_cleared(response: &Response) {
    let cookies = set_cookies(response);
    assert_eq!(cookies.len(), 2);
    assert!(cookies[0].starts_with("palmr_session=;"));
    assert!(cookies[1].starts_with("palmr_csrf=;"));
    assert!(cookies.iter().all(|cookie| cookie.contains("; Max-Age=0")));
}

#[tokio::test]
async fn it_sessions_list_and_revoke() {
    let harness = Harness::open().await;
    let owner = harness.user("sessions-owner", Role::User, false).await;
    let foreign = harness.user("sessions-foreign", Role::User, false).await;
    let current = harness.mint(owner).await;
    harness.clock.advance(Duration::from_secs(1));
    let other = harness.mint(owner).await;
    let remaining = harness.mint(owner).await;
    let foreign_session = harness.mint(foreign).await;

    let response = call_sessions(
        sessions_service(&harness),
        Method::GET,
        "/api/v1/sessions",
        &current,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookies(&response).is_empty());
    let body = response_json(response).await;
    assert_eq!(body["totalCount"], 3);
    assert_eq!(body["nextCursor"], Value::Null);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let current_items: Vec<&Value> = items
        .iter()
        .filter(|item| item["isCurrent"] == true)
        .collect();
    assert_eq!(current_items.len(), 1);
    assert_eq!(current_items[0]["id"], current.id.to_string());
    let mut keys: Vec<&str> = current_items[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "absoluteExpiresAt",
            "createdAt",
            "expiresAt",
            "id",
            "ipAddress",
            "isCurrent",
            "lastSeenAt",
            "origin",
            "userAgent",
        ]
    );
    assert_eq!(current_items[0]["origin"], "password");
    assert_eq!(current_items[0]["ipAddress"], "203.0.113.7");
    assert_eq!(current_items[0]["userAgent"], "Palmr test agent");
    assert!(!items
        .iter()
        .any(|item| item["id"] == foreign_session.id.to_string()));
    let (token_hash, csrf_hash): (String, String) =
        sqlx::query_as("SELECT token_hash, csrf_token_hash FROM sessions WHERE id = ?1")
            .bind(current.id.to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    let wire = body.to_string();
    for forbidden in [
        "token",
        "Hash",
        "csrf",
        "mfa",
        current.session_token.expose_secret(),
        current.csrf_token.expose_secret(),
        token_hash.as_str(),
        csrf_hash.as_str(),
    ] {
        assert!(
            !wire.contains(forbidden),
            "secret field/value leaked: {forbidden}"
        );
    }

    for missing in [
        format!("/api/v1/sessions/{}", foreign_session.id),
        "/api/v1/sessions/not-a-session-id".to_owned(),
        format!(
            "/api/v1/sessions/{}",
            super::model::SessionId::generate(&harness.clock)
        ),
    ] {
        let response = call_sessions(
            sessions_service(&harness),
            Method::DELETE,
            &missing,
            &current,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{missing}");
        assert_eq!(
            response_json(response).await["error"]["code"],
            "SESSION_NOT_FOUND"
        );
    }
    harness
        .service
        .authenticate(&foreign_session.session_token)
        .await
        .unwrap();

    for _ in 0..2 {
        let response = call_sessions(
            sessions_service(&harness),
            Method::DELETE,
            &format!("/api/v1/sessions/{}", other.id),
            &current,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(set_cookies(&response).is_empty());
    }
    assert!(matches!(
        harness.service.authenticate(&other.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
    let revoked_reason: Option<String> =
        sqlx::query_scalar("SELECT revoked_reason FROM sessions WHERE id = ?1")
            .bind(other.id.to_string())
            .fetch_one(harness.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(revoked_reason.as_deref(), Some("user_request"));

    harness.clock.advance(Duration::from_secs(5 * 60 + 1));
    let response = call_sessions(
        sessions_service(&harness),
        Method::DELETE,
        "/api/v1/sessions",
        &current,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "AUTH_RECENT_AUTH_REQUIRED"
    );
    harness
        .service
        .authenticate(&remaining.session_token)
        .await
        .unwrap();
    harness
        .service
        .mark_reauthenticated(current.id)
        .await
        .unwrap();

    let response = call_sessions(
        sessions_service(&harness),
        Method::DELETE,
        "/api/v1/sessions?includeCurrent=maybe",
        &current,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = call_sessions(
        sessions_service(&harness),
        Method::DELETE,
        "/api/v1/sessions",
        &current,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(set_cookies(&response).is_empty());
    harness
        .service
        .authenticate(&current.session_token)
        .await
        .unwrap();
    assert!(matches!(
        harness.service.authenticate(&remaining.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));
    harness
        .service
        .authenticate(&foreign_session.session_token)
        .await
        .unwrap();

    let replacement = harness.mint(owner).await;
    let response = call_sessions(
        sessions_service(&harness),
        Method::DELETE,
        "/api/v1/sessions?includeCurrent=true",
        &replacement,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_session_pair_cleared(&response);
    for revoked in [&replacement.session_token, &current.session_token] {
        assert!(matches!(
            harness.service.authenticate(revoked).await,
            Err(super::SessionError::AuthRequired)
        ));
    }

    let last = harness.mint(owner).await;
    let response = call_sessions(
        sessions_service(&harness),
        Method::DELETE,
        &format!("/api/v1/sessions/{}", last.id),
        &last,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_session_pair_cleared(&response);
    assert!(matches!(
        harness.service.authenticate(&last.session_token).await,
        Err(super::SessionError::AuthRequired)
    ));

    let audit = harness.audit_rows().await;
    let actions: Vec<&str> = audit.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(
        actions,
        [
            "SESSION_REVOKED",
            "ALL_SESSIONS_REVOKED",
            "ALL_SESSIONS_REVOKED",
            "SESSION_REVOKED",
        ]
    );
    assert!(audit
        .iter()
        .all(|row| row.1.as_deref() == Some("user-sessions-owner")));
    assert_eq!(audit[0].2.as_deref(), Some(other.id.to_string().as_str()));
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].3).unwrap(),
        serde_json::json!({ "reason": "user_request", "current": false })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&audit[1].3).unwrap(),
        serde_json::json!({ "reason": "user_request", "include_current": false, "revoked": 1 })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&audit[2].3).unwrap(),
        serde_json::json!({ "reason": "user_request", "include_current": true, "revoked": 2 })
    );
    assert_eq!(
        serde_json::from_str::<Value>(&audit[3].3).unwrap(),
        serde_json::json!({ "reason": "user_request", "current": true })
    );
    let audit_dump: String = sqlx::query_scalar(
        "SELECT group_concat(coalesce(actor_label,'') || coalesce(target_id,'') || \
                coalesce(metadata_json,'') || coalesce(user_agent,''), '|') FROM audit_events",
    )
    .fetch_one(harness.pools.reader().executor())
    .await
    .unwrap();
    for minted in [&current, &other, &remaining, &replacement, &last] {
        assert!(!audit_dump.contains(minted.session_token.expose_secret().as_str()));
        assert!(!audit_dump.contains(minted.csrf_token.expose_secret().as_str()));
    }
}

#[tokio::test]
async fn it_sessions_prune_lifecycle() {
    let harness = Harness::open().await;
    let user = harness.user("prune", Role::User, false).await;
    let session = harness.mint(user).await;
    let principal = harness
        .service
        .authenticate(&session.session_token)
        .await
        .unwrap();
    harness
        .service
        .revoke_all(
            &principal,
            RevokedReason::UserRequest,
            &ClientMetadata::none(),
        )
        .await
        .unwrap();

    for _ in 0..2 {
        super::prune::ensure_scheduled(&harness.pools, &harness.clock)
            .await
            .unwrap();
    }
    let scheduled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'sessions.prune' AND state = 'pending'",
    )
    .fetch_one(harness.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(scheduled, 1);

    harness.clock.advance(Duration::from_secs(36 * 86_400));
    prune_step(&harness.pools, &harness.clock, PruneStep::TerminalSessions)
        .await
        .unwrap();
    assert_eq!(harness.state(session.id).await.as_deref(), Some("revoked"));
    harness.clock.advance(Duration::from_secs(86_400));
    prune_step(&harness.pools, &harness.clock, PruneStep::TerminalSessions)
        .await
        .unwrap();
    assert_eq!(harness.state(session.id).await, None);
}

#[test]
fn unit_revocation_reason_vocabulary_matches_schema() {
    let reasons = [
        RevokedReason::Logout,
        RevokedReason::UserRequest,
        RevokedReason::AdminRequest,
        RevokedReason::PasswordChanged,
        RevokedReason::PasswordReset,
        RevokedReason::RoleChanged,
        RevokedReason::Deactivated,
        RevokedReason::Deleted,
        RevokedReason::MfaAbandoned,
        RevokedReason::Rotated,
        RevokedReason::PolicyChanged,
        RevokedReason::TrustedDeviceRevoked,
    ];
    assert_eq!(
        reasons.map(RevokedReason::as_str),
        [
            "logout",
            "user_request",
            "admin_request",
            "password_changed",
            "password_reset",
            "role_changed",
            "deactivated",
            "deleted",
            "mfa_abandoned",
            "rotated",
            "policy_changed",
            "trusted_device_revoked",
        ]
    );
}

#[utoipa::path(post, path = "/test/csrf/authenticated", responses((status = 204)))]
async fn csrf_write(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(patch, path = "/test/csrf/uploads/{id}", params(("id" = String, Path)), responses((status = 204)))]
async fn csrf_upload(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn csrf_routes() -> Routes<AppState> {
    let upload = RoutePolicy::new(
        AuthClass::Authenticated,
        RateLimitClass::None,
        Transport::BytePath(BytePath::new(
            RequestBody::Streamed,
            ResponseEncoding::Identity,
            Deadline::IdleOnly,
        )),
    )
    .with_request_content(RequestContent::OffsetOctetStream);
    Routes::new()
        .route(
            RoutePolicy::new(
                AuthClass::Authenticated,
                RateLimitClass::None,
                Transport::ControlPlane,
            ),
            routes!(csrf_write),
        )
        .route(upload, routes!(csrf_upload))
        .merge(super::routes::routes())
}

async fn csrf_call(
    service: impl tower::Service<Request, Response = Response, Error = std::convert::Infallible, Future: Send>
        + Clone,
    method: Method,
    uri: String,
    session: Option<&str>,
    csrf: (Option<&str>, Option<&str>),
) -> (StatusCode, Option<String>) {
    let mut cookies = Vec::new();
    if let Some(session) = session {
        cookies.push(format!("palmr_session={session}"));
    }
    if let Some(cookie) = csrf.0 {
        cookies.push(format!("palmr_csrf={cookie}"));
    }
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(
            "127.0.0.1:32100".parse::<SocketAddr>().unwrap(),
        ));
    if !cookies.is_empty() {
        builder = builder.header(COOKIE, cookies.join("; "));
    }
    if let Some(header) = csrf.1 {
        builder = builder.header(CSRF_HEADER, header);
    }
    let response = service
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    if status.is_success() {
        return (status, None);
    }
    let code = response_json(response).await["error"]["code"]
        .as_str()
        .map(ToOwned::to_owned);
    (status, code)
}

fn rejected(status: StatusCode, code: &str) -> (StatusCode, Option<String>) {
    (status, Some(code.to_owned()))
}

#[tokio::test]
async fn svc_csrf_precedence_over_auth() {
    let harness = Harness::open().await;
    let owner = harness.user("csrf-owner", Role::User, false).await;
    let foreign = harness.user("csrf-foreign", Role::User, false).await;
    let session = harness.mint(owner).await;
    let sibling = harness.mint(owner).await;
    let foreign_session = harness.mint(foreign).await;
    let service = app_service(&harness, csrf_routes());
    let write = "/test/csrf/authenticated";
    let token = session.session_token.expose_secret().as_str();
    let own = session.csrf_token.expose_secret().as_str();
    let fresh = Token::mint().unwrap().encode();
    let fresh = fresh.expose_secret().as_str();
    let other = Token::mint().unwrap().encode();
    let other = other.expose_secret().as_str();
    let missing = rejected(StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING");
    let invalid = rejected(StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID");
    let auth_required = rejected(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    let call = |method: Method, uri: &str, session, csrf| {
        csrf_call(service.clone(), method, uri.to_owned(), session, csrf)
    };

    assert_eq!(call(Method::POST, write, None, (None, None)).await, missing);
    assert_eq!(
        call(Method::POST, write, Some("forged"), (None, None)).await,
        missing
    );
    assert_eq!(
        call(Method::POST, write, None, (Some(fresh), Some(other))).await,
        invalid
    );
    assert_eq!(
        call(
            Method::POST,
            write,
            Some("forged"),
            (Some(fresh), Some(other))
        )
        .await,
        invalid
    );
    assert_eq!(
        call(Method::POST, write, None, (Some(fresh), Some(fresh))).await,
        auth_required
    );
    assert_eq!(
        call(
            Method::POST,
            write,
            Some("forged"),
            (Some(fresh), Some(fresh))
        )
        .await,
        auth_required
    );

    assert_eq!(
        call(Method::POST, write, Some(token), (None, None)).await,
        missing
    );
    assert_eq!(
        call(Method::POST, write, Some(token), (Some(own), None)).await,
        missing
    );
    assert_eq!(
        call(Method::POST, write, Some(token), (Some(own), Some(other))).await,
        invalid
    );
    for borrowed in [&sibling, &foreign_session] {
        let borrowed = borrowed.csrf_token.expose_secret().as_str();
        assert_eq!(
            call(
                Method::POST,
                write,
                Some(token),
                (Some(borrowed), Some(borrowed))
            )
            .await,
            invalid
        );
    }
    assert_eq!(
        call(Method::POST, write, Some(token), (Some(fresh), Some(fresh))).await,
        invalid
    );
    assert_eq!(
        call(Method::POST, write, Some(token), (Some(own), Some(own))).await,
        (StatusCode::NO_CONTENT, None)
    );

    let upload = "/test/csrf/uploads/0192f3a1";
    assert_eq!(
        call(Method::PATCH, upload, Some(token), (None, None)).await,
        missing
    );
    assert_eq!(
        call(
            Method::PATCH,
            upload,
            Some(token),
            (Some(fresh), Some(fresh))
        )
        .await,
        invalid
    );
    assert_eq!(
        call(Method::PATCH, upload, Some(token), (Some(own), Some(own))).await,
        (StatusCode::NO_CONTENT, None)
    );

    assert_eq!(
        call(Method::GET, "/api/v1/sessions", Some(token), (None, None)).await,
        (StatusCode::OK, None)
    );
    let revoke_sibling = &format!("/api/v1/sessions/{}", sibling.id);
    assert_eq!(
        call(Method::DELETE, revoke_sibling, Some(token), (None, None)).await,
        missing
    );
    let sibling_csrf = sibling.csrf_token.expose_secret().as_str();
    assert_eq!(
        call(
            Method::DELETE,
            revoke_sibling,
            Some(token),
            (Some(sibling_csrf), Some(sibling_csrf))
        )
        .await,
        invalid
    );
    assert_eq!(harness.state(sibling.id).await.as_deref(), Some("active"));

    let rotated = harness.service.rotate(session.id).await.unwrap();
    let rotated_token = rotated.session_token.expose_secret().as_str();
    let rotated_csrf = rotated.csrf_token.expose_secret().as_str();
    assert_eq!(
        call(
            Method::POST,
            write,
            Some(rotated_token),
            (Some(own), Some(own))
        )
        .await,
        invalid
    );
    assert_eq!(
        call(
            Method::POST,
            write,
            Some(rotated_token),
            (Some(rotated_csrf), Some(rotated_csrf))
        )
        .await,
        (StatusCode::NO_CONTENT, None)
    );
    assert_eq!(
        call(Method::POST, write, Some(token), (Some(own), Some(own))).await,
        auth_required
    );
}
