use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::response::Response;
use http::header::{CACHE_CONTROL, COOKIE, SET_COOKIE};
use http::{HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tower::ServiceExt;

use super::{EffectiveSettingsService, OperatorPolicy};
use crate::app::auth_class::AuthClass;
use crate::app::health::Health;
use crate::app::lifecycle::Readiness;
use crate::app::openapi::ApiDocs;
use crate::app::router::{application_routes, with_middleware, HttpEdge, RateLimitClass};
use crate::app::state::{AppState, StorageRuntime};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};
use crate::domain::bytes::ByteSize;
use crate::domain::clock::TestClock;
use crate::domain::email::Email;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::username::Username;
use crate::features::auth::sessions::{
    AuthMethod, MintedSession, NewSession, SessionRestriction, SessionService,
};
use crate::features::settings::model::AppSettings;
use crate::features::settings::routes::EFFECTIVE_SETTINGS_ROUTE;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{NewUser, QuotaOverride, UserId};
use crate::features::users::repo as users;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::csrf::{AnonymousCsrf, CsrfGuard};
use crate::infra::http::extractors::restriction_allows;
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const PATH: &str = "/api/v1/settings/effective";
const BODY_CAP: usize = 64 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;
const EFFECTIVE_FIELDS: [&str; 13] = [
    "aliasPattern",
    "maxConcurrentTransfers",
    "maxFileSizeBytes",
    "maxPublicLinkLifetimeDays",
    "passwordMinLength",
    "publicLinkPasswordMinLength",
    "quotaBytes",
    "receivedRetentionMaxDays",
    "smtpConfigured",
    "storageProvider",
    "trustedDeviceDurationDays",
    "trustedDevicesEnabled",
    "twoFactorRequired",
];

struct Harness {
    _root: TempDir,
    pools: DbPools,
    clock: TestClock,
    settings: SettingsHandle,
    config: OperatorConfig,
    sessions: SessionService,
}

impl Harness {
    async fn open(settings: AppSettings) -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 2, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(root.path()).unwrap();
        let clock = TestClock::new(datetime!(2026-09-25 12:00 UTC));
        let settings = SettingsHandle::new(settings);
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_BASE_URL",
            "https://files.example.test",
        )]))
        .unwrap()
        .config;
        let sessions = SessionService::new(
            pools.clone(),
            Arc::new(clock.clone()),
            settings.clone(),
            Arc::new(KeyRing::new(&instance_key)),
            &config.base_url,
        );
        Self {
            _root: root,
            pools,
            clock,
            settings,
            config,
            sessions,
        }
    }

    async fn user(
        &self,
        suffix: &str,
        role: Role,
        quota: QuotaOverride,
        must_change_password: bool,
    ) -> UserId {
        let new = NewUser {
            email: Email::parse(&format!("{suffix}@example.test")).unwrap(),
            username: Username::parse(&format!("user-{suffix}")).unwrap(),
            first_name: String::new(),
            last_name: String::new(),
            password_hash: None,
            must_change_password,
            role,
            is_active: true,
            quota,
            created_by: None,
        };
        self.pools
            .write_tx(&self.clock, "settings.test_user", async |tx| {
                users::insert(tx, &self.clock, &new).await
            })
            .await
            .unwrap()
            .id
    }

    async fn session(&self, user_id: UserId) -> MintedSession {
        self.sessions
            .mint(NewSession {
                user_id,
                auth_method: AuthMethod::Password,
                ip_address: None,
                user_agent: None,
            })
            .await
            .unwrap()
    }

    async fn get(&self, path: &str, session: Option<&MintedSession>) -> (StatusCode, Response) {
        let assembled = application_routes().build().unwrap();
        let docs = ApiDocs::new(assembled.openapi, &self.config.base_url).unwrap();
        let effective = EffectiveSettingsService::new(
            self.pools.reader().clone(),
            self.settings.clone(),
            OperatorPolicy {
                storage_provider: "local",
                max_concurrent_transfers: 5,
            },
        );
        let router = assembled
            .router
            .with_state(AppState::new(
                Arc::new(self.clock.clone()),
                Health::new(Readiness::new()),
                docs,
                self.settings.clone(),
                StorageRuntime::for_test(),
            ))
            .layer(axum::Extension(self.sessions.clone()))
            .layer(axum::Extension(effective));
        let edge = HttpEdge::new(
            Arc::new(self.clock.clone()),
            TrustedProxies::new(&self.config.trust_proxy),
            SecurityHeaders::new(&self.config),
            CsrfGuard::new(&self.config.base_url),
        );
        let mut request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        if let Some(session) = session {
            request.headers_mut().insert(
                COOKIE,
                HeaderValue::from_str(&format!(
                    "palmr_session={}; palmr_csrf={}",
                    session.session_token.expose_secret(),
                    session.csrf_token.expose_secret()
                ))
                .unwrap(),
            );
        }
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:32100".parse::<SocketAddr>().unwrap(),
        ));
        let response = with_middleware(router, &edge)
            .oneshot(request)
            .await
            .unwrap();
        (response.status(), response)
    }

    async fn effective(&self, session: &MintedSession) -> Value {
        let (status, response) = self.get(PATH, Some(session)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.headers().get(CACHE_CONTROL).unwrap(), "no-store");
        assert!(response.headers().get(SET_COOKIE).is_none());
        body_json(response).await
    }
}

