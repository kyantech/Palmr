use super::operator_cli::{run_reset, temporary_password};
use super::profile::{assert_code, Call, SecurityState, SessionRow};
use super::totp::{code_for, step_of};
use super::*;
use crate::app::router::application_routes as inventory_routes;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::service::{IssueSession, SessionIssue};
use crate::features::auth::sessions::{
    AuthMethod, MintedSession, SessionClient, SessionRestriction,
};
use crate::features::users::profile::{CurrentPasswordProof, ProfileError, VerifiedChange};
use crate::infra::http::extractors::restriction_allows;

const PROFILE: &str = "/api/v1/profile";
const SESSIONS: &str = "/api/v1/sessions";
const PASSWORD_PATH: &str = "/api/v1/profile/password";
const TWO_FACTOR: &str = "/api/v1/auth/2fa";
const ENROLL: &str = "/api/v1/auth/2fa/enroll";
const VERIFY: &str = "/api/v1/auth/2fa/enroll/verify";
const REGENERATE: &str = "/api/v1/auth/2fa/backup-codes/regenerate";
const LOGIN_TOTP: &str = "/api/v1/auth/login/totp";
const EFFECTIVE: &str = "/api/v1/settings/effective";
const BOOTSTRAP: &str = "/api/v1/bootstrap";
const REPLACEMENT: &str = "a replacement passphrase";
const STEP: Duration = Duration::from_secs(30);
const RECENT_AUTH_LAPSE: Duration = Duration::from_secs(30 * 60);

type Snapshot = (
    SecurityState,
    Vec<SessionRow>,
    Vec<(String, Option<String>)>,
    i64,
);

fn credentials(minted: &MintedSession) -> Credentials {
    Credentials {
        session: minted.session_token.expose_secret().clone(),
        csrf: minted.csrf_token.expose_secret().clone(),
    }
}

fn concrete(path: &str, clock: &TestClock) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                UserId::generate(clock).to_string()
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl Stack {
    async fn fs_reset(self, root: &Path, user: UserId) -> (Self, String) {
        let clock = self.clock.clone();
        self.stop().await;
        let (outcome, output) = run_reset(root, &clock, user).await;
        outcome.unwrap();
        (Self::start(root, &clock).await, temporary_password(&output))
    }

    async fn fs_me(&self, creds: &Credentials) -> Value {
        let me = self.get(ME, Some(&creds.session), 30).await;
        assert_eq!(me.status, StatusCode::OK, "{}", me.text());
        me.json()
    }

    async fn fs_forced_change(&self, creds: &Credentials, new: &str, host: u8) -> Fetched {
        self.call(
            Call::new(Method::POST, PASSWORD_PATH, creds).json(&json!({ "newPassword": new })),
            host,
        )
        .await
    }

    async fn fs_post(&self, path: &str, creds: &Credentials, body: Option<Value>) -> Fetched {
        let mut call = Call::new(Method::POST, path, creds);
        if let Some(body) = body {
            call = call.json(&body);
        }
        self.call(call, 31).await
    }

    async fn fs_link_identity(&self, user: UserId, subject: &str) {
        self.execute(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, client_id, token_auth_method, created_at, updated_at)
             VALUES ('provider-corp', 'corp', 'Corp', 'oidc', 'client', 'none',
                     '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')
             ON CONFLICT (id) DO NOTHING",
        )
        .await;
        self.execute(&format!(
            "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method, created_at)
             VALUES ('link-{subject}', '{user}', 'provider-corp', '{subject}', 'manual',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .await;
    }

    async fn fs_external_session(&self, user: UserId) -> (Credentials, SessionRestriction) {
        let prepared = self.sessions.prepare_credentials().unwrap();
        let issued = self
            .pools
            .write_tx(&self.clock, "auth.test_external", async |tx| {
                self.auth
                    .issue_session_in_tx(
                        tx,
                        IssueSession {
                            user_id: user,
                            method: AuthMethod::External,
                            client: SessionClient::default(),
                            credentials: &prepared,
                            verified_password_hash: None,
                            replaces: None,
                            promotes: None,
                            trusted_device: None,
                        },
                    )
                    .await
            })
            .await
            .unwrap();
        match issued {
            SessionIssue::Issued {
                session,
                restriction,
                method,
                ..
            } => {
                assert_eq!(method, AuthMethod::External);
                (credentials(&session), restriction)
            }
            SessionIssue::SecondFactorRequired(_) => {
                panic!("external sign-in must never be challenged with Palmr TOTP")
            }
            SessionIssue::Refused(refusal) => panic!("external sign-in refused: {refusal:?}"),
        }
    }

    async fn fs_sweep(
        &self,
        creds: &Credentials,
        restriction: SessionRestriction,
        code: &str,
    ) -> usize {
        let inventory = inventory_routes().build().unwrap().inventory;
        let mut denied = 0;
        for entry in inventory.entries() {
            if matches!(
                entry.policy().auth(),
                AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup
            ) || restriction_allows(restriction, entry.method(), entry.path())
            {
                continue;
            }
            let path = concrete(entry.path(), &self.clock);
            let mut call = Call::new(entry.method().clone(), &path, creds);
            if entry.policy().request_content() != crate::infra::http::csrf::RequestContent::Json {
                call.content_type = None;
            }
            let fetched = self.call(call, 40).await;
            if *entry.method() == Method::HEAD {
                assert_eq!(
                    (fetched.status, fetched.body.len()),
                    (StatusCode::FORBIDDEN, 0),
                    "HEAD {}: a HEAD response is refused without a body",
                    entry.path()
                );
                denied += 1;
                continue;
            }
            assert_eq!(
                (fetched.status, fetched.error_code()),
                (StatusCode::FORBIDDEN, code.to_owned()),
                "{} {}: {}",
                entry.method(),
                entry.path(),
                fetched.text()
            );
            denied += 1;
        }
        denied
    }

    async fn fs_snapshot(&mut self, user: UserId) -> Snapshot {
        self.flush_audit().await;
        (
            self.security_state(user).await,
            self.sessions_of(user).await,
            self.devices_of(user).await,
            self.scalar_i64("SELECT COUNT(*) FROM audit_events").await,
        )
    }

    async fn fs_make_admin(&self, user: UserId) {
        self.execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{user}'"
        ))
        .await;
    }

    async fn fs_policy(&self, required: bool) {
        self.setting(
            "two_factor_required",
            "boolean",
            if required { "true" } else { "false" },
        )
        .await;
    }
}

