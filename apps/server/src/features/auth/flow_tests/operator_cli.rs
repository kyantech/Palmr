use std::io::Read;
use std::sync::{Mutex, PoisonError};

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use super::profile::{assert_code, Call, SecurityState, SessionRow};
use super::*;
use crate::app::lifecycle::data_dir::DataDir;
use crate::cli::admin::{
    admin_recover, user_reset_password, AdminRecovered, PasswordReset, ADMIN_RECOVER_ACTOR,
    PASSWORD_RESET_ACTOR,
};
use crate::cli::error::CliError;
use crate::config::LogFormat;
use crate::features::audit::actions::{AdminRecoverFacts, PasswordResetFacts};
use crate::features::auth::login::password_login_enabled;
use crate::infra::db::InstanceLock;
use crate::infra::telemetry::build_dispatch;

const SESSIONS: &str = "/api/v1/sessions";
const PASSWORD_PATH: &str = "/api/v1/profile/password";
const REPLACEMENT: &str = "a password the user chose";
const LOCKED_UNTIL: &str = "2026-09-25T12:10:00.000Z";
const TEMPORARY_PREFIX: &str = "Temporary password: ";
const FILE_READ_LIMIT: u64 = 64 * 1024 * 1024;

type AuditRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

type RecoverySnapshot = (
    SecurityState,
    Vec<SessionRow>,
    Vec<(String, Option<String>)>,
    (i64, Option<String>, i64, Option<String>),
);

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
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

fn operator_config(root: &Path) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_DATA_DIR", root.to_str().unwrap()),
        ("PALMR_BASE_URL", BASE_URL),
    ]))
    .unwrap()
    .config
}

async fn run_recover(
    root: &Path,
    clock: &TestClock,
    selector: &str,
) -> (Result<AdminRecovered, CliError>, String) {
    let mut out = Vec::new();
    let outcome = admin_recover(
        &operator_config(root),
        selector,
        Arc::new(clock.clone()),
        &mut out,
    )
    .await;
    (outcome, String::from_utf8(out).unwrap())
}

async fn run_reset(
    root: &Path,
    clock: &TestClock,
    id: UserId,
) -> (Result<PasswordReset, CliError>, String) {
    let mut out = Vec::new();
    let outcome = user_reset_password(
        &operator_config(root),
        id,
        Arc::new(clock.clone()),
        &mut out,
    )
    .await;
    (outcome, String::from_utf8(out).unwrap())
}

fn temporary_password(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3, "{output:?}");
    let password = lines[1]
        .strip_prefix(TEMPORARY_PREFIX)
        .expect("the second line carries the temporary password")
        .to_owned();
    assert!(!password.is_empty());
    assert_eq!(
        output.matches(password.as_str()).count(),
        1,
        "the temporary password is printed exactly once"
    );
    password
}

fn verifies(stored: Option<&str>, password: &str) -> bool {
    matches!(
        verify_password(password.as_bytes(), stored.unwrap()).unwrap(),
        PasswordVerification::Verified { .. }
    )
}

