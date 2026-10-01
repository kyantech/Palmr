use std::io::Read;

use tracing_subscriber::filter::EnvFilter;

use super::admin_settings::{assert_detail, AuditRow, SETTINGS};
use super::password_reset::Capture;
use super::profile::{assert_code, Call};
use super::*;
use crate::config::LogFormat;
use crate::features::email::transport::{FailureReason, ProbeFailure, ProbeScript, Stage};
use crate::features::settings::snapshot::setting_aad;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::telemetry::build_dispatch;

const SMTP: &str = "smtp";
const TEST: &str = "smtp/test";
const SENTINEL: &str = "palmr-smtp-flow-sentinel-6b0d3f";
const REPLACEMENT: &str = "palmr-smtp-flow-replacement-91ac7e";
const UNSAVED_SENTINEL: &str = "palmr-smtp-unsaved-sentinel-4e82c5";
const FORGOT: &str = "/api/v1/auth/password/forgot";
const INVITES: &str = "/api/v1/admin/invites";
const USERS: &str = "/api/v1/admin/users";
const HOUR: Duration = Duration::from_secs(3600);

type SecretRow = (
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    i64,
    i64,
    String,
);

fn complete() -> Value {
    json!({
        "enabled": true,
        "host": "smtp.example.test",
        "port": 587,
        "security": "starttls",
        "username": "mailer",
        "password": SENTINEL,
        "fromName": "Palmr",
        "fromEmail": "palmr@example.test",
        "allowSelfSignedCertificate": false,
        "noAuth": false,
    })
}

fn unsaved_settings() -> Value {
    json!({
        "host": "relay.unsaved.example",
        "port": 2525,
        "security": "none",
        "username": "unsaved-user",
        "password": UNSAVED_SENTINEL,
        "fromEmail": "sender@unsaved.example",
        "fromName": "Unsaved",
        "noAuth": false,
    })
}