#[tokio::test]
async fn it_forced_password_change_blocks_app() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack.fs_make_admin(ada).await;
    let (mut stack, temporary) = stack.fs_reset(root.path(), ada).await;

    let unknown = stack.login("nobody", &temporary, 10).await;
    let wrong = stack.login("ada", WRONG, 10).await;
    assert_code(&wrong, StatusCode::UNAUTHORIZED, "AUTH_INVALID_CREDENTIALS");
    assert_eq!(
        wrong.error_without_request_id(),
        unknown.error_without_request_id()
    );
    assert!(wrong.set_cookies().is_empty());
    assert!(!wrong.text().contains("mustChangePassword"));
    assert!(!wrong.text().contains("must_change_password"));

    let login = stack.login("ada", &temporary, 11).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    assert_eq!(login.json()["mustChangePassword"], true);
    assert_eq!(login.json()["mfaEnrollmentRequired"], false);
    let restricted = Credentials::from(&login);
    let elsewhere = Credentials::from(&stack.login("ada", &temporary, 12).await);
    stack.trusted_device(ada, "device-forced", None).await;

    let me = stack.fs_me(&restricted).await;
    assert_eq!(me["restriction"], "must_change_password");
    assert_eq!(me["capabilities"]["hasLocalPassword"], true);
    let session_id = me["session"]["id"].clone();
    for path in [EFFECTIVE, BOOTSTRAP] {
        let allowed = stack.get(path, Some(&restricted.session), 11).await;
        assert_eq!(allowed.status, StatusCode::OK, "{path}: {}", allowed.text());
    }

    let before = stack.fs_snapshot(ada).await;
    let denied = stack
        .fs_sweep(
            &restricted,
            SessionRestriction::MustChangePassword,
            "AUTH_PASSWORD_CHANGE_REQUIRED",
        )
        .await;
    assert!(denied > 10, "{denied}");
    assert_eq!(
        stack.fs_snapshot(ada).await,
        before,
        "a denied request never reaches its handler"
    );

    clock.advance(RECENT_AUTH_LAPSE);
    let short = stack.fs_forced_change(&restricted, "short", 11).await;
    assert_code(
        &short,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_eq!(stack.fs_snapshot(ada).await.0, before.0);

    let changed = stack.fs_forced_change(&restricted, REPLACEMENT, 11).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let rotated = Credentials::from(&changed);
    assert_ne!(rotated.session, restricted.session);
    for stale in [&restricted, &elsewhere] {
        assert_code(
            &stack.get(ME, Some(&stale.session), 11).await,
            StatusCode::UNAUTHORIZED,
            "AUTH_REQUIRED",
        );
    }
    let me = stack.fs_me(&rotated).await;
    assert_eq!(me["restriction"], Value::Null);
    assert_eq!(
        me["session"]["id"], session_id,
        "the current session row is rotated, not replaced"
    );
    for path in [PROFILE, SESSIONS] {
        let allowed = stack.get(path, Some(&rotated.session), 11).await;
        assert_eq!(allowed.status, StatusCode::OK, "{path}: {}", allowed.text());
    }

    let state = stack.security_state(ada).await;
    assert!(!state.must_change_password);
    assert!(matches!(
        verify_password(
            REPLACEMENT.as_bytes(),
            state.password_hash.as_deref().unwrap()
        )
        .unwrap(),
        PasswordVerification::Verified { .. }
    ));
    let sessions = stack.sessions_of(ada).await;
    assert_eq!(
        sessions
            .iter()
            .filter(|row| row.state == "active")
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec![session_id.as_str().unwrap()]
    );
    assert!(sessions
        .iter()
        .any(|row| row.state == "revoked"
            && row.revoked_reason.as_deref() == Some("password_changed")));
    assert!(stack
        .devices_of(ada)
        .await
        .iter()
        .all(|(_, revoked_at)| revoked_at.is_some()));
    stack.flush_audit().await;
    let audit = stack.audit_rows("PASSWORD_CHANGED").await;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].2).unwrap(),
        json!({ "forced": true, "sessions_revoked": 1, "trusted_devices_revoked": 1 })
    );

    assert_code(
        &stack.login("ada", &temporary, 13).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );
    let released = stack.login("ada", REPLACEMENT, 14).await;
    assert_eq!(released.status, StatusCode::OK, "{}", released.text());
    assert_eq!(released.json()["mustChangePassword"], false);
    stack.stop().await;
}

