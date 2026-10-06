use std::sync::{Mutex, PoisonError};

use base64ct::Encoding;

use http::header::{FORWARDED, HOST, REFERER};
use sqlx::Row;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use super::profile::{assert_code, SecurityState};
use super::*;
use crate::config::LogFormat;
use crate::features::auth::sessions::{AuthMethod, NewSession};
use crate::features::email::register_jobs as register_email_jobs;
use crate::infra::db::InstanceId;
use crate::infra::jobs::backoff::Jitter;
use crate::infra::jobs::cli::run_once;
use crate::infra::jobs::prune_tokens::{prune_step, PruneStep};
use crate::infra::jobs::runtime::{Dispatcher, RuntimeTiming};
use crate::infra::jobs::{Claimant, JobAudit, JobKind, Registry};
use crate::infra::telemetry::build_dispatch;

const FORGOT: &str = "/api/v1/auth/password/forgot";
const CHECK: &str = "/api/v1/auth/password/reset/check";
const RESET: &str = "/api/v1/auth/password/reset";
const LINK_PREFIX: &str = "https://files.example.test/reset-password/";
const REPLACEMENT: &str = "a brand new passphrase";
const TRUST_PROXY: (&str, &str) = ("PALMR_TRUST_PROXY", "198.51.100.0/24");
const HOSTILE: &str = "evil.example";

type OutboxToken = (String, Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>);
type ResetRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl Stack {
    pub(super) async fn enable_smtp(&self) {
        self.setting_in("smtp", "smtp_enabled", "boolean", "true")
            .await;
        self.setting_in("smtp", "smtp_host", "string", "\"smtp.example.test\"")
            .await;
        self.setting_in(
            "smtp",
            "smtp_from_email",
            "string",
            "\"palmr@example.test\"",
        )
        .await;
        self.setting_in("smtp", "smtp_no_auth", "boolean", "true")
            .await;
    }

    async fn forgot(&self, identifier: &str, host: u8) -> Fetched {
        let body = json!({ "identifier": identifier });
        self.post_json(FORGOT, &body.to_string(), host, None).await
    }

    async fn reset_check(&self, token: &str, host: u8) -> Fetched {
        let body = json!({ "token": token });
        self.post_json(CHECK, &body.to_string(), host, None).await
    }

    async fn reset_password(&self, token: &str, password: &str, host: u8) -> Fetched {
        let body = json!({ "token": token, "newPassword": password });
        self.post_json(RESET, &body.to_string(), host, None).await
    }

    pub(super) async fn deliver_mail(&self) {
        let registry = register_email_jobs(Registry::production(), self.email.clone());
        let dispatcher = Dispatcher::new(
            self.pools.clone(),
            Arc::new(self.clock.clone()),
            registry,
            Jitter::from_fn(|| 0),
            JobAudit::detached(),
            RuntimeTiming::DEFAULT.lease_renewal,
        );
        let claimant = Claimant::worker(InstanceId::generate(&self.clock), 0);
        run_once(&dispatcher, &claimant, JobKind::EmailSend)
            .await
            .unwrap();
    }

    fn mailed_tokens(&self) -> Vec<String> {
        self.mail
            .captured()
            .iter()
            .map(|captured| mailed_token(&captured.message.text))
            .collect()
    }

    async fn issued_token(&self, identifier: &str, host: u8) -> String {
        let fetched = self.forgot(identifier, host).await;
        assert_eq!(fetched.status, StatusCode::ACCEPTED, "{}", fetched.text());
        self.deliver_mail().await;
        self.mailed_tokens().pop().unwrap()
    }

    async fn reset_rows(&self, user: UserId) -> Vec<ResetRow> {
        sqlx::query_as(
            "SELECT token_hash, expires_at, used_at, invalidated_at, requested_ip
               FROM password_reset_tokens WHERE user_id = ?1 ORDER BY id",
        )
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn totp_user(&self, username: &str, email: &str, hash: &Secret<String>) -> UserId {
        let user = self.user(UserSpec::local(username, email, hash)).await;
        let signed_in = self.signed_in(username, 90).await;
        self.tf_enable(&signed_in, 90).await;
        user
    }

    async fn lock(&self, user: UserId) {
        self.execute(&format!(
            "INSERT INTO account_lockouts (user_id, failed_count, first_failed_at, last_failed_at,
                                           locked_until, lock_count, updated_at)
             VALUES ('{user}', 5, '2026-09-25T11:59:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:10:00.000Z', 1, '2026-09-25T12:00:00.000Z')"
        ))
        .await;
    }

    pub(super) async fn every_stored_text(&self) -> Vec<(String, String)> {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap();
        let mut found = Vec::new();
        for table in tables {
            let rows = sqlx::query(&format!("SELECT * FROM \"{table}\""))
                .fetch_all(self.pools.reader().executor())
                .await
                .unwrap();
            for row in rows {
                for index in 0..row.len() {
                    if let Ok(Some(text)) = row.try_get::<Option<String>, _>(index) {
                        found.push((table.clone(), text));
                    } else if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(index) {
                        found.push((table.clone(), String::from_utf8_lossy(&bytes).into_owned()));
                    }
                }
            }
        }
        found
    }
}

