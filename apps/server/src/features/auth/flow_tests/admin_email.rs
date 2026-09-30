use tracing_subscriber::filter::EnvFilter;

use super::password_reset::Capture;
use super::profile::{assert_code, Call, SecurityState};
use super::*;
use crate::config::LogFormat;
use crate::infra::db::DbError;
use crate::infra::telemetry::build_dispatch;

const USERS: &str = "/api/v1/admin/users";
const VERIFY: &str = "/api/v1/auth/email/verify";
const LINK_PREFIX: &str = "https://files.example.test/verify-email/";
const ABSENT: &str = "0192f3a1-0000-7000-8000-00000000abcd";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

type AccountRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);
type VerificationRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);
type OutboxRow = (String, String, String, bool);
type Footprint = (i64, i64, i64, i64);

impl Stack {
    async fn start_email_change(
        &self,
        creds: &Credentials,
        id: &str,
        email: &str,
        host: u8,
    ) -> Fetched {
        let body = json!({ "email": email });
        self.call(
            Call::new(Method::POST, &format!("{USERS}/{id}/email"), creds).json(&body),
            host,
        )
        .await
    }

    async fn resend_email_change(&self, creds: &Credentials, id: &str, host: u8) -> Fetched {
        self.call(
            Call::new(Method::POST, &format!("{USERS}/{id}/email/resend"), creds),
            host,
        )
        .await
    }

    async fn cancel_email_change(&self, creds: &Credentials, id: &str, host: u8) -> Fetched {
        self.call(
            Call::new(Method::DELETE, &format!("{USERS}/{id}/email"), creds),
            host,
        )
        .await
    }

    async fn verify_email(&self, token: &str, host: u8) -> Fetched {
        let body = json!({ "token": token });
        self.post_json(VERIFY, &body.to_string(), host, None).await
    }

    async fn verification_tokens(&self) -> Vec<String> {
        self.mail
            .captured()
            .iter()
            .map(|captured| token_from(&captured.message.text))
            .collect()
    }

    async fn pending_token(&self, creds: &Credentials, id: &str, email: &str, host: u8) -> String {
        let started = self.start_email_change(creds, id, email, host).await;
        assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
        self.deliver_mail().await;
        self.verification_tokens().await.pop().unwrap()
    }

    async fn email_account(&self, user: UserId) -> AccountRow {
        sqlx::query_as(
            "SELECT email, email_normalized, pending_email, pending_email_normalized,
                    email_verified_at
               FROM users WHERE id = ?1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn verification_rows(&self, user: UserId) -> Vec<VerificationRow> {
        sqlx::query_as(
            "SELECT token_hash, email, email_normalized, created_at, expires_at, consumed_at,
                    invalidated_at
               FROM email_verifications
              WHERE user_id = ?1 AND purpose = 'email_change'
              ORDER BY id",
        )
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn email_outbox(&self) -> Vec<OutboxRow> {
        sqlx::query_as(
            "SELECT kind, to_email, state, token_ciphertext IS NOT NULL
               FROM email_outbox ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn email_footprint(&self) -> Footprint {
        (
            self.scalar_i64("SELECT COUNT(*) FROM email_verifications")
                .await,
            self.scalar_i64("SELECT COUNT(*) FROM email_outbox").await,
            self.scalar_i64("SELECT COUNT(*) FROM audit_events").await,
            self.scalar_i64("SELECT COUNT(*) FROM users WHERE pending_email IS NOT NULL")
                .await,
        )
    }

    async fn email_audit(
        &self,
        action: &str,
    ) -> Vec<(String, Option<String>, Option<String>, Value)> {
        let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(&format!(
            "SELECT actor_type, actor_user_id, target_id, metadata_json
               FROM audit_events WHERE action = '{action}' ORDER BY id"
        ))
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(actor, actor_id, target, metadata)| {
                (
                    actor,
                    actor_id,
                    target,
                    serde_json::from_str(&metadata).unwrap(),
                )
            })
            .collect()
    }

    async fn session_fates(&self, user: UserId) -> Vec<(String, Option<String>)> {
        self.sessions_of(user)
            .await
            .into_iter()
            .map(|row| (row.state, row.revoked_reason))
            .collect()
    }

    async fn status_of_me(&self, creds: &Credentials, host: u8) -> StatusCode {
        self.get(ME, Some(&creds.session), host).await.status
    }

    async fn login_status(&self, identifier: &str, host: u8) -> StatusCode {
        self.login(identifier, PASSWORD, host).await.status
    }

    async fn fail_audit_of(&self, action: &str) {
        self.execute(&format!(
            "CREATE TRIGGER injected_email_audit_failure BEFORE INSERT ON audit_events
             WHEN NEW.action = '{action}'
             BEGIN SELECT RAISE(ABORT, 'injected e-mail change audit failure'); END"
        ))
        .await;
    }

    async fn restore_audit(&self) {
        self.execute("DROP TRIGGER injected_email_audit_failure")
            .await;
    }

    async fn sso_link(&self, user: UserId) {
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
            "INSERT INTO identity_links (id, user_id, provider_id, subject, email_at_link,
                email_verified_at_link, link_method, state, created_at)
             VALUES ('link-id-{user}', '{user}', 'provider-{user}', 'subject-{user}',
                     'sso@provider.example', 1, 'auto_verified_email', 'active',
                     '2026-09-25T11:00:00.000Z')"
        ))
        .await;
    }

    async fn sso_link_rows(&self, user: UserId) -> Vec<String> {
        sqlx::query_scalar(&format!(
            "SELECT id || '|' || subject || '|' || COALESCE(email_at_link, '') || '|'
                    || email_verified_at_link || '|' || link_method || '|' || state
               FROM identity_links WHERE user_id = '{user}' ORDER BY id"
        ))
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn try_execute(&self, sql: &str) -> Result<(), DbError> {
        self.pools
            .write_tx(&self.clock, "auth.test_try_execute", async |tx| {
                sqlx::query(sql)
                    .execute(tx.executor())
                    .await
                    .map(|_| ())
                    .map_err(DbError::from)
            })
            .await
    }

    async fn sign_in_fleet(&self, username: &str, first_host: u8) -> (Credentials, Credentials) {
        (
            self.signed_in(username, first_host).await,
            self.signed_in(username, first_host + 1).await,
        )
    }
}

fn token_from(text: &str) -> String {
    let start = text.find(LINK_PREFIX).unwrap() + LINK_PREFIX.len();
    text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn unchanged_but_email(before: &SecurityState, after: &SecurityState) -> bool {
    SecurityState {
        email: before.email.clone(),
        ..after.clone()
    } == *before
}

#[tokio::test]
async fn it_email_change_pending_until_verified() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "Bea@Example.test", &hash))
        .await;
    let cyd = stack
        .user(UserSpec::local("cyd", "cyd@example.test", &hash))
        .await;
    let (first, second) = stack.sign_in_fleet("bea", 11).await;
    let cyd_session = stack.signed_in("cyd", 13).await;
    for (user, id) in [
        (bea, "device-bea-1"),
        (bea, "device-bea-2"),
        (cyd, "device-cyd"),
    ] {
        stack.trusted_device(user, id, None).await;
    }
    let before = stack.security_state(bea).await;
    let cyd_before = (
        stack.security_state(cyd).await,
        stack.sessions_of(cyd).await,
        stack.devices_of(cyd).await,
    );