#[tokio::test]
async fn it_forced_password_change_current_password_exception_is_narrow() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let carol = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("carol", "carol@example.test", &hash)
        })
        .await;
    let dave = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("dave", "dave@example.test", &hash)
        })
        .await;

    let unrestricted = stack.signed_in("ada", 10).await;
    let before = stack.security_state(ada).await;
    for (body, fields) in [
        (
            json!({ "newPassword": REPLACEMENT }),
            json!(["currentPassword"]),
        ),
        (json!({}), json!(["currentPassword", "newPassword"])),
        (
            json!({ "currentPassword": null, "newPassword": REPLACEMENT }),
            json!(["currentPassword"]),
        ),
    ] {
        let refused = stack
            .call(
                Call::new(Method::POST, PASSWORD_PATH, &unrestricted).json(&body),
                10,
            )
            .await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            fields,
            "{body}"
        );
    }
    assert_eq!(stack.security_state(ada).await, before);

    let (external, restriction) = stack.fs_external_session(carol).await;
    assert_eq!(restriction, SessionRestriction::MustChangePassword);
    let omitted = stack.fs_forced_change(&external, REPLACEMENT, 11).await;
    assert_code(
        &omitted,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        omitted.json()["error"]["details"]["fields"],
        json!(["currentPassword"]),
        "a session that never proved the temporary password must present it"
    );
    assert!(stack.security_state(carol).await.must_change_password);
    let presented = stack
        .change_password(&external, PASSWORD, REPLACEMENT, 11)
        .await;
    assert_eq!(
        presented.status,
        StatusCode::NO_CONTENT,
        "{}",
        presented.text()
    );

    let forced = stack.signed_in("dave", 12).await;
    let principal = stack
        .sessions
        .authenticate(&Secret::new(forced.session.clone()))
        .await
        .unwrap();
    assert_eq!(
        principal.restriction,
        SessionRestriction::MustChangePassword
    );
    stack
        .execute(&format!(
            "UPDATE users SET must_change_password = 0 WHERE id = '{dave}'"
        ))
        .await;
    let before = stack.security_state(dave).await;
    let sessions_before = stack.sessions_of(dave).await;
    let stored = Secret::new(before.password_hash.clone().unwrap());
    let replacement = hash_password(REPLACEMENT.as_bytes()).unwrap();
    let prepared = stack.sessions.prepare_credentials().unwrap();
    let client = ClientMetadata::none();
    let committed = stack
        .pools
        .write_tx(&stack.clock, "profile.test_forced_commit", async |tx| {
            stack
                .profile
                .commit_password_change(
                    tx,
                    VerifiedChange {
                        principal: &principal,
                        proof: CurrentPasswordProof::TemporaryPasswordSession,
                        verified: &stored,
                        replacement: &replacement,
                        credentials: &prepared,
                        client: &client,
                    },
                )
                .await
        })
        .await;
    assert!(
        matches!(committed, Err(ProfileError::Invalid { ref fields }) if *fields == ["currentPassword"]),
        "the exception is re-checked against the account inside the transaction"
    );
    assert_eq!(stack.security_state(dave).await, before);
    assert_eq!(stack.sessions_of(dave).await, sessions_before);

    let lifted = stack.fs_forced_change(&forced, REPLACEMENT, 12).await;
    assert_code(
        &lifted,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        lifted.json()["error"]["details"]["fields"],
        json!(["currentPassword"])
    );
    assert_eq!(stack.security_state(dave).await, before);
    stack.stop().await;
}