impl Stack {
    async fn secret_row(&self) -> Option<SecretRow> {
        sqlx::query_as(
            "SELECT value_json, secret_ciphertext, secret_nonce, key_version, is_secret, value_type
               FROM app_settings WHERE key = 'smtp_password'",
        )
        .fetch_optional(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn smtp_audit(&self) -> Vec<AuditRow> {
        self.settings_audit()
            .await
            .into_iter()
            .filter(|row| row.0 == "SMTP_SETTINGS_CHANGED")
            .collect()
    }

    async fn smtp_test(&self, creds: &Credentials, body: &Value) -> Fetched {
        self.settings_call(Method::POST, Some(TEST), creds, Some(body), 10)
            .await
    }

    async fn outbox_count(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM email_outbox").await
    }

    async fn job_count(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM jobs").await
    }

    async fn forgot_password(&self, identifier: &str, host: u8) -> Fetched {
        let body = json!({ "identifier": identifier });
        self.post_json(FORGOT, &body.to_string(), host, None).await
    }
}

fn database_files_containing(root: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for name in ["palmr.db", "palmr.db-wal", "palmr.db-shm"] {
        let path = root.join(name);
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        file.take(1 << 30).read_to_end(&mut bytes).unwrap();
        if bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
        {
            found.push(name.to_owned());
        }
    }
    found
}

fn audit_text(rows: &[AuditRow]) -> String {
    rows.iter()
        .map(|row| format!("{row:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn it_smtp_get_and_patch_round_trip_and_aggregate() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    let initial = stack.read_settings(SMTP, &admin).await;
    assert_eq!(
        initial,
        json!({
            "enabled": false,
            "host": null,
            "port": 587,
            "security": "starttls",
            "username": null,
            "passwordConfigured": false,
            "fromName": null,
            "fromEmail": null,
            "allowSelfSignedCertificate": false,
            "noAuth": false,
        })
    );

    let saved = stack.patch_ok(SMTP, &admin, &complete()).await;
    let expected = json!({
        "enabled": true,
        "host": "smtp.example.test",
        "port": 587,
        "security": "starttls",
        "username": "mailer",
        "passwordConfigured": true,
        "fromName": "Palmr",
        "fromEmail": "palmr@example.test",
        "allowSelfSignedCertificate": false,
        "noAuth": false,
    });
    assert_eq!(saved, expected);
    assert_eq!(stack.read_settings(SMTP, &admin).await, expected);
    let all = stack
        .settings_call(Method::GET, None, &admin, None, 10)
        .await
        .json();
    assert_eq!(all["smtp"], expected);
    assert_eq!(all["general"]["appName"], "Palmr");

    let changed = stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({
                "host": "  mail.example.test  ",
                "port": 465,
                "security": "implicit",
                "fromName": null,
                "allowSelfSignedCertificate": true,
                "noAuth": true,
            }),
        )
        .await;
    assert_eq!(changed["host"], "mail.example.test");
    assert_eq!(changed["port"], 465);
    assert_eq!(changed["security"], "implicit");
    assert_eq!(changed["fromName"], Value::Null);
    assert_eq!(changed["allowSelfSignedCertificate"], true);
    assert_eq!(changed["noAuth"], true);
    assert_eq!(changed["username"], "mailer");
    assert_eq!(changed["passwordConfigured"], true);

    let reloaded = SettingsService::load(
        &stack.pools,
        Arc::new(clock.clone()),
        &InstanceKey::load_or_create(root.path()).unwrap().0,
    )
    .await
    .unwrap();
    let smtp = &reloaded.current().smtp;
    assert_eq!(smtp.host.as_deref(), Some("mail.example.test"));
    assert_eq!(smtp.port, 465);
    assert_eq!(
        smtp.password.as_ref().unwrap().expose_secret().as_str(),
        SENTINEL
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_password_absent_set_replace_and_clear() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    assert!(stack.secret_row().await.is_none());

    let before = stack.all_stored_settings().await;
    let unset_clear = stack
        .patch_ok(SMTP, &admin, &json!({ "password": null }))
        .await;
    assert_eq!(unset_clear["passwordConfigured"], false);
    assert!(stack.secret_row().await.is_none());
    assert_eq!(stack.all_stored_settings().await, before);
    assert!(stack.smtp_audit().await.is_empty());

    stack
        .patch_ok(SMTP, &admin, &json!({ "password": SENTINEL }))
        .await;
    let first = stack.secret_row().await.unwrap();
    assert_eq!(first.0, None);
    assert_eq!((first.3, first.4, first.5.as_str()), (1, 1, "secret"));
    let ciphertext = first.1.clone().unwrap();
    let nonce = first.2.clone().unwrap();
    assert_eq!(nonce.len(), 24);
    assert!(!ciphertext
        .windows(SENTINEL.len())
        .any(|window| window == SENTINEL.as_bytes()));
    assert_eq!(
        stack
            .settings
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        SENTINEL
    );

    stack
        .patch_ok(SMTP, &admin, &json!({ "host": "smtp.example.test" }))
        .await;
    assert_eq!(stack.secret_row().await.unwrap(), first, "absent leaves it");
    assert_eq!(
        stack
            .settings
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        SENTINEL
    );

    stack
        .patch_ok(SMTP, &admin, &json!({ "password": REPLACEMENT }))
        .await;
    let replaced = stack.secret_row().await.unwrap();
    assert_ne!(replaced.1.as_ref().unwrap(), &ciphertext);
    assert_ne!(replaced.2.as_ref().unwrap(), &nonce);
    assert_eq!(
        stack
            .settings
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        REPLACEMENT
    );

    let cleared = stack
        .patch_ok(SMTP, &admin, &json!({ "password": null }))
        .await;
    assert_eq!(cleared["passwordConfigured"], false);
    assert!(
        stack.secret_row().await.is_none(),
        "clearing deletes the row"
    );
    assert!(stack.settings.current().smtp_password().is_none());
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM app_settings WHERE key = 'smtp_password'")
            .await,
        0
    );

    for invalid in [
        json!({ "password": 7 }),
        json!({ "password": "" }),
        json!({ "password": true }),
    ] {
        let rejected = stack.patch_settings(SMTP, &admin, &invalid).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&rejected, "key", &json!("password"));
        assert!(stack.secret_row().await.is_none());
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_password_ciphertext_cannot_be_transplanted() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    stack
        .patch_ok(SMTP, &admin, &json!({ "password": SENTINEL }))
        .await;
    let (_, ciphertext, nonce, version, ..) = stack.secret_row().await.unwrap();
    let sealed =
        SealedSecret::from_parts(ciphertext.unwrap(), nonce.as_deref().unwrap(), version).unwrap();

    let keys = stack.settings.keys();
    let opened = keys
        .open(SealPurpose::Smtp, &setting_aad("smtp_password"), &sealed)
        .unwrap();
    assert_eq!(opened.expose_secret().as_slice(), SENTINEL.as_bytes());
    for other_key in ["smtp_username", "smtp_host", "idp_client_secret", ""] {
        assert!(
            keys.open(SealPurpose::Smtp, &setting_aad(other_key), &sealed)
                .is_err(),
            "{other_key}"
        );
    }
    for other_purpose in [
        SealPurpose::Totp,
        SealPurpose::Idp,
        SealPurpose::OutboxToken,
    ] {
        assert!(
            keys.open(other_purpose, &setting_aad("smtp_password"), &sealed)
                .is_err(),
            "{other_purpose:?}"
        );
    }
    let other_instance = TempDir::new().unwrap();
    let other_key = InstanceKey::load_or_create(other_instance.path())
        .unwrap()
        .0;
    assert!(KeyRing::new(&other_key)
        .open(SealPurpose::Smtp, &setting_aad("smtp_password"), &sealed)
        .is_err());
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_audit_records_presence_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, admin_id) = stack.settings_admin().await;

    stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({
                "host": "smtp.example.test",
                "username": "mailer-a",
                "password": SENTINEL,
                "port": 2525,
                "fromEmail": "palmr@example.test",
            }),
        )
        .await;
    stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({ "password": REPLACEMENT, "username": "mailer-b" }),
        )
        .await;
    stack
        .patch_ok(SMTP, &admin, &json!({ "password": null, "noAuth": true }))
        .await;

