use std::collections::BTreeSet;

use super::profile::{assert_code, Call};
use super::totp::{capture_dispatch, code_for, stamp, step_of, wrong_code, Enabled};
use super::*;
use crate::features::auth::routes::LOGIN_TOTP_ROUTE;
use crate::features::auth::sessions::{prune_step, PruneStep, SessionError};

const LOGIN_TOTP: &str = "/api/v1/auth/login/totp";
const TWO_FACTOR: &str = "/api/v1/auth/2fa";
const SESSIONS: &str = "/api/v1/sessions";
const PROFILE: &str = "/api/v1/profile";
const STEP: Duration = Duration::from_secs(30);
const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct MfaRow {
    id: String,
    state: String,
    auth_method: String,
    token_hash: String,
    csrf_token_hash: String,
    mfa_token_hash: Option<String>,
    mfa_expires_at: Option<String>,
    mfa_attempts: i64,
    last_auth_at: String,
    revoked_reason: Option<String>,
}

const SELECT_MFA_ROW: &str = "SELECT id, state, auth_method, token_hash, csrf_token_hash,
        mfa_token_hash, mfa_expires_at, mfa_attempts, last_auth_at, revoked_reason
   FROM sessions";

impl Stack {
    async fn mfa_enrolled(&self, host: u8) -> (UserId, Enabled) {
        let hash = password_hash();
        let ada = self
            .user(UserSpec::local("ada", "ada@example.test", &hash))
            .await;
        (ada, self.mfa_enable(host).await)
    }

    async fn mfa_enable(&self, host: u8) -> Enabled {
        let session = self.signed_in("ada", host).await;
        let enabled = self.tf_enable(&session, host).await;
        self.clock.advance(STEP);
        enabled
    }

