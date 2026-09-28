use tracing_subscriber::filter::EnvFilter;

use super::operator_cli::Capture;
use super::profile::{assert_code, Call};
use super::*;
use crate::config::LogFormat;
use crate::features::auth::sessions::{AuthMethod, NewSession};
use crate::features::auth::totp::model::secret_aad;
use crate::features::auth::totp::repo;
use crate::features::auth::totp::routes::{
    DISABLE_ROUTE, ENROLL_ROUTE, REGENERATE_ROUTE, STATUS_ROUTE, VERIFY_ROUTE,
};
use crate::features::auth::totp::TotpError;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::hkdf::SealPurpose;
use crate::infra::crypto::totp::{backup_code_digest, time_step, TotpSecret};
use crate::infra::jobs::prune_tokens::{prune_step, PruneStep};
use crate::infra::telemetry::build_dispatch;

const TWO_FACTOR: &str = "/api/v1/auth/2fa";
const ENROLL: &str = "/api/v1/auth/2fa/enroll";
const VERIFY: &str = "/api/v1/auth/2fa/enroll/verify";
const DISABLE: &str = "/api/v1/auth/2fa/disable";
const REGENERATE: &str = "/api/v1/auth/2fa/backup-codes/regenerate";
const REAUTH: &str = "/api/v1/auth/reauthenticate";
const STEP: Duration = Duration::from_secs(30);
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
const RECENT_AUTH_LAPSE: Duration = Duration::from_secs(6 * 60);

struct Enrollment {
    id: String,
    uri: String,
    base32: String,
    expires_at: String,
    secret: TotpSecret,
}

struct Enabled {
    credentials: Credentials,
    codes: Vec<String>,
    enrollment: Enrollment,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SecretRow {
    state: String,
    secret_ciphertext: Vec<u8>,
    secret_nonce: Vec<u8>,
    key_version: i64,
    last_used_step: Option<i64>,
    confirmed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct BackupRow {
    batch_id: String,
    code_hash: String,
    used_at: Option<String>,
}

fn decode_base32(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for character in text.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'2'..=b'7' => character - b'2' + 26,
            _ => panic!("not base32: {character}"),
        };
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    bytes
}

fn step_of(at: OffsetDateTime) -> u64 {
    time_step(at.unix_timestamp()).unwrap()
}

fn code_for(secret: &TotpSecret, step: u64) -> String {
    String::from_utf8(secret.code_at(step).to_vec()).unwrap()
}

fn wrong_code(secret: &TotpSecret, now: OffsetDateTime) -> String {
    let window: Vec<String> = (step_of(now) - 1..=step_of(now) + 1)
        .map(|step| code_for(secret, step))
        .collect();
    (0..1_000_000_u32)
        .map(|value| format!("{value:06}"))
        .find(|candidate| !window.contains(candidate))
        .unwrap()
}

fn stamp(at: OffsetDateTime) -> String {
    Timestamp::try_from(at).unwrap().to_string()
}

impl Stack {
    async fn tf_post(
        &self,
        path: &str,
        credentials: &Credentials,
        body: Option<&Value>,
        host: u8,
    ) -> Fetched {
        let mut call = Call::new(Method::POST, path, credentials);
        if let Some(body) = body {
            call = call.json(body);
        }
        self.call(call, host).await
    }

    async fn tf_enroll(&self, credentials: &Credentials, host: u8) -> Enrollment {
        let fetched = self.tf_post(ENROLL, credentials, None, host).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
        let body = fetched.json();
        let base32 = body["secretBase32"].as_str().unwrap().to_owned();
        let secret = TotpSecret::from_opened(&Secret::new(decode_base32(&base32))).unwrap();
        Enrollment {
            id: body["enrollmentId"].as_str().unwrap().to_owned(),
            uri: body["otpauthUri"].as_str().unwrap().to_owned(),
            base32,
            expires_at: body["expiresAt"].as_str().unwrap().to_owned(),
            secret,
        }
    }

    async fn tf_verify(
        &self,
        credentials: &Credentials,
        enrollment_id: &str,
        code: &str,
        host: u8,
    ) -> Fetched {
        let body = json!({ "enrollmentId": enrollment_id, "code": code });
        self.tf_post(VERIFY, credentials, Some(&body), host).await
    }

