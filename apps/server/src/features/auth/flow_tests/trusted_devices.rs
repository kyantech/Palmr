use std::collections::BTreeSet;

use base64ct::{Base64UrlUnpadded, Encoding};
use http::header::USER_AGENT;

use super::operator_cli::run_recover;
use super::profile::{assert_code, Call};
use super::rate_limit::assert_throttled;
use super::totp::{capture_dispatch, code_for, stamp, step_of, wrong_code, Enabled};
use super::*;
use crate::features::auth::trusted_devices::model::TrustedDevicePolicy;
use crate::features::auth::trusted_devices::routes::{
    LIST_ROUTE, REVOKE_ALL_ROUTE, REVOKE_ONE_ROUTE,
};
use crate::infra::crypto::hash::sha256_hex;
use crate::infra::jobs::prune_tokens::{prune_step, steps, PruneStep, TRUSTED_DEVICE_RETENTION};

const DEVICES: &str = "/api/v1/auth/trusted-devices";
const LOGIN_TOTP: &str = "/api/v1/auth/login/totp";
const DISABLE: &str = "/api/v1/auth/2fa/disable";
const REGENERATE: &str = "/api/v1/auth/2fa/backup-codes/regenerate";
const SESSIONS: &str = "/api/v1/sessions";
const NEW_PASSWORD: &str = "a brand new passphrase";
const STEP: Duration = Duration::from_secs(30);
const DEVICE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const RECENT_AUTH_LAPSE: Duration = Duration::from_secs(6 * 60);
const TRUST_PROXY: (&str, &str) = ("PALMR_TRUST_PROXY", "198.51.100.0/24");
const FIREFOX: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.6; rv:130.0) Gecko/20100101 Firefox/130.0";
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct DeviceRow {
    id: String,
    user_id: String,
    token_hash: String,
    label: Option<String>,
    created_at: String,
    last_used_at: Option<String>,
    expires_at: String,
    revoked_at: Option<String>,
    ip: Option<String>,
    user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SessionLink {
    state: String,
    auth_method: String,
    trusted_device_id: Option<String>,
}

#[derive(Clone, Copy)]
struct Browser<'a> {
    host: u8,
    user_agent: Option<&'a str>,
    forwarded_for: Option<&'a str>,
    device: Option<&'a str>,
}

impl<'a> Browser<'a> {
    const fn at(host: u8, user_agent: &'a str) -> Self {
        Self {
            host,
            user_agent: Some(user_agent),
            forwarded_for: None,
            device: None,
        }
    }

    const fn with_device(self, device: &'a str) -> Self {
        Self {
            device: Some(device),
            ..self
        }
    }

    const fn without_device(self) -> Self {
        Self {
            device: None,
            ..self
        }
    }

    const fn via_ip(self, ip: &'a str) -> Self {
        Self {
            forwarded_for: Some(ip),
            ..self
        }
    }
}

struct Remembered {
    fetched: Fetched,
    device: String,
    credentials: Credentials,
}