    let rows = stack.smtp_audit().await;
    let summary: Vec<(String, Value)> = rows
        .iter()
        .map(|row| {
            (
                row.4.clone().unwrap(),
                serde_json::from_str(&row.7).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "smtp_host".to_owned(),
                json!({ "key": "smtp_host", "from": null, "to": "smtp.example.test" })
            ),
            (
                "smtp_port".to_owned(),
                json!({ "key": "smtp_port", "from": 587, "to": 2525 })
            ),
            (
                "smtp_username".to_owned(),
                json!({ "key": "smtp_username", "from": "unset", "to": "set" })
            ),
            (
                "smtp_password".to_owned(),
                json!({ "key": "smtp_password", "from": "unset", "to": "set" })
            ),
            (
                "smtp_from_email".to_owned(),
                json!({ "key": "smtp_from_email", "from": null, "to": "palmr@example.test" })
            ),
            (
                "smtp_username".to_owned(),
                json!({ "key": "smtp_username", "from": "set", "to": "set" })
            ),
            (
                "smtp_password".to_owned(),
                json!({ "key": "smtp_password", "from": "set", "to": "set" })
            ),
            (
                "smtp_password".to_owned(),
                json!({ "key": "smtp_password", "from": "set", "to": "unset" })
            ),
            (
                "smtp_no_auth".to_owned(),
                json!({ "key": "smtp_no_auth", "from": false, "to": true })
            ),
        ]
    );
    for row in &rows {
        assert_eq!(row.0, "SMTP_SETTINGS_CHANGED");
        assert_eq!(row.1, "user");
        assert_eq!(row.2.as_deref(), Some(admin_id.as_str()));
        assert_eq!(row.3.as_deref(), Some("setting"));
        assert_eq!(row.6, "success");
    }
    let text = audit_text(&rows);
    for forbidden in [
        SENTINEL,
        REPLACEMENT,
        "mailer-a",
        "mailer-b",
        "cipher",
        "nonce",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}: {text}");
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'SETTING_CHANGED'")
            .await,
        0,
        "SMTP changes must not be duplicated as generic setting changes"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_noop_patch_writes_nothing() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    stack.patch_ok(SMTP, &admin, &complete()).await;
    let stored = stack.all_stored_settings().await;
    let secret = stack.secret_row().await;
    let audits = stack.smtp_audit().await;
    let snapshot = stack.settings.current();