    async fn tf_enable(&self, credentials: &Credentials, host: u8) -> Enabled {
        let enrollment = self.tf_enroll(credentials, host).await;
        let code = code_for(&enrollment.secret, step_of(clock_now(&self.clock)));
        let verified = self
            .tf_verify(credentials, &enrollment.id, &code, host)
            .await;
        assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
        let codes = verified.json()["backupCodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap().to_owned())
            .collect();
        Enabled {
            credentials: Credentials::from(&verified),
            codes,
            enrollment,
        }
    }

    async fn tf_reauth(&self, credentials: &Credentials, code: Option<&str>, host: u8) -> Fetched {
        let body = json!({ "password": PASSWORD, "totpCode": code });
        self.tf_post(REAUTH, credentials, Some(&body), host).await
    }

    async fn tf_status(&self, credentials: &Credentials, host: u8) -> Value {
        let fetched = self.get(TWO_FACTOR, Some(&credentials.session), host).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn tf_secret_row(&self, user: UserId) -> Option<SecretRow> {
        sqlx::query_as(
            "SELECT state, secret_ciphertext, secret_nonce, key_version, last_used_step,
                    confirmed_at
               FROM totp_secrets WHERE user_id = ?1",
        )
        .bind(user.to_string())
        .fetch_optional(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn tf_backup_rows(&self, user: UserId) -> Vec<BackupRow> {
        sqlx::query_as(
            "SELECT batch_id, code_hash, used_at FROM totp_backup_codes
              WHERE user_id = ?1 ORDER BY code_hash",
        )
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn tf_enabled_flag(&self, user: UserId) -> bool {
        self.scalar_i64(&format!(
            "SELECT totp_enabled FROM users WHERE id = '{user}'"
        ))
        .await
            == 1
    }

    async fn tf_mint(&self, user: UserId) -> Credentials {
        let minted = self
            .sessions
            .mint(NewSession {
                user_id: user,
                auth_method: AuthMethod::PasswordTotp,
                ip_address: None,
                user_agent: None,
            })
            .await
            .unwrap();
        Credentials {
            session: minted.session_token.expose_secret().clone(),
            csrf: minted.csrf_token.expose_secret().clone(),
        }
    }

    async fn tf_consume_backup(&self, user: UserId, code: &str) -> bool {
        let digest = backup_code_digest(code).unwrap();
        let now = Timestamp::try_from(clock_now(&self.clock)).unwrap();
        self.pools
            .write_tx(&self.clock, "auth.test_consume_backup", async |tx| {
                repo::consume_backup_code(tx, user, &digest, now, Some("198.51.100.9")).await
            })
            .await
            .unwrap()
    }

    async fn tf_consume_step(&self, user: UserId, step: u64) -> bool {
        let now = Timestamp::try_from(clock_now(&self.clock)).unwrap();
        self.pools
            .write_tx(&self.clock, "auth.test_consume_step", async |tx| {
                repo::consume_step(tx, user, step, now).await
            })
            .await
            .unwrap()
    }

    async fn tf_last_auth(&self, credentials: &Credentials) -> String {
        sqlx::query_scalar("SELECT last_auth_at FROM sessions WHERE token_hash = ?1")
            .bind(digest(&credentials.session))
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn tf_session_states(&self, user: UserId) -> Vec<(String, Option<String>)> {
        self.sessions_of(user)
            .await
            .into_iter()
            .map(|row| (row.state, row.revoked_reason))
            .collect()
    }
}

fn capture_dispatch() -> (Capture, tracing::Dispatch) {
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    (capture, dispatch)
}

#[tokio::test]
async fn it_totp_enroll_requires_verification() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    let other = stack.signed_in("ada@example.test", 11).await;
    stack.trusted_device(ada, "device-a", None).await;

    let first = stack.tf_enroll(&current, 12).await;
    assert!(
        first
            .uri
            .starts_with("otpauth://totp/Palmr:ada@example.test?secret="),
        "{}",
        first.uri
    );
    assert!(first.uri.contains(&format!("secret={}&", first.base32)));
    assert!(first
        .uri
        .ends_with("&issuer=Palmr&algorithm=SHA1&digits=6&period=30"));
    assert_eq!(first.base32.len(), 32);
    assert_eq!(first.expires_at, stamp(START + PENDING_TTL));
    let pending = stack.tf_secret_row(ada).await.unwrap();
    assert_eq!(pending.state, "pending");
    assert_eq!(pending.confirmed_at, None);
    assert_eq!(pending.last_used_step, None);
    assert!(!stack.tf_enabled_flag(ada).await);
    assert_eq!(
        stack.tf_status(&current, 13).await,
        json!({
            "enabled": false,
            "enrolledAt": null,
            "backupCodesRemaining": 0,
            "requiredByPolicy": false,
            "canDisable": false
        })
    );
    let password_only = stack.login("ada", PASSWORD, 14).await;
    assert_eq!(
        password_only.status,
        StatusCode::OK,
        "{}",
        password_only.text()
    );
    let password_only = Credentials::from(&password_only);
    assert_eq!(
        stack.tf_reauth(&current, None, 15).await.status,
        StatusCode::NO_CONTENT
    );
    for path in [DISABLE, REGENERATE] {
        assert_code(
            &stack.tf_post(path, &current, None, 16).await,
            StatusCode::CONFLICT,
            "TOTP_NOT_ENROLLED",
        );
    }

    let now = clock_now(&clock);
    for code in [
        wrong_code(&first.secret, now),
        "12a456".to_owned(),
        String::new(),
    ] {
        assert_code(
            &stack.tf_verify(&current, &first.id, &code, 17).await,
            StatusCode::UNAUTHORIZED,
            "AUTH_2FA_INVALID",
        );
    }
    let valid = code_for(&first.secret, step_of(now));
    assert_code(
        &stack
            .tf_verify(&current, "0000000000000000000000000000000", &valid, 18)
            .await,
        StatusCode::CONFLICT,
        "TOTP_ENROLLMENT_PENDING_MISSING",
    );
    let empty_id = stack.tf_verify(&current, "", &valid, 18).await;
    assert_code(
        &empty_id,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        empty_id.json()["error"]["details"]["fields"],
        json!(["enrollmentId"])
    );
    assert_eq!(stack.tf_secret_row(ada).await.unwrap(), pending);
    assert!(!stack.tf_enabled_flag(ada).await);
    assert!(stack.tf_backup_rows(ada).await.is_empty());
    assert!(stack
        .tf_session_states(ada)
        .await
        .iter()
        .all(|(state, _)| state == "active"));
    assert!(stack.audit_rows("TWO_FACTOR_ENABLED").await.is_empty());

    let second = stack.tf_enroll(&current, 19).await;
    assert_ne!(second.id, first.id);
    assert_ne!(second.base32, first.base32);
    assert_code(
        &stack
            .tf_verify(
                &current,
                &first.id,
                &code_for(&second.secret, step_of(now)),
                20,
            )
            .await,
        StatusCode::CONFLICT,
        "TOTP_ENROLLMENT_PENDING_MISSING",
    );
    assert_code(
        &stack.tf_verify(&current, &second.id, &valid, 20).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_INVALID",
    );

    clock.advance(PENDING_TTL);
    for fetched in [
        stack.tf_post(ENROLL, &current, None, 21).await,
        stack
            .tf_verify(&current, &second.id, &code_for(&second.secret, 0), 21)
            .await,
    ] {
        assert_code(&fetched, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    }
    assert_eq!(
        stack.tf_reauth(&current, None, 22).await.status,
        StatusCode::NO_CONTENT
    );
    let now = clock_now(&clock);
    assert_code(
        &stack
            .tf_verify(
                &current,
                &second.id,
                &code_for(&second.secret, step_of(now)),
                23,
            )
            .await,
        StatusCode::CONFLICT,
        "TOTP_ENROLLMENT_PENDING_MISSING",
    );

    let third = stack.tf_enroll(&current, 24).await;
    let sealed_before = stack.tf_secret_row(ada).await.unwrap();
    let verified = stack
        .tf_verify(
            &current,
            &third.id,
            &code_for(&third.secret, step_of(now)),
            25,
        )
        .await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    assert_eq!(verified.headers.get("cache-control").unwrap(), "no-store");
    let body = verified.json();
    assert_eq!(body["generatedAt"], stamp(now));
    assert_eq!(body["backupCodes"].as_array().unwrap().len(), 10);
    let rotated = Credentials::from(&verified);
    assert_ne!(rotated.session, current.session);
    assert_ne!(rotated.csrf, current.csrf);

    let active = stack.tf_secret_row(ada).await.unwrap();
    assert_eq!(active.state, "active");
    assert_eq!(active.secret_ciphertext, sealed_before.secret_ciphertext);
    assert_eq!(active.secret_nonce, sealed_before.secret_nonce);
    assert_eq!(active.confirmed_at.as_deref(), Some(stamp(now).as_str()));
    assert_eq!(
        active.last_used_step,
        Some(i64::try_from(step_of(now)).unwrap())
    );
    assert!(stack.tf_enabled_flag(ada).await);
    assert_eq!(stack.tf_backup_rows(ada).await.len(), 10);
    assert_eq!(
        stack.tf_status(&rotated, 26).await,
        json!({
            "enabled": true,
            "enrolledAt": stamp(now),
            "backupCodesRemaining": 10,
            "requiredByPolicy": false,
            "canDisable": true
        })
    );
    let mut states = stack.tf_session_states(ada).await;
    states.sort();
    assert_eq!(
        states,
        [
            ("active".to_owned(), None),
            ("revoked".to_owned(), Some("policy_changed".to_owned())),
            ("revoked".to_owned(), Some("policy_changed".to_owned())),
        ]
    );
    for revoked in [&current, &other, &password_only] {
        assert_eq!(
            stack.get(ME, Some(&revoked.session), 27).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        stack.get(ME, Some(&rotated.session), 27).await.status,
        StatusCode::OK
    );
    assert_eq!(stack.devices_of(ada).await, [("device-a".to_owned(), None)]);
    let audit = stack.audit_rows("TWO_FACTOR_ENABLED").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, "ada");
    assert_eq!(audit[0].1.as_deref(), Some(ada.to_string().as_str()));
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].2).unwrap(),
        json!({ "sessions_revoked": 2, "backup_codes": 10 })
    );

    assert_code(
        &stack.tf_post(ENROLL, &rotated, None, 28).await,
        StatusCode::CONFLICT,
        "TOTP_ALREADY_ENABLED",
    );
    assert_code(
        &stack
            .tf_verify(
                &rotated,
                &third.id,
                &code_for(&third.secret, step_of(now) + 1),
                28,
            )
            .await,
        StatusCode::CONFLICT,
        "TOTP_ENROLLMENT_PENDING_MISSING",
    );
    let refused = stack.login("ada", PASSWORD, 29).await;
    assert_ne!(refused.status, StatusCode::OK);
    assert!(refused.set_cookies().is_empty());
    assert_eq!(stack.tf_session_states(ada).await.len(), 3);
    stack.stop().await;
}

#[tokio::test]
async fn it_totp_verify_failure_is_atomic() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    let other = stack.signed_in("ada", 11).await;
    let enrollment = stack.tf_enroll(&current, 12).await;
    let pending = stack.tf_secret_row(ada).await.unwrap();
    stack
        .execute(
            "CREATE TRIGGER fail_two_factor_enabled BEFORE INSERT ON audit_events
              WHEN NEW.action = 'TWO_FACTOR_ENABLED'
              BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END",
        )
        .await;

    let code = code_for(&enrollment.secret, step_of(clock_now(&clock)));
    let failed = stack.tf_verify(&current, &enrollment.id, &code, 13).await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert!(failed.set_cookies().is_empty());
    assert!(!failed.text().contains(&enrollment.base32));
    assert_eq!(stack.tf_secret_row(ada).await.unwrap(), pending);
    assert!(!stack.tf_enabled_flag(ada).await);
    assert!(stack.tf_backup_rows(ada).await.is_empty());
    assert!(stack
        .tf_session_states(ada)
        .await
        .iter()
        .all(|(state, _)| state == "active"));
    for credentials in [&current, &other] {
        assert_eq!(
            stack.get(ME, Some(&credentials.session), 14).await.status,
            StatusCode::OK
        );
    }

    stack.execute("DROP TRIGGER fail_two_factor_enabled").await;
    let verified = stack.tf_verify(&current, &enrollment.id, &code, 15).await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    assert!(stack.tf_enabled_flag(ada).await);
    stack.stop().await;
}

#[tokio::test]
async fn it_totp_replay_rejected() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let signed_in = stack.signed_in("ada", 10).await;
    let enabled = stack.tf_enable(&signed_in, 11).await;
    let session = enabled.credentials;
    let secret = enabled.enrollment.secret;
    let enabled_at = step_of(clock_now(&clock));
    let last_auth = stack.tf_last_auth(&session).await;

    for step in [enabled_at, enabled_at - 1] {
        let replayed = stack
            .tf_reauth(&session, Some(&code_for(&secret, step)), 12)
            .await;
        assert_code(&replayed, StatusCode::UNAUTHORIZED, "TOTP_CODE_REPLAYED");
        assert!(replayed.set_cookies().is_empty());
    }
    assert_eq!(stack.tf_last_auth(&session).await, last_auth);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts WHERE result <> 'success'")
            .await,
        0
    );