fn mailed_token(text: &str) -> String {
    let start = text.find(LINK_PREFIX).unwrap() + LINK_PREFIX.len();
    text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn sha256(token: &str) -> String {
    digest(token)
}

#[derive(Clone, Default)]
pub(super) struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    pub(super) fn text(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap()
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn it_forgot_non_enumerating() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
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
    stack.lock(locked).await;
    let second = stack
        .totp_user("second", "second@example.test", &hash)
        .await;
    let remembered = stack
        .user(UserSpec::local("kept", "kept@example.test", &hash))
        .await;
    stack
        .trusted_device(remembered, "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b70", None)
        .await;
    let emails_before = stack.scalar_i64("SELECT COUNT(*) FROM email_outbox").await;

    let cases = [
        ("unknown identifier", "ghost@example.test"),
        ("active local account by e-mail", "ADA@example.test"),
        ("inactive account", "inert"),
        ("SSO-only account", "sso@example.test"),
        ("locked account", "locked"),
        ("account with TOTP", "second"),
        ("account with trusted devices", "kept@example.test"),
    ];
    let mut observed = Vec::new();
    for (index, (case, identifier)) in cases.iter().enumerate() {
        let fetched = stack
            .forgot(identifier, 10 + u8::try_from(index).unwrap())
            .await;
        assert_eq!(fetched.status, StatusCode::ACCEPTED, "{case}");
        assert_eq!(fetched.json(), json!({ "accepted": true }), "{case}");
        assert!(fetched.set_cookies().is_empty(), "{case}");
        assert_eq!(
            fetched.headers.get("cache-control").unwrap(),
            "no-store",
            "{case}"
        );
        let mut headers: Vec<String> = fetched
            .headers
            .keys()
            .map(|name| name.as_str().to_owned())
            .filter(|name| name != "x-request-id")
            .collect();
        headers.sort();
        observed.push((fetched.status, fetched.body.clone(), headers));
    }
    assert!(
        observed.windows(2).all(|pair| pair[0] == pair[1]),
        "{observed:?}"
    );

    for (user, issued) in [
        (ada, 1),
        (inert, 0),
        (sso, 0),
        (locked, 1),
        (second, 1),
        (remembered, 1),
    ] {
        assert_eq!(stack.reset_rows(user).await.len(), issued, "{user}");
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
            .await,
        4
    );
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM email_outbox").await - emails_before,
        4
    );
    assert_eq!(
        stack.lock_row(locked).await.1.as_deref(),
        Some("2026-09-25T12:10:00.000Z")
    );
    assert_eq!(stack.devices_of(remembered).await[0].1, None);
    assert_eq!(stack.mail.count(), 0, "no e-mail is sent from the request");
    stack.stop().await;
}

#[tokio::test]
async fn it_forgot_smtp_unavailable_is_instance_wide() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let known = stack.forgot("ada", 10).await;
    let unknown = stack.forgot("ghost", 11).await;
    for fetched in [&known, &unknown] {
        assert_code(fetched, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
    }
    assert_eq!(
        known.error_without_request_id(),
        unknown.error_without_request_id()
    );
    stack
        .setting_in("smtp", "smtp_enabled", "boolean", "true")
        .await;
    assert_code(
        &stack.forgot("ada", 12).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    assert!(stack.reset_rows(ada).await.is_empty());
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM email_outbox").await,
        0
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'email.send'")
            .await,
        0
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_forgot_request_contract_has_no_url_field() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    for (index, field) in [
        "origin",
        "host",
        "baseUrl",
        "redirectUrl",
        "returnTo",
        "callbackUrl",
        "resetUrl",
    ]
    .iter()
    .enumerate()
    {
        let mut body = json!({ "identifier": "ada" });
        body[*field] = json!("https://evil.example/steal");
        let fetched = stack
            .post_json(
                FORGOT,
                &body.to_string(),
                20 + u8::try_from(index).unwrap(),
                None,
            )
            .await;
        assert_code(
            &fetched,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            fetched.json()["error"]["details"]["fields"],
            json!(["body"])
        );
    }
    for (index, body) in [
        json!({}),
        json!({ "identifier": "" }),
        json!({ "identifier": 7 }),
    ]
    .iter()
    .enumerate()
    {
        let fetched = stack
            .post_json(
                FORGOT,
                &body.to_string(),
                30 + u8::try_from(index).unwrap(),
                None,
            )
            .await;
        assert_code(
            &fetched,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            fetched.json()["error"]["details"]["fields"],
            json!(["identifier"])
        );
    }
    assert!(stack.reset_rows(ada).await.is_empty());

    let inventory = application_routes().build().unwrap().inventory;
    let schema = ApiDocs::new(
        application_routes().build().unwrap().openapi,
        &OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_BASE_URL",
            BASE_URL,
        )]))
        .unwrap()
        .config
        .base_url,
    )
    .unwrap();
    let document: Value = serde_json::from_slice(schema.document()).unwrap();
    let properties = &document["components"]["schemas"]["ForgotPasswordRequest"]["properties"];
    assert_eq!(
        properties.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["identifier"]
    );
    for (path, class) in [
        (FORGOT, RateLimitClass::AuthReset),
        (CHECK, RateLimitClass::AuthToken),
        (RESET, RateLimitClass::AuthToken),
    ] {
        let policy = inventory.get(&Method::POST, path).unwrap().policy();
        assert_eq!(policy.auth(), AuthClass::Public, "{path}");
        assert_eq!(policy.rate_limit(), class, "{path}");
        assert!(inventory.get(&Method::GET, path).is_none(), "{path}");
    }
    stack.stop().await;
}