async fn body_json(response: Response) -> Value {
    let bytes = Limited::new(response.into_body(), BODY_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn configured_smtp() -> AppSettings {
    let mut settings = AppSettings::defaults();
    settings.smtp.enabled = true;
    settings.smtp.host = Some("smtp.secret-host.example".to_owned());
    settings.smtp.username = Some("smtp-user-sentinel".to_owned());
    settings.smtp.password = Some(Secret::new("smtp-password-sentinel".to_owned()));
    settings.smtp.from_email = Some("palmr@example.test".to_owned());
    settings
}

#[tokio::test]
async fn it_settings_effective_requires_authentication() {
    let harness = Harness::open(AppSettings::defaults()).await;
    let (status, response) = harness.get(PATH, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await["error"]["code"],
        json!("AUTH_REQUIRED")
    );

    let assembled = application_routes().build().unwrap();
    let entry = assembled.inventory.get(&Method::GET, PATH).unwrap();
    assert_eq!(entry.policy(), EFFECTIVE_SETTINGS_ROUTE);
    assert_eq!(entry.policy().auth(), AuthClass::Authenticated);
    assert_eq!(entry.policy().rate_limit(), RateLimitClass::Read);
    assert_eq!(entry.policy().anonymous_csrf(), AnonymousCsrf::None);
}

#[tokio::test]
async fn it_settings_effective_shape_and_no_secrets() {
    let mut settings = configured_smtp();
    settings.security.password_min_length = 12;
    settings.security.public_link_password_min_length = 10;
    settings.security.two_factor_required = false;
    settings.security.trusted_device_duration_days = 14;
    settings.quotas.max_file_size_bytes = Some(ByteSize::try_from(2 * GIB).unwrap());
    settings.public_links.max_public_link_lifetime_days = Some(30);
    settings.retention.received_retention_days = Some(90);
    let harness = Harness::open(settings).await;
    let user = harness
        .user("shape", Role::User, QuotaOverride::Inherit, false)
        .await;
    let session = harness.session(user).await;
    let body = harness.effective(&session).await;

    let mut keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, EFFECTIVE_FIELDS);
    assert_eq!(
        body,
        json!({
            "passwordMinLength": 12,
            "publicLinkPasswordMinLength": 10,
            "maxFileSizeBytes": 2 * GIB,
            "quotaBytes": null,
            "maxPublicLinkLifetimeDays": 30,
            "twoFactorRequired": false,
            "trustedDevicesEnabled": true,
            "trustedDeviceDurationDays": 14,
            "receivedRetentionMaxDays": 90,
            "smtpConfigured": true,
            "storageProvider": "local",
            "maxConcurrentTransfers": 5,
            "aliasPattern": "^[A-Za-z0-9_-]{3,64}$"
        })
    );
    let text = body.to_string().to_ascii_lowercase();
    for leaked in [
        "sentinel",
        "secret-host",
        "palmr@example.test",
        "shape@example.test",
        "user-shape",
        "password\":",
        "session",
        "csrf",
        "accesskey",
        "secretkey",
        "bucket",
        "endpoint",
        "auditretention",
        "maxloginattempts",
    ] {
        assert!(!text.contains(leaked), "{leaked}");
    }
}

#[tokio::test]
async fn it_settings_effective_fresh_install_defaults() {
    let harness = Harness::open(AppSettings::defaults()).await;
    let admin = harness
        .user("fresh-admin", Role::Admin, QuotaOverride::Inherit, false)
        .await;
    let body = harness.effective(&harness.session(admin).await).await;
    assert_eq!(body["passwordMinLength"], json!(8));
    assert_eq!(body["publicLinkPasswordMinLength"], json!(8));
    assert_eq!(body["maxFileSizeBytes"], Value::Null);
    assert_eq!(body["quotaBytes"], Value::Null);
    assert_eq!(body["maxPublicLinkLifetimeDays"], Value::Null);
    assert_eq!(body["receivedRetentionMaxDays"], Value::Null);
    assert_eq!(body["smtpConfigured"], json!(false));
    assert_eq!(body["trustedDeviceDurationDays"], json!(30));
}

#[tokio::test]
async fn it_settings_effective_quota_uses_the_effective_quota_resolver() {
    let mut settings = AppSettings::defaults();
    settings.quotas.default_user_quota_bytes = Some(ByteSize::try_from(10 * GIB).unwrap());
    let harness = Harness::open(settings).await;
    let cases = [
        (
            "inherit",
            Role::User,
            QuotaOverride::Inherit,
            json!(10 * GIB),
        ),
        (
            "admin-inherit",
            Role::Admin,
            QuotaOverride::Inherit,
            json!(10 * GIB),
        ),
        (
            "unlimited",
            Role::User,
            QuotaOverride::Unlimited,
            Value::Null,
        ),
        (
            "bytes",
            Role::User,
            QuotaOverride::Bytes(ByteSize::try_from(GIB).unwrap()),
            json!(GIB),
        ),
        (
            "zero",
            Role::User,
            QuotaOverride::Bytes(ByteSize::try_from(0_u64).unwrap()),
            json!(0),
        ),
    ];
    for (suffix, role, quota, expected) in cases {
        let user = harness.user(suffix, role, quota, false).await;
        let body = harness.effective(&harness.session(user).await).await;
        assert_eq!(body["quotaBytes"], expected, "{suffix}");
        assert_eq!(
            body["quotaBytes"],
            json!(crate::features::users::service::effective_quota(
                quota,
                Some(ByteSize::try_from(10 * GIB).unwrap())
            )
            .map(ByteSize::get)),
            "{suffix}"
        );
    }
}

#[tokio::test]
async fn it_settings_effective_allowed_for_restricted_sessions() {
    assert!(restriction_allows(
        SessionRestriction::MustChangePassword,
        &Method::GET,
        PATH
    ));
    assert!(restriction_allows(
        SessionRestriction::MustEnrollTotp,
        &Method::GET,
        PATH
    ));

    let mut settings = AppSettings::defaults();
    settings.security.two_factor_required = true;
    let harness = Harness::open(settings).await;
    let password_user = harness
        .user("must-change", Role::User, QuotaOverride::Inherit, true)
        .await;
    let totp_user = harness
        .user("must-enroll", Role::User, QuotaOverride::Inherit, false)
        .await;
    for user in [password_user, totp_user] {
        let session = harness.session(user).await;
        let body = harness.effective(&session).await;
        assert_eq!(body["twoFactorRequired"], json!(true));

        let (status, response) = harness.get("/api/v1/sessions", Some(&session)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let code = body_json(response).await["error"]["code"].clone();
        assert!(
            code == json!("AUTH_PASSWORD_CHANGE_REQUIRED")
                || code == json!("AUTH_2FA_ENROLLMENT_REQUIRED"),
            "{code}"
        );
    }
}