    async fn mfa_challenge(&self, host: u8) -> String {
        let fetched = self.login("ada", PASSWORD, host).await;
        assert_code(&fetched, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
        assert!(fetched.set_cookies().is_empty());
        fetched.json()["error"]["details"]["mfaToken"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn mfa_complete(&self, token: &str, code: &str, host: u8) -> Fetched {
        self.mfa_complete_body(&json!({ "mfaToken": token, "code": code }), host)
            .await
    }

    async fn mfa_complete_body(&self, body: &Value, host: u8) -> Fetched {
        self.post_json(LOGIN_TOTP, &body.to_string(), host, None)
            .await
    }

    async fn mfa_pending_rows(&self, user: UserId) -> Vec<MfaRow> {
        sqlx::query_as(&format!(
            "{SELECT_MFA_ROW} WHERE user_id = ?1 AND state = 'mfa_pending' ORDER BY id"
        ))
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn mfa_pending_for(&self, token: &str) -> Option<MfaRow> {
        sqlx::query_as(&format!("{SELECT_MFA_ROW} WHERE mfa_token_hash = ?1"))
            .bind(digest(token))
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn mfa_row(&self, id: &str) -> Option<MfaRow> {
        sqlx::query_as(&format!("{SELECT_MFA_ROW} WHERE id = ?1"))
            .bind(id)
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn mfa_active_count(&self, user: UserId) -> i64 {
        self.scalar_i64(&format!(
            "SELECT COUNT(*) FROM sessions WHERE user_id = '{user}' AND state = 'active'"
        ))
        .await
    }

    async fn mfa_last_step(&self, user: UserId) -> Option<i64> {
        self.tf_secret_row(user).await.unwrap().last_used_step
    }

    async fn mfa_attempts(&self, user: UserId) -> Vec<(String, String)> {
        sqlx::query_as("SELECT method, result FROM login_attempts WHERE user_id = ?1 ORDER BY id")
            .bind(user.to_string())
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn mfa_user_id(&self, username: &str) -> UserId {
        let id: String = sqlx::query_scalar("SELECT id FROM users WHERE username = ?1")
            .bind(username)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap();
        id.parse().unwrap()
    }

    async fn mfa_success_methods(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT json_extract(metadata_json, '$.method') FROM audit_events
              WHERE action = 'LOGIN_SUCCEEDED' ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn mfa_db_text(&self) -> String {
        let mut text = String::new();
        for query in [
            "SELECT COALESCE(group_concat(id || '|' || user_id || '|' || token_hash || '|' ||
                    csrf_token_hash || '|' || state || '|' || auth_method || '|' ||
                    COALESCE(mfa_token_hash, '') || '|' || COALESCE(ip, '') || '|' ||
                    COALESCE(user_agent, ''), char(10)), '') FROM sessions",
            "SELECT COALESCE(group_concat(identifier_normalized || '|' || COALESCE(user_id, '') ||
                    '|' || method || '|' || result || '|' || COALESCE(request_id, ''),
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

    async fn mfa_secret_state(&self, user: UserId) -> String {
        sqlx::query_scalar("SELECT state FROM totp_secrets WHERE user_id = ?1")
            .bind(user.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    fn mfa_code(&self, enabled: &Enabled) -> String {
        code_for(&enabled.enrollment.secret, step_of(clock_now(&self.clock)))
    }

    fn mfa_wrong(&self, enabled: &Enabled) -> String {
        wrong_code(&enabled.enrollment.secret, clock_now(&self.clock))
    }
}

fn cookie_names(fetched: &Fetched) -> BTreeSet<String> {
    fetched
        .set_cookies()
        .iter()
        .map(|cookie| cookie.split('=').next().unwrap().to_owned())
        .collect()
}

fn openapi_document() -> Value {
    let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
        "PALMR_BASE_URL",
        BASE_URL,
    )]))
    .unwrap()
    .config;
    let docs = ApiDocs::new(
        application_routes().build().unwrap().openapi,
        &config.base_url,
    )
    .unwrap();
    serde_json::from_slice(docs.document()).unwrap()
}

fn fresh_token() -> String {
    Token::mint().unwrap().encode().expose_secret().clone()
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-030")]
#[tokio::test]
async fn regression_R030_two_factor_requires_password_step() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.setting("max_login_attempts", "integer", "50").await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let enabled_step = stack.mfa_last_step(ada).await;
    let code = stack.mfa_code(&enabled);

    for body in [
        json!({ "userId": ada.to_string(), "code": code }),
        json!({ "userId": ada.to_string(), "mfaToken": fresh_token(), "code": code }),
        json!({ "email": "ada@example.test", "code": code }),
        json!({ "username": "ada", "code": code }),
        json!({ "identifier": "ada", "password": PASSWORD, "code": code }),
    ] {
        let refused = stack.mfa_complete_body(&body, 11).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert!(refused.set_cookies().is_empty());
    }
    for forged in [fresh_token(), "not-a-challenge".to_owned()] {
        let refused = stack.mfa_complete(&forged, &code, 12).await;
        assert_code(
            &refused,
            StatusCode::UNAUTHORIZED,
            "AUTH_2FA_CHALLENGE_EXPIRED",
        );
        assert!(refused.set_cookies().is_empty());
    }
    let backup = &enabled.codes[0];
    assert_code(
        &stack
            .mfa_complete_body(&json!({ "code": backup }), 12)
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_code(
        &stack.mfa_complete(&fresh_token(), backup, 12).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert_eq!(stack.mfa_last_step(ada).await, enabled_step);
    assert!(stack
        .tf_backup_rows(ada)
        .await
        .iter()
        .all(|row| row.used_at.is_none()));
    assert_eq!(stack.mfa_active_count(ada).await, 1);
    assert!(stack.mfa_pending_rows(ada).await.is_empty());

    let document = openapi_document();
    let schemas = document["components"]["schemas"].as_object().unwrap();
    let request = &schemas["LoginTotpRequest"];
    let fields: BTreeSet<&str> = request["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from(["code", "mfaToken", "rememberDevice"])
    );
    assert_eq!(request["additionalProperties"], false);
    assert_eq!(
        document["paths"][LOGIN_TOTP]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/LoginTotpRequest"
    );
    for (name, schema) in schemas {
        let Some(properties) = schema["properties"].as_object() else {
            continue;
        };
        let names_account = ["userId", "email", "username", "identifier"]
            .iter()
            .any(|field| properties.contains_key(*field));
        let carries_code = ["code", "totpCode", "backupCode"]
            .iter()
            .any(|field| properties.contains_key(*field));
        assert!(
            !(names_account && carries_code),
            "{name} accepts an account identity together with a second-factor code"
        );
    }

    let token = stack.mfa_challenge(13).await;
    let completed = stack.mfa_complete(&token, &code, 14).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    stack.clock.advance(STEP);
    let replayed = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 15)
        .await;
    assert_code(
        &replayed,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert!(replayed.set_cookies().is_empty());
    let after_success = stack.mfa_last_step(ada).await;
    assert_eq!(
        after_success,
        Some(i64::try_from(step_of(clock_now(&clock)) - 1).unwrap())
    );

    let target = stack.mfa_challenge(16).await;
    let wrong = stack.mfa_wrong(&enabled);
    let outcomes: Vec<(StatusCode, String)> = {
        let mut outcomes = Vec::new();
        for _ in 0..20 {
            let fetched = stack.mfa_complete(&target, &wrong, 17).await;
            outcomes.push((fetched.status, fetched.error_code()));
        }
        outcomes
    };
    let expected: Vec<(StatusCode, String)> =
        std::iter::repeat_n((StatusCode::UNAUTHORIZED, "AUTH_2FA_INVALID".to_owned()), 5)
            .chain(std::iter::repeat_n(
                (
                    StatusCode::UNAUTHORIZED,
                    "AUTH_2FA_CHALLENGE_EXPIRED".to_owned(),
                ),
                5,
            ))
            .chain(std::iter::repeat_n(
                (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED".to_owned()),
                10,
            ))
            .collect();
    assert_eq!(outcomes, expected);
    assert!(stack.mfa_pending_for(&target).await.is_none());
    assert_code(
        &stack
            .mfa_complete(&target, &stack.mfa_code(&enabled), 18)
            .await,
        StatusCode::TOO_MANY_REQUESTS,
        "RATE_LIMITED",
    );
    assert_eq!(stack.mfa_last_step(ada).await, after_success);
    assert_eq!(
        stack
            .mfa_attempts(ada)
            .await
            .iter()
            .filter(|(_, result)| result == "totp_failed")
            .count(),
        5
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_mfa_token_browser_contract() {
    let (capture, dispatch) = capture_dispatch();
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let before = stack.mfa_active_count(ada).await;

    let wrong = stack.login("ada", WRONG, 11).await;
    assert_code(&wrong, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    assert!(stack.mfa_pending_rows(ada).await.is_empty());

    let challenged_at = clock_now(&clock);
    let challenged = stack.login("ada", PASSWORD, 12).await;
    assert_code(&challenged, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    assert!(challenged.set_cookies().is_empty());
    assert_eq!(challenged.headers.get("cache-control").unwrap(), "no-store");
    let body = challenged.json();
    let token = body["error"]["details"]["mfaToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(token.len(), 43);
    assert_eq!(
        Token::decode(&token).unwrap().expose_secret().len() * 8,
        256
    );
    assert_eq!(
        body,
        json!({
            "error": {
                "code": "AUTH_2FA_REQUIRED",
                "message": "Two-factor authentication is required",
                "requestId": challenged.headers.get("x-request-id").unwrap().to_str().unwrap(),
                "details": {
                    "mfaToken": token,
                    "expiresAt": stamp(challenged_at + CHALLENGE_TTL),
                    "methods": ["totp", "backup_code"],
                    "trustedDeviceOffered": true,
                },
            }
        })
    );
    assert!(!challenged.text().contains(&ada.to_string()));
    assert!(!challenged.text().contains("ada@example.test"));
    assert_eq!(stack.mfa_active_count(ada).await, before);

    let pending = stack.mfa_pending_rows(ada).await;
    assert_eq!(pending.len(), 1);
    let pending = pending.into_iter().next().unwrap();
    assert_eq!(pending.state, "mfa_pending");
    assert_eq!(pending.auth_method, "password");
    assert_eq!(
        pending.mfa_token_hash.as_deref(),
        Some(digest(&token).as_str())
    );
    assert_eq!(
        pending.mfa_expires_at.as_deref(),
        Some(stamp(challenged_at + CHALLENGE_TTL).as_str())
    );
    assert_eq!(pending.mfa_attempts, 0);
    assert_ne!(pending.token_hash, digest(&token));
    assert_ne!(pending.csrf_token_hash, digest(&token));
    assert_ne!(pending.token_hash, pending.csrf_token_hash);
    assert_eq!(
        stack.mfa_attempts(ada).await.last().unwrap(),
        &("password".to_owned(), "totp_required".to_owned())
    );

    let second = stack.mfa_challenge(13).await;
    assert_ne!(second, token);
    assert_eq!(stack.mfa_pending_rows(ada).await.len(), 2);

    for path in [ME, TWO_FACTOR, SESSIONS, PROFILE] {
        assert_code(
            &stack.get(path, Some(&token), 14).await,
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
        );
    }
    let known_session = fresh_token();
    let known_csrf = fresh_token();
    stack
        .execute(&format!(
            "UPDATE sessions SET token_hash = '{}', csrf_token_hash = '{}' WHERE id = '{}'",
            digest(&known_session),
            digest(&known_csrf),
            pending.id
        ))
        .await;
    for path in [ME, TWO_FACTOR, SESSIONS, PROFILE] {
        assert_code(
            &stack.get(path, Some(&known_session), 15).await,
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
        );
    }
    assert!(matches!(
        stack
            .sessions
            .authenticate(&Secret::new(known_session.clone()))
            .await,
        Err(SessionError::AuthRequired)
    ));
    let signed_out = stack
        .logout(
            LogoutRequest {
                session: Some(&known_session),
                csrf_cookie: Some(&known_csrf),
                csrf_header: Some(&known_csrf),
                origin: None,
            },
            15,
        )
        .await;
    assert_eq!(signed_out.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.mfa_row(&pending.id).await.unwrap().state,
        "mfa_pending"
    );
    let pending_credentials = Credentials {
        session: known_session.clone(),
        csrf: known_csrf.clone(),
    };
    let reauth_body = json!({ "password": PASSWORD, "totpCode": stack.mfa_code(&enabled) });
    assert_code(
        &stack
            .call(
                Call::new(
                    Method::POST,
                    "/api/v1/auth/reauthenticate",
                    &pending_credentials,
                )
                .json(&reauth_body),
                15,
            )
            .await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );

    let completed_at = clock_now(&clock);
    let completed = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 16)
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(
        cookie_names(&completed),
        BTreeSet::from(["palmr_csrf".to_owned(), "palmr_session".to_owned()])
    );
    assert_eq!(completed.headers.get("cache-control").unwrap(), "no-store");
    let credentials = Credentials::from(&completed);
    for earlier in [&token, &second, &known_session, &known_csrf] {
        assert_ne!(&credentials.session, earlier);
        assert_ne!(&credentials.csrf, earlier);
    }
    let body = completed.json();
    assert_eq!(body["user"]["id"], ada.to_string());
    assert_eq!(body["mustChangePassword"], false);
    assert_eq!(body["mfaEnrollmentRequired"], false);

    let promoted = stack.mfa_row(&pending.id).await.unwrap();
    assert_eq!(
        promoted,
        MfaRow {
            id: pending.id.clone(),
            state: "active".to_owned(),
            auth_method: "password_totp".to_owned(),
            token_hash: digest(&credentials.session),
            csrf_token_hash: digest(&credentials.csrf),
            mfa_token_hash: None,
            mfa_expires_at: None,
            mfa_attempts: 0,
            last_auth_at: stamp(completed_at),
            revoked_reason: None,
        }
    );
    let me = stack.get(ME, Some(&credentials.session), 17).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.json()["user"]["id"], ada.to_string());
    assert_code(
        &stack
            .mfa_complete(&token, &stack.mfa_code(&enabled), 18)
            .await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert_eq!(
        stack.mfa_attempts(ada).await.last().unwrap(),
        &("totp".to_owned(), "success".to_owned())
    );

    stack.flush_audit().await;
    assert_eq!(
        stack.mfa_success_methods().await.last().unwrap(),
        "password_totp"
    );
    let headers = format!("{:?} {:?}", challenged.headers, completed.headers);
    assert!(headers.contains(&credentials.session));
    let db = stack.mfa_db_text().await;
    let logs = capture.text();
    for raw in [&token, &second] {
        for haystack in [
            &db,
            &logs,
            &headers,
            &completed.text(),
            &me.text(),
            &wrong.text(),
        ] {
            assert!(!haystack.contains(raw.as_str()), "{haystack}");
        }
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_mfa_token_burns_after_5_failures() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.setting("max_login_attempts", "integer", "50").await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let enabled_step = stack.mfa_last_step(ada).await;

    let token = stack.mfa_challenge(11).await;
    let pending = stack.mfa_pending_for(&token).await.unwrap();
    let wrong = stack.mfa_wrong(&enabled);
    for attempt in 1..=5_i64 {
        assert_code(
            &stack.mfa_complete(&token, &wrong, 12).await,
            StatusCode::UNAUTHORIZED,
            "AUTH_2FA_INVALID",
        );
        let row = stack.mfa_row(&pending.id).await;
        if attempt < 5 {
            assert_eq!(row.unwrap().mfa_attempts, attempt);
        } else {
            assert_eq!(row, None);
        }
    }
    let burnt = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 12)
        .await;
    assert_code(
        &burnt,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert!(burnt.set_cookies().is_empty());
    assert_eq!(stack.mfa_last_step(ada).await, enabled_step);

    let mixed = stack.mfa_challenge(13).await;
    let durable_before = stack.lock_row(ada).await.0;
    let replayed = code_for(
        &enabled.enrollment.secret,
        u64::try_from(enabled_step.unwrap()).unwrap(),
    );
    for (index, (code, expected, durable)) in [
        (wrong.clone(), "AUTH_2FA_INVALID", 1),
        (replayed, "TOTP_CODE_REPLAYED", 0),
        ("AAAA-AAAA-AAAA-AAAA".to_owned(), "BACKUP_CODE_INVALID", 1),
        (enabled.codes[0][..9].to_owned(), "AUTH_2FA_INVALID", 1),
        ("12345".to_owned(), "AUTH_2FA_INVALID", 1),
    ]
    .into_iter()
    .enumerate()
    {
        let failed_before = stack.lock_row(ada).await.0;
        assert_code(
            &stack.mfa_complete(&mixed, &code, 14).await,
            StatusCode::UNAUTHORIZED,
            expected,
        );
        assert_eq!(
            stack.lock_row(ada).await.0,
            failed_before + durable,
            "{expected}"
        );
        if index < 4 {
            assert_eq!(
                stack.mfa_pending_for(&mixed).await.unwrap().mfa_attempts,
                i64::try_from(index).unwrap() + 1,
                "{expected}"
            );
        }
    }
    assert!(stack.mfa_pending_for(&mixed).await.is_none());
    assert_eq!(stack.lock_row(ada).await.0, durable_before + 4);
    assert_code(
        &stack.mfa_complete(&mixed, &enabled.codes[0], 14).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert!(stack
        .tf_backup_rows(ada)
        .await
        .iter()
        .all(|row| row.used_at.is_none()));

    let raced = stack.mfa_challenge(15).await;
    let racers = (0..9_u8).map(|index| stack.mfa_complete(&raced, &wrong, 20 + index));
    let codes: Vec<String> = futures_util::future::join_all(racers)
        .await
        .iter()
        .map(Fetched::error_code)
        .collect();
    assert_eq!(
        codes
            .iter()
            .filter(|code| *code == "AUTH_2FA_INVALID")
            .count(),
        5,
        "{codes:?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| *code == "AUTH_2FA_CHALLENGE_EXPIRED")
            .count(),
        4,
        "{codes:?}"
    );
    assert!(stack.mfa_pending_for(&raced).await.is_none());
    let failures = stack
        .mfa_attempts(ada)
        .await
        .into_iter()
        .filter(|(_, result)| result == "totp_failed")
        .count();
    assert_eq!(failures, 14);
    assert_eq!(stack.mfa_last_step(ada).await, enabled_step);

    let again = stack.mfa_challenge(16).await;
    let completed = stack
        .mfa_complete(&again, &stack.mfa_code(&enabled), 17)
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(stack.lock_row(ada).await.0, 0);
    stack.stop().await;
}

pub(super) async fn totp_credential_surface_rate_limits(stack: &Stack) {
    let inventory = application_routes().build().unwrap().inventory;
    let policy = inventory.get(&Method::POST, LOGIN_TOTP).unwrap().policy();
    assert_eq!(policy, LOGIN_TOTP_ROUTE);
    assert_eq!(policy.rate_limit(), RateLimitClass::AuthTotp);
    assert_eq!(policy.auth(), AuthClass::Public);

    let ada = stack.mfa_user_id("ada").await;
    let enabled = stack.mfa_enable(70).await;
    let step = stack.mfa_last_step(ada).await;

    let token = stack.mfa_challenge(71).await;
    for _ in 0..10 {
        assert_code(
            &stack.mfa_complete(&token, "", 72).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    let throttled = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 73)
        .await;
    assert_code(&throttled, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(
        throttled.json()["error"]["details"]["scope"],
        "rl.auth.totp"
    );
    assert!(throttled.retry_after().is_some());
    assert!(throttled.set_cookies().is_empty());
    assert_eq!(stack.mfa_last_step(ada).await, step);
    assert_eq!(stack.mfa_pending_for(&token).await.unwrap().mfa_attempts, 0);

    let other = stack.mfa_challenge(74).await;
    let admitted = stack
        .mfa_complete(&other, &stack.mfa_code(&enabled), 73)
        .await;
    assert_eq!(admitted.status, StatusCode::OK, "{}", admitted.text());

    stack.clock.advance(Duration::from_secs(60));
    let refilled = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 73)
        .await;
    assert_eq!(refilled.status, StatusCode::OK, "{}", refilled.text());

    for _ in 0..10 {
        assert_eq!(
            stack.post_json(LOGIN_TOTP, "{}", 75, None).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let unkeyed = stack.post_json(LOGIN_TOTP, "{}", 75, None).await;
    assert_code(&unkeyed, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(unkeyed.json()["error"]["details"]["scope"], "rl.auth.totp");
}

#[tokio::test]
async fn it_mfa_failures_feed_durable_lockout() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    stack.setting("max_login_attempts", "integer", "3").await;
    stack.setting("login_lockout_minutes", "integer", "7").await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let step = stack.mfa_last_step(ada).await;

    let token = stack.mfa_challenge(11).await;
    let failed_at = clock_now(&clock);
    for _ in 0..3 {
        assert_code(
            &stack
                .mfa_complete(&token, &stack.mfa_wrong(&enabled), 12)
                .await,
            StatusCode::UNAUTHORIZED,
            "AUTH_2FA_INVALID",
        );
    }
    assert_eq!(
        stack.lock_row(ada).await,
        (3, Some(stamp(failed_at + Duration::from_secs(7 * 60))), 1)
    );
    let locked = stack
        .mfa_complete(&token, &stack.mfa_code(&enabled), 13)
        .await;
    assert_code(&locked, StatusCode::TOO_MANY_REQUESTS, "AUTH_LOCKED");
    assert_eq!(locked.retry_after(), Some(7 * 60));
    assert!(locked.set_cookies().is_empty());
    assert_eq!(stack.mfa_last_step(ada).await, step);
    assert_eq!(stack.mfa_pending_for(&token).await.unwrap().mfa_attempts, 3);
    assert_code(
        &stack.login("ada", PASSWORD, 14).await,
        StatusCode::TOO_MANY_REQUESTS,
        "AUTH_LOCKED",
    );
    assert_code(
        &stack.login("ada", WRONG, 15).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );

    stack.clock.advance(Duration::from_secs(7 * 60));
    assert_code(
        &stack
            .mfa_complete(&token, &stack.mfa_code(&enabled), 16)
            .await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    let fresh = stack.mfa_challenge(17).await;
    let completed = stack
        .mfa_complete(&fresh, &stack.mfa_code(&enabled), 18)
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(stack.lock_row(ada).await.0, 0);

    let attempts = stack.mfa_attempts(ada).await;
    let second_factor: Vec<&str> = attempts
        .iter()
        .filter(|(method, _)| method == "totp")
        .map(|(_, result)| result.as_str())
        .collect();
    assert_eq!(
        second_factor,
        [
            "totp_failed",
            "totp_failed",
            "totp_failed",
            "locked_out",
            "success"
        ]
    );
    stack.flush_audit().await;
    let actions: Vec<(String, Option<String>)> = stack
        .audit_actions()
        .await
        .into_iter()
        .map(|(action, _, _, _, code)| (action, code))
        .collect();
    let failed = (
        "LOGIN_FAILED".to_owned(),
        Some("AUTH_2FA_INVALID".to_owned()),
    );
    assert_eq!(
        actions.iter().filter(|action| **action == failed).count(),
        3
    );
    assert!(actions.contains(&(
        "LOGIN_LOCKED_OUT".to_owned(),
        Some("AUTH_LOCKED".to_owned())
    )));
    stack.stop().await;
}

#[tokio::test]
async fn it_mfa_backup_code_completion_is_atomic_and_single_use() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let step = stack.mfa_last_step(ada).await;
    for code in &enabled.codes {
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 4, "{code}");
        assert!(groups.iter().all(|group| group.len() == 4
            && group
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))));
    }

    let token = stack.mfa_challenge(11).await;
    let pending = stack.mfa_pending_for(&token).await.unwrap();
    let used_at = clock_now(&clock);
    let completed = stack.mfa_complete(&token, &enabled.codes[0], 12).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let credentials = Credentials::from(&completed);
    let promoted = stack.mfa_row(&pending.id).await.unwrap();
    assert_eq!(promoted.state, "active");
    assert_eq!(promoted.auth_method, "password_backup_code");
    assert_eq!(promoted.token_hash, digest(&credentials.session));
    assert_eq!(promoted.mfa_token_hash, None);
    let first = backup_digest(&enabled.codes[0]);
    let rows = stack.tf_backup_rows(ada).await;
    for row in &rows {
        if row.code_hash == first {
            assert_eq!(row.used_at.as_deref(), Some(stamp(used_at).as_str()));
        } else {
            assert_eq!(row.used_at, None);
        }
    }
    assert_eq!(stack.mfa_last_step(ada).await, step);
    assert_code(
        &stack.mfa_complete(&token, &enabled.codes[1], 13).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );

    let next = stack.mfa_challenge(14).await;
    assert_code(
        &stack.mfa_complete(&next, &enabled.codes[0], 15).await,
        StatusCode::UNAUTHORIZED,
        "BACKUP_CODE_INVALID",
    );
    let forty_bit = &enabled.codes[1][..9];
    assert_code(
        &stack.mfa_complete(&next, forty_bit, 15).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_INVALID",
    );
    assert_eq!(stack.mfa_pending_for(&next).await.unwrap().mfa_attempts, 2);
    let normalized = enabled.codes[1].replace('-', "").to_ascii_lowercase();
    let completed = stack.mfa_complete(&next, &normalized, 16).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let used: Vec<String> = stack
        .tf_backup_rows(ada)
        .await
        .into_iter()
        .filter(|row| row.used_at.is_some())
        .map(|row| row.code_hash)
        .collect();
    assert_eq!(used.len(), 2);
    assert!(used.contains(&backup_digest(&enabled.codes[1])));
    assert_eq!(
        stack
            .mfa_attempts(ada)
            .await
            .into_iter()
            .filter(|(method, _)| method == "backup_code")
            .map(|(_, result)| result)
            .collect::<Vec<_>>(),
        ["success", "totp_failed", "success"]
    );
    stack.flush_audit().await;
    assert_eq!(
        stack.mfa_success_methods().await,
        ["password", "password_backup_code", "password_backup_code"]
    );
    stack.stop().await;
}

fn backup_digest(code: &str) -> String {
    crate::infra::crypto::totp::backup_code_digest(code)
        .unwrap()
        .as_str()
        .to_owned()
}

#[tokio::test]
async fn it_mfa_promotion_failure_rolls_back_consumption() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let step = stack.mfa_last_step(ada).await;
    let token = stack.mfa_challenge(11).await;
    let pending = stack.mfa_pending_for(&token).await.unwrap();

    stack
        .execute(
            "CREATE TRIGGER fail_promotion BEFORE UPDATE OF state ON sessions
              WHEN OLD.state = 'mfa_pending' AND NEW.state = 'active'
              BEGIN SELECT RAISE(ABORT, 'injected promotion failure'); END",
        )
        .await;
    let code = stack.mfa_code(&enabled);
    for submitted in [code.as_str(), enabled.codes[0].as_str()] {
        let failed = stack.mfa_complete(&token, submitted, 12).await;
        assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
        assert!(failed.set_cookies().is_empty());
    }
    assert_eq!(stack.mfa_last_step(ada).await, step);
    assert!(stack
        .tf_backup_rows(ada)
        .await
        .iter()
        .all(|row| row.used_at.is_none()));
    assert_eq!(stack.mfa_row(&pending.id).await.unwrap(), pending);
    assert!(!stack
        .mfa_attempts(ada)
        .await
        .iter()
        .any(|(method, _)| method != "password"));
    stack.execute("DROP TRIGGER fail_promotion").await;

    let completed = stack.mfa_complete(&token, &code, 13).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let backup = stack.mfa_challenge(14).await;
    let completed = stack.mfa_complete(&backup, &enabled.codes[0], 15).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_mfa_completion_rotates_presented_session() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let presented = &enabled.credentials;

    let token = stack.mfa_challenge(11).await;
    let pending = stack.mfa_pending_for(&token).await.unwrap();
    let completed = stack
        .post_json(
            LOGIN_TOTP,
            &json!({ "mfaToken": token, "code": stack.mfa_code(&enabled) }).to_string(),
            12,
            Some(&format!("palmr_session={}", presented.session)),
        )
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let credentials = Credentials::from(&completed);
    assert_ne!(credentials.session, presented.session);
    assert_ne!(digest(&credentials.session), pending.token_hash);
    assert_ne!(digest(&credentials.csrf), pending.csrf_token_hash);
    assert_eq!(
        session_state(&stack, &presented.session).await,
        ("revoked".to_owned(), Some("rotated".to_owned()))
    );
    assert_eq!(stack.mfa_active_count(ada).await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_mfa_challenge_expires_before_prune() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (ada, enabled) = stack.mfa_enrolled(10).await;
    let step = stack.mfa_last_step(ada).await;

    let stale = stack.mfa_challenge(11).await;
    let stale_row = stack.mfa_pending_for(&stale).await.unwrap();
    stack.clock.advance(CHALLENGE_TTL);
    let expired = stack
        .mfa_complete(&stale, &stack.mfa_code(&enabled), 12)
        .await;
    assert_code(
        &expired,
        StatusCode::UNAUTHORIZED,
        "AUTH_2FA_CHALLENGE_EXPIRED",
    );
    assert!(expired.set_cookies().is_empty());
    assert_eq!(stack.mfa_last_step(ada).await, step);
    assert_eq!(stack.mfa_row(&stale_row.id).await.unwrap(), stale_row);

    let live = stack.mfa_challenge(13).await;
    let report = prune_step(&stack.pools, &stack.clock, PruneStep::StaleMfaPending)
        .await
        .unwrap();
    assert_eq!(report.affected, 1);
    assert_eq!(stack.mfa_row(&stale_row.id).await, None);
    assert!(stack.mfa_pending_for(&live).await.is_some());
    assert_eq!(stack.mfa_active_count(ada).await, 1);

    let completed = stack
        .mfa_complete(&live, &stack.mfa_code(&enabled), 14)
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    stack.stop().await;
}

const REAUTH: &str = "/api/v1/auth/reauthenticate";

fn assert_step_up_throttled(fetched: &Fetched) {
    assert_code(fetched, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(fetched.json()["error"]["details"]["scope"], "rl.auth.totp");
    assert!(fetched.retry_after().is_some());
}

#[tokio::test]
async fn it_rl_auth_totp_reauthenticate_is_session_keyed() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let first = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;
    let reauth = async |credentials: &Credentials, body: &Value, host: u8| {
        stack
            .call(
                Call::new(Method::POST, REAUTH, credentials).json(body),
                host,
            )
            .await
    };
    let password = json!({ "password": PASSWORD });
    let verifications = stack.auth.verifications_performed();

    for _ in 0..10 {
        assert_code(
            &reauth(&first, &json!({}), 20).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    assert_step_up_throttled(&reauth(&first, &password, 20).await);
    assert_step_up_throttled(&reauth(&first, &password, 21).await);
    assert_eq!(stack.auth.verifications_performed(), verifications);

    assert_eq!(
        reauth(&second, &password, 20).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stack.auth.verifications_performed(), verifications + 1);

    stack.clock.advance(Duration::from_secs(60));
    assert_eq!(
        reauth(&first, &password, 20).await.status,
        StatusCode::NO_CONTENT
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_rl_auth_totp_enroll_verify_is_session_keyed() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let ada_session = stack.signed_in("ada", 10).await;
    let bob_session = stack.signed_in("bob", 11).await;
    let enrollment = stack.tf_enroll(&ada_session, 12).await;
    let wrong = wrong_code(&enrollment.secret, clock_now(&clock));
    let current = |at: OffsetDateTime| code_for(&enrollment.secret, step_of(at));
    let enabled = async || {
        stack
            .scalar_i64(&format!(
                "SELECT totp_enabled FROM users WHERE id = '{ada}'"
            ))
            .await
    };

    for _ in 0..10 {
        assert_code(
            &stack
                .tf_verify(&ada_session, &enrollment.id, &wrong, 20)
                .await,
            StatusCode::UNAUTHORIZED,
            "AUTH_2FA_INVALID",
        );
    }
    let code = current(clock_now(&clock));
    assert_step_up_throttled(
        &stack
            .tf_verify(&ada_session, &enrollment.id, &code, 20)
            .await,
    );
    assert_step_up_throttled(
        &stack
            .tf_verify(&ada_session, &enrollment.id, &code, 21)
            .await,
    );
    assert_eq!(enabled().await, 0);
    assert_eq!(stack.mfa_secret_state(ada).await, "pending");

    let bob_enrollment = stack.tf_enroll(&bob_session, 20).await;
    let bob_code = code_for(&bob_enrollment.secret, step_of(clock_now(&clock)));
    let bob_verified = stack
        .tf_verify(&bob_session, &bob_enrollment.id, &bob_code, 20)
        .await;
    assert_eq!(
        bob_verified.status,
        StatusCode::OK,
        "{}",
        bob_verified.text()
    );

    stack.clock.advance(Duration::from_secs(60));
    let verified = stack
        .tf_verify(
            &ada_session,
            &enrollment.id,
            &current(clock_now(&clock)),
            20,
        )
        .await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    assert_eq!(enabled().await, 1);
    stack.stop().await;
}