async fn hostile_forgot(stack: &Stack, host: u8) -> Fetched {
    let request = Request::builder()
        .method(Method::POST)
        .uri(FORGOT)
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, BASE_URL)
        .header(HOST, HOSTILE)
        .header("x-forwarded-host", HOSTILE)
        .header("x-forwarded-proto", "http")
        .header(
            FORWARDED,
            format!("for=203.0.113.9;host={HOSTILE};proto=http"),
        )
        .header(REFERER, format!("https://{HOSTILE}/login"))
        .body(Body::from(json!({ "identifier": "ada" }).to_string()))
        .unwrap();
    stack.send(with_peer(request, host)).await
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-037")]
#[tokio::test]
async fn regression_R037_password_reset_link_host_from_base_url() {
    for env in [&[][..], &[TRUST_PROXY][..]] {
        let root = TempDir::new().unwrap();
        let stack =
            Stack::start_configured(root.path(), &TestClock::new(START), Routes::new(), env).await;
        stack.enable_smtp().await;
        let hash = password_hash();
        let ada = stack
            .user(UserSpec::local("ada", "ada@example.test", &hash))
            .await;

        let accepted = hostile_forgot(&stack, 10).await;
        assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text());

        let foreign_origin = Request::builder()
            .method(Method::POST)
            .uri(FORGOT)
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, format!("https://{HOSTILE}"))
            .body(Body::from(json!({ "identifier": "ada" }).to_string()))
            .unwrap();
        let rejected = stack.send(with_peer(foreign_origin, 11)).await;
        assert_code(&rejected, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED");
        assert_eq!(stack.reset_rows(ada).await.len(), 1);

        stack.deliver_mail().await;
        let captured = stack.mail.captured();
        assert_eq!(captured.len(), 1);
        let message = &captured[0].message;
        let token = mailed_token(&message.text);
        assert_eq!(token.len(), 43);
        let link = format!("{LINK_PREFIX}{token}");
        assert!(message.text.contains(&link), "{}", message.text);
        let html = message.html.replace("&#x2f;", "/").replace("&#47;", "/");
        assert!(html.contains(&link), "{html}");
        assert!(!message.text.contains("?token="));
        for text in [&message.text, &message.html, &message.subject] {
            assert!(!text.contains(HOSTILE), "{text}");
            assert!(!text.contains("http://"), "{text}");
        }
        for line in message.text.lines().filter(|line| line.contains("://")) {
            assert!(line.contains(LINK_PREFIX), "{line}");
        }
        assert_eq!(stack.reset_rows(ada).await[0].0, sha256(&token));
        stack.stop().await;
    }
}