fn read_bounded(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        file.take(FILE_READ_LIMIT).read_to_end(&mut bytes).unwrap();
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

impl Stack {
    async fn lock_account(&self, user: UserId) {
        self.execute(&format!(
            "INSERT INTO account_lockouts (user_id, failed_count, first_failed_at, last_failed_at,
                                           locked_until, lock_count, updated_at)
             VALUES ('{user}', 5, '2026-09-25T11:59:00.000Z', '2026-09-25T12:00:00.000Z',
                     '{LOCKED_UNTIL}', 1, '2026-09-25T12:00:00.000Z')
             ON CONFLICT (user_id) DO UPDATE SET failed_count = 5,
                 locked_until = '{LOCKED_UNTIL}', lock_count = lock_count + 1"
        ))
        .await;
    }

    async fn lockout_of(&self, user: UserId) -> (i64, Option<String>, i64, Option<String>) {
        sqlx::query_as(
            "SELECT failed_count, locked_until, lock_count, cleared_by
               FROM account_lockouts WHERE user_id = ?1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn operator_audit(&self, action: &str) -> Vec<AuditRow> {
        sqlx::query_as(
            "SELECT actor_type, actor_user_id, actor_label, target_type, target_id, result,
                    error_code, client_ip, metadata_json
               FROM audit_events WHERE action = ?1 ORDER BY id",
        )
        .bind(action)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn all_audit_text(&self) -> String {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT action || '|' || actor_type || '|' || COALESCE(actor_label, '') || '|' ||
                    COALESCE(target_label, '') || '|' || COALESCE(error_code, '') || '|' ||
                    metadata_json
               FROM audit_events",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(row,)| row)
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn recovery_snapshot(&self, user: UserId) -> RecoverySnapshot {
        (
            self.security_state(user).await,
            self.sessions_of(user).await,
            self.devices_of(user).await,
            self.lockout_of(user).await,
        )
    }

    async fn count(&self, sql: &str) -> i64 {
        self.scalar_i64(sql).await
    }

    async fn deactivation(&self, user: UserId) -> (Option<String>, Option<String>) {
        sqlx::query_as("SELECT deactivated_at, deactivated_by FROM users WHERE id = ?1")
            .bind(user.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }
}

fn assert_operator_row(row: &AuditRow, actor: &str, target: UserId) -> Value {
    let (actor_type, actor_user_id, label, target_type, target_id, result, error, ip, metadata) =
        row;
    assert_eq!(actor_type, "operator_cli");
    assert_eq!(actor_user_id, &None);
    assert_eq!(label.as_deref(), Some(actor));
    assert_eq!(target_type.as_deref(), Some("user"));
    assert_eq!(target_id.as_deref(), Some(target.to_string().as_str()));
    assert_eq!(result, "success");
    assert_eq!(error, &None);
    assert_eq!(ip, &None);
    serde_json::from_str(metadata).unwrap()
}

fn assert_all_revoked(rows: &[SessionRow], reason: &str) {
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(row.state, "revoked", "{row:?}");
        assert_eq!(row.revoked_reason.as_deref(), Some(reason), "{row:?}");
    }
}

#[tokio::test]
async fn it_cli_admin_recover_promotes_and_unlocks() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let grace = stack
        .user(UserSpec::local("grace", "grace@example.test", &hash))
        .await;
    let root_admin = stack
        .user(UserSpec::local("root", "root@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{root_admin}'"
        ))
        .await;
    let stale = [
        stack.signed_in("grace", 10).await,
        stack.signed_in("grace@example.test", 11).await,
    ];
    let admin_session = stack.signed_in("root", 12).await;
    stack.trusted_device(grace, "device-grace-1", None).await;
    stack
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z',
                              deactivated_by = '{root_admin}'
              WHERE id = '{grace}'"
        ))
        .await;
    stack.lock_account(grace).await;
    let users_before = stack.count("SELECT COUNT(*) FROM users").await;
    stack.stop().await;

    let (outcome, output) = run_recover(root.path(), &clock, "  GRACE@Example.TEST ").await;
    let recovered = outcome.unwrap();
    let promoted = AdminRecoverFacts {
        role_changed: true,
        activated: true,
        lockout_cleared: true,
        password_login_reenabled: false,
        sessions_revoked: 2,
    };
    assert_eq!(
        recovered,
        AdminRecovered {
            user: grace,
            username: "grace".to_owned(),
            facts: promoted,
        }
    );
    assert_eq!(output.lines().count(), 1, "{output:?}");
    assert!(
        output.starts_with(&format!(
            "Admin recovery completed for user {grace} (grace)"
        )),
        "{output:?}"
    );

    let stack = Stack::start(root.path(), &clock).await;
    let state = stack.security_state(grace).await;
    assert_eq!(state.role, "admin");
    assert!(state.is_active);
    assert!(!state.must_change_password);
    assert_eq!(stack.deactivation(grace).await, (None, None));
    assert_eq!(
        stack.lockout_of(grace).await,
        (0, None, 1, None),
        "the lockout is cleared by the operator, not by an application user"
    );
    assert!(password_login_enabled(&stack.settings.handle().load()));
    assert_all_revoked(&stack.sessions_of(grace).await, "role_changed");
    assert_eq!(stack.sessions_of(root_admin).await[0].state, "active");
    assert_eq!(
        stack.devices_of(grace).await,
        vec![("device-grace-1".to_owned(), None)],
        "promotion alone does not revoke trusted devices"
    );

    let audit = stack.operator_audit("OPERATOR_CLI_ADMIN_RECOVER").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(
        assert_operator_row(&audit[0], ADMIN_RECOVER_ACTOR, grace),
        json!({
            "role_changed": true,
            "activated": true,
            "lockout_cleared": true,
            "password_login_reenabled": false,
            "sessions_revoked": 2,
        })
    );

    for old in &stale {
        assert_eq!(
            stack.get(ME, Some(&old.session), 20).await.status,
            StatusCode::UNAUTHORIZED,
            "a session minted under the old role never carries Admin authority"
        );
    }
    let fresh = stack.signed_in("grace", 21).await;
    let me = stack.get(ME, Some(&fresh.session), 22).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.json()["user"]["role"], "admin");
    assert_eq!(me.json()["restriction"], Value::Null);
    stack.stop().await;

    let before = {
        let stack = Stack::start(root.path(), &clock).await;
        let snapshot = (
            stack.security_state(root_admin).await,
            stack.sessions_of(root_admin).await,
        );
        stack.stop().await;
        snapshot
    };
    for selector in [root_admin.to_string(), "ROOT".to_owned()] {
        let (outcome, output) = run_recover(root.path(), &clock, &selector).await;
        assert_eq!(
            outcome.unwrap().facts,
            AdminRecoverFacts::default(),
            "an active Admin is recovered idempotently"
        );
        assert_eq!(output.lines().count(), 1, "{output:?}");
    }
    let stack = Stack::start(root.path(), &clock).await;
    assert_eq!(
        (
            stack.security_state(root_admin).await,
            stack.sessions_of(root_admin).await,
        ),
        before
    );
    assert_eq!(
        stack.get(ME, Some(&admin_session.session), 23).await.status,
        StatusCode::OK
    );
    assert_eq!(
        stack.count("SELECT COUNT(*) FROM users").await,
        users_before
    );
    assert_eq!(
        stack.count("SELECT COUNT(*) FROM account_lockouts").await,
        1
    );
    let audit = stack.operator_audit("OPERATOR_CLI_ADMIN_RECOVER").await;
    assert_eq!(audit.len(), 3);
    for row in &audit[1..] {
        assert_eq!(
            assert_operator_row(row, ADMIN_RECOVER_ACTOR, root_admin)["role_changed"],
            false
        );
    }
    stack.stop().await;

    for unknown in [
        "nobody@example.test",
        "   ",
        "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d",
    ] {
        let (outcome, output) = run_recover(root.path(), &clock, unknown).await;
        let error = outcome.unwrap_err();
        assert!(matches!(&error, CliError::UserNotFound { .. }), "{error:?}");
        assert_eq!(error.exit_code(), 67);
        assert!(output.is_empty());
    }
    let stack = Stack::start(root.path(), &clock).await;
    assert_eq!(
        stack.count("SELECT COUNT(*) FROM users").await,
        users_before
    );
    assert_eq!(
        stack
            .operator_audit("OPERATOR_CLI_ADMIN_RECOVER")
            .await
            .len(),
        3
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_cli_user_reset_password_forces_change() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.setting("password_min_length", "integer", "64").await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let bob = stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let stale = [
        stack.signed_in("ada", 10).await,
        stack.signed_in("ada", 11).await,
        stack.signed_in("ada@example.test", 12).await,
    ];
    let bystander = stack.signed_in("bob", 13).await;
    stack.trusted_device(ada, "device-ada-1", None).await;
    stack.trusted_device(ada, "device-ada-2", None).await;
    stack
        .trusted_device(ada, "device-ada-3", Some("2026-09-20T00:00:00.000Z"))
        .await;
    stack.trusted_device(bob, "device-bob-1", None).await;
    assert_eq!(
        stack.login("ada", WRONG, 14).await.status,
        StatusCode::UNAUTHORIZED
    );
    stack.lock_account(ada).await;
    let attempts_before = stack.count("SELECT COUNT(*) FROM login_attempts").await;
    let old_hash = stack.security_state(ada).await.password_hash;
    stack.stop().await;
    clock.advance(Duration::from_secs(60));

    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let (outcome, output) = {
        let _guard = tracing::dispatcher::set_default(&dispatch);
        run_reset(root.path(), &clock, ada).await
    };
    let reset = outcome.unwrap();
    assert_eq!(
        reset.facts,
        PasswordResetFacts {
            had_local_password: true,
            sessions_revoked: 3,
            trusted_devices_revoked: 2,
            lockout_cleared: true,
        }
    );
    assert!(output.starts_with(&format!(
        "Password reset completed for user {ada} (ada): revoked 3 session(s) and 2 trusted device(s); lockout cleared: yes.\n"
    )));
    assert!(output.ends_with("\nThe user must change this password at the next login.\n"));
    let temporary = temporary_password(&output);
    assert_eq!(
        temporary.len(),
        64,
        "the configured minimum length is honoured"
    );
    for rendered in [format!("{reset:?}"), format!("{reset:#?}"), capture.text()] {
        assert!(!rendered.contains(&temporary), "{rendered}");
        assert!(!rendered.contains("$argon2"), "{rendered}");
    }

    let stack = Stack::start(root.path(), &clock).await;
    let state = stack.security_state(ada).await;
    assert!(state.must_change_password);
    assert_ne!(state.password_hash, old_hash);
    assert!(state
        .password_hash
        .as_deref()
        .unwrap()
        .starts_with("$argon2id$"));
    assert!(verifies(state.password_hash.as_deref(), &temporary));
    assert!(!verifies(state.password_hash.as_deref(), PASSWORD));
    assert_eq!(
        state.password_updated_at.as_deref(),
        Some("2026-09-25T12:01:00.000Z")
    );
    assert_all_revoked(&stack.sessions_of(ada).await, "password_reset");
    assert_eq!(stack.sessions_of(bob).await[0].state, "active");
    let devices = stack.devices_of(ada).await;
    assert_eq!(devices.len(), 3);
    assert!(devices.iter().all(|(_, revoked_at)| revoked_at.is_some()));
    assert_eq!(devices[2].1.as_deref(), Some("2026-09-20T00:00:00.000Z"));
    assert_eq!(
        stack.devices_of(bob).await,
        vec![("device-bob-1".to_owned(), None)]
    );
    assert_eq!(stack.lockout_of(ada).await, (0, None, 1, None));
    assert_eq!(
        stack.count("SELECT COUNT(*) FROM login_attempts").await,
        attempts_before,
        "login-attempt forensic history is kept"
    );

    let audit = stack.operator_audit("OPERATOR_CLI_PASSWORD_RESET").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(
        assert_operator_row(&audit[0], PASSWORD_RESET_ACTOR, ada),
        json!({
            "had_local_password": true,
            "sessions_revoked": 3,
            "trusted_devices_revoked": 2,
            "lockout_cleared": true,
        })
    );
    let audit_text = stack.all_audit_text().await;
    assert!(!audit_text.contains(&temporary));
    assert!(!audit_text.contains("$argon2"));

    for old in &stale {
        assert_eq!(
            stack.get(ME, Some(&old.session), 20).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        stack.get(ME, Some(&bystander.session), 21).await.status,
        StatusCode::OK
    );
    assert_code(
        &stack.login("ada", PASSWORD, 22).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );

    let restricted_login = stack.login("ada", &temporary, 23).await;
    assert_eq!(
        restricted_login.status,
        StatusCode::OK,
        "{}",
        restricted_login.text()
    );
    assert_eq!(restricted_login.json()["mustChangePassword"], true);
    let restricted = Credentials::from(&restricted_login);
    assert_eq!(
        stack.get(ME, Some(&restricted.session), 24).await.json()["restriction"],
        "must_change_password"
    );
    assert_code(
        &stack.get(SESSIONS, Some(&restricted.session), 25).await,
        StatusCode::FORBIDDEN,
        "AUTH_PASSWORD_CHANGE_REQUIRED",
    );

    let changed = stack
        .call(
            Call::new(Method::POST, PASSWORD_PATH, &restricted).json(&json!({
                "currentPassword": temporary,
                "newPassword": format!("{REPLACEMENT} {}", "x".repeat(64)),
            })),
            26,
        )
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let released = stack
        .login("ada", &format!("{REPLACEMENT} {}", "x".repeat(64)), 27)
        .await;
    assert_eq!(released.status, StatusCode::OK, "{}", released.text());
    assert_eq!(released.json()["mustChangePassword"], false);
    stack.stop().await;

    for file in ["palmr.db", "palmr.db-wal"] {
        let bytes = read_bounded(&root.path().join(file));
        assert!(
            !contains(&bytes, &temporary),
            "the temporary password is never persisted in {file}"
        );
    }
}

#[tokio::test]
async fn it_cli_user_reset_password_establishes_local_credential() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    assert_eq!(stack.security_state(sso).await.password_hash, None);
    stack.stop().await;

    let (outcome, output) = run_reset(root.path(), &clock, sso).await;
    assert!(!outcome.unwrap().facts.had_local_password);
    let temporary = temporary_password(&output);
    assert_eq!(temporary.len(), 43);

    let stack = Stack::start(root.path(), &clock).await;
    let state = stack.security_state(sso).await;
    assert!(state.must_change_password);
    assert!(verifies(state.password_hash.as_deref(), &temporary));
    assert_eq!(
        assert_operator_row(
            &stack.operator_audit("OPERATOR_CLI_PASSWORD_RESET").await[0],
            PASSWORD_RESET_ACTOR,
            sso,
        )["had_local_password"],
        false
    );
    let login = stack.login("sso@example.test", &temporary, 10).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    assert_eq!(login.json()["mustChangePassword"], true);
    let me = stack
        .get(ME, Some(&Credentials::from(&login).session), 11)
        .await;
    assert_eq!(me.json()["restriction"], "must_change_password");
    assert_eq!(me.json()["capabilities"]["hasLocalPassword"], true);
    stack.stop().await;
}

#[tokio::test]
async fn it_cli_recovery_rollback_prints_no_credential() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let live = stack.signed_in("ada", 10).await;
    stack.trusted_device(ada, "device-ada-1", None).await;
    stack.lock_account(ada).await;
    stack
        .execute(
            "CREATE TRIGGER inject_operator_audit_failure BEFORE INSERT ON audit_events
              WHEN NEW.action LIKE 'OPERATOR_CLI_%'
              BEGIN SELECT RAISE(ABORT, 'injected operator audit failure'); END",
        )
        .await;
    let before = stack.recovery_snapshot(ada).await;
    stack.stop().await;

    let (outcome, output) = run_reset(root.path(), &clock, ada).await;
    let error = outcome.unwrap_err();
    assert!(
        matches!(&error, CliError::RecoveryFailed { .. }),
        "{error:?}"
    );
    assert_eq!(error.exit_code(), 1);
    assert!(error
        .to_string()
        .contains("injected operator audit failure"));
    assert!(
        output.is_empty(),
        "a rolled-back reset never prints a password: {output:?}"
    );

    let (outcome, output) = run_recover(root.path(), &clock, "ada").await;
    assert!(matches!(
        outcome.unwrap_err(),
        CliError::RecoveryFailed { .. }
    ));
    assert!(output.is_empty());

    let stack = Stack::start(root.path(), &clock).await;
    assert_eq!(stack.recovery_snapshot(ada).await, before);
    assert_eq!(before.0.role, "user");
    assert_eq!(
        stack
            .count("SELECT COUNT(*) FROM audit_events WHERE action LIKE 'OPERATOR_CLI_%'")
            .await,
        0
    );
    assert_eq!(
        stack.get(ME, Some(&live.session), 11).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_cli_recovery_refuses_while_instance_lock_held() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let live = stack.signed_in("ada", 10).await;
    stack.lock_account(ada).await;
    let before = (
        stack.security_state(ada).await,
        stack.sessions_of(ada).await,
        stack.lockout_of(ada).await,
    );
    stack.stop().await;

    let data_dir = DataDir::prepare(root.path()).unwrap();
    let (server, _) = InstanceLock::acquire(data_dir.root(), &clock).unwrap();
    let (recovered, recover_output) = run_recover(root.path(), &clock, "ada").await;
    let (reset, reset_output) = run_reset(root.path(), &clock, ada).await;
    for error in [recovered.unwrap_err(), reset.unwrap_err()] {
        assert!(
            matches!(
                &error,
                CliError::DataDirInUse {
                    allows_concurrent: false,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(error.exit_code(), 78);
        assert!(!error.to_string().contains("--allow-concurrent"));
    }
    assert!(recover_output.is_empty());
    assert!(reset_output.is_empty());
    server.release().unwrap();

    let stack = Stack::start(root.path(), &clock).await;
    assert_eq!(
        (
            stack.security_state(ada).await,
            stack.sessions_of(ada).await,
            stack.lockout_of(ada).await,
        ),
        before
    );
    assert_eq!(
        stack
            .count("SELECT COUNT(*) FROM audit_events WHERE action LIKE 'OPERATOR_CLI_%'")
            .await,
        0
    );
    assert_eq!(
        stack.get(ME, Some(&live.session), 11).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}