    clock.advance(Duration::from_secs(1));
    let next = code_for(&secret, enabled_at + 1);
    assert_eq!(
        stack.tf_reauth(&session, Some(&next), 13).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stack.tf_last_auth(&session).await, stamp(clock_now(&clock)));
    assert_eq!(
        stack.tf_secret_row(ada).await.unwrap().last_used_step,
        Some(i64::try_from(enabled_at + 1).unwrap())
    );
    assert_code(
        &stack.tf_reauth(&session, Some(&next), 14).await,
        StatusCode::UNAUTHORIZED,
        "TOTP_CODE_REPLAYED",
    );

    let wrong = stack
        .tf_reauth(&session, Some(&wrong_code(&secret, clock_now(&clock))), 15)
        .await;
    assert_code(&wrong, StatusCode::UNAUTHORIZED, "AUTH_2FA_INVALID");
    assert_eq!(
        stack
            .scalar_i64(
                "SELECT COUNT(*) FROM login_attempts WHERE method = 'totp' AND result = 'totp_failed'"
            )
            .await,
        1
    );
    assert_eq!(stack.lock_row(ada).await.0, 1);
    let missing = stack.tf_reauth(&session, None, 16).await;
    assert_code(
        &missing,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        missing.json()["error"]["details"]["fields"],
        json!(["totpCode"])
    );