#[tokio::test]
async fn it_reset_email_uses_account_locale_and_configured_expiry() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    stack
        .setting("password_reset_validity_minutes", "integer", "30")
        .await;
    let hash = password_hash();
    stack
        .user(UserSpec {
            locale: Some(LocaleCode::PtBr),
            ..UserSpec::local("ada", "ada@example.test", &hash)
        })
        .await;
    stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;

    let token = stack.issued_token("ada", 10).await;
    let captured = stack.mail.captured();
    let message = &captured[0].message;
    assert!(
        message.text.contains("Este link expira em 30 minutos."),
        "{}",
        message.text
    );
    assert!(
        !message.text.contains("This link expires"),
        "{}",
        message.text
    );
    assert!(message.html.contains("lang=\"pt-BR\""), "{}", message.html);
    assert!(!message.text.is_empty() && !message.html.is_empty());

    let checked = stack.reset_check(&token, 11).await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text());
    assert_eq!(checked.json()["expiresAt"], "2026-09-25T12:30:00.000Z");

    stack.issued_token("bob", 12).await;
    let english = &stack.mail.captured()[1].message;
    assert!(
        english.text.contains("This link expires in 30 minutes."),
        "{}",
        english.text
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_token_single_use_and_hashed() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
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

    let accepted = stack.forgot("ada", 10).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    let (ciphertext, params, state): (Option<Vec<u8>>, String, String) = sqlx::query_as(
        "SELECT token_ciphertext, params_json, state FROM email_outbox WHERE kind = 'password_reset'",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(state, "pending");
    let sealed = ciphertext.unwrap();
    assert!(!params.contains("token"), "{params}");

    stack.deliver_mail().await;
    let token = stack.mailed_tokens().pop().unwrap();
    assert_eq!(token.len(), 43);
    assert_eq!(
        base64ct::Base64UrlUnpadded::decode_vec(&token)
            .unwrap()
            .len()
            * 8,
        256
    );
    assert!(!String::from_utf8_lossy(&sealed).contains(&token));
    let rows = stack.reset_rows(ada).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, sha256(&token));
    assert_eq!(rows[0].1, "2026-09-25T13:00:00.000Z");
    assert_eq!(rows[0].4.as_deref(), Some("198.51.100.10"));
    let wiped: OutboxToken = sqlx::query_as(
        "SELECT state, token_ciphertext, token_nonce, key_version FROM email_outbox
          WHERE kind = 'password_reset'",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(wiped, ("sent".to_owned(), None, None, None));

    let reset = stack.reset_password(&token, REPLACEMENT, 11).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.text());
    assert!(reset.body.is_empty());
    assert!(reset.set_cookies().is_empty());
    assert_code(
        &stack.reset_password(&token, "another passphrase", 12).await,
        StatusCode::GONE,
        "RESET_TOKEN_USED",
    );
    assert_code(
        &stack.reset_check(&token, 13).await,
        StatusCode::GONE,
        "RESET_TOKEN_USED",
    );
    assert_eq!(
        stack.login("ada", REPLACEMENT, 14).await.status,
        StatusCode::OK
    );
    assert_eq!(
        stack.login("ada", PASSWORD, 15).await.status,
        StatusCode::UNAUTHORIZED
    );

    let second = stack.issued_token("ada", 16).await;
    let (first, other) = tokio::join!(
        stack.reset_password(&second, "alpha passphrase one", 17),
        stack.reset_password(&second, "bravo passphrase two", 18),
    );
    let mut statuses = [first.status, other.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::NO_CONTENT, StatusCode::GONE]);
    let (winner, loser) = if first.status == StatusCode::NO_CONTENT {
        ("alpha passphrase one", &other)
    } else {
        ("bravo passphrase two", &first)
    };
    assert_eq!(loser.error_code(), "RESET_TOKEN_USED");
    assert_eq!(stack.login("ada", winner, 19).await.status, StatusCode::OK);
    assert_eq!(stack.audit_rows("PASSWORD_RESET_COMPLETED").await.len(), 2);

    let logs = capture.text();
    assert!(
        logs.contains("password_reset"),
        "the log capture recorded the reset flow"
    );
    let stored = stack.every_stored_text().await;
    for raw in [&token, &second] {
        assert!(
            !logs.contains(raw.as_str()),
            "a reset token reached the logs"
        );
        assert!(
            !logs.contains(&sha256(raw)),
            "a reset token digest reached the logs"
        );
        assert!(!logs.contains(&format!("{LINK_PREFIX}{raw}")));
        for (table, text) in &stored {
            assert!(
                !text.contains(raw.as_str()),
                "a raw reset token is stored in {table}"
            );
        }
    }
    for (_, _, metadata) in stack.audit_rows("PASSWORD_RESET_COMPLETED").await {
        assert!(
            !metadata.contains("token") && !metadata.contains("password_hash"),
            "{metadata}"
        );
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_is_refused_while_password_login_is_disabled() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let token = stack.issued_token("ada", 10).await;
    let before = stack.security_state(ada).await;
    let rows = stack.reset_rows(ada).await;
    assert_eq!(rows.len(), 1);

    stack
        .execute(
            "UPDATE app_settings SET value_json = 'false' WHERE key = 'password_login_enabled'",
        )
        .await;
    stack.settings.reload().await.unwrap();

    let refused = stack.reset_password(&token, REPLACEMENT, 11).await;
    assert_code(
        &refused,
        StatusCode::FORBIDDEN,
        "AUTH_PASSWORD_LOGIN_DISABLED",
    );
    assert_eq!(stack.security_state(ada).await, before);
    assert_eq!(
        stack.reset_rows(ada).await,
        rows,
        "the token is not consumed"
    );
    assert_eq!(
        stack
            .scalar_i64(
                "SELECT COUNT(*) FROM audit_events WHERE action = 'PASSWORD_RESET_COMPLETED'"
            )
            .await,
        0
    );

    let mail_before = stack.mail.captured().len();
    let silent = stack.forgot("ada", 12).await;
    assert_eq!(silent.status, StatusCode::ACCEPTED);
    stack.deliver_mail().await;
    assert_eq!(stack.mail.captured().len(), mail_before);
    assert_eq!(stack.reset_rows(ada).await, rows);

    stack
        .execute("UPDATE app_settings SET value_json = 'true' WHERE key = 'password_login_enabled'")
        .await;
    stack.settings.reload().await.unwrap();
    let completed = stack.reset_password(&token, REPLACEMENT, 13).await;
    assert_eq!(
        completed.status,
        StatusCode::NO_CONTENT,
        "{}",
        completed.text()
    );
    assert_ne!(
        stack.security_state(ada).await.password_hash,
        before.password_hash
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_invalidates_older_tokens() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let older = stack.issued_token("ada", 10).await;
    assert_eq!(stack.reset_check(&older, 11).await.status, StatusCode::OK);
    let newer = stack.issued_token("ada@example.test", 12).await;
    assert_ne!(older, newer);

    let rows = stack.reset_rows(ada).await;
    assert_eq!(rows.len(), 2);
    assert!(rows[0].3.is_some(), "the older token is marked invalidated");
    assert!(rows[1].2.is_none() && rows[1].3.is_none());

    assert_code(
        &stack.reset_check(&older, 13).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );
    assert_code(
        &stack.reset_password(&older, REPLACEMENT, 14).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );
    assert_eq!(
        stack.security_state(ada).await.password_hash.as_deref(),
        Some(hash.expose_secret().as_str())
    );
    assert_eq!(stack.reset_check(&newer, 15).await.status, StatusCode::OK);
    let reset = stack.reset_password(&newer, REPLACEMENT, 16).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.text());
    assert_code(
        &stack.reset_check(&older, 17).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );

    let latest = stack.issued_token("ada", 18).await;
    stack
        .execute(
            "CREATE TRIGGER test_refuse_outbox BEFORE INSERT ON email_outbox
             BEGIN SELECT RAISE(ABORT, 'refused by test'); END",
        )
        .await;
    stack.clock.advance(Duration::from_secs(20 * 60));
    let failed = stack.forgot("ada", 19).await;
    assert!(failed.status.is_server_error(), "{}", failed.text());
    assert_eq!(stack.reset_rows(ada).await.len(), 3);
    assert_eq!(stack.reset_check(&latest, 20).await.status, StatusCode::OK);
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_check_reports_token_state_only() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    stack.setting("password_min_length", "integer", "12").await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let token = stack.issued_token("ada", 10).await;

    let checked = stack.reset_check(&token, 11).await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text());
    assert_eq!(checked.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(
        checked.json(),
        json!({
            "valid": true,
            "passwordMinLength": 12,
            "expiresAt": "2026-09-25T13:00:00.000Z",
        })
    );
    let text = checked.text();
    for private in [
        "ada",
        "example.test",
        "Lovelace",
        "user",
        "role",
        "locale",
        "totp",
    ] {
        assert!(!text.contains(private), "{private} leaked: {text}");
    }

    for (index, malformed) in ["", "short", &"A".repeat(44), &format!("{}=", &token[..42])]
        .iter()
        .enumerate()
    {
        assert_code(
            &stack
                .reset_check(malformed, 20 + u8::try_from(index).unwrap())
                .await,
            StatusCode::BAD_REQUEST,
            "RESET_TOKEN_INVALID",
        );
    }
    let unknown = Token::mint().unwrap().encode();
    let unknown = stack.reset_check(unknown.expose_secret(), 30).await;
    assert_code(&unknown, StatusCode::BAD_REQUEST, "RESET_TOKEN_INVALID");
    assert_eq!(unknown.json()["error"]["details"], json!({}));

    let extra = stack
        .post_json(
            CHECK,
            &json!({ "token": token, "email": "ada@example.test" }).to_string(),
            31,
            None,
        )
        .await;
    assert_code(&extra, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let by_query = stack.get(&format!("{CHECK}?token={token}"), None, 32).await;
    assert_eq!(by_query.status, StatusCode::METHOD_NOT_ALLOWED);

    clock.advance(Duration::from_secs(60 * 60));
    let expired = stack.reset_check(&token, 33).await;
    assert_code(&expired, StatusCode::GONE, "RESET_TOKEN_EXPIRED");
    assert_code(
        &stack.reset_password(&token, REPLACEMENT, 34).await,
        StatusCode::GONE,
        "RESET_TOKEN_EXPIRED",
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_security_transition() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack.totp_user("ada", "ada@example.test", &hash).await;
    let bystander = stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let bystander_session = stack.signed_in("bob", 40).await;
    for (id, revoked) in [
        ("01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b71", None),
        (
            "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b72",
            Some("2026-09-25T11:00:00.000Z"),
        ),
    ] {
        stack.trusted_device(ada, id, revoked).await;
    }
    stack
        .trusted_device(bystander, "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b73", None)
        .await;
    let extra = stack
        .sessions
        .mint(NewSession {
            user_id: ada,
            auth_method: AuthMethod::PasswordTotp,
            ip_address: None,
            user_agent: None,
        })
        .await
        .unwrap();
    stack
        .execute(&format!(
            "UPDATE users SET must_change_password = 1 WHERE id = '{ada}'"
        ))
        .await;
    stack.lock(ada).await;
    let secret_before = stack.tf_secret_row(ada).await.unwrap();
    let backups_before = stack.tf_backup_rows(ada).await;
    assert!(!backups_before.is_empty());
    let token = stack.issued_token("ada", 41).await;

    assert_code(
        &stack.reset_password(&token, "short", 42).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    let violation = stack.reset_password(&token, "short", 43).await;
    assert_eq!(
        violation.json()["error"]["details"],
        json!({ "minLength": 8 })
    );

    let state_before = stack.security_state(ada).await;
    let sessions_before = stack.sessions_of(ada).await;
    stack
        .execute(
            "CREATE TRIGGER test_refuse_reset_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'PASSWORD_RESET_COMPLETED'
             BEGIN SELECT RAISE(ABORT, 'refused by test'); END",
        )
        .await;
    let failed = stack.reset_password(&token, REPLACEMENT, 44).await;
    assert!(failed.status.is_server_error(), "{}", failed.text());
    let state_after: SecurityState = stack.security_state(ada).await;
    assert_eq!(state_after.password_hash, state_before.password_hash);
    assert!(state_after.must_change_password);
    assert_eq!(
        stack
            .sessions_of(ada)
            .await
            .iter()
            .map(|row| row.state.clone())
            .collect::<Vec<_>>(),
        sessions_before
            .iter()
            .map(|row| row.state.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(stack.devices_of(ada).await[0].1, None);
    assert_eq!(stack.lock_row(ada).await.0, 5);
    assert!(stack.reset_rows(ada).await[0].2.is_none());
    assert_eq!(stack.reset_check(&token, 45).await.status, StatusCode::OK);
    stack.execute("DROP TRIGGER test_refuse_reset_audit").await;

    let reset = stack.reset_password(&token, REPLACEMENT, 46).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.text());

    let state = stack.security_state(ada).await;
    assert!(!state.must_change_password);
    assert_ne!(state.password_hash, state_before.password_hash);
    assert!(state
        .password_hash
        .as_deref()
        .unwrap()
        .starts_with("$argon2id$"));
    assert!(state.password_updated_at.is_some());
    let sessions = stack.sessions_of(ada).await;
    assert!(!sessions.is_empty());
    for row in &sessions {
        assert_eq!(row.state, "revoked", "{}", row.id);
    }
    assert!(sessions
        .iter()
        .any(|row| row.revoked_reason.as_deref() == Some("password_reset")));
    assert_eq!(
        session_state(&stack, extra.session_token.expose_secret()).await,
        ("revoked".to_owned(), Some("password_reset".to_owned()))
    );
    assert!(stack
        .devices_of(ada)
        .await
        .iter()
        .all(|(_, revoked)| revoked.is_some()));
    assert_eq!(stack.devices_of(bystander).await[0].1, None);
    assert_eq!(
        stack
            .get(ME, Some(&bystander_session.session), 47)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(stack.lock_row(ada).await.0, 0);
    assert_eq!(stack.lock_row(ada).await.1, None);
    assert_eq!(stack.tf_secret_row(ada).await.unwrap(), secret_before);
    assert_eq!(stack.tf_backup_rows(ada).await, backups_before);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT totp_enabled FROM users WHERE id = '{ada}'"
            ))
            .await,
        1
    );
    assert_eq!(
        stack
            .get(ME, Some(extra.session_token.expose_secret()), 48)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    let login = stack.login("ada", REPLACEMENT, 49).await;
    assert_code(&login, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    assert!(login.set_cookies().is_empty());

    let audit = stack.audit_rows("PASSWORD_RESET_COMPLETED").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0.as_str(), "ada");
    assert_eq!(audit[0].1.as_deref(), Some(ada.to_string().as_str()));
    let metadata: Value = serde_json::from_str(&audit[0].2).unwrap();
    assert_eq!(metadata["lockout_cleared"], true);
    assert_eq!(metadata["trusted_devices_revoked"], 1);
    assert!(metadata["sessions_revoked"].as_u64().unwrap() >= 2);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM sessions WHERE state = 'active'")
            .await,
        1,
        "only the bystander stays signed in and no session is created by the reset"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_forced_and_mandatory_2fa_states() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("ada", "ada@example.test", &hash)
        })
        .await;
    let token = stack.issued_token("ada", 10).await;
    assert_eq!(
        stack.reset_password(&token, REPLACEMENT, 11).await.status,
        StatusCode::NO_CONTENT
    );
    let login = stack.login("ada", REPLACEMENT, 12).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    assert_eq!(login.json()["mustChangePassword"], false);
    assert_eq!(login.json()["mfaEnrollmentRequired"], false);

    stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let token = stack.issued_token("ada", 13).await;
    assert_eq!(
        stack
            .reset_password(&token, "yet another passphrase", 14)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let login = stack.login("ada", "yet another passphrase", 15).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    assert_eq!(login.json()["mustChangePassword"], false);
    assert_eq!(login.json()["mfaEnrollmentRequired"], true);
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_refuses_accounts_that_lost_eligibility() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let token = stack.issued_token("ada", 10).await;
    stack
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z'
              WHERE id = '{ada}'"
        ))
        .await;
    assert_code(
        &stack.reset_check(&token, 11).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );
    assert_code(
        &stack.reset_password(&token, REPLACEMENT, 12).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );
    let state = stack.security_state(ada).await;
    assert_eq!(
        state.password_hash.as_deref(),
        Some(hash.expose_secret().as_str())
    );
    assert!(stack.reset_rows(ada).await[0].2.is_none());
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_rate_limits() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    let mut limited = Vec::new();
    for (host, targets) in [
        (50, ["one", "two", "three", "ada"]),
        (51, ["four", "five", "six", "ghost"]),
    ] {
        for identifier in &targets[..3] {
            assert_eq!(
                stack.forgot(identifier, host).await.status,
                StatusCode::ACCEPTED
            );
        }
        let by_ip = stack.forgot(targets[3], host).await;
        assert_code(&by_ip, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
        assert_eq!(
            by_ip.json()["error"]["details"],
            json!({ "scope": "rl.auth.reset" })
        );
        assert!(by_ip.retry_after().is_some());
        limited.push(by_ip);
    }
    assert_eq!(
        limited[0].error_without_request_id(),
        limited[1].error_without_request_id(),
        "the address limit answers the same for a known and an unknown target"
    );
    assert_eq!(limited[0].retry_after(), limited[1].retry_after());
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
            .await,
        0,
        "a request refused by the address limit never reaches issuance"
    );
    assert_eq!(stack.forgot("ada", 52).await.status, StatusCode::ACCEPTED);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
            .await,
        1
    );

    let before = stack
        .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
        .await;
    for _ in 0..10 {
        let unknown = Token::mint().unwrap().encode();
        assert_code(
            &stack.reset_check(unknown.expose_secret(), 80).await,
            StatusCode::BAD_REQUEST,
            "RESET_TOKEN_INVALID",
        );
        let reset = stack
            .reset_password(unknown.expose_secret(), REPLACEMENT, 80)
            .await;
        assert_code(&reset, StatusCode::BAD_REQUEST, "RESET_TOKEN_INVALID");
    }
    let throttled = stack.reset_password("x", REPLACEMENT, 80).await;
    assert_code(&throttled, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(
        throttled.json()["error"]["details"]["scope"],
        "rl.auth.token"
    );
    let throttled = stack.reset_check("x", 80).await;
    assert_eq!(
        throttled.json()["error"]["details"]["scope"],
        "rl.auth.token"
    );
    assert_eq!(
        stack.reset_check("x", 81).await.error_code(),
        "RESET_TOKEN_INVALID",
        "the token bucket is keyed by client address"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
            .await,
        before
    );
    stack.stop().await;
}