#[tokio::test]
async fn it_forced_password_change_preserves_existing_totp() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let signed_in = stack.signed_in("ada", 10).await;
    let enabled = stack.tf_enable(&signed_in, 10).await;
    clock.advance(STEP);
    let secret_before = stack.tf_secret_row(ada).await;
    let backups_before = stack.tf_backup_rows(ada).await;
    assert_eq!(backups_before.len(), 10);

    let (stack, temporary) = stack.fs_reset(root.path(), ada).await;
    assert_eq!(stack.tf_secret_row(ada).await, secret_before);
    assert_eq!(stack.tf_backup_rows(ada).await, backups_before);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT totp_enabled FROM users WHERE id = '{ada}'"
            ))
            .await,
        1
    );

    let challenged = stack.login("ada", &temporary, 11).await;
    assert_code(&challenged, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    assert!(challenged.set_cookies().is_empty());
    assert!(!challenged.text().contains("mustChangePassword"));
    let token = challenged.json()["error"]["details"]["mfaToken"]
        .as_str()
        .unwrap()
        .to_owned();
    let code = code_for(&enabled.enrollment.secret, step_of(clock_now(&clock)));
    let completed = stack
        .post_json(
            LOGIN_TOTP,
            &json!({ "mfaToken": token, "code": code, "rememberDevice": true }).to_string(),
            11,
            None,
        )
        .await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(completed.json()["mustChangePassword"], true);
    let restricted = Credentials::from(&completed);
    let device = completed.cookie("palmr_device");
    assert_eq!(
        stack.fs_me(&restricted).await["restriction"],
        "must_change_password"
    );
    assert_code(
        &stack.get(PROFILE, Some(&restricted.session), 11).await,
        StatusCode::FORBIDDEN,
        "AUTH_PASSWORD_CHANGE_REQUIRED",
    );

    let device_cookie = format!("palmr_device={device}");
    let trusted = stack
        .login_with("ada", &temporary, 12, Some(&device_cookie))
        .await;
    assert_eq!(trusted.status, StatusCode::OK, "{}", trusted.text());
    assert_eq!(trusted.json()["mustChangePassword"], true);
    let trusted = Credentials::from(&trusted);
    assert_eq!(
        stack.fs_me(&trusted).await["restriction"],
        "must_change_password"
    );

    clock.advance(RECENT_AUTH_LAPSE);
    let changed = stack.fs_forced_change(&restricted, REPLACEMENT, 11).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let rotated = Credentials::from(&changed);
    assert_eq!(stack.fs_me(&rotated).await["restriction"], Value::Null);
    let status = stack.get(TWO_FACTOR, Some(&rotated.session), 11).await;
    assert_eq!(status.json()["enabled"], true);
    assert_eq!(status.json()["backupCodesRemaining"], 10);
    assert_code(
        &stack.get(ME, Some(&trusted.session), 12).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    assert!(stack
        .devices_of(ada)
        .await
        .iter()
        .all(|(_, revoked_at)| revoked_at.is_some()));
    let again = stack
        .login_with("ada", REPLACEMENT, 13, Some(&device_cookie))
        .await;
    assert_code(&again, StatusCode::UNAUTHORIZED, "AUTH_2FA_REQUIRED");
    stack.stop().await;
}

#[tokio::test]
async fn it_forced_states_password_change_precedes_enrollment() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.fs_policy(true).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("ada", "ada@example.test", &hash)
        })
        .await;

    let login = stack.login("ada", PASSWORD, 10).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    assert_eq!(login.json()["mustChangePassword"], true);
    assert_eq!(login.json()["mfaEnrollmentRequired"], false);
    let restricted = Credentials::from(&login);
    assert_eq!(
        stack.fs_me(&restricted).await["restriction"],
        "must_change_password"
    );
    assert_code(
        &stack.get(TWO_FACTOR, Some(&restricted.session), 10).await,
        StatusCode::FORBIDDEN,
        "AUTH_PASSWORD_CHANGE_REQUIRED",
    );
    assert_code(
        &stack.fs_post(ENROLL, &restricted, None).await,
        StatusCode::FORBIDDEN,
        "AUTH_PASSWORD_CHANGE_REQUIRED",
    );

    let changed = stack.fs_forced_change(&restricted, REPLACEMENT, 10).await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let enrolling = Credentials::from(&changed);
    assert!(!stack.security_state(ada).await.must_change_password);
    assert_eq!(
        stack.fs_me(&enrolling).await["restriction"],
        "mfa_enrollment_required",
        "the next restriction applies to the same session without a new sign-in"
    );
    for path in [PROFILE, SESSIONS] {
        assert_code(
            &stack.get(path, Some(&enrolling.session), 10).await,
            StatusCode::FORBIDDEN,
            "AUTH_2FA_ENROLLMENT_REQUIRED",
        );
    }
    assert_code(
        &stack
            .fs_forced_change(&enrolling, "another passphrase", 10)
            .await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );

    clock.advance(RECENT_AUTH_LAPSE);
    let status = stack.get(TWO_FACTOR, Some(&enrolling.session), 10).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.text());
    assert_eq!(status.json()["requiredByPolicy"], true);
    assert_eq!(status.json()["enabled"], false);
    let enrollment = stack.tf_enroll(&enrolling, 10).await;
    assert_eq!(
        stack.fs_me(&enrolling).await["restriction"],
        "mfa_enrollment_required",
        "a pending enrollment does not satisfy the policy"
    );
    assert_code(
        &stack.get(PROFILE, Some(&enrolling.session), 10).await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );
    let code = code_for(&enrollment.secret, step_of(clock_now(&clock)));
    let verified = stack.tf_verify(&enrolling, &enrollment.id, &code, 10).await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    let released = Credentials::from(&verified);
    assert_code(
        &stack.get(ME, Some(&enrolling.session), 10).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    assert_eq!(stack.fs_me(&released).await["restriction"], Value::Null);
    for path in [PROFILE, SESSIONS] {
        let allowed = stack.get(path, Some(&released.session), 10).await;
        assert_eq!(allowed.status, StatusCode::OK, "{path}: {}", allowed.text());
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_mandatory_2fa_excludes_sso_only() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.fs_policy(true).await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    stack.fs_link_identity(sso, "sso-subject").await;
    let hybrid = stack
        .user(UserSpec::local("hybrid", "hybrid@example.test", &hash))
        .await;
    stack.fs_link_identity(hybrid, "hybrid-subject").await;
    stack
        .user(UserSpec::local("local", "local@example.test", &hash))
        .await;

    let (sso_session, restriction) = stack.fs_external_session(sso).await;
    assert_eq!(restriction, SessionRestriction::None);
    let me = stack.fs_me(&sso_session).await;
    assert_eq!(me["restriction"], Value::Null);
    assert_eq!(me["capabilities"]["hasLocalPassword"], false);
    assert_eq!(me["capabilities"]["identityLinkCount"], 1);
    for path in [PROFILE, SESSIONS] {
        let allowed = stack.get(path, Some(&sso_session.session), 10).await;
        assert_eq!(allowed.status, StatusCode::OK, "{path}: {}", allowed.text());
    }
    let status = stack.get(TWO_FACTOR, Some(&sso_session.session), 10).await;
    assert_eq!(status.json()["requiredByPolicy"], false);
    assert_code(
        &stack.login("sso", PASSWORD, 10).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );
    assert!(
        stack.tf_secret_row(sso).await.is_none(),
        "no local credential or TOTP is invented for an SSO-only account"
    );
    assert!(stack.security_state(sso).await.password_hash.is_none());

    let (hybrid_external, restriction) = stack.fs_external_session(hybrid).await;
    assert_eq!(restriction, SessionRestriction::MustEnrollTotp);
    assert_eq!(
        stack.fs_me(&hybrid_external).await["restriction"],
        "mfa_enrollment_required"
    );
    assert_code(
        &stack.get(PROFILE, Some(&hybrid_external.session), 11).await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );
    let hybrid_login = stack.login("hybrid", PASSWORD, 11).await;
    assert_eq!(
        hybrid_login.status,
        StatusCode::OK,
        "{}",
        hybrid_login.text()
    );
    assert_eq!(hybrid_login.json()["mfaEnrollmentRequired"], true);

    let local_login = stack.login("local", PASSWORD, 12).await;
    assert_eq!(local_login.status, StatusCode::OK, "{}", local_login.text());
    assert_eq!(local_login.json()["mfaEnrollmentRequired"], true);
    let local = Credentials::from(&local_login);
    assert_code(
        &stack.get(PROFILE, Some(&local.session), 12).await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );

    let enabled = stack.tf_enable(&Credentials::from(&hybrid_login), 11).await;
    assert_eq!(
        stack.fs_me(&enabled.credentials).await["restriction"],
        Value::Null
    );
    let (hybrid_again, restriction) = stack.fs_external_session(hybrid).await;
    assert_eq!(
        restriction,
        SessionRestriction::None,
        "external sign-in is neither challenged nor restricted once TOTP is active"
    );
    assert_eq!(
        stack
            .get(PROFILE, Some(&hybrid_again.session), 11)
            .await
            .status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_mandatory_2fa_policy_applies_on_next_request() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let ada = stack.signed_in("ada", 10).await;
    let bob = stack.signed_in("bob", 11).await;
    let bob = stack.tf_enable(&bob, 11).await.credentials;
    let ada_session = stack.fs_me(&ada).await["session"]["id"].clone();
    for creds in [&ada, &bob] {
        assert_eq!(stack.fs_me(creds).await["restriction"], Value::Null);
        assert_eq!(
            stack.get(PROFILE, Some(&creds.session), 10).await.status,
            StatusCode::OK
        );
    }

    stack.fs_policy(true).await;
    let me = stack.fs_me(&ada).await;
    assert_eq!(me["restriction"], "mfa_enrollment_required");
    assert_eq!(me["session"]["id"], ada_session);
    assert_code(
        &stack.get(PROFILE, Some(&ada.session), 10).await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );
    assert_eq!(stack.fs_me(&bob).await["restriction"], Value::Null);
    assert_eq!(
        stack.get(PROFILE, Some(&bob.session), 11).await.status,
        StatusCode::OK,
        "an active TOTP enrollment satisfies the policy"
    );
    let bob_status = stack.get(TWO_FACTOR, Some(&bob.session), 11).await;
    assert_eq!(bob_status.json()["requiredByPolicy"], true);
    assert_eq!(bob_status.json()["canDisable"], false);

    stack.fs_policy(false).await;
    assert_eq!(stack.fs_me(&ada).await["restriction"], Value::Null);
    assert_eq!(
        stack.get(PROFILE, Some(&ada.session), 10).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_mandatory_enrollment_waives_recent_auth_only_while_restricted() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let user = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let ada = stack.signed_in("ada", 10).await;
    clock.advance(RECENT_AUTH_LAPSE);

    for path in [ENROLL, VERIFY] {
        let body = (path == VERIFY).then(|| json!({ "enrollmentId": "x", "code": "000000" }));
        let refused = stack.fs_post(path, &ada, body).await;
        assert_code(&refused, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    }
    assert!(stack.tf_secret_row(user).await.is_none());

    stack.fs_policy(true).await;
    let enrollment = stack.tf_enroll(&ada, 10).await;
    let code = code_for(&enrollment.secret, step_of(clock_now(&clock)));
    let verified = stack.tf_verify(&ada, &enrollment.id, &code, 10).await;
    assert_eq!(verified.status, StatusCode::OK, "{}", verified.text());
    let released = Credentials::from(&verified);
    assert_eq!(stack.fs_me(&released).await["restriction"], Value::Null);

    clock.advance(RECENT_AUTH_LAPSE);
    assert_code(
        &stack.fs_post(REGENERATE, &released, None).await,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_code(
        &stack.fs_post(ENROLL, &released, None).await,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    stack.stop().await;
}
