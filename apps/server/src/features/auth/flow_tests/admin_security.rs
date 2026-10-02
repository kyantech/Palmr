use tracing_subscriber::filter::EnvFilter;

use super::password_reset::Capture;
use super::profile::{assert_code, Call};
use super::*;
use crate::config::LogFormat;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::telemetry::build_dispatch;

const USERS: &str = "/api/v1/admin/users";
const PROFILE_PASSWORD: &str = "/api/v1/profile/password";
const PROFILE_USAGE: &str = "/api/v1/profile/usage";
const ABSENT: &str = "0192f3a1-0000-7000-8000-00000000abcd";
const REPLACEMENT: &str = "a replacement passphrase";

type UserRow = (String, i64, String, Option<i64>);
type LockRow = (i64, Option<String>, i64, Option<String>, Option<String>);

impl Stack {
    async fn security_call(
        &self,
        method: Method,
        path: &str,
        creds: &Credentials,
        body: Option<&Value>,
        host: u8,
    ) -> Fetched {
        let mut call = Call::new(method, path, creds);
        if let Some(body) = body {
            call = call.json(body);
        }
        self.call(call, host).await
    }

    async fn reset_password_of(&self, creds: &Credentials, id: &str, host: u8) -> Fetched {
        self.security_call(
            Method::POST,
            &format!("{USERS}/{id}/password-reset"),
            creds,
            None,
            host,
        )
        .await
    }

    pub(super) async fn unlock_user_of(&self, creds: &Credentials, id: &str, host: u8) -> Fetched {
        self.security_call(
            Method::POST,
            &format!("{USERS}/{id}/unlock"),
            creds,
            None,
            host,
        )
        .await
    }

    async fn revoke_sessions_of(&self, creds: &Credentials, id: &str, host: u8) -> Fetched {
        self.security_call(
            Method::DELETE,
            &format!("{USERS}/{id}/sessions"),
            creds,
            None,
            host,
        )
        .await
    }

    async fn put_quota(&self, creds: &Credentials, id: &str, body: &Value, host: u8) -> Fetched {
        self.security_call(
            Method::PUT,
            &format!("{USERS}/{id}/quota"),
            creds,
            Some(body),
            host,
        )
        .await
    }

    async fn user_detail(&self, creds: &Credentials, id: &str, host: u8) -> Value {
        let fetched = self
            .security_call(Method::GET, &format!("{USERS}/{id}"), creds, None, host)
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn signed_in_ok(&self, identifier: &str, password: &str, host: u8) -> Credentials {
        let fetched = self.login(identifier, password, host).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        Credentials::from(&fetched)
    }

    async fn me_status(&self, creds: &Credentials, host: u8) -> StatusCode {
        self.get(ME, Some(&creds.session), host).await.status
    }

    async fn usage_quota(&self, creds: &Credentials, host: u8) -> Value {
        let usage = self.get(PROFILE_USAGE, Some(&creds.session), host).await;
        assert_eq!(usage.status, StatusCode::OK, "{}", usage.text());
        usage.json()["quotaBytes"].clone()
    }

    async fn seed_reset_link(&self, user: UserId) {
        self.execute(&format!(
            "INSERT INTO password_reset_tokens (id, user_id, token_hash, created_at, expires_at)
             VALUES ('link-{user}', '{user}', '{:x<64}', '2026-09-25T12:00:00.000Z',
                     '2026-09-26T12:00:00.000Z')",
            format!("link-{user}")
        ))
        .await;
    }

    async fn seed_lockout(&self, user: UserId) {
        self.execute(&format!(
            "INSERT INTO account_lockouts (user_id, failed_count, first_failed_at, last_failed_at,
                                           locked_until, lock_count, updated_at)
             VALUES ('{user}', 5, '2026-09-25T11:59:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:10:00.000Z', 2, '2026-09-25T12:00:00.000Z')"
        ))
        .await;
    }