    clock.advance(Duration::from_secs(1));
    let started = stack
        .start_email_change(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    assert!(started.body.is_empty());
    assert_eq!(started.headers.get("cache-control").unwrap(), "no-store");

    assert_eq!(
        stack.email_account(bea).await,
        (
            "Bea@Example.test".to_owned(),
            "bea@example.test".to_owned(),
            Some("Grace@NewDomain.example".to_owned()),
            Some("grace@newdomain.example".to_owned()),
            None
        ),
        "the canonical address is untouched and the new one is only pending"
    );
    assert!(unchanged_but_email(
        &before,
        &stack.security_state(bea).await
    ));
    let rows = stack.verification_rows(bea).await;
    assert_eq!(rows.len(), 1);
    let (token_hash, email, normalized, created, expires, consumed, invalidated) = &rows[0];
    assert_eq!(token_hash.len(), 64);
    assert_eq!(email, "Grace@NewDomain.example");
    assert_eq!(normalized, "grace@newdomain.example");
    assert_eq!(created, "2026-09-25T12:00:01.000Z");
    assert_eq!(expires, "2026-09-26T12:00:01.000Z");
    assert!(consumed.is_none() && invalidated.is_none());
    assert_eq!(
        stack.email_outbox().await,
        [(
            "email_verification".to_owned(),
            "Grace@NewDomain.example".to_owned(),
            "pending".to_owned(),
            true
        )]
    );

    assert_eq!(
        stack.login_status("Bea@Example.test", 30).await,
        StatusCode::OK,
        "the old address still signs in while the change is pending"
    );
    assert_eq!(stack.login_status("bea", 31).await, StatusCode::OK);
    let unverified = stack.login("grace@newdomain.example", PASSWORD, 32).await;
    assert_code(
        &unverified,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );
    assert_eq!(stack.status_of_me(&first, 33).await, StatusCode::OK);
    assert_eq!(stack.status_of_me(&second, 33).await, StatusCode::OK);
    let me = stack.get(ME, Some(&first.session), 34).await.json();
    assert_eq!(me["user"]["email"], "Bea@Example.test");
    assert_eq!(me["user"]["pendingEmail"], "Grace@NewDomain.example");

    let requested = stack.email_audit("USER_EMAIL_CHANGE_REQUESTED").await;
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].0, "user");
    assert_eq!(requested[0].1.as_deref(), Some(operator_id.as_str()));
    assert_eq!(requested[0].2.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(
        requested[0].3,
        json!({
            "from_email": "Bea@Example.test",
            "to_email": "Grace@NewDomain.example",
            "replaced_pending": false,
            "self_change": false
        })
    );

    stack.deliver_mail().await;
    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].message.to.email.as_str(),
        "Grace@NewDomain.example"
    );
    let token = token_from(&captured[0].message.text);
    assert!(
        captured[0].message.text.contains("1440"),
        "{}",
        captured[0].message.text
    );
    assert_eq!(digest(&token), *token_hash, "only the digest is stored");
    assert_eq!(
        stack.email_outbox().await,
        [(
            "email_verification".to_owned(),
            "Grace@NewDomain.example".to_owned(),
            "sent".to_owned(),
            false
        )],
        "the sealed delivery copy is wiped once sent"
    );
    assert_eq!(
        stack.email_account(bea).await.0,
        "Bea@Example.test",
        "delivery alone changes nothing"
    );

    clock.advance(Duration::from_secs(60));
    let verified = stack.verify_email(&token, 40).await;
    assert_eq!(
        verified.status,
        StatusCode::NO_CONTENT,
        "{}",
        verified.text()
    );
    assert!(verified.body.is_empty());
    assert_eq!(verified.headers.get("cache-control").unwrap(), "no-store");
    assert!(verified.set_cookies().is_empty());

    assert_eq!(
        stack.email_account(bea).await,
        (
            "Grace@NewDomain.example".to_owned(),
            "grace@newdomain.example".to_owned(),
            None,
            None,
            Some("2026-09-25T12:01:01.000Z".to_owned())
        )
    );
    let after = stack.security_state(bea).await;
    assert_eq!(after.email, "Grace@NewDomain.example");
    assert!(unchanged_but_email(&before, &after));
    let rows = stack.verification_rows(bea).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].5.as_deref(), Some("2026-09-25T12:01:01.000Z"));
    assert!(rows[0].6.is_none());

    for (state, reason) in stack.session_fates(bea).await {
        assert_eq!(
            (state.as_str(), reason.as_deref()),
            ("revoked", Some("admin_request"))
        );
    }
    assert_eq!(
        stack.status_of_me(&first, 41).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        stack.status_of_me(&second, 41).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(stack.status_of_me(&cyd_session, 41).await, StatusCode::OK);
    assert_eq!(stack.status_of_me(&operator, 41).await, StatusCode::OK);
    let devices = stack.devices_of(bea).await;
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().all(|(_, revoked)| revoked.is_some()));
    assert_eq!(
        (
            stack.security_state(cyd).await,
            stack.sessions_of(cyd).await,
            stack.devices_of(cyd).await,
        ),
        cyd_before,
        "another user is untouched"
    );

    let old = stack.login("Bea@Example.test", PASSWORD, 50).await;
    assert_code(&old, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    assert_eq!(
        stack.login_status("GRACE@newdomain.EXAMPLE", 51).await,
        StatusCode::OK
    );
    assert_eq!(stack.login_status("bea", 52).await, StatusCode::OK);

    let confirmed = stack.email_audit("USER_EMAIL_CHANGE_CONFIRMED").await;
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].0, "user");
    assert_eq!(confirmed[0].1.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(confirmed[0].2.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(
        confirmed[0].3,
        json!({
            "from_email": "Bea@Example.test",
            "to_email": "Grace@NewDomain.example",
            "sessions_revoked": 4,
            "trusted_devices_revoked": 2
        })
    );

    let replay = stack.verify_email(&token, 42).await;
    assert_code(
        &replay,
        StatusCode::BAD_REQUEST,
        "EMAIL_VERIFICATION_TOKEN_INVALID",
    );
    assert_eq!(
        stack.email_audit("USER_EMAIL_CHANGE_CONFIRMED").await.len(),
        1
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_target_uniqueness() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let cyd = stack
        .user(UserSpec::local("cyd", "cyd@example.test", &hash))
        .await;
    let dee = stack
        .user(UserSpec::local("dee", "dee@example.test", &hash))
        .await;
    let untouched = stack.email_footprint().await;
    let audit_before = untouched.2;

    for (host, candidate) in [(20, "cyd@example.test"), (21, "CYD@EXAMPLE.TEST")] {
        let taken = stack
            .start_email_change(&operator, &bea.to_string(), candidate, host)
            .await;
        assert_code(&taken, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
        assert_eq!(stack.email_footprint().await, untouched);
    }
    let own = stack
        .start_email_change(&operator, &bea.to_string(), "BEA@example.test", 22)
        .await;
    assert_code(&own, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(stack.email_footprint().await, untouched);

    let started = stack
        .start_email_change(&operator, &bea.to_string(), "Fresh@Example.test", 23)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    let pending = (1, 1, audit_before + 1, 1);
    assert_eq!(stack.email_footprint().await, pending);

    for (host, candidate) in [(24, "fresh@example.test"), (25, "FRESH@EXAMPLE.TEST")] {
        let collision = stack
            .start_email_change(&operator, &dee.to_string(), candidate, host)
            .await;
        assert_code(&collision, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
        assert_eq!(
            stack.email_footprint().await,
            pending,
            "a pending address on another user is refused without side effects"
        );
    }
    let canonical_elsewhere = stack
        .start_email_change(&operator, &cyd.to_string(), "dee@example.test", 26)
        .await;
    assert_code(
        &canonical_elsewhere,
        StatusCode::CONFLICT,
        "USER_EMAIL_TAKEN",
    );
    assert_eq!(stack.email_footprint().await, pending);

    let race = stack
        .try_execute(&format!(
            "UPDATE users SET pending_email = 'Fresh@Example.test',
                    pending_email_normalized = 'fresh@example.test' WHERE id = '{dee}'"
        ))
        .await;
    assert!(
        matches!(race, Err(DbError::UniqueViolation(_))),
        "the database stays the authority when two changes race to one pending address: {race:?}"
    );
    assert_eq!(stack.email_account(dee).await.2, None);

    let absent = stack
        .start_email_change(&operator, ABSENT, "someone@example.test", 27)
        .await;
    assert_code(&absent, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    let malformed = stack
        .start_email_change(&operator, &dee.to_string(), "not an address", 28)
        .await;
    assert_code(
        &malformed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(stack.email_footprint().await, pending);
    stack.stop().await;
}

#[tokio::test]
async fn it_email_verify_rechecks_uniqueness_when_claimed_canonically_while_pending() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let bea_session = stack.signed_in("bea", 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    let token = stack
        .pending_token(&operator, &bea.to_string(), "Fresh@Example.test", 20)
        .await;

    let created = stack
        .call(
            Call::new(Method::POST, USERS, &operator).json(&json!({
                "firstName": "Eve",
                "lastName": "Early",
                "username": "eve",
                "email": "FRESH@example.test",
                "role": "user",
                "password": "a long enough temporary passphrase",
                "locale": "en-US",
            })),
            21,
        )
        .await;
    assert_eq!(
        created.status,
        StatusCode::CREATED,
        "a pending address does not reserve the address against canonical creation: {}",
        created.text()
    );

    let account = stack.email_account(bea).await;
    let rows = stack.verification_rows(bea).await;
    let sessions = stack.sessions_of(bea).await;
    let devices = stack.devices_of(bea).await;
    let audit = stack.email_footprint().await.2;

    clock.advance(Duration::from_secs(60));
    let refused = stack.verify_email(&token, 30).await;
    assert_code(&refused, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
    assert_eq!(stack.email_account(bea).await, account);
    assert_eq!(stack.verification_rows(bea).await, rows);
    assert!(
        rows[0].5.is_none(),
        "the token is not consumed by the refusal"
    );
    assert_eq!(stack.sessions_of(bea).await, sessions);
    assert_eq!(stack.devices_of(bea).await, devices);
    assert_eq!(stack.status_of_me(&bea_session, 31).await, StatusCode::OK);
    assert_eq!(stack.email_footprint().await.2, audit);
    assert!(stack
        .email_audit("USER_EMAIL_CHANGE_CONFIRMED")
        .await
        .is_empty());
    assert_eq!(
        stack.login_status("bea@example.test", 32).await,
        StatusCode::OK
    );

    let cancelled = stack
        .cancel_email_change(&operator, &bea.to_string(), 33)
        .await;
    assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_self_email_change_requires_recent_auth() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let root_id: UserId = operator_id.parse().unwrap();
    let untouched = stack.email_footprint().await;
    let before = stack.email_account(root_id).await;

    clock.advance(Duration::from_secs(31 * 60));
    let lapsed = stack
        .start_email_change(&operator, &operator_id, "root.next@example.test", 20)
        .await;
    assert_code(&lapsed, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    let resend = stack.resend_email_change(&operator, &operator_id, 21).await;
    assert_code(&resend, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    let cancel = stack.cancel_email_change(&operator, &operator_id, 22).await;
    assert_code(&cancel, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(stack.email_footprint().await, untouched);
    assert_eq!(stack.email_account(root_id).await, before);
    assert_eq!(
        stack.status_of_me(&operator, 23).await,
        StatusCode::OK,
        "the lapsed session still authenticates; only the sensitive action is refused"
    );

    let fresh = stack.signed_in("root", 24).await;
    let token = stack
        .pending_token(&fresh, &operator_id, "Root.Next@Example.test", 25)
        .await;
    let pending = stack.email_account(root_id).await;
    assert_eq!(pending.0, "root@example.test");
    assert_eq!(pending.2.as_deref(), Some("Root.Next@Example.test"));
    assert_eq!(
        stack.login_status("root@example.test", 26).await,
        StatusCode::OK
    );
    let requested = stack.email_audit("USER_EMAIL_CHANGE_REQUESTED").await;
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].3["self_change"], true);

    let verified = stack.verify_email(&token, 27).await;
    assert_eq!(
        verified.status,
        StatusCode::NO_CONTENT,
        "{}",
        verified.text()
    );
    assert_eq!(
        stack.email_account(root_id).await.0,
        "Root.Next@Example.test"
    );
    for (state, reason) in stack.session_fates(root_id).await {
        assert_eq!(
            (state.as_str(), reason.as_deref()),
            ("revoked", Some("admin_request")),
            "the actor session is revoked like every other session of the target"
        );
    }
    assert_eq!(
        stack.status_of_me(&fresh, 28).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        stack.status_of_me(&operator, 28).await,
        StatusCode::UNAUTHORIZED
    );
    let old = stack.login("root@example.test", PASSWORD, 29).await;
    assert_code(&old, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    assert_eq!(
        stack.login_status("root.next@example.test", 30).await,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_new_request_invalidates_the_previous_verification() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let bea_session = stack.signed_in("bea", 11).await;

    let first = stack
        .pending_token(&operator, &bea.to_string(), "first@example.test", 20)
        .await;
    clock.advance(Duration::from_secs(60));
    let queued = stack
        .start_email_change(&operator, &bea.to_string(), "Second@Example.test", 21)
        .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.text());

    let rows = stack.verification_rows(bea).await;
    assert_eq!(rows.len(), 2);
    let live: Vec<_> = rows
        .iter()
        .filter(|row| row.5.is_none() && row.6.is_none())
        .collect();
    assert_eq!(
        live.len(),
        1,
        "never two live rows for one user and purpose"
    );
    assert_eq!(live[0].2, "second@example.test");
    let superseded = rows
        .iter()
        .find(|row| row.2 == "first@example.test")
        .unwrap();
    assert_eq!(superseded.6.as_deref(), Some("2026-09-25T12:01:00.000Z"));
    assert!(superseded.5.is_none());
    let account = stack.email_account(bea).await;
    assert_eq!(account.0, "bea@example.test");
    assert_eq!(account.2.as_deref(), Some("Second@Example.test"));
    assert_eq!(account.3.as_deref(), Some("second@example.test"));

    let old = stack.verify_email(&first, 22).await;
    assert_code(
        &old,
        StatusCode::BAD_REQUEST,
        "EMAIL_VERIFICATION_TOKEN_INVALID",
    );
    assert_eq!(stack.email_account(bea).await, account);

    stack.deliver_mail().await;
    let tokens = stack.verification_tokens().await;
    assert_eq!(
        tokens.len(),
        2,
        "the first message was delivered before the replacement"
    );
    let second = tokens[1].clone();
    assert_ne!(first, second);
    let requested = stack.email_audit("USER_EMAIL_CHANGE_REQUESTED").await;
    assert_eq!(requested.len(), 2);
    assert_eq!(requested[1].3["replaced_pending"], true);
    assert_eq!(requested[0].3["replaced_pending"], false);
    assert_eq!(requested[1].2.as_deref(), Some(bea.to_string().as_str()));
    assert_eq!(requested[1].1.as_deref(), Some(operator_id.as_str()));

    let promoted = stack.verify_email(&second, 23).await;
    assert_eq!(
        promoted.status,
        StatusCode::NO_CONTENT,
        "{}",
        promoted.text()
    );
    assert_eq!(stack.email_account(bea).await.0, "Second@Example.test");
    assert_eq!(
        stack.status_of_me(&bea_session, 24).await,
        StatusCode::UNAUTHORIZED
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_replacement_cancels_the_undelivered_message() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    for (host, candidate) in [(20, "first@example.test"), (21, "second@example.test")] {
        let started = stack
            .start_email_change(&operator, &bea.to_string(), candidate, host)
            .await;
        assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    }
    assert_eq!(
        stack.email_outbox().await,
        [
            (
                "email_verification".to_owned(),
                "first@example.test".to_owned(),
                "canceled".to_owned(),
                false
            ),
            (
                "email_verification".to_owned(),
                "second@example.test".to_owned(),
                "pending".to_owned(),
                true
            ),
        ],
        "the superseded message is cancelled with its sealed token wiped"
    );
    stack.deliver_mail().await;
    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].message.to.email.as_str(), "second@example.test");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_resend_rotates_the_token_and_keeps_the_expiry() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let bea_session = stack.signed_in("bea", 11).await;

    let started = stack
        .start_email_change(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    let original = stack.verification_rows(bea).await;
    assert_eq!(original.len(), 1);
    let account = stack.email_account(bea).await;

    clock.advance(Duration::from_secs(5 * 60));
    let resent = stack
        .resend_email_change(&operator, &bea.to_string(), 21)
        .await;
    assert_eq!(resent.status, StatusCode::ACCEPTED, "{}", resent.text());
    assert!(resent.body.is_empty());
    let rotated = stack.verification_rows(bea).await;
    assert_eq!(rotated.len(), 1, "the live row is rotated in place");
    assert_ne!(rotated[0].0, original[0].0, "the token digest is replaced");
    assert_eq!(
        (&rotated[0].1, &rotated[0].2, &rotated[0].3, &rotated[0].4),
        (
            &original[0].1,
            &original[0].2,
            &original[0].3,
            &original[0].4
        ),
        "address, creation time and expiry are unchanged"
    );
    assert!(rotated[0].5.is_none() && rotated[0].6.is_none());
    assert_eq!(stack.email_account(bea).await, account);
    assert_eq!(
        stack.email_outbox().await,
        [
            (
                "email_verification".to_owned(),
                "Grace@NewDomain.example".to_owned(),
                "canceled".to_owned(),
                false
            ),
            (
                "email_verification".to_owned(),
                "Grace@NewDomain.example".to_owned(),
                "pending".to_owned(),
                true
            ),
        ]
    );

    stack.deliver_mail().await;
    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 1, "only the rotated message is delivered");
    assert!(
        captured[0].message.text.contains("1435"),
        "{}",
        captured[0].message.text
    );
    let second = token_from(&captured[0].message.text);
    assert_eq!(digest(&second), rotated[0].0);

    clock.advance(Duration::from_secs(5 * 60));
    let operator = stack.signed_in("root", 14).await;
    let again = stack
        .resend_email_change(&operator, &bea.to_string(), 22)
        .await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.text());
    let stale = stack.verify_email(&second, 23).await;
    assert_code(
        &stale,
        StatusCode::BAD_REQUEST,
        "EMAIL_VERIFICATION_TOKEN_INVALID",
    );
    stack.deliver_mail().await;
    let tokens = stack.verification_tokens().await;
    assert_eq!(tokens.len(), 2);
    let third = tokens[1].clone();
    assert_ne!(third, second);
    let latest = stack.verification_rows(bea).await;
    assert_eq!(
        latest[0].4, original[0].4,
        "resending never extends the lifetime"
    );
    assert_eq!(stack.status_of_me(&bea_session, 24).await, StatusCode::OK);
    assert_eq!(stack.email_account(bea).await, account);
    assert_eq!(
        stack.email_audit("USER_EMAIL_CHANGE_REQUESTED").await.len(),
        1,
        "a resend is not an audited change"
    );

    let verified = stack.verify_email(&third, 25).await;
    assert_eq!(
        verified.status,
        StatusCode::NO_CONTENT,
        "{}",
        verified.text()
    );
    assert_eq!(stack.email_account(bea).await.0, "Grace@NewDomain.example");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_resend_refusals_leave_state_untouched() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let quiet = stack.email_footprint().await;

    let nothing = stack
        .resend_email_change(&operator, &bea.to_string(), 20)
        .await;
    assert_code(
        &nothing,
        StatusCode::CONFLICT,
        "EMAIL_VERIFICATION_NOT_PENDING",
    );
    assert_eq!(stack.email_footprint().await, quiet);

    let started = stack
        .start_email_change(&operator, &bea.to_string(), "first@example.test", 21)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    let cancelled = stack
        .cancel_email_change(&operator, &bea.to_string(), 22)
        .await;
    assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
    let after_cancel = stack
        .resend_email_change(&operator, &bea.to_string(), 23)
        .await;
    assert_code(
        &after_cancel,
        StatusCode::CONFLICT,
        "EMAIL_VERIFICATION_NOT_PENDING",
    );

    let started = stack
        .start_email_change(&operator, &bea.to_string(), "second@example.test", 24)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    clock.advance(DAY + Duration::from_secs(3600));
    let operator = stack.signed_in("root", 14).await;
    let lapsed = stack
        .resend_email_change(&operator, &bea.to_string(), 25)
        .await;
    assert_code(
        &lapsed,
        StatusCode::CONFLICT,
        "EMAIL_VERIFICATION_NOT_PENDING",
    );
    let lapsed_rows = stack.verification_rows(bea).await;

    let started = stack
        .start_email_change(&operator, &bea.to_string(), "third@example.test", 26)
        .await;
    assert_eq!(
        started.status,
        StatusCode::ACCEPTED,
        "a new request replaces the lapsed verification: {}",
        started.text()
    );
    assert_eq!(
        stack.verification_rows(bea).await.len(),
        lapsed_rows.len() + 1
    );
    stack
        .setting_in("smtp", "smtp_enabled", "boolean", "false")
        .await;
    let before = (
        stack.verification_rows(bea).await,
        stack.email_outbox().await,
        stack.email_account(bea).await,
    );
    let offline = stack
        .resend_email_change(&operator, &bea.to_string(), 27)
        .await;
    assert_code(&offline, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
    assert_eq!(
        (
            stack.verification_rows(bea).await,
            stack.email_outbox().await,
            stack.email_account(bea).await,
        ),
        before
    );

    let absent = stack.resend_email_change(&operator, ABSENT, 28).await;
    assert_code(&absent, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_cancel_invalidates_the_token_and_is_idempotent() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "Bea@Example.test", &hash))
        .await;
    let bea_session = stack.signed_in("bea", 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    let token = stack
        .pending_token(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    let queued = stack
        .resend_email_change(&operator, &bea.to_string(), 21)
        .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED);
    let sessions = stack.sessions_of(bea).await;
    let devices = stack.devices_of(bea).await;
    let audit = stack.email_footprint().await.2;

    clock.advance(Duration::from_secs(60));
    let cancelled = stack
        .cancel_email_change(&operator, &bea.to_string(), 22)
        .await;
    assert_eq!(
        cancelled.status,
        StatusCode::NO_CONTENT,
        "{}",
        cancelled.text()
    );
    assert!(cancelled.body.is_empty());
    assert_eq!(
        stack.email_account(bea).await,
        (
            "Bea@Example.test".to_owned(),
            "bea@example.test".to_owned(),
            None,
            None,
            None
        )
    );
    let rows = stack.verification_rows(bea).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].6.as_deref(), Some("2026-09-25T12:01:00.000Z"));
    assert!(rows[0].5.is_none());
    let outbox = stack.email_outbox().await;
    assert_eq!(
        outbox.last().unwrap().2,
        "canceled",
        "the undelivered rotated message is cancelled"
    );
    assert!(outbox.iter().all(|(_, _, _, sealed)| !sealed));
    assert_eq!(
        stack.sessions_of(bea).await,
        sessions,
        "no session is revoked"
    );
    assert_eq!(stack.devices_of(bea).await, devices);
    assert_eq!(stack.status_of_me(&bea_session, 23).await, StatusCode::OK);
    assert_eq!(
        stack.email_footprint().await.2,
        audit,
        "cancelling is not audited"
    );

    let dead = stack.verify_email(&token, 24).await;
    assert_code(
        &dead,
        StatusCode::BAD_REQUEST,
        "EMAIL_VERIFICATION_TOKEN_INVALID",
    );
    assert_eq!(stack.email_account(bea).await.0, "Bea@Example.test");

    let touched: String = sqlx::query_scalar("SELECT updated_at FROM users WHERE id = ?1")
        .bind(bea.to_string())
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    clock.advance(Duration::from_secs(60));
    let again = stack
        .cancel_email_change(&operator, &bea.to_string(), 25)
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT, "{}", again.text());
    let untouched: String = sqlx::query_scalar("SELECT updated_at FROM users WHERE id = ?1")
        .bind(bea.to_string())
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(untouched, touched, "a repeated cancel changes nothing");
    assert_eq!(stack.verification_rows(bea).await, rows);

    let absent = stack.cancel_email_change(&operator, ABSENT, 26).await;
    assert_code(&absent, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    let malformed = stack.cancel_email_change(&operator, "not-a-uuid", 27).await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "USER_NOT_FOUND");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_smtp_unavailable_records_nothing() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let quiet = stack.email_footprint().await;
    let jobs = stack.scalar_i64("SELECT COUNT(*) FROM jobs").await;
    let before = stack.email_account(bea).await;

    let offline = stack
        .start_email_change(&operator, &bea.to_string(), "grace@newdomain.example", 20)
        .await;
    assert_code(&offline, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
    assert_eq!(stack.email_footprint().await, quiet);
    assert_eq!(stack.email_account(bea).await, before);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM jobs").await,
        jobs,
        "no delivery job is queued"
    );
    let absent = stack
        .start_email_change(&operator, ABSENT, "grace@newdomain.example", 21)
        .await;
    assert_code(&absent, StatusCode::NOT_FOUND, "USER_NOT_FOUND");

    stack.enable_smtp().await;
    let online = stack
        .start_email_change(&operator, &bea.to_string(), "grace@newdomain.example", 22)
        .await;
    assert_eq!(online.status, StatusCode::ACCEPTED, "{}", online.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_email_verify_rejects_malformed_unknown_and_expired_tokens() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let bea_session = stack.signed_in("bea", 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    let token = stack
        .pending_token(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    let unknown = Token::mint().unwrap().encode();

    for (host, presented) in [
        (30, ""),
        (31, "short"),
        (32, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"),
        (33, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        (34, unknown.expose_secret().as_str()),
    ] {
        let rejected = stack.verify_email(presented, host).await;
        assert_code(
            &rejected,
            StatusCode::BAD_REQUEST,
            "EMAIL_VERIFICATION_TOKEN_INVALID",
        );
        assert!(!rejected.text().contains(presented) || presented.is_empty());
    }
    let missing = stack.post_json(VERIFY, "{}", 35, None).await;
    assert_code(
        &missing,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );

    let account = stack.email_account(bea).await;
    let rows = stack.verification_rows(bea).await;
    let sessions = stack.sessions_of(bea).await;
    let devices = stack.devices_of(bea).await;
    let audit = stack.email_footprint().await.2;

    clock.advance(DAY);
    let expired = stack.verify_email(&token, 36).await;
    assert_code(
        &expired,
        StatusCode::GONE,
        "EMAIL_VERIFICATION_TOKEN_EXPIRED",
    );
    assert_eq!(stack.email_account(bea).await, account);
    assert_eq!(stack.verification_rows(bea).await, rows);
    assert_eq!(stack.sessions_of(bea).await, sessions);
    assert_eq!(stack.devices_of(bea).await, devices);
    assert_eq!(stack.email_footprint().await.2, audit);
    assert_eq!(stack.status_of_me(&bea_session, 37).await, StatusCode::OK);

    let operator = stack.signed_in("root", 24).await;
    let renewed = stack
        .pending_token(&operator, &bea.to_string(), "Grace@NewDomain.example", 21)
        .await;
    let promoted = stack.verify_email(&renewed, 38).await;
    assert_eq!(
        promoted.status,
        StatusCode::NO_CONTENT,
        "{}",
        promoted.text()
    );
    assert_eq!(stack.email_account(bea).await.0, "Grace@NewDomain.example");
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_email_verify_concurrent_consumption_promotes_once() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let (first, second) = stack.sign_in_fleet("bea", 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    let token = stack
        .pending_token(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;

    let results = tokio::join!(
        stack.verify_email(&token, 30),
        stack.verify_email(&token, 31),
        stack.verify_email(&token, 32),
        stack.verify_email(&token, 33),
    );
    let results = [results.0, results.1, results.2, results.3];
    let promoted = results
        .iter()
        .filter(|fetched| fetched.status == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(promoted, 1, "exactly one attempt promotes the address");
    for loser in results
        .iter()
        .filter(|fetched| fetched.status != StatusCode::NO_CONTENT)
    {
        assert_code(
            loser,
            StatusCode::BAD_REQUEST,
            "EMAIL_VERIFICATION_TOKEN_INVALID",
        );
    }
    assert_eq!(stack.email_account(bea).await.0, "Grace@NewDomain.example");
    let confirmed = stack.email_audit("USER_EMAIL_CHANGE_CONFIRMED").await;
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].3["sessions_revoked"], 2);
    assert_eq!(confirmed[0].3["trusted_devices_revoked"], 1);
    assert_eq!(
        stack.status_of_me(&first, 34).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        stack.status_of_me(&second, 34).await,
        StatusCode::UNAUTHORIZED
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_audit_failure_rolls_back_the_request() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let quiet = stack.email_footprint().await;
    let jobs = stack.scalar_i64("SELECT COUNT(*) FROM jobs").await;
    let before = stack.email_account(bea).await;

    stack.fail_audit_of("USER_EMAIL_CHANGE_REQUESTED").await;
    let failed = stack
        .start_email_change(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(stack.email_account(bea).await, before);
    assert_eq!(stack.email_footprint().await, quiet);
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM jobs").await, jobs);

    stack.restore_audit().await;
    let started = stack
        .start_email_change(&operator, &bea.to_string(), "Grace@NewDomain.example", 21)
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_email_verify_audit_failure_rolls_back_the_promotion() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let bea = stack
        .user(UserSpec::local("bea", "bea@example.test", &hash))
        .await;
    let (first, second) = stack.sign_in_fleet("bea", 11).await;
    stack.trusted_device(bea, "device-bea", None).await;
    let token = stack
        .pending_token(&operator, &bea.to_string(), "Grace@NewDomain.example", 20)
        .await;
    let account = stack.email_account(bea).await;
    let state = stack.security_state(bea).await;
    let rows = stack.verification_rows(bea).await;
    let sessions = stack.sessions_of(bea).await;
    let devices = stack.devices_of(bea).await;

    stack.fail_audit_of("USER_EMAIL_CHANGE_CONFIRMED").await;
    clock.advance(Duration::from_secs(60));
    let failed = stack.verify_email(&token, 30).await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(stack.email_account(bea).await, account);
    assert_eq!(stack.security_state(bea).await, state);
    assert_eq!(stack.verification_rows(bea).await, rows);
    assert_eq!(stack.sessions_of(bea).await, sessions);
    assert_eq!(stack.devices_of(bea).await, devices);
    assert_eq!(stack.status_of_me(&first, 31).await, StatusCode::OK);
    assert_eq!(stack.status_of_me(&second, 31).await, StatusCode::OK);
    assert!(stack
        .email_audit("USER_EMAIL_CHANGE_CONFIRMED")
        .await
        .is_empty());
    assert_eq!(
        stack.login_status("bea@example.test", 32).await,
        StatusCode::OK
    );

    stack.restore_audit().await;
    let promoted = stack.verify_email(&token, 33).await;
    assert_eq!(
        promoted.status,
        StatusCode::NO_CONTENT,
        "the unconsumed token still works once the audit write can succeed: {}",
        promoted.text()
    );
    assert_eq!(stack.email_account(bea).await.0, "Grace@NewDomain.example");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_sso_only_account_keeps_its_identity_links() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let operator = stack.operator(10).await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    stack.sso_link(sso).await;
    let links = stack.sso_link_rows(sso).await;
    assert_eq!(links.len(), 1);

    let token = stack
        .pending_token(&operator, &sso.to_string(), "SSO.Next@Example.test", 20)
        .await;
    assert_eq!(stack.sso_link_rows(sso).await, links);
    let verified = stack.verify_email(&token, 21).await;
    assert_eq!(
        verified.status,
        StatusCode::NO_CONTENT,
        "{}",
        verified.text()
    );

    let account = stack.email_account(sso).await;
    assert_eq!(account.0, "SSO.Next@Example.test");
    assert_eq!(account.2, None);
    assert_eq!(
        stack.sso_link_rows(sso).await,
        links,
        "the provider identity is neither created, synchronized nor suspended here"
    );
    let password: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
            .bind(sso.to_string())
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(password, None, "the account stays SSO-only");
    stack.stop().await;
}

#[tokio::test]
async fn it_email_change_token_is_never_stored_logged_or_returned() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
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

    let body = json!({ "email": "Grace@NewDomain.example" }).to_string();
    let spoofed = Request::builder()
        .method(Method::POST)
        .uri(format!("{USERS}/{bea}/email"))
        .header("host", "evil.example")
        .header("x-forwarded-host", "evil.example")
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, BASE_URL)
        .header(
            COOKIE,
            format!(
                "palmr_session={}; palmr_csrf={}",
                operator.session, operator.csrf
            ),
        )
        .header(CSRF_HEADER, &operator.csrf)
        .body(Body::from(body))
        .unwrap();
    let started = stack.send(with_peer(spoofed, 20)).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    assert!(!started.text().contains("token"));
    stack.deliver_mail().await;
    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 1);
    let text = captured[0].message.text.clone();
    assert!(text.contains(LINK_PREFIX), "{text}");
    assert!(!text.contains("evil.example"), "{text}");
    assert!(!captured[0].message.html.contains("evil.example"));
    let token = token_from(&text);
    let digest = digest(&token);

    let verified = stack.verify_email(&token, 21).await;
    assert_eq!(verified.status, StatusCode::NO_CONTENT);
    let unknown = Token::mint().unwrap().encode();
    let rejected = stack.verify_email(unknown.expose_secret(), 22).await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    let replay = stack.verify_email(&token, 23).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);

    let logs = capture.text();
    for secret in [
        token.as_str(),
        digest.as_str(),
        unknown.expose_secret().as_str(),
    ] {
        assert!(!logs.contains(secret), "the logs carry token material");
        for response in [&started, &verified, &rejected, &replay] {
            assert!(!response.text().contains(secret));
        }
        for (table, stored) in stack.every_stored_text().await {
            if table == "email_verifications" && stored == digest {
                continue;
            }
            assert!(
                !stored.contains(secret),
                "{table} stores token material outside the digest column"
            );
        }
    }
    for action in ["USER_EMAIL_CHANGE_REQUESTED", "USER_EMAIL_CHANGE_CONFIRMED"] {
        for row in stack.email_audit(action).await {
            let text = row.3.to_string().to_lowercase();
            let lowered = token.to_lowercase();
            for forbidden in [
                lowered.as_str(),
                digest.as_str(),
                "token",
                "cipher",
                "secret",
            ] {
                assert!(!text.contains(forbidden), "{action}: {text}");
            }
        }
    }
    stack.stop().await;
}