    clock.advance(STEP);
    assert_code(
        &stack.tf_reauth(&session, Some(&next), 17).await,
        StatusCode::UNAUTHORIZED,
        "TOTP_CODE_REPLAYED",
    );
    let later = code_for(&secret, enabled_at + 2);
    assert_eq!(
        stack.tf_reauth(&session, Some(&later), 18).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stack.lock_row(ada).await.0, 0);
    stack.stop().await;

    let restarted = Stack::start(root.path(), &clock).await;
    assert_code(
        &restarted.tf_reauth(&session, Some(&later), 19).await,
        StatusCode::UNAUTHORIZED,
        "TOTP_CODE_REPLAYED",
    );

    clock.advance(STEP);
    let raced = code_for(&secret, enabled_at + 3);
    let (first, second) = tokio::join!(
        restarted.tf_reauth(&session, Some(&raced), 20),
        restarted.tf_reauth(&session, Some(&raced), 21),
    );
    let mut outcomes = [first.status, second.status];
    outcomes.sort();
    assert_eq!(outcomes, [StatusCode::NO_CONTENT, StatusCode::UNAUTHORIZED]);
    let loser = if first.status == StatusCode::UNAUTHORIZED {
        &first
    } else {
        &second
    };
    assert_eq!(loser.error_code(), "TOTP_CODE_REPLAYED");
    assert!(!restarted.tf_consume_step(ada, enabled_at + 3).await);
    assert!(!restarted.tf_consume_step(ada, enabled_at + 2).await);
    assert!(restarted.tf_consume_step(ada, enabled_at + 4).await);
    restarted.stop().await;
}

#[tokio::test]
async fn it_backup_codes_single_use_hashed() {
    let (capture, dispatch) = capture_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let signed_in = stack.signed_in("ada", 10).await;
    let enabled = stack.tf_enable(&signed_in, 11).await;
    let session = enabled.credentials;
    let secret = enabled.enrollment.secret;
    let codes = enabled.codes;

    assert_eq!(codes.len(), 10);
    let mut expected: Vec<String> = codes
        .iter()
        .map(|code| {
            let groups: Vec<&str> = code.split('-').collect();
            assert_eq!(groups.len(), 4, "{code}");
            assert!(groups.iter().all(|group| group.len() == 4
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))));
            backup_code_digest(code).unwrap().as_str().to_owned()
        })
        .collect();
    expected.sort();
    expected.dedup();
    assert_eq!(expected.len(), 10);
    let rows = stack.tf_backup_rows(ada).await;
    assert_eq!(
        rows.iter()
            .map(|row| row.code_hash.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(rows.iter().all(|row| row.used_at.is_none()));
    assert!(rows.iter().all(|row| row.batch_id == rows[0].batch_id));
    assert!(rows
        .iter()
        .all(|row| TokenDigest::parse(&row.code_hash).is_ok()));

    let status = stack.get(TWO_FACTOR, Some(&session.session), 12).await;
    assert_eq!(status.json()["backupCodesRemaining"], 10);

    assert!(stack.tf_consume_backup(ada, &codes[0]).await);
    assert!(!stack.tf_consume_backup(ada, &codes[0]).await);
    assert!(
        !stack
            .tf_consume_backup(ada, &codes[0].to_ascii_lowercase())
            .await
    );
    let (first, second) = tokio::join!(
        stack.tf_consume_backup(ada, &codes[1]),
        stack.tf_consume_backup(ada, &codes[1]),
    );
    assert!(first ^ second);
    assert_eq!(
        stack.tf_status(&session, 13).await["backupCodesRemaining"],
        8
    );
    let used: Vec<BackupRow> = stack
        .tf_backup_rows(ada)
        .await
        .into_iter()
        .filter(|row| row.used_at.is_some())
        .collect();
    assert_eq!(used.len(), 2);
    let before_regenerate = stack.tf_backup_rows(ada).await;

    let extra = stack.tf_mint(ada).await;
    clock.advance(RECENT_AUTH_LAPSE);
    assert_code(
        &stack.tf_post(REGENERATE, &session, None, 14).await,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(stack.tf_backup_rows(ada).await, before_regenerate);
    let now = clock_now(&clock);
    assert_eq!(
        stack
            .tf_reauth(&session, Some(&code_for(&secret, step_of(now))), 15)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    stack
        .execute(
            "CREATE TRIGGER fail_regenerate BEFORE INSERT ON audit_events
              WHEN NEW.action = 'TWO_FACTOR_BACKUP_CODES_REGENERATED'
              BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END",
        )
        .await;
    assert_code(
        &stack.tf_post(REGENERATE, &session, None, 16).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
    );
    assert_eq!(stack.tf_backup_rows(ada).await, before_regenerate);
    stack.execute("DROP TRIGGER fail_regenerate").await;

    let regenerated = stack.tf_post(REGENERATE, &session, None, 17).await;
    assert_eq!(regenerated.status, StatusCode::OK, "{}", regenerated.text());
    assert_eq!(
        regenerated.headers.get("cache-control").unwrap(),
        "no-store"
    );
    assert!(regenerated.set_cookies().is_empty());
    let body = regenerated.json();
    assert_eq!(body["generatedAt"], stamp(now));
    let fresh: Vec<String> = body["backupCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(fresh.len(), 10);
    assert!(fresh.iter().all(|code| !codes.contains(code)));
    let mut fresh_hashes: Vec<String> = fresh
        .iter()
        .map(|code| backup_code_digest(code).unwrap().as_str().to_owned())
        .collect();
    fresh_hashes.sort();
    let rows = stack.tf_backup_rows(ada).await;
    assert_eq!(
        rows.iter()
            .map(|row| row.code_hash.clone())
            .collect::<Vec<_>>(),
        fresh_hashes
    );
    assert!(rows.iter().all(|row| row.used_at.is_none()
        && row.batch_id == rows[0].batch_id
        && row.batch_id != before_regenerate[0].batch_id));
    assert!(!stack.tf_consume_backup(ada, &codes[2]).await);
    assert!(stack.tf_consume_backup(ada, &fresh[0]).await);
    let audit = stack
        .audit_rows("TWO_FACTOR_BACKUP_CODES_REGENERATED")
        .await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].2).unwrap(),
        json!({ "backup_codes_deleted": 10, "backup_codes": 10 })
    );
    for credentials in [&session, &extra] {
        assert_eq!(
            stack.get(ME, Some(&credentials.session), 18).await.status,
            StatusCode::OK
        );
    }

    let audit_text = stack.all_audit_text().await;
    let stored: Vec<(String,)> = sqlx::query_as(
        "SELECT id || '|' || batch_id || '|' || code_hash || '|' || COALESCE(used_ip, '')
           FROM totp_backup_codes",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    let logs = capture.text();
    for code in codes.iter().chain(&fresh) {
        let normalized = code.replace('-', "");
        for haystack in [audit_text.as_str(), logs.as_str(), status.text().as_str()]
            .into_iter()
            .chain(stored.iter().map(|(row,)| row.as_str()))
        {
            assert!(!haystack.contains(code.as_str()));
            assert!(!haystack.contains(&normalized));
        }
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_totp_disable_blocked_by_policy() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let signed_in = stack.signed_in("ada", 10).await;
    let enabled = stack.tf_enable(&signed_in, 11).await;
    let session = enabled.credentials;
    let secret = enabled.enrollment.secret;
    let extra = stack.tf_mint(ada).await;
    stack.trusted_device(ada, "device-a", None).await;
    let secret_before = stack.tf_secret_row(ada).await;
    let codes_before = stack.tf_backup_rows(ada).await;
    let sessions_before = stack.tf_session_states(ada).await;

    stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let status = stack.tf_status(&session, 12).await;
    assert_eq!(status["requiredByPolicy"], true);
    assert_eq!(status["canDisable"], false);
    assert_eq!(status["enabled"], true);
    assert_code(
        &stack.tf_post(DISABLE, &session, None, 13).await,
        StatusCode::FORBIDDEN,
        "TOTP_REQUIRED_BY_POLICY",
    );
    assert_eq!(stack.tf_secret_row(ada).await, secret_before);
    assert!(stack.tf_enabled_flag(ada).await);
    assert_eq!(stack.tf_backup_rows(ada).await, codes_before);
    assert_eq!(stack.tf_session_states(ada).await, sessions_before);
    assert_eq!(stack.devices_of(ada).await, [("device-a".to_owned(), None)]);
    assert!(stack.audit_rows("TWO_FACTOR_DISABLED").await.is_empty());

    stack
        .setting("two_factor_required", "boolean", "false")
        .await;
    clock.advance(RECENT_AUTH_LAPSE);
    assert_code(
        &stack.tf_post(DISABLE, &session, None, 14).await,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(stack.tf_secret_row(ada).await, secret_before);
    let now = clock_now(&clock);
    assert_eq!(
        stack
            .tf_reauth(&session, Some(&code_for(&secret, step_of(now))), 15)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let disabled = stack.tf_post(DISABLE, &session, None, 16).await;
    assert_eq!(
        disabled.status,
        StatusCode::NO_CONTENT,
        "{}",
        disabled.text()
    );
    let cleared = disabled.set_cookies();
    for name in ["palmr_session=", "palmr_csrf="] {
        assert!(
            cleared
                .iter()
                .any(|cookie| cookie.starts_with(name) && cookie.contains("Max-Age=0")),
            "{cleared:?}"
        );
    }
    assert_eq!(stack.tf_secret_row(ada).await, None);
    assert!(!stack.tf_enabled_flag(ada).await);
    assert!(stack.tf_backup_rows(ada).await.is_empty());
    assert!(stack
        .tf_session_states(ada)
        .await
        .iter()
        .all(|(state, reason)| state == "revoked" && reason.as_deref() == Some("policy_changed")));
    assert!(stack.devices_of(ada).await[0].1.is_some());
    for credentials in [&session, &extra] {
        assert_eq!(
            stack.get(ME, Some(&credentials.session), 17).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let audit = stack.audit_rows("TWO_FACTOR_DISABLED").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].1.as_deref(), Some(ada.to_string().as_str()));
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].2).unwrap(),
        json!({
            "sessions_revoked": 2,
            "trusted_devices_revoked": 1,
            "backup_codes_deleted": 10
        })
    );

    let fresh = stack.signed_in("ada", 18).await;
    for path in [DISABLE, REGENERATE] {
        assert_code(
            &stack.tf_post(path, &fresh, None, 19).await,
            StatusCode::CONFLICT,
            "TOTP_NOT_ENROLLED",
        );
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_totp_secret_encrypted_at_rest() {
    let (capture, dispatch) = capture_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let bob = stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let ada_session = stack.signed_in("ada", 10).await;
    let bob_session = stack.signed_in("bob", 11).await;

    let enrollment = stack.tf_enroll(&ada_session, 12).await;
    let raw = enrollment.secret.expose_secret().to_vec();
    let row = stack.tf_secret_row(ada).await.unwrap();
    assert_eq!(row.key_version, 1);
    assert_eq!(row.secret_nonce.len(), 24);
    assert_eq!(row.secret_ciphertext.len(), raw.len() + 16);
    assert!(!row
        .secret_ciphertext
        .windows(4)
        .any(|window| raw.windows(4).any(|chunk| chunk == window)));
    assert!(!String::from_utf8_lossy(&row.secret_ciphertext).contains(&enrollment.base32));

    let keys = stack.settings.keys();
    let sealed =
        SealedSecret::from_parts(row.secret_ciphertext.clone(), &row.secret_nonce, 1).unwrap();
    assert_eq!(
        keys.open(SealPurpose::Totp, &secret_aad(ada), &sealed)
            .unwrap()
            .expose_secret(),
        &raw
    );
    assert!(keys
        .open(SealPurpose::Totp, &secret_aad(bob), &sealed)
        .is_err());
    assert!(keys
        .open(SealPurpose::Smtp, &secret_aad(ada), &sealed)
        .is_err());

    let now = clock_now(&clock);
    let wrong = stack
        .tf_verify(
            &ada_session,
            &enrollment.id,
            &wrong_code(&enrollment.secret, now),
            13,
        )
        .await;
    let verified = stack
        .tf_verify(
            &ada_session,
            &enrollment.id,
            &code_for(&enrollment.secret, step_of(now)),
            14,
        )
        .await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    let ada_session = Credentials::from(&verified);
    let active = stack.tf_secret_row(ada).await.unwrap();
    assert_eq!(active.secret_ciphertext, row.secret_ciphertext);
    assert_eq!(active.secret_nonce, row.secret_nonce);
    let status = stack.get(TWO_FACTOR, Some(&ada_session.session), 15).await;

    stack
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{bob}'"
        ))
        .await;
    stack
        .execute(&format!(
            "INSERT INTO totp_secrets (user_id, secret_ciphertext, secret_nonce, key_version,
                                       state, confirmed_at, created_at, updated_at)
             SELECT '{bob}', secret_ciphertext, secret_nonce, key_version, 'active',
                    confirmed_at, created_at, updated_at
               FROM totp_secrets WHERE user_id = '{ada}'"
        ))
        .await;
    let bob_auth = stack.tf_last_auth(&bob_session).await;
    let moved = stack
        .tf_reauth(
            &bob_session,
            Some(&code_for(&enrollment.secret, step_of(now) + 1)),
            16,
        )
        .await;
    assert_code(&moved, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(stack.tf_last_auth(&bob_session).await, bob_auth);
    assert_eq!(stack.tf_secret_row(bob).await.unwrap().last_used_step, None);

    let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    let audit_text = stack.all_audit_text().await;
    let logs = capture.text();
    for haystack in [
        wrong.text(),
        verified.text(),
        status.text(),
        moved.text(),
        audit_text,
        logs,
    ] {
        assert!(!haystack.contains(&enrollment.base32), "{haystack}");
        assert!(!haystack.contains(&enrollment.uri));
        assert!(!haystack.contains(&hex));
    }
    let error = TotpError::Crypto(crate::infra::crypto::CryptoError::AuthenticationFailed);
    assert!(!format!("{error} {error:?}").contains(&enrollment.base32));
    stack.stop().await;
}

#[tokio::test]
async fn it_totp_pending_enrollment_pruned_after_ttl() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let bob = stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let ada_session = stack.signed_in("ada", 10).await;
    let bob_session = stack.signed_in("bob", 11).await;
    stack.tf_enable(&ada_session, 12).await;
    stack.tf_enroll(&bob_session, 13).await;

    clock.advance(PENDING_TTL - Duration::from_secs(1));
    let early = prune_step(&stack.pools, &clock, PruneStep::PendingTotpEnrollments)
        .await
        .unwrap();
    assert_eq!(early.deleted, 0);
    clock.advance(Duration::from_secs(1));
    let due = prune_step(&stack.pools, &clock, PruneStep::PendingTotpEnrollments)
        .await
        .unwrap();
    assert_eq!(due.deleted, 1);
    assert_eq!(stack.tf_secret_row(bob).await, None);
    assert_eq!(stack.tf_secret_row(ada).await.unwrap().state, "active");
    stack.stop().await;
}

#[test]
fn unit_totp_route_policies_match_contract() {
    let inventory = application_routes().build().unwrap().inventory;
    for (method, path, policy, auth, rate_limit) in [
        (
            Method::GET,
            TWO_FACTOR,
            STATUS_ROUTE,
            AuthClass::Authenticated,
            "rl.read",
        ),
        (
            Method::POST,
            ENROLL,
            ENROLL_ROUTE,
            AuthClass::AuthenticatedRecentAuth,
            "rl.write",
        ),
        (
            Method::POST,
            VERIFY,
            VERIFY_ROUTE,
            AuthClass::AuthenticatedRecentAuth,
            "rl.auth.totp",
        ),
        (
            Method::POST,
            DISABLE,
            DISABLE_ROUTE,
            AuthClass::AuthenticatedRecentAuth,
            "rl.write",
        ),
        (
            Method::POST,
            REGENERATE,
            REGENERATE_ROUTE,
            AuthClass::AuthenticatedRecentAuth,
            "rl.write",
        ),
    ] {
        let entry = inventory.get(&method, path).unwrap().policy();
        assert_eq!(entry, policy, "{method} {path}");
        assert_eq!(entry.auth(), auth, "{method} {path}");
        assert_eq!(entry.rate_limit().as_str(), rate_limit, "{method} {path}");
    }
}