    async fn seed_totp(&self, user: UserId) {
        self.execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{user}'"
        ))
        .await;
        self.execute(&format!(
            "INSERT INTO totp_secrets (user_id, secret_ciphertext, secret_nonce, state,
                                       last_used_step, confirmed_at, created_at, updated_at)
             VALUES ('{user}', x'0102030405060708', zeroblob(24), 'active', 7,
                     '2026-09-25T11:00:00.000Z', '2026-09-25T11:00:00.000Z',
                     '2026-09-25T11:00:00.000Z')"
        ))
        .await;
        self.execute(&format!(
            "INSERT INTO totp_backup_codes (id, user_id, batch_id, code_hash, created_at)
             VALUES ('code-{user}', '{user}', 'batch', '{:x<64}', '2026-09-25T11:00:00.000Z')",
            "c"
        ))
        .await;
    }

    async fn seed_identity_link(&self, user: UserId) {
        let key = format!("idp-{}", &user.to_string()[28..]);
        self.execute(&format!(
            "INSERT INTO identity_providers (id, key, display_name, kind, client_id,
                client_secret_ciphertext, client_secret_nonce, created_at, updated_at)
             VALUES ('provider-{user}', '{key}', 'Provider', 'oidc', 'client-id',
                     x'deadbeefdeadbeef', zeroblob(24), '2026-09-25T11:00:00.000Z',
                     '2026-09-25T11:00:00.000Z')"
        ))
        .await;
        self.execute(&format!(
            "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method, state,
                created_at)
             VALUES ('link-id-{user}', '{user}', 'provider-{user}', 'subject-{user}',
                     'auto_verified_email', 'active', '2026-09-25T11:00:00.000Z')"
        ))
        .await;
    }

    async fn unrelated_state(
        &self,
        user: UserId,
    ) -> (UserRow, Vec<String>, Vec<String>, Vec<String>) {
        let account: UserRow = sqlx::query_as(
            "SELECT role, is_active, quota_override_mode, quota_bytes FROM users WHERE id = ?1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        let totp = self
            .strings(&format!(
                "SELECT user_id || '|' || state || '|' || COALESCE(last_used_step, '') || '|'
                        || hex(secret_ciphertext) || '|' || updated_at
                   FROM totp_secrets WHERE user_id = '{user}'"
            ))
            .await;
        let codes = self
            .strings(&format!(
                "SELECT id || '|' || code_hash || '|' || COALESCE(used_at, '')
                   FROM totp_backup_codes WHERE user_id = '{user}' ORDER BY id"
            ))
            .await;
        let links = self
            .strings(&format!(
                "SELECT id || '|' || state || '|' || COALESCE(suspended_at, '')
                   FROM identity_links WHERE user_id = '{user}' ORDER BY id"
            ))
            .await;
        let totp_flag: bool = sqlx::query_scalar("SELECT totp_enabled FROM users WHERE id = ?1")
            .bind(user.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap();
        let mut totp = totp;
        totp.push(format!("enabled={totp_flag}"));
        (account, totp, codes, links)
    }

    async fn strings(&self, sql: &str) -> Vec<String> {
        sqlx::query_scalar(sql)
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn lock_details(&self, user: UserId) -> Option<LockRow> {
        sqlx::query_as(&format!(
            "SELECT failed_count, locked_until, lock_count, cleared_at, cleared_by
               FROM account_lockouts WHERE user_id = '{user}'"
        ))
        .fetch_optional(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn audit_entries(
        &self,
        action: &str,
    ) -> Vec<(String, Option<String>, Option<String>, String)> {
        sqlx::query_as(&format!(
            "SELECT actor_type, actor_user_id, target_id, metadata_json
               FROM audit_events WHERE action = '{action}' ORDER BY id"
        ))
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn total_audit_entries(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM audit_events").await
    }

    async fn reset_link_state(&self, user: UserId) -> (Option<String>, Option<String>) {
        sqlx::query_as(&format!(
            "SELECT used_at, invalidated_at FROM password_reset_tokens WHERE user_id = '{user}'"
        ))
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn quota_columns(&self, user: UserId) -> (String, Option<i64>, String) {
        sqlx::query_as(&format!(
            "SELECT quota_override_mode, quota_bytes, updated_at FROM users WHERE id = '{user}'"
        ))
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }
}

fn metadata(row: &(String, Option<String>, Option<String>, String)) -> Value {
    serde_json::from_str(&row.3).unwrap()
}

fn object_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[tokio::test]
async fn it_admin_password_reset_effects() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let cyd = stack
        .user(UserSpec::local("cyd", "cyd@example.test", &hash))
        .await;
    let first = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let second = stack.signed_in_ok("bea", PASSWORD, 12).await;
    let cyd_session = stack.signed_in_ok("cyd", PASSWORD, 13).await;
    for (user, id) in [
        (bea, "device-bea-1"),
        (bea, "device-bea-2"),
        (cyd, "device-cyd"),
    ] {
        stack.trusted_device(user, id, None).await;
    }
    stack.seed_reset_link(bea).await;
    stack.seed_reset_link(cyd).await;
    stack.seed_identity_link(bea).await;
    stack.seed_lockout(bea).await;
    let locked = stack.login("bea", PASSWORD, 14).await;
    assert_code(&locked, StatusCode::TOO_MANY_REQUESTS, "AUTH_LOCKED");

    let before = stack.security_state(bea).await;
    let unrelated = stack.unrelated_state(bea).await;
    let cyd_before = (
        stack.security_state(cyd).await,
        stack.sessions_of(cyd).await,
        stack.devices_of(cyd).await,
        stack.reset_link_state(cyd).await,
    );
    let minimum = AccountPasswordPolicy::from_settings(&stack.settings.current()).min_length();

    clock.advance(Duration::from_secs(1));
    let reset = stack
        .reset_password_of(&operator, &bea.to_string(), 20)
        .await;
    assert_eq!(reset.status, StatusCode::OK, "{}", reset.text());
    assert_eq!(reset.headers.get("cache-control").unwrap(), "no-store");
    assert!(
        reset.set_cookies().is_empty(),
        "no session is minted or expired"
    );
    let body = reset.json();
    assert_eq!(
        object_keys(&body),
        ["mustChangePassword", "temporaryPassword"]
    );
    assert_eq!(body["mustChangePassword"], true);
    let temporary = body["temporaryPassword"].as_str().unwrap().to_owned();
    assert!(temporary.chars().count() >= usize::try_from(minimum).unwrap());
    assert_ne!(temporary, PASSWORD);

    let after = stack.security_state(bea).await;
    assert_ne!(after.password_hash, before.password_hash);
    assert!(after
        .password_hash
        .as_deref()
        .unwrap()
        .starts_with("$argon2id$"));
    assert_ne!(after.password_updated_at, before.password_updated_at);
    assert!(after.must_change_password);
    assert_eq!(
        (
            &after.first_name,
            &after.last_name,
            &after.email,
            &after.username,
            &after.role,
            after.is_active
        ),
        (
            &before.first_name,
            &before.last_name,
            &before.email,
            &before.username,
            &before.role,
            before.is_active
        )
    );
    assert_eq!(stack.unrelated_state(bea).await, unrelated);

    for session in stack.sessions_of(bea).await {
        assert_eq!(
            (session.state.as_str(), session.revoked_reason.as_deref()),
            ("revoked", Some("password_reset"))
        );
    }
    assert_eq!(stack.me_status(&first, 30).await, StatusCode::UNAUTHORIZED);
    assert_eq!(stack.me_status(&second, 30).await, StatusCode::UNAUTHORIZED);
    assert_eq!(stack.me_status(&cyd_session, 30).await, StatusCode::OK);
    assert_eq!(stack.me_status(&operator, 30).await, StatusCode::OK);
    let devices = stack.devices_of(bea).await;
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().all(|(_, revoked)| revoked.is_some()));
    let (link_used, link_invalidated) = stack.reset_link_state(bea).await;
    assert!(link_used.is_none() && link_invalidated.is_some());
    assert_eq!(
        (
            stack.security_state(cyd).await,
            stack.sessions_of(cyd).await,
            stack.devices_of(cyd).await,
            stack.reset_link_state(cyd).await,
        ),
        cyd_before,
        "another user is untouched"
    );

    let (failed, locked_until, lock_count, cleared_at, cleared_by) =
        stack.lock_details(bea).await.unwrap();
    assert_eq!((failed, locked_until, lock_count), (0, None, 2));
    assert!(cleared_at.is_some());
    assert_eq!(cleared_by.as_deref(), Some(operator_id.as_str()));

    let old = stack.login("bea", PASSWORD, 40).await;
    assert_code(&old, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    let forced = stack.login("bea", &temporary, 41).await;
    assert_eq!(forced.status, StatusCode::OK, "{}", forced.text());
    assert_eq!(forced.json()["mustChangePassword"], true);
    let restricted = Credentials::from(&forced);
    let me = stack.get(ME, Some(&restricted.session), 41).await.json();
    assert_eq!(me["restriction"], "must_change_password");
    let released = stack
        .security_call(
            Method::POST,
            PROFILE_PASSWORD,
            &restricted,
            Some(&json!({ "newPassword": REPLACEMENT })),
            41,
        )
        .await;
    assert_eq!(
        released.status,
        StatusCode::NO_CONTENT,
        "{}",
        released.text()
    );
    assert!(!stack.security_state(bea).await.must_change_password);
    assert_eq!(
        stack.login("bea", REPLACEMENT, 42).await.status,
        StatusCode::OK
    );

    let rows = stack.audit_entries("USER_PASSWORD_RESET_BY_ADMIN").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "user");
    assert_eq!(rows[0].1.as_deref(), Some(operator_id.as_str()));
    assert_eq!(rows[0].2.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(
        metadata(&rows[0]),
        json!({
            "sessions_revoked": 2,
            "trusted_devices_revoked": 2,
            "reset_links_invalidated": 1,
            "lockout_cleared": true
        })
    );
    assert!(!rows[0].3.contains(&temporary));
    assert!(stack.audit_entries("ALL_SESSIONS_REVOKED").await.is_empty());
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_password_reset_keeps_totp_and_requires_the_second_factor() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let dora = stack
        .user(UserSpec::local("dora", "dora@example.test", &hash))
        .await;
    stack.seed_totp(dora).await;
    stack.seed_identity_link(dora).await;
    let before = stack.unrelated_state(dora).await;

    let reset = stack
        .reset_password_of(&operator, &dora.to_string(), 20)
        .await;
    assert_eq!(reset.status, StatusCode::OK, "{}", reset.text());
    let temporary = reset.json()["temporaryPassword"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(stack.unrelated_state(dora).await, before);
    assert!(stack.security_state(dora).await.must_change_password);

    let challenged = stack.login("dora", &temporary, 30).await;
    assert_code(&challenged, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    let old = stack.login("dora", PASSWORD, 31).await;
    assert_code(&old, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_password_reset_rejects_ineligible_targets_and_callers_without_side_effects() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    stack.seed_identity_link(sso).await;
    let bea_session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let sso_before = stack.unrelated_state(sso).await;
    let bea_before = stack.security_state(bea).await;
    let audit_before = stack.total_audit_entries().await;

    let refused = stack
        .reset_password_of(&operator, &sso.to_string(), 20)
        .await;
    assert_code(&refused, StatusCode::CONFLICT, "USER_HAS_NO_LOCAL_AUTH");
    assert!(!refused.text().contains("temporaryPassword"));
    let no_hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
            .bind(sso.to_string())
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        no_hash, None,
        "an SSO-only account never gains a local credential"
    );
    assert_eq!(stack.unrelated_state(sso).await, sso_before);

    for id in [ABSENT, "not-a-uuid"] {
        let missing = stack.reset_password_of(&operator, id, 21).await;
        assert_code(&missing, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    }

    let forbidden = stack
        .reset_password_of(&bea_session, &bea.to_string(), 22)
        .await;
    assert_code(&forbidden, StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(stack.security_state(bea).await, bea_before);

    clock.advance(Duration::from_secs(31 * 60));
    let lapsed = stack
        .reset_password_of(&operator, &bea.to_string(), 23)
        .await;
    assert_code(&lapsed, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(stack.security_state(bea).await, bea_before);
    assert_eq!(stack.total_audit_entries().await, audit_before);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM sessions WHERE state = 'revoked'")
            .await,
        0
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_password_reset_of_the_acting_admin_revokes_the_current_session() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let other = stack.signed_in_ok("root", PASSWORD, 11).await;

    let reset = stack.reset_password_of(&operator, &operator_id, 20).await;
    assert_eq!(reset.status, StatusCode::OK, "{}", reset.text());
    assert_eq!(reset.cookie("palmr_session"), "");
    assert_eq!(reset.cookie("palmr_csrf"), "");
    let temporary = reset.json()["temporaryPassword"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        stack.me_status(&operator, 21).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(stack.me_status(&other, 21).await, StatusCode::UNAUTHORIZED);
    let forced = stack.login("root", &temporary, 22).await;
    assert_eq!(forced.json()["mustChangePassword"], true);
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_password_reset_secret_is_never_stored_logged_or_replayable() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let mut issued = Vec::new();
    for host in [20, 21] {
        let reset = stack
            .reset_password_of(&operator, &bea.to_string(), host)
            .await;
        assert_eq!(reset.status, StatusCode::OK, "{}", reset.text());
        issued.push(
            reset.json()["temporaryPassword"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(issued[0], issued[1], "every reset issues a fresh password");
    assert_eq!(
        stack.login("bea", &issued[0], 30).await.error_code(),
        "AUTH_INVALID_CREDENTIALS",
        "a superseded temporary password no longer works"
    );
    assert_eq!(
        stack.login("bea", &issued[1], 31).await.status,
        StatusCode::OK
    );

    let detail = stack
        .get(&format!("{USERS}/{bea}"), Some(&operator.session), 32)
        .await;
    let list = stack.read_users(&operator, 33).await;
    let logs = capture.text();
    for temporary in &issued {
        for (table, text) in stack.every_stored_text().await {
            assert!(
                !text.contains(temporary),
                "{table} stores a temporary password"
            );
        }
        assert!(!detail.text().contains(temporary));
        assert!(!list.text().contains(temporary));
        assert!(
            !logs.contains(temporary),
            "the logs carry a temporary password"
        );
    }
    assert!(!detail.text().contains("temporaryPassword"));
    for row in stack.audit_entries("USER_PASSWORD_RESET_BY_ADMIN").await {
        for temporary in &issued {
            assert!(!row.3.contains(temporary));
        }
        assert!(!row.3.to_lowercase().contains("password"));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_revoke_sessions_keeps_trusted_devices_and_audits_once() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let cyd = stack
        .user(UserSpec::local("cyd", "cyd@example.test", &hash))
        .await;
    let first = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let second = stack.signed_in_ok("bea", PASSWORD, 12).await;
    let cyd_session = stack.signed_in_ok("cyd", PASSWORD, 13).await;
    stack.trusted_device(bea, "device-bea", None).await;
    stack.trusted_device(cyd, "device-cyd", None).await;
    let bea_state = stack.security_state(bea).await;

    let revoked = stack
        .revoke_sessions_of(&operator, &bea.to_string(), 20)
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.text());
    assert!(revoked.body.is_empty());
    assert!(revoked.set_cookies().is_empty());
    assert_eq!(stack.me_status(&first, 21).await, StatusCode::UNAUTHORIZED);
    assert_eq!(stack.me_status(&second, 21).await, StatusCode::UNAUTHORIZED);
    assert_eq!(stack.me_status(&cyd_session, 21).await, StatusCode::OK);
    assert_eq!(stack.me_status(&operator, 21).await, StatusCode::OK);
    for session in stack.sessions_of(bea).await {
        assert_eq!(
            (session.state.as_str(), session.revoked_reason.as_deref()),
            ("revoked", Some("admin_request"))
        );
    }
    assert_eq!(
        stack.devices_of(bea).await,
        vec![("device-bea".to_owned(), None)],
        "trusted devices are kept"
    );
    assert_eq!(
        stack.devices_of(cyd).await,
        vec![("device-cyd".to_owned(), None)]
    );
    assert_eq!(
        stack.security_state(bea).await,
        bea_state,
        "credentials are kept"
    );
    assert_eq!(
        stack.login("bea", PASSWORD, 22).await.status,
        StatusCode::OK,
        "the user can sign in again"
    );

    let rows = stack.audit_entries("ALL_SESSIONS_REVOKED").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "user");
    assert_eq!(rows[0].1.as_deref(), Some(operator_id.as_str()));
    assert_eq!(rows[0].2.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(
        metadata(&rows[0]),
        json!({ "reason": "admin_request", "include_current": false, "revoked": 2 })
    );

    let fresh = stack.signed_in_ok("bea", PASSWORD, 23).await;
    let again = stack
        .revoke_sessions_of(&operator, &bea.to_string(), 24)
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.me_status(&fresh, 25).await, StatusCode::UNAUTHORIZED);
    let repeat = stack
        .revoke_sessions_of(&operator, &bea.to_string(), 26)
        .await;
    assert_eq!(repeat.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.audit_entries("ALL_SESSIONS_REVOKED").await.len(),
        2,
        "a call that revokes nothing records nothing"
    );

    for id in [ABSENT, "not-a-uuid"] {
        assert_code(
            &stack.revoke_sessions_of(&operator, id, 27).await,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        );
    }
    assert_code(
        &stack
            .revoke_sessions_of(&cyd_session, &bea.to_string(), 28)
            .await,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
    clock.advance(Duration::from_secs(31 * 60));
    assert_code(
        &stack
            .revoke_sessions_of(&operator, &cyd.to_string(), 29)
            .await,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(stack.me_status(&cyd_session, 30).await, StatusCode::OK);
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_revoke_sessions_of_the_acting_admin_revokes_the_current_session() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let other = stack.signed_in_ok("root", PASSWORD, 11).await;
    let id: UserId = operator_id.parse().unwrap();
    stack.trusted_device(id, "device-root", None).await;

    let revoked = stack.revoke_sessions_of(&operator, &operator_id, 20).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.text());
    assert_eq!(revoked.cookie("palmr_session"), "");
    assert_eq!(revoked.cookie("palmr_csrf"), "");
    assert_eq!(
        stack.me_status(&operator, 21).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(stack.me_status(&other, 21).await, StatusCode::UNAUTHORIZED);
    for session in stack.sessions_of(id).await {
        assert_eq!(
            (session.state.as_str(), session.revoked_reason.as_deref()),
            ("revoked", Some("admin_request"))
        );
    }
    assert_eq!(
        stack.devices_of(id).await,
        vec![("device-root".to_owned(), None)]
    );
    let rows = stack.audit_entries("ALL_SESSIONS_REVOKED").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        metadata(&rows[0]),
        json!({ "reason": "admin_request", "include_current": true, "revoked": 2 })
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_unlock_clears_the_durable_lockout_and_nothing_else() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let inert = stack
        .user(UserSpec {
            active: false,
            ..UserSpec::local("inert", "inert@example.test", &hash)
        })
        .await;
    let calm = stack
        .user(UserSpec::local("calm", "calm@example.test", &hash))
        .await;
    let session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    stack.seed_totp(bea).await;
    stack.seed_lockout(bea).await;
    stack.seed_lockout(inert).await;
    let bea_before = (
        stack.security_state(bea).await,
        stack.unrelated_state(bea).await,
        stack.devices_of(bea).await,
    );
    let audit_before = stack.total_audit_entries().await;
    let locked = stack.login("bea", PASSWORD, 12).await;
    assert_code(&locked, StatusCode::TOO_MANY_REQUESTS, "AUTH_LOCKED");

    let detail = stack
        .get(&format!("{USERS}/{bea}"), Some(&operator.session), 13)
        .await
        .json();
    assert_eq!(detail["isLockedOut"], true);
    assert_eq!(detail["lockout"]["failedCount"], 5);

    let unlocked = stack.unlock_user_of(&operator, &bea.to_string(), 20).await;
    assert_eq!(
        unlocked.status,
        StatusCode::NO_CONTENT,
        "{}",
        unlocked.text()
    );
    assert!(unlocked.body.is_empty());
    assert!(unlocked.set_cookies().is_empty());
    let (failed, until, lock_count, cleared_at, cleared_by) =
        stack.lock_details(bea).await.unwrap();
    assert_eq!((failed, until, lock_count), (0, None, 2));
    assert_eq!(cleared_by.as_deref(), Some(operator_id.as_str()));
    let cleared_at = cleared_at.unwrap();
    assert_eq!(
        (
            stack.security_state(bea).await,
            stack.unrelated_state(bea).await,
            stack.devices_of(bea).await,
        ),
        bea_before,
        "password, TOTP, devices, role and activation are untouched"
    );
    assert_eq!(stack.me_status(&session, 21).await, StatusCode::OK);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM sessions WHERE state = 'revoked'")
            .await,
        0,
        "unlock revokes no session"
    );
    assert_eq!(stack.total_audit_entries().await, audit_before);
    let detail = stack
        .get(&format!("{USERS}/{bea}"), Some(&operator.session), 22)
        .await
        .json();
    assert_eq!(detail["isLockedOut"], false);
    assert_eq!(detail["lockout"]["lockedUntil"], Value::Null);
    assert_eq!(detail["lockout"]["failedCount"], 0);
    assert_eq!(detail["lockout"]["lockCount"], 2);
    let proven = stack.login("bea", PASSWORD, 23).await;
    assert_code(&proven, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");

    clock.advance(Duration::from_secs(5));
    let repeated = stack.unlock_user_of(&operator, &bea.to_string(), 24).await;
    assert_eq!(repeated.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.lock_details(bea).await.unwrap().3.as_deref(),
        Some(cleared_at.as_str()),
        "an unlock with nothing to clear changes nothing"
    );

    let inactive = stack
        .unlock_user_of(&operator, &inert.to_string(), 25)
        .await;
    assert_eq!(inactive.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.lock_details(inert).await.unwrap().0, 0);
    let still_inactive: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = ?1")
        .bind(inert.to_string())
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert!(!still_inactive, "unlock never activates an account");

    let untouched = stack.unlock_user_of(&operator, &calm.to_string(), 26).await;
    assert_eq!(untouched.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.lock_details(calm).await,
        None,
        "no lockout row is created"
    );

    for id in [ABSENT, "not-a-uuid"] {
        assert_code(
            &stack.unlock_user_of(&operator, id, 27).await,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        );
    }
    assert_code(
        &stack.unlock_user_of(&session, &calm.to_string(), 28).await,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
    assert_eq!(stack.total_audit_entries().await, audit_before);
    stack.stop().await;
}

#[tokio::test]
async fn it_quota_override_unlimited_is_null() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let id = bea.to_string();

    let columns = || async { stack.quota_columns(bea).await };
    assert_eq!(columns().await.0, "inherit");
    assert_eq!(columns().await.1, None);
    assert_eq!(stack.usage_quota(&session, 12).await, Value::Null);

    let inherit = stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 20)
        .await;
    assert_eq!(inherit.status, StatusCode::OK, "{}", inherit.text());
    assert_eq!(
        inherit.json(),
        json!({
            "mode": "inherit",
            "quotaBytes": null,
            "instanceDefaultQuotaBytes": null,
            "effectiveQuotaBytes": null,
            "belowCurrentUsage": false
        })
    );
    assert!(
        stack
            .audit_entries("QUOTA_OVERRIDE_CHANGED")
            .await
            .is_empty(),
        "an unchanged override is not a change"
    );

    stack
        .setting_in("quotas", "default_user_quota_bytes", "integer", "10000")
        .await;
    let inherited = stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 21)
        .await;
    assert_eq!(
        inherited.json(),
        json!({
            "mode": "inherit",
            "quotaBytes": null,
            "instanceDefaultQuotaBytes": 10000,
            "effectiveQuotaBytes": 10000,
            "belowCurrentUsage": false
        })
    );
    assert_eq!(stack.usage_quota(&session, 22).await, 10000);

    let unlimited = stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 23)
        .await;
    assert_eq!(unlimited.status, StatusCode::OK, "{}", unlimited.text());
    assert_eq!(
        unlimited.json(),
        json!({
            "mode": "unlimited",
            "quotaBytes": null,
            "instanceDefaultQuotaBytes": 10000,
            "effectiveQuotaBytes": null,
            "belowCurrentUsage": false
        }),
        "explicit Unlimited is distinct from inherit under a finite default"
    );
    let (mode, bytes, _) = columns().await;
    assert_eq!((mode.as_str(), bytes), ("unlimited", None));
    assert_eq!(stack.usage_quota(&session, 24).await, Value::Null);

    let zero = stack
        .put_quota(
            &operator,
            &id,
            &json!({ "mode": "bytes", "quotaBytes": 0 }),
            25,
        )
        .await;
    assert_eq!(
        zero.json(),
        json!({
            "mode": "bytes",
            "quotaBytes": 0,
            "instanceDefaultQuotaBytes": 10000,
            "effectiveQuotaBytes": 0,
            "belowCurrentUsage": false
        })
    );
    let (mode, bytes, _) = columns().await;
    assert_eq!((mode.as_str(), bytes), ("bytes", Some(0)));
    assert_eq!(
        stack.usage_quota(&session, 26).await,
        0,
        "a zero-byte cap is not Unlimited"
    );

    let capped = stack
        .put_quota(
            &operator,
            &id,
            &json!({ "mode": "bytes", "quotaBytes": 107_374_182_400_u64 }),
            27,
        )
        .await;
    assert_eq!(capped.json()["quotaBytes"], 107_374_182_400_u64);
    assert_eq!(capped.json()["effectiveQuotaBytes"], 107_374_182_400_u64);
    assert_eq!(stack.usage_quota(&session, 28).await, 107_374_182_400_u64);

    let back = stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 29)
        .await;
    assert_eq!(back.json()["quotaBytes"], Value::Null);
    let (mode, bytes, _) = columns().await;
    assert_eq!((mode.as_str(), bytes), ("inherit", None));
    assert_eq!(stack.usage_quota(&session, 30).await, 10000);

    let rows = stack.audit_entries("QUOTA_OVERRIDE_CHANGED").await;
    let recorded: Vec<Value> = rows.iter().map(metadata).collect();
    assert_eq!(
        recorded,
        vec![
            json!({ "from_mode": "inherit", "from_quota_bytes": null, "to_mode": "unlimited", "to_quota_bytes": null }),
            json!({ "from_mode": "unlimited", "from_quota_bytes": null, "to_mode": "bytes", "to_quota_bytes": 0 }),
            json!({ "from_mode": "bytes", "from_quota_bytes": 0, "to_mode": "bytes", "to_quota_bytes": 107_374_182_400_u64 }),
            json!({ "from_mode": "bytes", "from_quota_bytes": 107_374_182_400_u64, "to_mode": "inherit", "to_quota_bytes": null }),
        ]
    );
    for row in &rows {
        assert_eq!(row.0, "user");
        assert_eq!(row.1.as_deref(), Some(operator_id.as_str()));
        assert_eq!(row.2.as_deref(), Some(id.as_str()));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_user_detail_exposes_the_persisted_quota_override_mode() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let id = bea.to_string();

    let detail = stack.user_detail(&operator, &id, 11).await;
    assert_eq!(detail["quotaOverrideMode"], "inherit");
    assert_eq!(detail["quotaBytes"], Value::Null);
    assert_eq!(detail["effectiveQuotaBytes"], Value::Null);

    let unlimited = stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 12)
        .await;
    assert_eq!(unlimited.status, StatusCode::OK, "{}", unlimited.text());
    let detail = stack.user_detail(&operator, &id, 13).await;
    assert_eq!(
        detail["quotaOverrideMode"], "unlimited",
        "explicit Unlimited stays distinct from inherit while the instance default is Unlimited"
    );
    assert_eq!(detail["quotaBytes"], Value::Null);
    assert_eq!(detail["effectiveQuotaBytes"], Value::Null);
    assert_eq!(stack.quota_columns(bea).await.0, "unlimited");

    let zero = stack
        .put_quota(
            &operator,
            &id,
            &json!({ "mode": "bytes", "quotaBytes": 0 }),
            14,
        )
        .await;
    assert_eq!(zero.status, StatusCode::OK, "{}", zero.text());
    let detail = stack.user_detail(&operator, &id, 15).await;
    assert_eq!(detail["quotaOverrideMode"], "bytes");
    assert_eq!(detail["quotaBytes"], 0);
    assert_eq!(detail["effectiveQuotaBytes"], 0);

    let back = stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 16)
        .await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.text());
    let detail = stack.user_detail(&operator, &id, 17).await;
    assert_eq!(detail["quotaOverrideMode"], "inherit");
    assert_eq!(detail["quotaBytes"], Value::Null);
    assert_eq!(detail["effectiveQuotaBytes"], Value::Null);

    stack
        .setting_in("quotas", "default_user_quota_bytes", "integer", "10000")
        .await;
    let detail = stack.user_detail(&operator, &id, 18).await;
    assert_eq!(detail["quotaOverrideMode"], "inherit");
    assert_eq!(detail["effectiveQuotaBytes"], 10000);
    stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 19)
        .await;
    let detail = stack.user_detail(&operator, &id, 20).await;
    assert_eq!(detail["quotaOverrideMode"], "unlimited");
    assert_eq!(detail["effectiveQuotaBytes"], Value::Null);
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_quota_below_usage_is_allowed_warns_and_blocks_future_admission_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 5000 WHERE id IN ('{bea}', '{operator_id}')"
        ))
        .await;
    let id = bea.to_string();

    for (cap, below) in [(1000_u64, true), (4999, true), (5000, false), (6000, false)] {
        let reply = stack
            .put_quota(
                &operator,
                &id,
                &json!({ "mode": "bytes", "quotaBytes": cap }),
                20,
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        assert_eq!(reply.json()["belowCurrentUsage"], below, "cap {cap}");
        assert_eq!(reply.json()["effectiveQuotaBytes"], cap);
        assert_eq!(stack.usage_quota(&session, 21).await, cap);
        let used: i64 = stack
            .scalar_i64(&format!("SELECT used_bytes FROM users WHERE id = '{bea}'"))
            .await;
        assert_eq!(used, 5000, "lowering a quota deletes and recounts nothing");
    }

    stack
        .setting_in("quotas", "default_user_quota_bytes", "integer", "100")
        .await;
    let inherited = stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 22)
        .await;
    assert_eq!(inherited.json()["belowCurrentUsage"], true);
    assert_eq!(inherited.json()["effectiveQuotaBytes"], 100);
    let unlimited = stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 23)
        .await;
    assert_eq!(unlimited.json()["belowCurrentUsage"], false);

    let own = stack
        .put_quota(
            &operator,
            &operator_id,
            &json!({ "mode": "bytes", "quotaBytes": 1000 }),
            24,
        )
        .await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text());
    assert_eq!(own.json()["belowCurrentUsage"], true);
    assert_eq!(
        stack.usage_quota(&operator, 25).await,
        1000,
        "the Admin role gets no quota bypass"
    );
    let default_admin = stack
        .put_quota(&operator, &operator_id, &json!({ "mode": "inherit" }), 26)
        .await;
    assert_eq!(default_admin.json()["effectiveQuotaBytes"], 100);
    assert_eq!(default_admin.json()["belowCurrentUsage"], true);
    assert_eq!(stack.usage_quota(&operator, 27).await, 100);
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_quota_validates_the_three_state_body_and_the_caller() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let cyd = stack
        .user(UserSpec::local("cyd", "cyd@example.test", &hash))
        .await;
    let bea_session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let id = bea.to_string();
    let audit_before = stack.total_audit_entries().await;

    let safe_max = 9_007_199_254_740_991_u64;
    let accepted = stack
        .put_quota(
            &operator,
            &id,
            &json!({ "mode": "bytes", "quotaBytes": safe_max }),
            20,
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.text());
    assert_eq!(accepted.json()["quotaBytes"], safe_max);
    stack
        .put_quota(&operator, &id, &json!({ "mode": "inherit" }), 21)
        .await;
    let reset = stack.quota_columns(bea).await;
    assert_eq!((reset.0.as_str(), reset.1), ("inherit", None));

    let rejected = [
        (json!({}), "mode"),
        (json!({ "mode": "Inherit" }), "mode"),
        (json!({ "mode": "none" }), "mode"),
        (json!({ "mode": null }), "mode"),
        (json!({ "mode": "bytes" }), "quotaBytes"),
        (json!({ "mode": "bytes", "quotaBytes": null }), "quotaBytes"),
        (json!({ "mode": "bytes", "quotaBytes": -1 }), "quotaBytes"),
        (
            json!({ "mode": "bytes", "quotaBytes": safe_max + 1 }),
            "quotaBytes",
        ),
        (
            json!({ "mode": "bytes", "quotaBytes": 9_223_372_036_854_775_808_u64 }),
            "quotaBytes",
        ),
        (json!({ "mode": "bytes", "quotaBytes": 1.5 }), "quotaBytes"),
        (json!({ "mode": "bytes", "quotaBytes": "5" }), "quotaBytes"),
        (json!({ "mode": "inherit", "quotaBytes": 5 }), "quotaBytes"),
        (
            json!({ "mode": "inherit", "quotaBytes": null }),
            "quotaBytes",
        ),
        (
            json!({ "mode": "unlimited", "quotaBytes": 0 }),
            "quotaBytes",
        ),
        (
            json!({ "mode": "unlimited", "quotaBytes": null }),
            "quotaBytes",
        ),
        (
            json!({ "mode": "unlimited", "quotaBytes": -1 }),
            "quotaBytes",
        ),
        (
            json!({ "mode": "bytes", "quotaBytes": 1, "extra": true }),
            "body",
        ),
    ];
    for (payload, field) in rejected {
        let reply = stack.put_quota(&operator, &id, &payload, 22).await;
        assert_code(&reply, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
        assert_eq!(
            reply.json()["error"]["details"]["fields"],
            json!([field]),
            "{payload}"
        );
    }
    let array = stack.put_quota(&operator, &id, &json!([]), 22).await;
    assert_code(&array, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(stack.quota_columns(bea).await, reset);

    for target in [ABSENT, "not-a-uuid"] {
        assert_code(
            &stack
                .put_quota(&operator, target, &json!({ "mode": "inherit" }), 23)
                .await,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        );
    }
    assert_code(
        &stack
            .put_quota(
                &bea_session,
                &cyd.to_string(),
                &json!({ "mode": "unlimited" }),
                24,
            )
            .await,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
    assert_eq!(stack.quota_columns(cyd).await.0, "inherit");

    clock.advance(Duration::from_secs(31 * 60));
    let lapsed = stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 25)
        .await;
    assert_eq!(
        lapsed.status,
        StatusCode::OK,
        "a quota override needs an Admin session, not a recent authentication: {}",
        lapsed.text()
    );
    assert_eq!(
        stack.total_audit_entries().await,
        audit_before + 3,
        "the two accepted bodies and the final override are the only changes"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_quota_audit_is_atomic_with_the_override() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let session = stack.signed_in_ok("bea", PASSWORD, 11).await;
    let id = bea.to_string();
    stack
        .put_quota(
            &operator,
            &id,
            &json!({ "mode": "bytes", "quotaBytes": 500 }),
            20,
        )
        .await;
    let before = stack.quota_columns(bea).await;
    let audit_before = stack.audit_entries("QUOTA_OVERRIDE_CHANGED").await.len();

    stack
        .execute(
            "CREATE TRIGGER quota_injected_audit_failure BEFORE INSERT ON audit_events
             WHEN NEW.action = 'QUOTA_OVERRIDE_CHANGED'
             BEGIN SELECT RAISE(ABORT, 'injected quota audit failure'); END",
        )
        .await;
    for body in [
        json!({ "mode": "unlimited" }),
        json!({ "mode": "inherit" }),
        json!({ "mode": "bytes", "quotaBytes": 9 }),
    ] {
        let failed = stack.put_quota(&operator, &id, &body, 21).await;
        assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
        assert_eq!(stack.quota_columns(bea).await, before, "{body}");
        assert_eq!(stack.usage_quota(&session, 22).await, 500);
    }
    stack
        .execute("DROP TRIGGER quota_injected_audit_failure")
        .await;
    assert_eq!(
        stack.audit_entries("QUOTA_OVERRIDE_CHANGED").await.len(),
        audit_before
    );

    let applied = stack
        .put_quota(&operator, &id, &json!({ "mode": "unlimited" }), 23)
        .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text());
    assert_eq!(stack.usage_quota(&session, 24).await, Value::Null);
    assert_eq!(
        stack.audit_entries("QUOTA_OVERRIDE_CHANGED").await.len(),
        audit_before + 1
    );
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_admin_security_routes_share_the_admin_write_bucket() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let first = stack.operator(10).await;
    let second = stack.signed_in_ok("root", PASSWORD, 11).await;
    let absent_quota = json!({ "mode": "inherit" });
    for round in 0..60 {
        let reply = match round % 4 {
            0 => stack.reset_password_of(&first, ABSENT, 30).await,
            1 => stack.unlock_user_of(&first, ABSENT, 30).await,
            2 => stack.revoke_sessions_of(&first, ABSENT, 30).await,
            _ => stack.put_quota(&first, ABSENT, &absent_quota, 30).await,
        };
        assert_code(&reply, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    }
    assert_code(
        &stack.unlock_user_of(&first, ABSENT, 30).await,
        StatusCode::TOO_MANY_REQUESTS,
        "RATE_LIMITED",
    );
    assert_code(
        &stack.reset_password_of(&first, ABSENT, 30).await,
        StatusCode::TOO_MANY_REQUESTS,
        "RATE_LIMITED",
    );
    assert_code(
        &stack.unlock_user_of(&second, ABSENT, 30).await,
        StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
    stack.stop().await;
}