impl Stack {
    async fn td_send(
        &self,
        method: Method,
        path: &str,
        browser: Browser<'_>,
        session: Option<&Credentials>,
        body: Option<&Value>,
    ) -> Fetched {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(ORIGIN, BASE_URL);
        if body.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        if let Some(agent) = browser.user_agent {
            builder = builder.header(USER_AGENT, agent);
        }
        if let Some(ip) = browser.forwarded_for {
            builder = builder.header("x-forwarded-for", ip);
        }
        let mut cookies = Vec::new();
        if let Some(credentials) = session {
            cookies.push(format!("palmr_session={}", credentials.session));
            cookies.push(format!("palmr_csrf={}", credentials.csrf));
            builder = builder.header(CSRF_HEADER, &credentials.csrf);
        }
        if let Some(device) = browser.device {
            cookies.push(format!("palmr_device={device}"));
        }
        if !cookies.is_empty() {
            builder = builder.header(COOKIE, cookies.join("; "));
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        self.send(with_peer(builder.body(body).unwrap(), browser.host))
            .await
    }

    async fn td_login(&self, identifier: &str, password: &str, browser: Browser<'_>) -> Fetched {
        let body = json!({ "identifier": identifier, "password": password });
        self.td_send(Method::POST, LOGIN, browser, None, Some(&body))
            .await
    }

    async fn td_challenge(&self, identifier: &str, browser: Browser<'_>) -> (String, bool) {
        let fetched = self.td_login(identifier, PASSWORD, browser).await;
        assert_code(&fetched, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
        assert!(fetched.set_cookies().is_empty());
        let details = fetched.json()["error"]["details"].clone();
        (
            details["mfaToken"].as_str().unwrap().to_owned(),
            details["trustedDeviceOffered"].as_bool().unwrap(),
        )
    }

    async fn td_complete(
        &self,
        token: &str,
        code: &str,
        remember: Option<Value>,
        browser: Browser<'_>,
    ) -> Fetched {
        let mut body = json!({ "mfaToken": token, "code": code });
        if let Some(remember) = remember {
            body["rememberDevice"] = remember;
        }
        self.td_send(Method::POST, LOGIN_TOTP, browser, None, Some(&body))
            .await
    }

    async fn td_enrolled(&self, username: &str, host: u8) -> (UserId, Enabled) {
        let hash = password_hash();
        let email = format!("{username}@example.test");
        let user = self.user(UserSpec::local(username, &email, &hash)).await;
        let session = self.signed_in(username, host).await;
        let enabled = self.tf_enable(&session, host).await;
        self.clock.advance(STEP);
        (user, enabled)
    }

    fn td_code(&self, enabled: &Enabled) -> String {
        code_for(&enabled.enrollment.secret, step_of(clock_now(&self.clock)))
    }

    async fn td_remember(
        &self,
        identifier: &str,
        enabled: &Enabled,
        browser: Browser<'_>,
    ) -> Remembered {
        let (token, offered) = self.td_challenge(identifier, browser).await;
        assert!(offered);
        let fetched = self
            .td_complete(&token, &self.td_code(enabled), Some(json!(true)), browser)
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        self.clock.advance(STEP);
        Remembered {
            device: fetched.cookie("palmr_device"),
            credentials: Credentials::from(&fetched),
            fetched,
        }
    }

    async fn td_rows(&self, user: UserId) -> Vec<DeviceRow> {
        sqlx::query_as(
            "SELECT id, user_id, token_hash, label, created_at, last_used_at, expires_at,
                    revoked_at, ip, user_agent
               FROM trusted_devices WHERE user_id = ?1 ORDER BY id",
        )
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn td_row(&self, device: &str) -> DeviceRow {
        sqlx::query_as(
            "SELECT id, user_id, token_hash, label, created_at, last_used_at, expires_at,
                    revoked_at, ip, user_agent
               FROM trusted_devices WHERE token_hash = ?1",
        )
        .bind(digest(device))
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn td_link(&self, session: &str) -> SessionLink {
        sqlx::query_as(
            "SELECT state, auth_method, trusted_device_id FROM sessions WHERE token_hash = ?1",
        )
        .bind(digest(session))
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn td_active_sessions(&self, user: UserId) -> i64 {
        self.scalar_i64(&format!(
            "SELECT COUNT(*) FROM sessions WHERE user_id = '{user}' AND state = 'active'"
        ))
        .await
    }

    async fn td_pending_count(&self, user: UserId) -> i64 {
        self.scalar_i64(&format!(
            "SELECT COUNT(*) FROM sessions WHERE user_id = '{user}' AND state = 'mfa_pending'"
        ))
        .await
    }

    async fn td_last_attempt(&self, user: UserId) -> (String, String) {
        sqlx::query_as(
            "SELECT method, result FROM login_attempts WHERE user_id = ?1 ORDER BY id DESC LIMIT 1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn td_last_step(&self, user: UserId) -> Option<i64> {
        self.tf_secret_row(user).await.unwrap().last_used_step
    }

    async fn td_pending(&self, token: &str) -> Option<(String, i64)> {
        sqlx::query_as("SELECT state, mfa_attempts FROM sessions WHERE mfa_token_hash = ?1")
            .bind(digest(token))
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn td_list(
        &self,
        credentials: &Credentials,
        device: Option<&str>,
        query: &str,
    ) -> Fetched {
        let browser = Browser {
            device,
            ..Browser::at(40, FIREFOX)
        };
        self.td_send(
            Method::GET,
            &format!("{DEVICES}{query}"),
            browser,
            Some(credentials),
            None,
        )
        .await
    }

    async fn td_delete(
        &self,
        path: &str,
        credentials: &Credentials,
        device: Option<&str>,
    ) -> Fetched {
        let browser = Browser {
            device,
            ..Browser::at(41, FIREFOX)
        };
        self.td_send(Method::DELETE, path, browser, Some(credentials), None)
            .await
    }

    async fn td_insert_device(&self, user: UserId, token: &str, expires_at: &str) {
        self.execute(&format!(
            "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at)
             VALUES ('{id}', '{user}', '{hash}', '{now}', '{expires_at}')",
            id = crate::features::auth::trusted_devices::model::TrustedDeviceId::generate(
                &self.clock
            ),
            hash = digest(token),
            now = stamp(clock_now(&self.clock)),
        ))
        .await;
    }

    async fn td_db_text(&self) -> String {
        let mut text = String::new();
        for query in [
            "SELECT COALESCE(group_concat(id || '|' || user_id || '|' || token_hash || '|' ||
                    COALESCE(label, '') || '|' || COALESCE(ip, '') || '|' ||
                    COALESCE(user_agent, ''), char(10)), '') FROM trusted_devices",
            "SELECT COALESCE(group_concat(id || '|' || token_hash || '|' || csrf_token_hash || '|' ||
                    COALESCE(mfa_token_hash, '') || '|' || COALESCE(trusted_device_id, ''),
                    char(10)), '') FROM sessions",
            "SELECT COALESCE(group_concat(identifier_normalized || '|' || method || '|' || result ||
                    '|' || COALESCE(user_agent, '') || '|' || COALESCE(request_id, ''),
                    char(10)), '') FROM login_attempts",
        ] {
            let rows: String = sqlx::query_scalar(query)
                .fetch_one(self.pools.reader().executor())
                .await
                .unwrap();
            text.push_str(&rows);
            text.push('\n');
        }
        text.push_str(&self.all_audit_text().await);
        text
    }
}

fn cookie_names(fetched: &Fetched) -> BTreeSet<String> {
    fetched
        .set_cookies()
        .iter()
        .map(|cookie| cookie.split('=').next().unwrap().to_owned())
        .collect()
}

fn fresh_token() -> String {
    Token::mint().unwrap().encode().expose_secret().clone()
}

fn assert_second_factor_required(fetched: &Fetched) {
    assert_code(fetched, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    assert!(
        fetched.set_cookies().is_empty(),
        "{:?}",
        fetched.set_cookies()
    );
}

fn assert_trusted_sign_in(fetched: &Fetched) -> Credentials {
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    assert_eq!(
        cookie_names(fetched),
        BTreeSet::from(["palmr_csrf".to_owned(), "palmr_session".to_owned()]),
        "a used device is honored, never rotated"
    );
    Credentials::from(fetched)
}

fn expired_device_cookie(fetched: &Fetched) -> Vec<String> {
    fetched
        .set_cookies()
        .into_iter()
        .filter(|cookie| cookie.starts_with("palmr_device="))
        .collect()
}

const EXPIRED_DEVICE: &str = "palmr_device=; Path=/; SameSite=Lax; Max-Age=0; Secure; HttpOnly";

#[allow(non_snake_case, reason = "the accepted regression identifier is R-043")]
#[tokio::test]
async fn regression_R043_trusted_device_random_token() {
    let (capture, dispatch) = capture_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack =
        Stack::start_configured(root.path(), &clock, Routes::new(), &[TRUST_PROXY]).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;
    let (grace, _) = stack.td_enrolled("grace", 11).await;
    let home = Browser::at(20, FIREFOX).via_ip("203.0.113.10");

    let enrolled_at = clock_now(&clock);
    let remembered = stack.td_remember("ada", &ada_tf, home).await;
    let device = remembered.device.clone();
    assert_eq!(
        cookie_names(&remembered.fetched),
        BTreeSet::from([
            "palmr_csrf".to_owned(),
            "palmr_device".to_owned(),
            "palmr_session".to_owned()
        ])
    );
    assert_eq!(
        expired_device_cookie(&remembered.fetched),
        [format!(
            "palmr_device={device}; Path=/; SameSite=Lax; Max-Age=2592000; Secure; HttpOnly"
        )]
    );
    assert_eq!(device.len(), 43);
    assert_eq!(
        Token::decode(&device).unwrap().expose_secret().len() * 8,
        256
    );
    assert!(!remembered.fetched.text().contains(&device));

    let rows = stack.td_rows(ada).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = rows[0].clone();
    assert_eq!(row.token_hash, digest(&device));
    assert_eq!(row.user_id, ada.to_string());
    assert_eq!(row.label.as_deref(), Some("Firefox on macOS"));
    assert_eq!(row.ip.as_deref(), Some("203.0.113.10"));
    assert_eq!(row.user_agent.as_deref(), Some(FIREFOX));
    assert_eq!(row.created_at, stamp(enrolled_at));
    assert_eq!(row.expires_at, stamp(enrolled_at + DEVICE_LIFETIME));
    assert_eq!(
        (row.last_used_at.as_deref(), row.revoked_at.as_deref()),
        (None, None)
    );
    for derived in [
        format!("{FIREFOX}203.0.113.10"),
        format!("{FIREFOX}|203.0.113.10"),
        format!("203.0.113.10{FIREFOX}"),
    ] {
        assert_ne!(row.token_hash, sha256_hex(derived.as_bytes()).as_str());
    }

    let same_user = stack
        .td_login("ada", PASSWORD, home.with_device(&device))
        .await;
    let trusted = assert_trusted_sign_in(&same_user);
    assert_eq!(
        stack.td_link(&trusted.session).await,
        SessionLink {
            state: "active".to_owned(),
            auth_method: "password_trusted_device".to_owned(),
            trusted_device_id: Some(row.id.clone()),
        }
    );
    assert_eq!(
        stack.td_row(&device).await.last_used_at,
        Some(stamp(clock_now(&clock)))
    );
    assert_eq!(stack.td_pending_count(ada).await, 0);
    assert_eq!(
        stack.td_last_attempt(ada).await,
        ("trusted_device".to_owned(), "success".to_owned())
    );

    let same_ua_ip_no_cookie = stack.td_login("ada", PASSWORD, home.without_device()).await;
    assert_second_factor_required(&same_ua_ip_no_cookie);
    assert_eq!(
        same_ua_ip_no_cookie.json()["error"]["details"]["trustedDeviceOffered"],
        true
    );
    assert_eq!(stack.td_pending_count(ada).await, 1);

    clock.advance(Duration::from_secs(61));
    let roaming = Browser::at(22, CHROME).via_ip("192.0.2.77");
    let moved = stack
        .td_login("ada", PASSWORD, roaming.with_device(&device))
        .await;
    assert_trusted_sign_in(&moved);
    let after_move = stack.td_row(&device).await;
    assert_eq!(after_move.last_used_at, Some(stamp(clock_now(&clock))));
    assert_eq!(
        (after_move.ip.as_deref(), after_move.user_agent.as_deref()),
        (Some("203.0.113.10"), Some(FIREFOX)),
        "enrollment metadata is display-only and never rewritten by use"
    );

    let grace_active = stack.td_active_sessions(grace).await;
    clock.advance(Duration::from_secs(61));
    let copied = stack
        .td_login("grace", PASSWORD, home.with_device(&device))
        .await;
    assert_second_factor_required(&copied);
    assert_eq!(stack.td_active_sessions(grace).await, grace_active);
    assert!(stack.td_rows(grace).await.is_empty());
    assert_eq!(stack.td_row(&device).await, after_move);

    stack.flush_audit().await;
    let persisted = stack.td_db_text().await;
    assert!(!persisted.contains(&device));
    let logs = capture.text();
    assert!(!logs.is_empty());
    assert!(!logs.contains(&device));
    assert!(!logs.contains(&digest(&device)));
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_ua_ip_spoof_irrelevant() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start_configured(root.path(), &clock, Routes::new(), &[TRUST_PROXY]).await;
    let (_, ada_tf) = stack.td_enrolled("ada", 10).await;
    let home = Browser::at(20, FIREFOX).via_ip("203.0.113.10");
    let device = stack.td_remember("ada", &ada_tf, home).await.device;
    let row = stack.td_row(&device).await;

    let mut v3_identity = [0_u8; 32];
    v3_identity.copy_from_slice(&hex_bytes(
        sha256_hex(format!("{FIREFOX}203.0.113.10").as_bytes()).as_str(),
    ));
    let forged = [
        None,
        Some(fresh_token()),
        Some(Base64UrlUnpadded::encode_string(&v3_identity)),
        Some(row.token_hash.clone()),
        Some("not-a-device-token".to_owned()),
    ];
    for (index, forged) in forged.iter().enumerate() {
        let browser = Browser {
            device: forged.as_deref(),
            ..home
        };
        let fetched = stack.td_login("ada", PASSWORD, browser).await;
        assert_second_factor_required(&fetched);
        assert_eq!(stack.td_row(&device).await, row, "forgery {index}");
    }

    clock.advance(Duration::from_secs(61));
    let variants = [
        Browser::at(21, CHROME),
        Browser::at(22, "curl/8.9.1").via_ip("192.0.2.200"),
        Browser {
            user_agent: None,
            ..Browser::at(23, FIREFOX).via_ip("2001:db8::7")
        },
        home,
    ];
    for browser in variants {
        let fetched = stack
            .td_login("ada", PASSWORD, browser.with_device(&device))
            .await;
        let credentials = assert_trusted_sign_in(&fetched);
        assert_eq!(
            stack.td_link(&credentials.session).await.auth_method,
            "password_trusted_device"
        );
    }
    stack.stop().await;
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect()
}

#[tokio::test]
async fn it_trusted_device_expiry_and_revoke() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;

    let laptop = Browser::at(20, FIREFOX);
    let expiring = stack.td_remember("ada", &ada_tf, laptop).await.device;
    clock.advance(DEVICE_LIFETIME - STEP - Duration::from_secs(60));
    assert_trusted_sign_in(
        &stack
            .td_login("ada", PASSWORD, laptop.with_device(&expiring))
            .await,
    );
    clock.advance(Duration::from_secs(120));
    assert_second_factor_required(
        &stack
            .td_login("ada", PASSWORD, laptop.with_device(&expiring))
            .await,
    );
    let expired_row = stack.td_row(&expiring).await;
    assert_eq!(
        expired_row.revoked_at, None,
        "expiry needs no sweep to stop trust"
    );

    let phone = Browser::at(21, CHROME);
    let revoked_later = stack.td_remember("ada", &ada_tf, phone).await;
    let listed = stack
        .td_list(&revoked_later.credentials, Some(&expiring), "")
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let items = listed.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1, "an expired device is not listed");
    assert_eq!(items[0]["isCurrent"], false);