type Shape = (StatusCode, Bytes, Vec<(String, String)>);

fn shape(fetched: &Fetched) -> Shape {
    let mut headers: Vec<(String, String)> = fetched
        .headers
        .iter()
        .filter(|(name, _)| name.as_str() != "x-request-id")
        .map(|(name, value)| {
            let value = value.to_str().unwrap();
            let value = if name.as_str() == "content-security-policy" {
                value
                    .split(' ')
                    .map(|part| {
                        if part.starts_with("'nonce-") {
                            "'nonce'"
                        } else {
                            part
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                value.to_owned()
            };
            (name.as_str().to_owned(), value)
        })
        .collect();
    headers.sort();
    (fetched.status, fetched.body.clone(), headers)
}

impl Stack {
    async fn reset_mail_count(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM email_outbox WHERE kind = 'password_reset'")
            .await
    }

    async fn ada_and_bob(&self) -> (UserId, UserId) {
        let hash = password_hash();
        let ada = self
            .user(UserSpec::local("ada", "ada@example.com", &hash))
            .await;
        let bob = self
            .user(UserSpec::local("bob", "bob@example.com", &hash))
            .await;
        (ada, bob)
    }

    async fn exhaust_ada(&self) -> Vec<Fetched> {
        let mut accepted = Vec::new();
        for (identifier, host) in [
            ("ada@example.com", 101),
            ("ada", 102),
            ("ADA@EXAMPLE.COM", 103),
        ] {
            let fetched = self.forgot(identifier, host).await;
            assert_eq!(fetched.status, StatusCode::ACCEPTED, "{identifier}");
            self.deliver_mail().await;
            accepted.push(fetched);
        }
        accepted
    }
}

#[tokio::test]
async fn it_forgot_account_cap_is_silent_and_shared_by_aliases() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let (ada, bob) = stack.ada_and_bob().await;

    let accepted = stack.exhaust_ada().await;
    assert_eq!(stack.reset_rows(ada).await.len(), 3);
    assert_eq!(stack.mail.count(), 3);
    let latest = stack.mailed_tokens().pop().unwrap();
    let latest_row = stack.reset_rows(ada).await.pop().unwrap();
    assert_eq!(latest_row.0, sha256(&latest));
    assert!(latest_row.2.is_none() && latest_row.3.is_none());
    let mails = stack.reset_mail_count().await;
    let outbox_before: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, state FROM email_outbox WHERE kind = 'password_reset' ORDER BY id",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    let jobs = stack
        .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'email.send'")
        .await;

    for (identifier, host) in [("ada", 104), ("ada@example.com", 105)] {
        let suppressed = stack.forgot(identifier, host).await;
        assert_eq!(shape(&suppressed), shape(&accepted[0]), "{identifier}");
        assert_eq!(suppressed.json(), json!({ "accepted": true }));
    }
    stack.deliver_mail().await;
    assert_eq!(stack.reset_rows(ada).await.len(), 3, "no fourth token");
    assert_eq!(
        stack.reset_mail_count().await,
        mails,
        "no fourth reset e-mail"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'email.send'")
            .await,
        jobs
    );
    assert_eq!(stack.mail.count(), 3);
    let outbox_after: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, state FROM email_outbox WHERE kind = 'password_reset' ORDER BY id",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(outbox_after, outbox_before);

    assert_eq!(stack.reset_rows(ada).await.pop().unwrap(), latest_row);
    let checked = stack.reset_check(&latest, 106).await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text());
    assert_eq!(checked.json()["expiresAt"], latest_row.1.as_str());

    let bob_request = stack.forgot("bob@example.com", 107).await;
    assert_eq!(shape(&bob_request), shape(&accepted[0]));
    assert_eq!(stack.reset_rows(bob).await.len(), 1);
    assert_eq!(stack.forgot("bob", 108).await.status, StatusCode::ACCEPTED);
    assert_eq!(stack.reset_rows(bob).await.len(), 2);

    let reset = stack.reset_password(&latest, REPLACEMENT, 109).await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_forgot_exhausted_alias_indistinguishable_from_unknown() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    stack.enable_smtp().await;
    let (ada, _) = stack.ada_and_bob().await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.com", &hash)
        })
        .await;
    let inert = stack
        .user(UserSpec {
            active: false,
            ..UserSpec::local("inert", "inert@example.com", &hash)
        })
        .await;
    stack.exhaust_ada().await;

    let alias = stack.forgot("ada", 120).await;
    let unknown = stack.forgot("stranger@example.com", 121).await;
    let reference = shape(&unknown);
    assert_eq!(shape(&alias), reference);
    for (identifier, host) in [
        ("sso", 122),
        ("inert@example.com", 123),
        ("bob", 124),
        ("sso@example.com", 125),
        ("SSO@EXAMPLE.COM", 126),
        ("sso", 127),
    ] {
        assert_eq!(
            shape(&stack.forgot(identifier, host).await),
            reference,
            "{identifier}"
        );
    }
    for fetched in [&alias, &unknown] {
        assert_eq!(fetched.status, StatusCode::ACCEPTED);
        assert_eq!(fetched.json(), json!({ "accepted": true }));
        assert!(fetched.retry_after().is_none());
        assert!(!fetched.text().contains(&ada.to_string()));
    }
    assert_eq!(stack.reset_rows(ada).await.len(), 3);
    assert!(stack.reset_rows(sso).await.is_empty());
    assert!(stack.reset_rows(inert).await.is_empty());
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM password_reset_tokens")
            .await,
        4
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reset_tokens_pruned_after_grace() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let used = stack.issued_token("ada", 10).await;
    assert_eq!(
        stack.reset_password(&used, REPLACEMENT, 11).await.status,
        StatusCode::NO_CONTENT
    );
    let expiring = stack.issued_token("ada", 12).await;

    clock.advance(Duration::from_secs(60 * 60));
    assert_code(
        &stack.reset_check(&expiring, 13).await,
        StatusCode::GONE,
        "RESET_TOKEN_EXPIRED",
    );
    let early = prune_step(&stack.pools, &clock, PruneStep::PasswordResetTokens)
        .await
        .unwrap();
    assert_eq!(early.deleted, 0);
    assert_eq!(stack.reset_rows(ada).await.len(), 2);
    assert_code(
        &stack.reset_check(&used, 14).await,
        StatusCode::GONE,
        "RESET_TOKEN_USED",
    );

    clock.advance(Duration::from_secs(24 * 60 * 60));
    let due = prune_step(&stack.pools, &clock, PruneStep::PasswordResetTokens)
        .await
        .unwrap();
    assert_eq!(due.deleted, 2);
    assert!(stack.reset_rows(ada).await.is_empty());
    assert_code(
        &stack.reset_check(&expiring, 15).await,
        StatusCode::BAD_REQUEST,
        "RESET_TOKEN_INVALID",
    );
    stack.stop().await;
}

#[test]
fn unit_reset_errors_map_to_canonical_codes() {
    use crate::features::auth::password_reset::error::PasswordResetError;

    for (error, status, code) in [
        (PasswordResetError::TokenInvalid, 400, "RESET_TOKEN_INVALID"),
        (PasswordResetError::TokenExpired, 410, "RESET_TOKEN_EXPIRED"),
        (PasswordResetError::TokenUsed, 410, "RESET_TOKEN_USED"),
        (
            PasswordResetError::SmtpUnavailable,
            409,
            "FEATURE_UNAVAILABLE_SMTP",
        ),
        (
            PasswordResetError::PasswordLoginDisabled,
            403,
            "AUTH_PASSWORD_LOGIN_DISABLED",
        ),
        (
            PasswordResetError::PasswordPolicyViolation { min_length: 8 },
            422,
            "PASSWORD_POLICY_VIOLATION",
        ),
    ] {
        let api = error.api_error();
        assert_eq!(api.status().as_u16(), status, "{code}");
        assert_eq!(api.code().as_str(), code);
    }
}