    clock.advance(Duration::from_secs(60));
    let mut repeated = complete();
    repeated.as_object_mut().unwrap().remove("password");
    stack.patch_ok(SMTP, &admin, &repeated).await;
    stack.patch_ok(SMTP, &admin, &json!({})).await;
    assert_eq!(stack.all_stored_settings().await, stored);
    assert_eq!(stack.secret_row().await, secret);
    assert_eq!(stack.smtp_audit().await, audits);
    assert_eq!(stack.settings.current().smtp.host, snapshot.smtp.host);
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_audit_failure_rolls_back_secret_and_settings_and_keeps_snapshot() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    let before_empty = stack.all_stored_settings().await;
    stack
        .execute(
            "CREATE TRIGGER fail_smtp_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'SMTP_SETTINGS_CHANGED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let snapshot = stack.settings.current();
    let failed = stack.patch_settings(SMTP, &admin, &complete()).await;
    assert!(failed.status.is_server_error(), "{}", failed.text());
    assert!(!failed.text().contains("audit unavailable"));
    assert!(!failed.text().contains(SENTINEL));
    assert_eq!(stack.all_stored_settings().await, before_empty);
    assert!(stack.secret_row().await.is_none(), "no secret survives");
    assert!(Arc::ptr_eq(&stack.settings.current(), &snapshot));
    assert!(stack.smtp_audit().await.is_empty());

    stack.execute("DROP TRIGGER fail_smtp_audit").await;
    stack.patch_ok(SMTP, &admin, &complete()).await;
    let stored = stack.all_stored_settings().await;
    let secret = stack.secret_row().await;
    let audits = stack.smtp_audit().await;
    let snapshot = stack.settings.current();

    stack
        .execute(
            "CREATE TRIGGER fail_smtp_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'SMTP_SETTINGS_CHANGED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    for body in [
        json!({ "noAuth": true, "password": null, "host": "elsewhere.example.test" }),
        json!({ "password": REPLACEMENT }),
        json!({ "port": 25, "password": REPLACEMENT, "fromName": null }),
    ] {
        let failed = stack.patch_settings(SMTP, &admin, &body).await;
        assert!(failed.status.is_server_error(), "{body}: {}", failed.text());
        assert_eq!(stack.all_stored_settings().await, stored, "{body}");
        assert_eq!(stack.secret_row().await, secret, "{body}");
        assert_eq!(stack.smtp_audit().await, audits, "{body}");
        assert!(Arc::ptr_eq(&stack.settings.current(), &snapshot), "{body}");
        assert_eq!(
            stack
                .settings
                .current()
                .smtp_password()
                .unwrap()
                .expose_secret()
                .as_str(),
            SENTINEL
        );
    }
    stack.execute("DROP TRIGGER fail_smtp_audit").await;
    stack
        .patch_ok(SMTP, &admin, &json!({ "password": REPLACEMENT }))
        .await;
    assert!(!Arc::ptr_eq(&stack.settings.current(), &snapshot));
    assert_eq!(
        stack
            .settings
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        REPLACEMENT
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_patch_requires_recent_auth_and_test_does_not() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    stack.patch_ok(SMTP, &admin, &complete()).await;
    let ada = stack.settings_user("ada", 11).await;

    clock.advance(Duration::from_secs(6 * 60));
    let before = stack.all_stored_settings().await;
    let secret = stack.secret_row().await;
    let stale = stack
        .patch_settings(SMTP, &admin, &json!({ "host": "stale.example.test" }))
        .await;
    assert_code(&stale, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    let stale_password = stack
        .patch_settings(SMTP, &admin, &json!({ "password": null }))
        .await;
    assert_code(
        &stale_password,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.secret_row().await, secret);
    let read = stack
        .settings_call(Method::GET, Some(SMTP), &admin, None, 10)
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());

    let tested = stack
        .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
        .await;
    assert_eq!(tested.status, StatusCode::OK, "{}", tested.text());

    for (method, group, body) in [
        (Method::GET, SMTP, None),
        (
            Method::PATCH,
            SMTP,
            Some(json!({ "host": "x.example.test" })),
        ),
        (
            Method::POST,
            TEST,
            Some(json!({ "to": "ada@example.test" })),
        ),
    ] {
        let forbidden = stack
            .settings_call(method.clone(), Some(group), &ada, body.as_ref(), 11)
            .await;
        assert_code(&forbidden, StatusCode::FORBIDDEN, "FORBIDDEN");
    }
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.mail.count(), 1, "only the admin's test was sent");

    let reauth = stack.reauth_with_password(&admin, 10).await;
    assert_eq!(reauth.status, StatusCode::NO_CONTENT, "{}", reauth.text());
    let fresh = stack
        .patch_ok(SMTP, &admin, &json!({ "host": "fresh.example.test" }))
        .await;
    assert_eq!(fresh["host"], "fresh.example.test");
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_validation_and_incremental_configuration() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let before = stack.all_stored_settings().await;

    let invalid: [(Value, &str); 21] = [
        (json!({ "enabled": "yes" }), "enabled"),
        (json!({ "enabled": null }), "enabled"),
        (json!({ "host": "" }), "host"),
        (json!({ "host": "smtp host" }), "host"),
        (json!({ "host": "https://smtp.example.test" }), "host"),
        (json!({ "host": "user@smtp.example.test" }), "host"),
        (json!({ "host": 5 }), "host"),
        (json!({ "port": 0 }), "port"),
        (json!({ "port": 65536 }), "port"),
        (json!({ "port": -1 }), "port"),
        (json!({ "port": "587" }), "port"),
        (json!({ "port": 587.5 }), "port"),
        (json!({ "port": null }), "port"),
        (json!({ "security": "tls" }), "security"),
        (json!({ "security": "STARTTLS" }), "security"),
        (json!({ "security": null }), "security"),
        (json!({ "username": "" }), "username"),
        (json!({ "fromName": "" }), "fromName"),
        (json!({ "fromEmail": "not-an-address" }), "fromEmail"),
        (json!({ "noAuth": 1 }), "noAuth"),
        (
            json!({ "allowSelfSignedCertificate": "true" }),
            "allowSelfSignedCertificate",
        ),
    ];
    for (body, key) in &invalid {
        let rejected = stack.patch_settings(SMTP, &admin, body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&rejected, "key", &json!(key));
    }
    let unknown = stack
        .patch_settings(
            SMTP,
            &admin,
            &json!({ "host": "a.example", "passwordConfigured": true }),
        )
        .await;
    assert_code(
        &unknown,
        StatusCode::UNPROCESSABLE_ENTITY,
        "SETTING_UNKNOWN",
    );
    let snake = stack
        .patch_settings(SMTP, &admin, &json!({ "from_email": "a@b.test" }))
        .await;
    assert_code(&snake, StatusCode::UNPROCESSABLE_ENTITY, "SETTING_UNKNOWN");
    let shape = stack.patch_settings(SMTP, &admin, &json!([1])).await;
    assert_code(&shape, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(stack.all_stored_settings().await, before);
    assert!(stack.smtp_audit().await.is_empty());

    stack
        .patch_ok(SMTP, &admin, &json!({ "host": "smtp.example.test" }))
        .await;
    stack.patch_ok(SMTP, &admin, &json!({ "port": 2525 })).await;
    stack
        .patch_ok(SMTP, &admin, &json!({ "fromEmail": "palmr@example.test" }))
        .await;
    let incomplete = stack.read_settings(SMTP, &admin).await;
    assert_eq!(
        incomplete["enabled"], false,
        "incremental saves are allowed"
    );
    assert_eq!(stack.effective(&admin).await["smtpConfigured"], false);

    let missing_credentials = stack
        .patch_settings(SMTP, &admin, &json!({ "enabled": true }))
        .await;
    assert_code(
        &missing_credentials,
        StatusCode::UNPROCESSABLE_ENTITY,
        "SETTING_VALUE_INVALID",
    );
    assert_detail(&missing_credentials, "key", &json!("username"));
    assert_detail(&missing_credentials, "requiredWhenEnabled", &json!(true));
    assert_eq!(stack.read_settings(SMTP, &admin).await["enabled"], false);

    stack
        .patch_ok(SMTP, &admin, &json!({ "username": "mailer" }))
        .await;
    let missing_password = stack
        .patch_settings(SMTP, &admin, &json!({ "enabled": true }))
        .await;
    assert_detail(&missing_password, "key", &json!("password"));
    stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({ "enabled": true, "password": SENTINEL }),
        )
        .await;
    assert_eq!(stack.effective(&admin).await["smtpConfigured"], true);

    let stored = stack.all_stored_settings().await;
    let secret = stack.secret_row().await;
    for (body, key) in [
        (json!({ "host": null }), "host"),
        (json!({ "fromEmail": null }), "fromEmail"),
        (json!({ "username": null }), "username"),
        (json!({ "password": null }), "password"),
        (json!({ "enabled": true, "host": null, "port": 25 }), "host"),
    ] {
        let rejected = stack.patch_settings(SMTP, &admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&rejected, "key", &json!(key));
        assert_eq!(stack.all_stored_settings().await, stored, "{body}");
        assert_eq!(stack.secret_row().await, secret, "{body}");
    }
    assert_eq!(stack.effective(&admin).await["smtpConfigured"], true);

    stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({ "noAuth": true, "username": null, "password": null }),
        )
        .await;
    assert_eq!(stack.effective(&admin).await["smtpConfigured"], true);
    let auth_again = stack
        .patch_settings(SMTP, &admin, &json!({ "noAuth": false }))
        .await;
    assert_detail(&auth_again, "key", &json!("username"));