    let via_device = assert_trusted_sign_in(
        &stack
            .td_login("ada", PASSWORD, phone.with_device(&revoked_later.device))
            .await,
    );
    let phone_row = stack.td_row(&revoked_later.device).await;
    let active_before = stack.td_active_sessions(ada).await;
    let revoked = stack
        .td_delete(
            &format!("{DEVICES}/{}", phone_row.id),
            &via_device,
            Some(&revoked_later.device),
        )
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.text());
    assert_eq!(revoked.set_cookies(), [EXPIRED_DEVICE]);
    assert_eq!(
        stack.td_row(&revoked_later.device).await.revoked_at,
        Some(stamp(clock_now(&clock)))
    );
    assert_eq!(stack.td_active_sessions(ada).await, active_before);
    assert_eq!(
        stack.td_link(&via_device.session).await,
        SessionLink {
            state: "active".to_owned(),
            auth_method: "password_trusted_device".to_owned(),
            trusted_device_id: Some(phone_row.id.clone()),
        },
        "revoking a device is not retroactive session revocation"
    );
    assert_eq!(
        stack.get(ME, Some(&via_device.session), 30).await.status,
        StatusCode::OK
    );
    assert_second_factor_required(
        &stack
            .td_login("ada", PASSWORD, phone.with_device(&revoked_later.device))
            .await,
    );
    let again = stack
        .td_delete(
            &format!("{DEVICES}/{}", phone_row.id),
            &via_device,
            Some(&revoked_later.device),
        )
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    assert_eq!(again.set_cookies(), [EXPIRED_DEVICE]);

    let desk = stack
        .td_remember("ada", &ada_tf, Browser::at(22, FIREFOX))
        .await;
    let tablet = stack
        .td_remember("ada", &ada_tf, Browser::at(23, CHROME))
        .await;
    let other = stack
        .td_delete(
            &format!("{DEVICES}/{}", stack.td_row(&tablet.device).await.id),
            &tablet.credentials,
            Some(&desk.device),
        )
        .await;
    assert_eq!(other.status, StatusCode::NO_CONTENT);
    assert!(
        other.set_cookies().is_empty(),
        "another device's cookie is left alone"
    );

    let kiosk = stack
        .td_remember("ada", &ada_tf, Browser::at(24, FIREFOX))
        .await;
    let active_before = stack.td_active_sessions(ada).await;
    let everything = stack
        .td_delete(DEVICES, &kiosk.credentials, Some(&desk.device))
        .await;
    assert_eq!(
        everything.status,
        StatusCode::NO_CONTENT,
        "{}",
        everything.text()
    );
    assert_eq!(everything.set_cookies(), [EXPIRED_DEVICE]);
    assert_eq!(stack.td_active_sessions(ada).await, active_before);
    for device in [&desk.device, &kiosk.device] {
        assert!(stack.td_row(device).await.revoked_at.is_some());
        assert_second_factor_required(
            &stack
                .td_login(
                    "ada",
                    PASSWORD,
                    Browser::at(25, FIREFOX).with_device(device),
                )
                .await,
        );
    }
    assert!(stack
        .tf_backup_rows(ada)
        .await
        .iter()
        .all(|row| row.used_at.is_none()));
    assert!(stack.tf_secret_row(ada).await.is_some());

    let audit = stack.audit_rows("TRUSTED_DEVICE_REVOKED").await;
    let metadata: Vec<Value> = audit
        .iter()
        .map(|(_, _, metadata)| serde_json::from_str(metadata).unwrap())
        .collect();
    assert_eq!(
        metadata,
        [
            json!({ "scope": "one", "current": true }),
            json!({ "scope": "one", "current": false }),
            json!({ "scope": "all", "revoked": 3 }),
        ]
    );
    assert_eq!(audit[0].1.as_deref(), Some(phone_row.id.as_str()));

    let survivor = stack
        .td_remember("ada", &ada_tf, Browser::at(26, FIREFOX))
        .await
        .device;
    let early = prune_step(&stack.pools, &clock, PruneStep::TrustedDevices)
        .await
        .unwrap();
    assert_eq!(
        early.deleted, 0,
        "revoked and expired rows are kept for 24 h"
    );
    assert_eq!(stack.td_rows(ada).await.len(), 6);
    clock.advance(TRUSTED_DEVICE_RETENTION + Duration::from_secs(1));
    let due = prune_step(&stack.pools, &clock, PruneStep::TrustedDevices)
        .await
        .unwrap();
    assert_eq!(due.deleted, 5);
    let remaining = stack.td_rows(ada).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].token_hash, digest(&survivor));

    stack.flush_audit().await;
    let persisted = stack.td_db_text().await;
    for device in [&expiring, &revoked_later.device, &desk.device, &survivor] {
        assert!(!persisted.contains(device.as_str()));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_created_only_after_second_factor() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;
    let browser = Browser::at(20, FIREFOX);

    let (token, _) = stack.td_challenge("ada", browser).await;
    let wrong = stack
        .td_complete(
            &token,
            &wrong_code(&ada_tf.enrollment.secret, clock_now(&clock)),
            Some(json!(true)),
            browser,
        )
        .await;
    assert_code(&wrong, StatusCode::UNAUTHORIZED, "AUTH_2FA_INVALID");
    assert!(wrong.set_cookies().is_empty());
    let malformed = stack
        .td_complete(&token, "12", Some(json!(true)), browser)
        .await;
    assert_code(&malformed, StatusCode::UNAUTHORIZED, "AUTH_2FA_INVALID");
    let not_boolean = stack
        .td_complete(&token, &stack.td_code(&ada_tf), Some(json!("yes")), browser)
        .await;
    assert_code(
        &not_boolean,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        not_boolean.json()["error"]["details"]["fields"],
        json!(["rememberDevice"])
    );
    assert!(stack.td_rows(ada).await.is_empty());

    let declined = stack
        .td_complete(&token, &stack.td_code(&ada_tf), Some(json!(false)), browser)
        .await;
    assert_eq!(declined.status, StatusCode::OK, "{}", declined.text());
    assert!(!cookie_names(&declined).contains("palmr_device"));
    clock.advance(STEP);

    let (token, _) = stack.td_challenge("ada", browser).await;
    let omitted = stack
        .td_complete(&token, &stack.td_code(&ada_tf), None, browser)
        .await;
    assert_eq!(omitted.status, StatusCode::OK, "{}", omitted.text());
    assert!(!cookie_names(&omitted).contains("palmr_device"));
    clock.advance(STEP);

    let (token, _) = stack.td_challenge("ada", browser).await;
    let null = stack
        .td_complete(&token, &stack.td_code(&ada_tf), Some(Value::Null), browser)
        .await;
    assert_eq!(null.status, StatusCode::OK, "{}", null.text());
    assert!(!cookie_names(&null).contains("palmr_device"));
    assert!(stack.td_rows(ada).await.is_empty());
    clock.advance(STEP);

    let expired = stack
        .td_complete(
            &fresh_token(),
            &stack.td_code(&ada_tf),
            Some(json!(true)),
            browser,
        )
        .await;
    assert_code(
        &expired,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert!(stack.td_rows(ada).await.is_empty());

    let (token, _) = stack.td_challenge("ada", browser).await;
    let backup = stack
        .td_complete(&token, &ada_tf.codes[0], Some(json!(true)), browser)
        .await;
    assert_eq!(backup.status, StatusCode::OK, "{}", backup.text());
    let device = backup.cookie("palmr_device");
    assert_eq!(
        stack.td_link(&Credentials::from(&backup).session).await,
        SessionLink {
            state: "active".to_owned(),
            auth_method: "password_backup_code".to_owned(),
            trusted_device_id: None,
        },
        "the session that proved the factor did not skip it"
    );
    assert_eq!(stack.td_rows(ada).await.len(), 1);
    assert_trusted_sign_in(
        &stack
            .td_login(
                "ada",
                PASSWORD,
                Browser::at(21, FIREFOX).with_device(&device),
            )
            .await,
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_creation_rolls_back_with_promotion() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;
    let browser = Browser::at(20, FIREFOX);
    let step_before = stack.td_last_step(ada).await;
    let backup_before = stack.tf_backup_rows(ada).await;
    let active_before = stack.td_active_sessions(ada).await;

    let (token, _) = stack.td_challenge("ada", browser).await;
    stack
        .execute(
            "CREATE TRIGGER fail_trusted_device BEFORE INSERT ON trusted_devices
             BEGIN SELECT RAISE(ABORT, 'forced trusted-device failure'); END",
        )
        .await;
    for code in [stack.td_code(&ada_tf), ada_tf.codes[0].clone()] {
        let failed = stack
            .td_complete(&token, &code, Some(json!(true)), browser)
            .await;
        assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
        assert!(failed.set_cookies().is_empty());
        assert!(stack.td_rows(ada).await.is_empty());
        assert_eq!(
            stack.td_pending(&token).await,
            Some(("mfa_pending".to_owned(), 0))
        );
        assert_eq!(stack.td_last_step(ada).await, step_before);
        assert_eq!(stack.tf_backup_rows(ada).await, backup_before);
        assert_eq!(stack.td_active_sessions(ada).await, active_before);
    }
    stack.execute("DROP TRIGGER fail_trusted_device").await;

    let completed = stack
        .td_complete(&token, &stack.td_code(&ada_tf), Some(json!(true)), browser)
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert!(cookie_names(&completed).contains("palmr_device"));
    assert_eq!(stack.td_rows(ada).await.len(), 1);
    assert_eq!(stack.td_active_sessions(ada).await, active_before + 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_policy_disable() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;
    let browser = Browser::at(20, FIREFOX);
    let remembered = stack.td_remember("ada", &ada_tf, browser).await;
    let device = remembered.device.clone();

    for (key, value_type, value, days) in [
        ("trusted_devices_enabled", "boolean", "false", 30),
        ("trusted_device_duration_days", "integer", "0", 0),
    ] {
        let row = stack.td_row(&device).await;
        stack.setting(key, value_type, value).await;
        assert!(!stack.auth.trusted_device_policy().enabled, "{key}");

        let refused = stack
            .td_login(
                "ada",
                PASSWORD,
                Browser::at(21, FIREFOX).with_device(&device),
            )
            .await;
        assert_second_factor_required(&refused);
        assert_eq!(
            refused.json()["error"]["details"]["trustedDeviceOffered"],
            false
        );
        assert_eq!(stack.td_row(&device).await, row, "{key}");

        let token = refused.json()["error"]["details"]["mfaToken"]
            .as_str()
            .unwrap()
            .to_owned();
        let step_before = stack.td_last_step(ada).await;
        let disabled = stack
            .td_complete(&token, &stack.td_code(&ada_tf), Some(json!(true)), browser)
            .await;
        assert_code(&disabled, StatusCode::FORBIDDEN, "TRUSTED_DEVICE_DISABLED");
        assert!(disabled.set_cookies().is_empty());
        assert_eq!(stack.td_rows(ada).await, std::slice::from_ref(&row));
        assert_eq!(
            stack.td_pending(&token).await,
            Some(("mfa_pending".to_owned(), 0)),
            "{key}: a refused remember request consumes nothing"
        );
        assert_eq!(stack.td_last_step(ada).await, step_before);

        let plain = stack
            .td_complete(&token, &stack.td_code(&ada_tf), Some(json!(false)), browser)
            .await;
        assert_eq!(plain.status, StatusCode::OK, "{}", plain.text());
        assert!(!cookie_names(&plain).contains("palmr_device"));
        clock.advance(STEP);

        let listed = stack
            .td_list(&Credentials::from(&plain), Some(&device), "")
            .await;
        assert_eq!(
            listed.json()["policy"],
            json!({ "enabled": false, "durationDays": days })
        );

        stack
            .setting("trusted_devices_enabled", "boolean", "true")
            .await;
        stack
            .setting("trusted_device_duration_days", "integer", "30")
            .await;
        clock.advance(Duration::from_secs(61));
        assert_trusted_sign_in(
            &stack
                .td_login(
                    "ada",
                    PASSWORD,
                    Browser::at(22, FIREFOX).with_device(&device),
                )
                .await,
        );
    }

    let enabled = TrustedDevicePolicy {
        enabled: true,
        duration_days: 30,
    };
    assert!(!steps(enabled).contains(&PruneStep::DisabledTrustedDevices));
    stack
        .setting("trusted_devices_enabled", "boolean", "false")
        .await;
    let disabled = stack.auth.trusted_device_policy();
    assert_eq!(
        steps(disabled),
        [
            PruneStep::IdempotencyRecords,
            PruneStep::PendingTotpEnrollments,
            PruneStep::TrustedDevices,
            PruneStep::PasswordResetTokens,
            PruneStep::ExpiredInvites,
            PruneStep::TerminalInvites,
            PruneStep::DisabledTrustedDevices,
        ]
    );
    let swept = prune_step(&stack.pools, &clock, PruneStep::DisabledTrustedDevices)
        .await
        .unwrap();
    assert_eq!(swept.deleted, 1);
    assert!(stack.td_rows(ada).await.is_empty());
    assert_eq!(
        stack.td_link(&remembered.credentials.session).await.state,
        "active"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_list_is_owner_scoped() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, ada_tf) = stack.td_enrolled("ada", 10).await;
    let (grace, grace_tf) = stack.td_enrolled("grace", 11).await;

    let laptop = stack
        .td_remember("ada", &ada_tf, Browser::at(20, FIREFOX))
        .await;
    let phone = stack
        .td_remember("ada", &ada_tf, Browser::at(21, CHROME))
        .await;
    let foreign = stack
        .td_remember("grace", &grace_tf, Browser::at(22, FIREFOX))
        .await;
    let laptop_row = stack.td_row(&laptop.device).await;
    let phone_row = stack.td_row(&phone.device).await;
    let foreign_row = stack.td_row(&foreign.device).await;
    let owner = &phone.credentials;

    let listed = stack.td_list(owner, Some(&laptop.device), "").await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(
        listed.json(),
        json!({
            "items": [
                {
                    "id": phone_row.id,
                    "label": "Chrome on Windows",
                    "ipAtEnrollment": null,
                    "createdAt": phone_row.created_at,
                    "lastSeenAt": phone_row.created_at,
                    "expiresAt": phone_row.expires_at,
                    "isCurrent": false,
                },
                {
                    "id": laptop_row.id,
                    "label": "Firefox on macOS",
                    "ipAtEnrollment": null,
                    "createdAt": laptop_row.created_at,
                    "lastSeenAt": laptop_row.created_at,
                    "expiresAt": laptop_row.expires_at,
                    "isCurrent": true,
                },
            ],
            "nextCursor": null,
            "totalCount": 2,
            "policy": { "enabled": true, "durationDays": 30 },
        })
    );
    let text = listed.text();
    for secret in [
        laptop.device.as_str(),
        phone.device.as_str(),
        foreign.device.as_str(),
        laptop_row.token_hash.as_str(),
        phone_row.token_hash.as_str(),
        foreign_row.id.as_str(),
        FIREFOX,
        "tokenHash",
        "userId",
    ] {
        assert!(!text.contains(secret), "{secret}");
    }

    for presented in [None, Some(foreign.device.as_str()), Some("garbage")] {
        let listed = stack.td_list(owner, presented, "").await;
        assert!(listed.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["isCurrent"] == false));
    }
    let own_marker = stack.td_list(owner, Some(&phone.device), "").await;
    assert_eq!(own_marker.json()["items"][0]["isCurrent"], true);

    clock.advance(Duration::from_secs(61));
    assert_trusted_sign_in(
        &stack
            .td_login(
                "ada",
                PASSWORD,
                Browser::at(23, FIREFOX).with_device(&laptop.device),
            )
            .await,
    );
    let first = stack.td_list(owner, None, "?limit=1").await;
    assert_eq!(first.json()["items"][0]["id"], laptop_row.id);
    assert_eq!(
        first.json()["items"][0]["lastSeenAt"],
        stamp(clock_now(&clock))
    );
    let cursor = first.json()["nextCursor"].as_str().unwrap().to_owned();
    let second = stack
        .td_list(owner, None, &format!("?limit=1&cursor={cursor}"))
        .await;
    assert_eq!(second.json()["items"][0]["id"], phone_row.id);
    assert_eq!(second.json()["nextCursor"], Value::Null);

    let foreign_path = format!("{DEVICES}/{}", foreign_row.id);
    let unknown_path = format!(
        "{DEVICES}/{}",
        crate::features::auth::trusted_devices::model::TrustedDeviceId::generate(&clock)
    );
    let mut bodies = Vec::new();
    for path in [
        foreign_path.as_str(),
        unknown_path.as_str(),
        "/api/v1/auth/trusted-devices/not-a-device",
    ] {
        let fetched = stack.td_delete(path, owner, Some(&foreign.device)).await;
        assert_code(&fetched, StatusCode::NOT_FOUND, "TRUSTED_DEVICE_NOT_FOUND");
        assert!(fetched.set_cookies().is_empty());
        bodies.push(fetched.error_without_request_id());
    }
    assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(stack.td_row(&foreign.device).await, foreign_row);

    let grace_list = stack.td_list(&foreign.credentials, None, "").await;
    assert_eq!(grace_list.json()["totalCount"], 1);
    assert_eq!(grace_list.json()["items"][0]["id"], foreign_row.id);

    clock.advance(RECENT_AUTH_LAPSE);
    let owner_rows = stack.td_rows(ada).await;
    for path in [format!("{DEVICES}/{}", laptop_row.id), DEVICES.to_owned()] {
        let stale = stack.td_delete(&path, owner, Some(&laptop.device)).await;
        assert_code(&stale, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
        assert!(stale.set_cookies().is_empty());
    }
    assert_eq!(stack.td_rows(ada).await, owner_rows);
    assert_eq!(
        stack.td_list(owner, None, "").await.status,
        StatusCode::OK,
        "listing needs no recent authentication"
    );
    let anonymous = stack
        .td_send(Method::GET, DEVICES, Browser::at(24, FIREFOX), None, None)
        .await;
    assert_code(&anonymous, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    assert_eq!(stack.td_rows(grace).await, [foreign_row]);
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_revocation_matrix() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let (_, ada_tf) = stack.td_enrolled("ada", 10).await;
    let browser = Browser::at(20, FIREFOX);

    let first = stack.td_remember("ada", &ada_tf, browser).await;
    let regenerated = stack
        .call(Call::new(Method::POST, REGENERATE, &first.credentials), 20)
        .await;
    assert_eq!(regenerated.status, StatusCode::OK, "{}", regenerated.text());
    assert_eq!(stack.td_row(&first.device).await.revoked_at, None);

    let via_device = assert_trusted_sign_in(
        &stack
            .td_login("ada", PASSWORD, browser.with_device(&first.device))
            .await,
    );
    let logged_out = stack.logout(LogoutRequest::with(&via_device), 20).await;
    assert_eq!(logged_out.status, StatusCode::NO_CONTENT);
    assert!(!cookie_names(&logged_out).contains("palmr_device"));
    let revoked_sessions = stack
        .call(
            Call::new(
                Method::DELETE,
                &format!("{SESSIONS}?includeCurrent=true"),
                &first.credentials,
            ),
            20,
        )
        .await;
    assert_eq!(revoked_sessions.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.td_row(&first.device).await.revoked_at, None);

    let current = assert_trusted_sign_in(
        &stack
            .td_login("ada", PASSWORD, browser.with_device(&first.device))
            .await,
    );
    let second = stack
        .td_remember("ada", &ada_tf, Browser::at(21, CHROME))
        .await;
    let changed = stack
        .change_password(&current, PASSWORD, NEW_PASSWORD, 20)
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    for device in [&first.device, &second.device] {
        assert_eq!(
            stack.td_row(device).await.revoked_at,
            Some(stamp(clock_now(&clock)))
        );
        assert_second_factor_required(
            &stack
                .td_login("ada", NEW_PASSWORD, browser.with_device(device))
                .await,
        );
    }
    assert_eq!(
        stack.audit_rows("PASSWORD_CHANGED").await.last().unwrap().2,
        json!({ "forced": false, "sessions_revoked": 1, "trusted_devices_revoked": 2 }).to_string()
    );

    let (grace, grace_tf) = stack.td_enrolled("grace", 11).await;
    let grace_device = stack
        .td_remember("grace", &grace_tf, Browser::at(22, FIREFOX))
        .await;
    let disabled = stack
        .call(
            Call::new(Method::POST, DISABLE, &grace_device.credentials),
            22,
        )
        .await;
    assert!(disabled.status.is_success(), "{}", disabled.text());
    assert!(stack
        .td_row(&grace_device.device)
        .await
        .revoked_at
        .is_some());
    assert_eq!(stack.tf_secret_row(grace).await, None);

    let hash = password_hash();
    let linus = stack
        .user(UserSpec::local("linus", "linus@example.test", &hash))
        .await;
    let preexisting = fresh_token();
    stack
        .td_insert_device(linus, &preexisting, "2026-12-01T00:00:00.000Z")
        .await;
    let linus_session = stack.signed_in("linus", 23).await;
    stack.tf_enable(&linus_session, 23).await;
    assert_eq!(stack.td_row(&preexisting).await.revoked_at, None);
    assert_trusted_sign_in(
        &stack
            .td_login(
                "linus",
                PASSWORD,
                Browser::at(24, FIREFOX).with_device(&preexisting),
            )
            .await,
    );

    stack.flush_audit().await;
    stack.stop().await;
    let (outcome, _) = run_recover(root.path(), &clock, "linus").await;
    assert!(outcome.unwrap().facts.role_changed);
    let stack = Stack::start(root.path(), &clock).await;
    assert_eq!(stack.security_state(linus).await.role, "admin");
    assert!(stack
        .sessions_of(linus)
        .await
        .iter()
        .all(|row| row.state == "revoked"));
    assert_eq!(
        stack.td_row(&preexisting).await.revoked_at,
        None,
        "a role change revokes sessions, not trusted devices"
    );
    let promoted = assert_trusted_sign_in(
        &stack
            .td_login(
                "linus",
                PASSWORD,
                Browser::at(25, FIREFOX).with_device(&preexisting),
            )
            .await,
    );
    assert_eq!(
        stack.td_link(&promoted.session).await.auth_method,
        "password_trusted_device"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_trusted_device_routes_use_session_keyed_limits() {
    let inventory = application_routes().build().unwrap().inventory;
    for (method, path, expected) in [
        (Method::GET, DEVICES, LIST_ROUTE),
        (
            Method::DELETE,
            "/api/v1/auth/trusted-devices/{id}",
            REVOKE_ONE_ROUTE,
        ),
        (Method::DELETE, DEVICES, REVOKE_ALL_ROUTE),
    ] {
        assert_eq!(inventory.get(&method, path).unwrap().policy(), expected);
    }
    assert_eq!(
        (LIST_ROUTE.auth(), LIST_ROUTE.rate_limit()),
        (AuthClass::Authenticated, RateLimitClass::Read)
    );
    for route in [REVOKE_ONE_ROUTE, REVOKE_ALL_ROUTE] {
        assert_eq!(
            (route.auth(), route.rate_limit()),
            (AuthClass::AuthenticatedRecentAuth, RateLimitClass::Write)
        );
    }

    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (_, ada_tf) = stack.td_enrolled("ada", 10).await;
    let first = stack
        .td_remember("ada", &ada_tf, Browser::at(20, FIREFOX))
        .await;
    let second = stack
        .td_remember("ada", &ada_tf, Browser::at(20, FIREFOX))
        .await;

    stack
        .exhaust_session_bucket(RateLimitClass::Read, &first.credentials)
        .await;
    let throttled = stack.td_list(&first.credentials, None, "").await;
    assert_throttled(&throttled, RateLimitClass::Read);
    assert!(!throttled.text().contains(&first.credentials.session));
    assert_eq!(
        stack.td_list(&second.credentials, None, "").await.status,
        StatusCode::OK,
        "another session behind the same address keeps its own budget"
    );

    stack
        .exhaust_session_bucket(RateLimitClass::Write, &first.credentials)
        .await;
    let first_row = stack.td_row(&first.device).await;
    for path in [format!("{DEVICES}/{}", first_row.id), DEVICES.to_owned()] {
        let throttled = stack
            .td_delete(&path, &first.credentials, Some(&first.device))
            .await;
        assert_throttled(&throttled, RateLimitClass::Write);
        assert!(throttled.set_cookies().is_empty());
    }
    assert_eq!(stack.td_row(&first.device).await, first_row);
    let revoked = stack
        .td_delete(
            &format!("{DEVICES}/{}", first_row.id),
            &second.credentials,
            None,
        )
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.text());
    stack.stop().await;
}