    stack
        .patch_ok(SMTP, &admin, &json!({ "enabled": false, "host": null }))
        .await;
    assert_eq!(stack.effective(&admin).await["smtpConfigured"], false);
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_password_never_returned() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let mut responses: Vec<(String, Fetched)> = Vec::new();
    let patched = stack.patch_settings(SMTP, &admin, &complete()).await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    assert_eq!(patched.json()["passwordConfigured"], true);
    assert!(patched.json().get("password").is_none());
    responses.push(("patch".to_owned(), patched));
    for (name, group) in [("group", Some(SMTP)), ("aggregate", None)] {
        let fetched = stack
            .settings_call(Method::GET, group, &admin, None, 10)
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        responses.push((name.to_owned(), fetched));
    }
    let aggregate = responses[2].1.json();
    assert_eq!(aggregate["smtp"]["passwordConfigured"], true);
    assert!(aggregate["smtp"].get("password").is_none());
    let effective = stack
        .get("/api/v1/settings/effective", Some(&admin.session), 11)
        .await;
    responses.push(("effective".to_owned(), effective));

    for (name, body) in [
        ("invalid-port", json!({ "port": 0, "password": SENTINEL })),
        ("unknown", json!({ "bogus": 1, "password": SENTINEL })),
        (
            "invalid-type",
            json!({ "password": SENTINEL, "enabled": "x" }),
        ),
        ("empty-host", json!({ "password": SENTINEL, "host": "" })),
    ] {
        let rejected = stack.patch_settings(SMTP, &admin, &body).await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}: {}",
            rejected.text()
        );
        responses.push((name.to_owned(), rejected));
    }
    let incomplete = stack
        .patch_settings(
            SMTP,
            &admin,
            &json!({ "noAuth": false, "username": null, "password": SENTINEL }),
        )
        .await;
    responses.push(("incomplete".to_owned(), incomplete));
    let tested = stack
        .smtp_test(
            &admin,
            &json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;
    assert_eq!(tested.status, StatusCode::OK, "{}", tested.text());
    responses.push(("unsaved-test".to_owned(), tested));
    stack.mail.script_probe(ProbeScript::Fail(ProbeFailure::new(
        Stage::Auth,
        FailureReason::CredentialsRejected,
    )));
    let rejected_test = stack
        .smtp_test(
            &admin,
            &json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;
    assert_code(&rejected_test, StatusCode::BAD_GATEWAY, "SMTP_TEST_FAILED");
    responses.push(("failed-test".to_owned(), rejected_test));
    let invalid_test = stack
        .smtp_test(
            &admin,
            &json!({ "to": "not-an-address", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;
    responses.push(("invalid-test".to_owned(), invalid_test));

    for (name, fetched) in &responses {
        let text = fetched.text();
        for secret in [SENTINEL, UNSAVED_SENTINEL] {
            assert!(!text.contains(secret), "{name}: {text}");
        }
        assert!(!text.to_lowercase().contains("\"password\":\""), "{name}");
        for (header, value) in &fetched.headers {
            let value = value.to_str().unwrap_or_default();
            assert!(!value.contains(SENTINEL), "{name} {header}");
        }
    }

    let settings = stack.settings.current();
    for rendered in [format!("{settings:?}"), format!("{settings:#?}")] {
        assert!(!rendered.contains(SENTINEL), "{rendered}");
        assert!(rendered.contains("<redacted>"));
    }
    let captured_config = stack.mail.captured();
    let rendered = format!("{captured_config:?}");
    assert!(!rendered.contains(UNSAVED_SENTINEL), "{rendered}");

    let logs = capture.text();
    for secret in [SENTINEL, UNSAVED_SENTINEL] {
        assert!(!logs.contains(secret), "{logs}");
    }
    let audit = audit_text(&stack.settings_audit().await);
    for secret in [SENTINEL, UNSAVED_SENTINEL] {
        assert!(!audit.contains(secret), "{audit}");
    }

    let document = String::from_utf8(crate::app::openapi::export_document().unwrap()).unwrap();
    for secret in [SENTINEL, UNSAVED_SENTINEL] {
        assert!(!document.contains(secret));
    }
    stack.stop().await;
}

#[tokio::test]
#[allow(non_snake_case)]
async fn regression_R044_secrets_never_returned_or_plaintext() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    stack.patch_ok(SMTP, &admin, &complete()).await;
    stack
        .patch_ok(SMTP, &admin, &json!({ "password": REPLACEMENT }))
        .await;
    stack
        .smtp_test(
            &admin,
            &json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;

    let (value_json, ciphertext, nonce, version, is_secret, value_type) =
        stack.secret_row().await.unwrap();
    assert_eq!(value_json, None, "no JSON representation of the secret");
    assert_eq!((version, is_secret, value_type.as_str()), (1, 1, "secret"));
    assert_eq!(nonce.unwrap().len(), 24);
    assert!(ciphertext.unwrap().len() > REPLACEMENT.len());

    for needle in [SENTINEL, REPLACEMENT, UNSAVED_SENTINEL] {
        for (table, text) in stack.every_stored_text().await {
            assert!(!text.contains(needle), "{table}: {needle}");
        }
        assert_eq!(
            database_files_containing(root.path(), needle),
            Vec::<String>::new(),
            "{needle} must not reach the database file or its WAL"
        );
    }

    let stored_smtp = stack.read_settings(SMTP, &admin).await;
    assert_eq!(stored_smtp["passwordConfigured"], true);
    assert!(!stored_smtp.to_string().contains(REPLACEMENT));

    let stranger = TempDir::new().unwrap();
    let wrong_key = InstanceKey::load_or_create(stranger.path()).unwrap().0;
    let refused =
        SettingsService::load(&stack.pools, Arc::new(stack.clock.clone()), &wrong_key).await;
    assert!(
        refused.is_err(),
        "the password is unreadable without instance.key"
    );

    stack
        .patch_ok(SMTP, &admin, &json!({ "noAuth": true, "password": null }))
        .await;
    assert!(stack.secret_row().await.is_none());
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_test_endpoint_contract() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    let incomplete = stack
        .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
        .await;
    assert_code(
        &incomplete,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_detail(&incomplete, "fields", &json!(["host"]));
    assert_eq!(stack.mail.count(), 0);

    stack.patch_ok(SMTP, &admin, &complete()).await;
    let ok = stack
        .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(
        ok.headers.get("cache-control").unwrap().to_str().unwrap(),
        "no-store"
    );
    let report = ok.json();
    assert_eq!(report["ok"], true);
    assert_eq!(
        report["stages"],
        json!([
            { "name": "connect", "ok": true },
            { "name": "starttls", "ok": true },
            { "name": "auth", "ok": true },
            { "name": "send", "ok": true },
        ])
    );
    assert!(report["durationMs"].is_u64());
    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].config.host, "smtp.example.test");
    assert_eq!(captured[0].message.to.email.as_str(), "ada@example.test");
    assert_eq!(
        captured[0]
            .config
            .password
            .as_ref()
            .unwrap()
            .expose_secret()
            .as_str(),
        SENTINEL
    );
    assert_eq!(
        stack.outbox_count().await,
        0,
        "the test never uses the outbox"
    );
    assert_eq!(stack.job_count().await, 0);

    let plain = stack
        .smtp_test(
            &admin,
            &json!({
                "to": "ada@example.test",
                "useUnsavedSettings": {
                    "host": "plain.example.test", "port": 25, "security": "none",
                    "fromEmail": "palmr@example.test", "noAuth": true,
                },
            }),
        )
        .await;
    assert_eq!(plain.status, StatusCode::OK, "{}", plain.text());
    assert_eq!(
        plain.json()["stages"],
        json!([{ "name": "connect", "ok": true }, { "name": "send", "ok": true }])
    );

    clock.advance(HOUR);
    for (stage, reason, expected_stage) in [
        (Stage::Connect, FailureReason::Unreachable, "connect"),
        (Stage::Starttls, FailureReason::Tls, "starttls"),
        (Stage::Auth, FailureReason::CredentialsRejected, "auth"),
    ] {
        stack
            .mail
            .script_probe(ProbeScript::Fail(ProbeFailure::new(stage, reason)));
        let failed = stack
            .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
            .await;
        assert_code(&failed, StatusCode::BAD_GATEWAY, "SMTP_TEST_FAILED");
        assert_detail(&failed, "stage", &json!(expected_stage));
        let message = failed.json()["error"]["message"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(message, reason.message());
        for leaked in [SENTINEL, "mailer", "smtp.example.test"] {
            assert!(
                !failed.text().contains(leaked),
                "{leaked}: {}",
                failed.text()
            );
        }
    }
    clock.advance(HOUR);
    stack.mail.script_probe(ProbeScript::Succeed);

    for (body, fields) in [
        (json!({}), json!(["to"])),
        (json!({ "to": "nope" }), json!(["to"])),
        (json!({ "to": "a@b.test", "bogus": 1 }), json!(["body"])),
        (
            json!({ "to": "a@b.test", "useUnsavedSettings": { "host": "x.test" } }),
            json!([
                "useUnsavedSettings.port",
                "useUnsavedSettings.security",
                "useUnsavedSettings.fromEmail"
            ]),
        ),
        (
            json!({ "to": "a@b.test", "useUnsavedSettings": {
                "host": "x.test", "port": 25, "security": "none",
                "fromEmail": "palmr@example.test", "username": "u" } }),
            json!(["useUnsavedSettings.password"]),
        ),
    ] {
        let rejected = stack.smtp_test(&admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_detail(&rejected, "fields", &fields);
    }
    clock.advance(HOUR);
    let path = format!("{SETTINGS}/{TEST}");
    let mut malformed = Call::new(Method::POST, &path, &admin);
    malformed.body = Some("{".to_owned());
    let invalid_json = stack.call(malformed, 10).await;
    assert_code(&invalid_json, StatusCode::BAD_REQUEST, "INVALID_JSON");
    assert_eq!(
        stack.mail.count(),
        5,
        "rejected requests never reach the transport"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_test_is_rate_limited_instance_wide() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    stack.patch_ok(SMTP, &admin, &complete()).await;

    for _ in 0..5 {
        let ok = stack
            .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
            .await;
        assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    }
    let limited = stack
        .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
        .await;
    assert_code(&limited, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(stack.mail.count(), 5);
    let read = stack
        .settings_call(Method::GET, Some(SMTP), &admin, None, 10)
        .await;
    assert_eq!(read.status, StatusCode::OK, "reads use their own budget");

    clock.advance(HOUR);
    let again = stack
        .smtp_test(&admin, &json!({ "to": "ada@example.test" }))
        .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_unsaved_test_values_are_never_persisted_or_logged() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    stack.patch_ok(SMTP, &admin, &complete()).await;
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let stored = stack.all_stored_settings().await;
    let secret = stack.secret_row().await;
    let audits = stack.settings_audit().await;
    let snapshot = stack.settings.current();

    let ok = stack
        .smtp_test(
            &admin,
            &json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    let captured = stack.mail.captured();
    assert_eq!(captured[0].config.host, "relay.unsaved.example");
    assert_eq!(captured[0].config.port, 2525);
    assert_eq!(
        captured[0]
            .config
            .password
            .as_ref()
            .unwrap()
            .expose_secret()
            .as_str(),
        UNSAVED_SENTINEL
    );

    stack.mail.script_probe(ProbeScript::Fail(ProbeFailure::new(
        Stage::Send,
        FailureReason::MessageRejected,
    )));
    let failed = stack
        .smtp_test(
            &admin,
            &json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved_settings() }),
        )
        .await;
    assert_code(&failed, StatusCode::BAD_GATEWAY, "SMTP_TEST_FAILED");

    assert_eq!(stack.all_stored_settings().await, stored);
    assert_eq!(stack.secret_row().await, secret);
    assert_eq!(stack.settings_audit().await, audits);
    assert!(Arc::ptr_eq(&stack.settings.current(), &snapshot));
    assert_eq!(
        stack.settings.current().smtp.host.as_deref(),
        Some("smtp.example.test")
    );
    assert_eq!(
        stack
            .settings
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        SENTINEL
    );
    for needle in ["relay.unsaved.example", UNSAVED_SENTINEL, "unsaved-user"] {
        for (table, text) in stack.every_stored_text().await {
            assert!(!text.contains(needle), "{table}: {needle}");
        }
    }
    assert!(database_files_containing(root.path(), UNSAVED_SENTINEL).is_empty());
    let logs = capture.text();
    for needle in [UNSAVED_SENTINEL, SENTINEL, "unsaved-user"] {
        assert!(!logs.contains(needle), "{needle}: {logs}");
    }
    assert!(logs.contains("the SMTP test failed"), "{logs}");
    assert!(logs.contains("\"stage\":\"send\""), "{logs}");
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_changes_reach_e_mail_dependent_features_on_the_next_request() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let ada = stack.settings_user("ada", 11).await;
    let ada_id = stack.operator_id("ada").await;
    let invite = json!({ "email": "bo@example.test", "role": "user", "sendEmail": true });
    let call_invite = async |stack: &Stack| {
        stack
            .call(Call::new(Method::POST, INVITES, &admin).json(&invite), 10)
            .await
    };
    let call_email_change = async |stack: &Stack| {
        let body = json!({ "email": "ada.new@example.test" });
        stack
            .call(
                Call::new(Method::POST, &format!("{USERS}/{ada_id}/email"), &admin).json(&body),
                10,
            )
            .await
    };

    assert_eq!(stack.effective(&ada).await["smtpConfigured"], false);
    assert_code(
        &stack.forgot_password("ada", 12).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    assert_code(
        &call_invite(&stack).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    assert_code(
        &call_email_change(&stack).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    assert_eq!(stack.outbox_count().await, 0);

    stack.patch_ok(SMTP, &admin, &complete()).await;
    assert_eq!(stack.effective(&ada).await["smtpConfigured"], true);
    let forgot = stack.forgot_password("ada", 12).await;
    assert_eq!(forgot.status, StatusCode::ACCEPTED, "{}", forgot.text());
    let invited = call_invite(&stack).await;
    assert_eq!(invited.status, StatusCode::CREATED, "{}", invited.text());
    let started = call_email_change(&stack).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    assert_eq!(stack.outbox_count().await, 3);

    stack
        .patch_ok(SMTP, &admin, &json!({ "enabled": false }))
        .await;
    assert_eq!(stack.effective(&ada).await["smtpConfigured"], false);
    assert_code(
        &stack.forgot_password("ada", 12).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    assert_code(
        &call_invite(&stack).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );

    stack.deliver_mail().await;
    let outbox: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT state, last_error FROM email_outbox ORDER BY id")
            .fetch_all(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(outbox.len(), 3);
    for (state, last_error) in &outbox {
        assert_eq!(state, "failed");
        assert_eq!(last_error.as_deref(), Some("EMAIL_NOT_CONFIGURED"));
    }
    assert_eq!(stack.mail.count(), 0, "nothing is sent while unavailable");
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_absent_core_features_work() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let ada = stack.settings_user("ada", 11).await;

    assert!(!stack.settings.current().smtp.is_available());
    let effective = stack.effective(&ada).await;
    assert_eq!(effective["smtpConfigured"], false);

    let me = stack.get(ME, Some(&ada.session), 11).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    let profile = stack.get("/api/v1/profile", Some(&ada.session), 11).await;
    assert_eq!(profile.status, StatusCode::OK, "{}", profile.text());
    let users = stack.read_users(&admin, 10).await;
    assert_eq!(users.status, StatusCode::OK, "{}", users.text());
    for (group, body) in [
        ("general", json!({ "appName": "No Mail Needed" })),
        ("quotas", json!({ "maxFileSizeBytes": 1024 })),
    ] {
        stack.patch_ok(group, &admin, &body).await;
    }
    let manual_invite = stack
        .call(
            Call::new(Method::POST, INVITES, &admin)
                .json(&json!({ "email": "bo@example.test", "role": "user", "sendEmail": false })),
            10,
        )
        .await;
    assert_eq!(
        manual_invite.status,
        StatusCode::CREATED,
        "{}",
        manual_invite.text()
    );
    let relogin = stack.login("ada", PASSWORD, 11).await;
    assert_eq!(relogin.status, StatusCode::OK, "{}", relogin.text());

    assert_code(
        &stack.forgot_password("ada", 12).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    stack.deliver_mail().await;
    assert_eq!(stack.outbox_count().await, 0);
    assert_eq!(stack.mail.count(), 0);

    stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({ "host": "smtp.example.test", "fromEmail": "palmr@example.test" }),
        )
        .await;
    assert!(
        !stack.settings.current().smtp.is_available(),
        "a configured but disabled SMTP stays unavailable"
    );
    assert_code(
        &stack.forgot_password("ada", 12).await,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );
    let still_me = stack.get(ME, Some(&ada.session), 11).await;
    assert_eq!(still_me.status, StatusCode::OK);
    stack.stop().await;
}

#[test]
fn unit_smtp_capability_has_one_canonical_decision() {
    for (name, source) in [
        (
            "password_reset",
            include_str!("../password_reset/service.rs"),
        ),
        ("invites", include_str!("../invites/service.rs")),
        ("email_change", include_str!("../../users/email_change.rs")),
        ("effective", include_str!("../../settings/effective.rs")),
    ] {
        assert!(source.contains("smtp.is_available()"), "{name}");
        assert!(!source.contains("SmtpConfig::from_settings"), "{name}");
        assert!(!source.contains("smtp.enabled"), "{name}");
    }
}

#[tokio::test]
async fn it_smtp_username_and_password_have_no_policy_ceilings() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    let username = format!("user\nname-{}", "u".repeat(5000));
    let password = format!("p\u{e9}{}", "w".repeat(200_000));
    let saved = stack
        .patch_ok(
            SMTP,
            &admin,
            &json!({ "username": username, "password": password }),
        )
        .await;
    assert_eq!(saved["username"], json!(username));
    assert_eq!(saved["passwordConfigured"], true);
    let current = stack.settings.current();
    assert_eq!(current.smtp.username.as_deref(), Some(username.as_str()));
    assert_eq!(
        current.smtp_password().unwrap().expose_secret().as_str(),
        password
    );
    let reloaded = SettingsService::load(
        &stack.pools,
        Arc::new(stack.clock.clone()),
        &InstanceKey::load_or_create(root.path()).unwrap().0,
    )
    .await
    .unwrap();
    assert_eq!(
        reloaded
            .current()
            .smtp_password()
            .unwrap()
            .expose_secret()
            .as_str(),
        password
    );

    let beyond_storage = stack
        .patch_settings(SMTP, &admin, &json!({ "username": "u".repeat(65_536) }))
        .await;
    assert_code(
        &beyond_storage,
        StatusCode::UNPROCESSABLE_ENTITY,
        "SETTING_VALUE_INVALID",
    );
    assert_detail(&beyond_storage, "key", &json!("username"));
    assert_eq!(
        stack.settings.current().smtp.username.as_deref(),
        Some(username.as_str())
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_smtp_unavailable_still_allows_manual_invites_but_not_delivery() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    for configured_but_disabled in [false, true] {
        if configured_but_disabled {
            stack
                .patch_ok(
                    SMTP,
                    &admin,
                    &json!({ "host": "smtp.example.test", "fromEmail": "palmr@example.test" }),
                )
                .await;
        }
        assert!(!stack.settings.current().smtp.is_available());
        let email = format!("manual{}@example.test", u8::from(configured_but_disabled));
        let invites_before = stack.scalar_i64("SELECT COUNT(*) FROM invites").await;

        let created = stack
            .call(
                Call::new(Method::POST, INVITES, &admin)
                    .json(&json!({ "email": email, "role": "user", "sendEmail": false })),
                10,
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        let body = created.json();
        let url = body["inviteUrl"].as_str().unwrap();
        assert!(
            url.starts_with("https://files.example.test/invite/"),
            "{url}"
        );
        assert!(url.len() > "https://files.example.test/invite/".len());
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM invites").await,
            invites_before + 1
        );
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invites WHERE email = ?1")
            .bind(&email)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
        assert_eq!(stored, 1);
        assert_eq!(stack.outbox_count().await, 0);
        assert_eq!(stack.job_count().await, 0);
        stack.deliver_mail().await;
        assert_eq!(stack.mail.count(), 0);

        let refused = stack
            .call(
                Call::new(Method::POST, INVITES, &admin).json(
                    &json!({ "email": "wants-mail@example.test", "role": "user", "sendEmail": true }),
                ),
                10,
            )
            .await;
        assert_code(&refused, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM invites").await,
            invites_before + 1,
            "a refused delivery request records nothing"
        );
        assert_eq!(stack.outbox_count().await, 0);

        let id = body["id"].as_str().unwrap();
        let resend = stack
            .call(
                Call::new(Method::POST, &format!("{INVITES}/{id}/resend"), &admin),
                10,
            )
            .await;
        assert_code(&resend, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
        assert_eq!(stack.outbox_count().await, 0);
        assert_eq!(stack.mail.count(), 0);
    }
    stack.stop().await;
}
