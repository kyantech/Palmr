use super::link::Member;
use super::*;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::{AuthMethod, AuthenticatedPrincipal, SessionRestriction};
use crate::features::identity_providers::callback::ExternalLoginService;
use crate::features::identity_providers::input::{UpdateInput, UpdateProviderRequest};
use crate::features::identity_providers::link::{UnlinkCommand, UnlinkScope};
use crate::features::identity_providers::model::{IdentityLinkId, ProviderId};
use crate::features::identity_providers::password_login::{
    assert_safe_sso_after_change, Projection, SsoGuardError,
};

const PASSWORD_LOGIN: &str = "/api/v1/admin/auth/password-login";
const SECURITY_SETTINGS: &str = "/api/v1/admin/settings/security";
const PROVIDER_ADMIN: &str = "/api/v1/admin/providers";
const USERS: &str = "/api/v1/admin/users";
const BOOTSTRAP: &str = "/api/v1/bootstrap";
const UNSAFE: &str = "PASSWORD_LOGIN_DISABLE_UNSAFE";

const PROVIDERS_OFF: (&str, &str) = (
    UNSAFE,
    "External identity providers are disabled for this instance",
);
const NO_ENABLED: (&str, &str) = ("NO_VALIDATED_PROVIDER", "No identity provider is enabled");
const NO_VALIDATED: (&str, &str) = (
    "NO_VALIDATED_PROVIDER",
    "No enabled provider has been successfully tested",
);
const NO_LINKED_ADMIN: (&str, &str) = (
    UNSAFE,
    "No active Administrator is linked to a validated provider",
);
const ACTOR_NOT_LINKED: (&str, &str) = (
    UNSAFE,
    "The acting Administrator is not linked to a validated provider",
);

const USABLE_PATHS: &str = "SELECT COUNT(*)
    FROM identity_links l
    JOIN identity_providers p ON p.id = l.provider_id
    JOIN users u ON u.id = l.user_id
    WHERE l.state = 'active' AND p.is_enabled = 1 AND u.role = 'admin' AND u.is_active = 1";

const FINGERPRINT: &str = "SELECT
    (SELECT COUNT(*) FROM audit_events) || '|' ||
    (SELECT COUNT(*) FROM sessions WHERE state <> 'active' OR revoked_at IS NOT NULL) || '|' ||
    (SELECT COUNT(*) FROM trusted_devices WHERE revoked_at IS NOT NULL) || '|' ||
    (SELECT COALESCE(group_concat(id || state, ','), '')
       FROM (SELECT id, state FROM identity_links ORDER BY id)) || '|' ||
    (SELECT COALESCE(group_concat(id || role || is_active, ','), '')
       FROM (SELECT id, role, is_active FROM users ORDER BY id)) || '|' ||
    (SELECT COALESCE(group_concat(id || is_enabled || COALESCE(validated_at, '-')
                                  || COALESCE(validation_error, '-') || client_id || updated_at, ','), '')
       FROM (SELECT * FROM identity_providers ORDER BY id)) || '|' ||
    (SELECT group_concat(key || value_json, ',')
       FROM (SELECT key, value_json FROM app_settings ORDER BY key))";

impl Federation {
    async fn root_id(&self) -> UserId {
        self.stack.operator_id("root").await.parse().unwrap()
    }

    async fn admin_member(&self, username: &str) -> Member {
        let email = format!("{username}@example.test");
        let id = self
            .stack
            .user(UserSpec::local(username, &email, &password_hash()))
            .await;
        self.stack
            .execute(&format!(
                "UPDATE users SET role = 'admin' WHERE id = '{id}'"
            ))
            .await;
        let creds = self.stack.signed_in(username, self.next_peer()).await;
        Member { id, creds }
    }

    fn principal(&self, id: UserId, username: &str) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            user_id: id,
            username: username.to_owned(),
            session_id: crate::domain::id::Id::generate(&self.stack.clock),
            role: Role::Admin,
            restriction: SessionRestriction::None,
            last_auth_at: Timestamp::try_from(self.stack.clock.now()).unwrap(),
            recent_auth: true,
            auth_method: AuthMethod::Password,
        }
    }

    async fn put_password_login(&self, creds: &Credentials, body: Value) -> Fetched {
        self.stack
            .call(
                Call::new(Method::PUT, PASSWORD_LOGIN, creds).json(&body),
                self.next_peer(),
            )
            .await
    }

    async fn set_password_login(&self, creds: &Credentials, enabled: bool) -> Fetched {
        self.put_password_login(creds, json!({ "enabled": enabled, "confirm": true }))
            .await
    }

    async fn password_login_state(&self, creds: &Credentials) -> Value {
        let fetched = self.send_as(Method::GET, PASSWORD_LOGIN, creds).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn bootstrap(&self) -> Value {
        let fetched = self.stack.get(BOOTSTRAP, None, self.next_peer()).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn password_login_row(&self) -> String {
        sqlx::query_scalar(
            "SELECT value_json FROM app_settings WHERE key = 'password_login_enabled'",
        )
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    fn snapshot_password_login(&self) -> bool {
        self.stack
            .settings
            .current()
            .security
            .password_login_enabled
    }

    fn stamp(&self, age_seconds: i64) -> String {
        Timestamp::try_from(self.stack.clock.now() - time::Duration::seconds(age_seconds))
            .unwrap()
            .to_string()
    }

    async fn mark_validated(&self, slug: &str, age_seconds: i64) {
        let at = self.stamp(age_seconds);
        self.stack
            .execute(&format!(
                "UPDATE identity_providers SET validated_at = '{at}', validation_error = NULL
                  WHERE key = '{slug}'"
            ))
            .await;
    }

    async fn set_validation(&self, slug: &str, validated_at: Option<i64>, error: Option<&str>) {
        let at =
            validated_at.map_or_else(|| "NULL".to_owned(), |age| format!("'{}'", self.stamp(age)));
        let error = error.map_or_else(|| "NULL".to_owned(), |text| format!("'{text}'"));
        self.stack
            .execute(&format!(
                "UPDATE identity_providers SET validated_at = {at}, validation_error = {error}
                  WHERE key = '{slug}'"
            ))
            .await;
    }

    async fn set_flag(&self, key: &str, value: bool) {
        self.stack
            .execute(&format!(
                "UPDATE app_settings SET value_json = '{value}' WHERE key = '{key}'"
            ))
            .await;
        self.stack.settings.reload().await.unwrap();
    }

    async fn fingerprint(&self) -> String {
        sqlx::query_scalar(FINGERPRINT)
            .fetch_one(self.stack.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn usable_paths(&self) -> i64 {
        self.count(USABLE_PATHS).await
    }

    async fn audits(&self, action: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
    }

    async fn audit_metadata(&self, action: &str) -> Vec<Value> {
        let rows: Vec<Option<String>> = sqlx::query_scalar(&format!(
            "SELECT metadata_json FROM audit_events WHERE action = '{action}' ORDER BY id"
        ))
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    async fn assert_disable_refused(&self, creds: &Credentials, expected: (&str, &str)) {
        let before = self.fingerprint().await;
        let fetched = self
            .put_password_login(creds, json!({ "enabled": false, "confirm": true }))
            .await;
        assert_eq!(fetched.status, StatusCode::CONFLICT, "{}", fetched.text());
        assert_eq!(fetched.error_code(), UNSAFE);
        let blockers = fetched.json()["error"]["details"]["blockers"].clone();
        assert_eq!(
            blockers,
            json!([{ "code": expected.0, "detail": expected.1 }]),
            "{}",
            fetched.text()
        );
        assert_eq!(self.fingerprint().await, before, "{expected:?}");
        assert_eq!(self.password_login_row().await, "true");
        assert!(self.snapshot_password_login());
        assert_eq!(self.audits("PASSWORD_LOGIN_DISABLED").await, 0);
        let state = self.password_login_state(creds).await;
        assert_eq!(state["passwordLoginEnabled"], true);
        assert_eq!(state["canDisable"], false, "{expected:?}");
        assert_eq!(state["blockers"], blockers);
    }

    async fn sso_only(&self) -> (UserId, String) {
        let root = self.root_id().await;
        let corp = self.oidc("corp", json!({})).await;
        self.link("corp", root, "root-corp", "active").await;
        self.mark_validated("corp", 0).await;
        let fetched = self.set_password_login(&self.admin, false).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        assert!(!self.snapshot_password_login());
        (root, corp["id"].as_str().unwrap().to_owned())
    }

    async fn delete_provider(&self, creds: &Credentials, id: &str) -> Fetched {
        self.send_as(Method::DELETE, &format!("{PROVIDER_ADMIN}/{id}"), creds)
            .await
    }

    async fn patch_provider_body(&self, id: &str, body: Value) -> Fetched {
        self.stack.patch_provider(&self.admin, id, &body).await
    }

    async fn provider_enabled(&self, slug: &str) -> i64 {
        self.count(&format!(
            "SELECT is_enabled FROM identity_providers WHERE key = '{slug}'"
        ))
        .await
    }

    async fn provider_validation(&self, slug: &str) -> (Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT validated_at, validation_error FROM identity_providers WHERE key = ?1",
        )
        .bind(slug)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn send_as(&self, method: Method, path: &str, creds: &Credentials) -> Fetched {
        self.stack
            .call(Call::new(method, path, creds), self.next_peer())
            .await
    }

    async fn call_json(
        &self,
        method: Method,
        path: &str,
        creds: &Credentials,
        body: Value,
    ) -> Fetched {
        self.stack
            .call(Call::new(method, path, creds).json(&body), self.next_peer())
            .await
    }

    async fn assert_refused_unchanged(&self, fetched: &Fetched, before: &str, code: &str) {
        assert_eq!(fetched.status, StatusCode::CONFLICT, "{}", fetched.text());
        assert_eq!(fetched.error_code(), code, "{}", fetched.text());
        assert_eq!(self.fingerprint().await, before);
    }
}

fn bad_request_fields(fetched: &Fetched) -> Value {
    assert_eq!(
        fetched.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        fetched.text()
    );
    assert_eq!(fetched.error_code(), "VALIDATION_ERROR");
    fetched.json()["error"]["details"]["fields"].clone()
}

#[tokio::test]
async fn it_sso_only_preconditions() {
    let f = Federation::start().await;
    let root = f.root_id().await;
    let beta = f.admin_member("beta").await;
    assert_eq!(f.password_login_row().await, "true");
    assert!(f.snapshot_password_login());

    f.assert_disable_refused(&f.admin, NO_ENABLED).await;

    let corp = f.oidc("corp", json!({})).await;
    let corp_id = corp["id"].as_str().unwrap().to_owned();
    f.link("corp", root, "root-corp", "active").await;
    f.mark_validated("corp", 0).await;
    f.stack
        .patched(&f.admin, &corp_id, &json!({ "enabled": false }))
        .await;
    f.assert_disable_refused(&f.admin, NO_ENABLED).await;
    f.stack
        .patched(&f.admin, &corp_id, &json!({ "enabled": true }))
        .await;

    f.set_validation("corp", None, None).await;
    f.assert_disable_refused(&f.admin, NO_VALIDATED).await;
    f.set_validation("corp", None, Some("discovery unreachable"))
        .await;
    f.assert_disable_refused(&f.admin, NO_VALIDATED).await;
    f.set_validation("corp", Some(0), Some("left over error"))
        .await;
    f.assert_disable_refused(&f.admin, NO_VALIDATED).await;

    for (age, usable) in [
        (24 * 3600 - 1, true),
        (24 * 3600, true),
        (24 * 3600 + 1, false),
    ] {
        f.mark_validated("corp", age).await;
        let state = f.password_login_state(&f.admin).await;
        assert_eq!(state["canDisable"], usable, "validated {age}s ago");
        assert_eq!(
            state["safeAdminLoginPaths"][0]["providerValidated"], usable,
            "validated {age}s ago"
        );
    }
    f.mark_validated("corp", 24 * 3600 + 1).await;
    f.assert_disable_refused(&f.admin, NO_VALIDATED).await;

    f.mark_validated("corp", 60).await;
    f.stack.execute("DELETE FROM identity_links").await;
    f.assert_disable_refused(&f.admin, NO_LINKED_ADMIN).await;

    let ada = f.local_member("ada").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    f.assert_disable_refused(&f.admin, NO_LINKED_ADMIN).await;

    f.stack
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z'
              WHERE id = '{}'",
            beta.id
        ))
        .await;
    f.link("corp", beta.id, "beta-corp", "active").await;
    f.assert_disable_refused(&f.admin, NO_LINKED_ADMIN).await;

    f.stack.execute("DELETE FROM identity_links").await;
    f.link("corp", root, "root-corp", "suspended").await;
    f.assert_disable_refused(&f.admin, NO_LINKED_ADMIN).await;

    f.stack
        .execute(&format!(
            "UPDATE users SET is_active = 1, deactivated_at = NULL, deactivated_by = NULL
              WHERE id = '{}'",
            beta.id
        ))
        .await;
    f.link("corp", beta.id, "beta-corp", "active").await;
    f.assert_disable_refused(&f.admin, ACTOR_NOT_LINKED).await;
    let paths = f.password_login_state(&f.admin).await["safeAdminLoginPaths"].clone();
    assert_eq!(
        paths,
        json!([{
            "userId": beta.id.to_string(),
            "username": "beta",
            "providerSlug": "corp",
            "providerValidated": true
        }])
    );

    let spare = f.oidc("spare", json!({})).await;
    let spare_id = spare["id"].as_str().unwrap().to_owned();
    f.link("spare", root, "root-spare", "active").await;
    f.assert_disable_refused(&f.admin, ACTOR_NOT_LINKED).await;
    f.mark_validated("spare", 0).await;
    f.stack
        .patched(&f.admin, &spare_id, &json!({ "enabled": false }))
        .await;
    f.assert_disable_refused(&f.admin, ACTOR_NOT_LINKED).await;
    f.stack
        .patched(&f.admin, &spare_id, &json!({ "enabled": true }))
        .await;
    f.set_validation("spare", None, Some("probe failed")).await;
    f.assert_disable_refused(&f.admin, ACTOR_NOT_LINKED).await;

    for body in [
        json!({ "enabled": false, "confirm": false }),
        json!({ "enabled": false }),
        json!({ "enabled": false, "confirm": "yes" }),
        json!({ "enabled": "no", "confirm": true }),
        json!({ "enabled": false, "confirm": true, "extra": 1 }),
        json!({ "confirm": true }),
    ] {
        let before = f.fingerprint().await;
        let fetched = f.put_password_login(&f.admin, body.clone()).await;
        let fields = bad_request_fields(&fetched);
        assert!(fields.is_array(), "{body}");
        assert_eq!(f.fingerprint().await, before, "{body}");
    }
    let unconfirmed = f
        .put_password_login(&f.admin, json!({ "enabled": false, "confirm": false }))
        .await;
    assert_eq!(bad_request_fields(&unconfirmed), json!(["confirm"]));

    f.stack
        .execute("DELETE FROM identity_links WHERE state = 'suspended'")
        .await;
    f.link("corp", root, "root-corp-2", "active").await;
    f.stack
        .execute("DELETE FROM identity_links WHERE subject = 'root-spare'")
        .await;
    f.mark_validated("corp", 60).await;

    f.set_flag("auth_providers_enabled", false).await;
    f.assert_disable_refused(&f.admin, PROVIDERS_OFF).await;
    let off = f.password_login_state(&f.admin).await;
    assert_eq!(off["safeAdminLoginPaths"], json!([]));
    f.set_flag("auth_providers_enabled", true).await;

    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.fingerprint().await;
    let stale = f.set_password_login(&f.admin, false).await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    assert_eq!(stale.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(f.fingerprint().await, before);
    assert!(f.snapshot_password_login());

    let fresh = f.stack.signed_in("root", f.next_peer()).await;
    f.mark_validated("corp", 60).await;
    let ready = f.password_login_state(&fresh).await;
    assert_eq!(ready["canDisable"], true, "{ready}");
    assert_eq!(ready["blockers"], json!([]));
    let passwords_before = f
        .count("SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL")
        .await;
    let sessions_before = f.session_count().await;

    let disabled = f.set_password_login(&fresh, false).await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());
    let body = disabled.json();
    assert_eq!(body["passwordLoginEnabled"], false);
    assert_eq!(body["canDisable"], false);
    assert_eq!(body["blockers"], json!([]));
    assert_eq!(body["safeAdminLoginPaths"].as_array().unwrap().len(), 2);
    assert_eq!(f.password_login_row().await, "false");
    assert!(!f.snapshot_password_login());
    assert_eq!(f.bootstrap().await["passwordLoginEnabled"], false);
    assert_eq!(f.audits("PASSWORD_LOGIN_DISABLED").await, 1);
    assert_eq!(
        f.audit_metadata("PASSWORD_LOGIN_DISABLED").await,
        [json!({ "safe_admin_path_count": 2 })]
    );
    let (actor, target_type, target_id): (String, String, String) = sqlx::query_as(
        "SELECT actor_user_id, target_type, target_id FROM audit_events
          WHERE action = 'PASSWORD_LOGIN_DISABLED'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        (actor, target_type.as_str(), target_id.as_str()),
        (root.to_string(), "setting", "password_login_enabled")
    );
    assert_eq!(f.audits("SETTING_CHANGED").await, 0);
    assert_eq!(f.audits("SECURITY_POLICY_CHANGED").await, 0);
    assert_eq!(f.me_status_of(&fresh).await, StatusCode::OK);
    assert_eq!(f.session_count().await, sessions_before);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL")
            .await,
        passwords_before
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM sessions WHERE revoked_at IS NOT NULL")
            .await,
        0
    );

    let again = f.set_password_login(&fresh, false).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["passwordLoginEnabled"], false);
    assert_eq!(f.audits("PASSWORD_LOGIN_DISABLED").await, 1);

    let enabled = f.set_password_login(&fresh, true).await;
    assert_eq!(enabled.status, StatusCode::OK, "{}", enabled.text());
    assert_eq!(enabled.json()["passwordLoginEnabled"], true);
    assert_eq!(f.password_login_row().await, "true");
    assert!(f.snapshot_password_login());
    assert_eq!(f.bootstrap().await["passwordLoginEnabled"], true);
    assert_eq!(f.audits("PASSWORD_LOGIN_ENABLED").await, 1);
    let again = f.set_password_login(&fresh, true).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(f.audits("PASSWORD_LOGIN_ENABLED").await, 1);

    let anonymous = f
        .stack
        .call(
            Call {
                session: None,
                ..Call::new(Method::GET, PASSWORD_LOGIN, &fresh)
            },
            f.next_peer(),
        )
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let member = f.send_as(Method::GET, PASSWORD_LOGIN, &ada.creds).await;
    assert_eq!(member.status, StatusCode::FORBIDDEN);
    f.stack.stop().await;
}

impl Federation {
    async fn me_status_of(&self, creds: &Credentials) -> StatusCode {
        self.send_as(Method::GET, "/api/v1/auth/me", creds)
            .await
            .status
    }
}

#[tokio::test]
async fn it_password_login_disabled_login_403() {
    let f = Federation::start().await;
    let root = f.root_id().await;
    f.oidc("corp", json!({})).await;
    f.link("corp", root, "root-sub", "active").await;
    f.mark_validated("corp", 0).await;
    let ada = f.local_member("ada").await;
    f.link("corp", ada.id, "ada-sub", "active").await;

    let before_enabled = f.stack.login("ada", PASSWORD, f.next_peer()).await;
    assert_eq!(before_enabled.status, StatusCode::OK);
    let wrong = f.stack.login("ada", WRONG, f.next_peer()).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let disabled = f.set_password_login(&f.admin, false).await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());

    let attempts = f.count("SELECT COUNT(*) FROM login_attempts").await;
    let lockouts = f.count("SELECT COUNT(*) FROM account_lockouts").await;
    let sessions = f.session_count().await;
    let verifications = f.stack.auth.verifications_performed();
    let passwords = f
        .count("SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL")
        .await;
    let audits = f.count("SELECT COUNT(*) FROM audit_events").await;

    let mut bodies = Vec::new();
    for (identifier, password) in [
        ("root", PASSWORD),
        ("root", WRONG),
        ("ROOT", PASSWORD),
        ("ada", PASSWORD),
        ("ada@example.test", WRONG),
        ("nobody", PASSWORD),
        ("nobody@example.test", "x"),
    ] {
        let fetched = f.stack.login(identifier, password, f.next_peer()).await;
        assert_eq!(fetched.status, StatusCode::FORBIDDEN, "{identifier}");
        assert_eq!(fetched.error_code(), "AUTH_PASSWORD_LOGIN_DISABLED");
        assert!(fetched.set_cookies().is_empty(), "{identifier}");
        bodies.push(fetched.error_without_request_id());
    }
    assert!(
        bodies.windows(2).all(|pair| pair[0] == pair[1]),
        "{bodies:?}"
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM login_attempts").await,
        attempts
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM account_lockouts").await,
        lockouts
    );
    assert_eq!(f.session_count().await, sessions);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM sessions WHERE state = 'mfa_pending'")
            .await,
        0
    );
    assert_eq!(f.count("SELECT COUNT(*) FROM audit_events").await, audits);
    assert_eq!(f.stack.auth.verifications_performed(), verifications);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM users WHERE password_hash IS NOT NULL")
            .await,
        passwords
    );

    let bootstrap = f.bootstrap().await;
    assert_eq!(bootstrap["passwordLoginEnabled"], false);
    assert_eq!(
        bootstrap["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|provider| provider["slug"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["corp"]
    );

    let external = f
        .login_oidc(
            "corp",
            json!({ "sub": "root-sub", "email": "root@example.test", "email_verified": true }),
            &[],
        )
        .await;
    let external = assert_signed_in(&external, "/overview");
    assert_eq!(f.me_status_of(&external).await, StatusCode::OK);
    assert_eq!(f.me_status_of(&ada.creds).await, StatusCode::OK);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM sessions WHERE revoked_at IS NOT NULL")
            .await,
        0
    );

    let reenabled = f.set_password_login(&f.admin, true).await;
    assert_eq!(reenabled.status, StatusCode::OK, "{}", reenabled.text());
    assert!(f.snapshot_password_login());
    assert_eq!(f.bootstrap().await["passwordLoginEnabled"], true);
    for username in ["ada", "root"] {
        let fetched = f.stack.login(username, PASSWORD, f.next_peer()).await;
        assert_eq!(fetched.status, StatusCode::OK, "{username}");
    }
    f.stack.stop().await;
}

#[derive(Clone, Copy, Debug)]
enum Op {
    DemoteA,
    DeactivateA,
    UnlinkA,
    DisableProviderA,
    DemoteB,
    DeactivateB,
    UnlinkB,
    DisableProviderB,
}

struct Race {
    actor: AuthenticatedPrincipal,
    a: UserId,
    b: UserId,
    provider_a: ProviderId,
    provider_b: ProviderId,
    external: ExternalLoginService,
}

impl Race {
    async fn run(&self, f: &Federation, op: Op) -> Result<(), String> {
        let client = ClientMetadata::none();
        let code = |error: &crate::infra::http::error::ApiError| error.code().as_str().to_owned();
        match op {
            Op::DemoteA | Op::DemoteB => {
                let target = if matches!(op, Op::DemoteA) {
                    self.a
                } else {
                    self.b
                };
                f.stack
                    .admin_users
                    .change_role(&self.actor, target, Role::User, &client)
                    .await
                    .map(|_| ())
                    .map_err(|error| code(&error.api_error()))
            }
            Op::DeactivateA | Op::DeactivateB => {
                let target = if matches!(op, Op::DeactivateA) {
                    self.a
                } else {
                    self.b
                };
                f.stack
                    .admin_users
                    .deactivate(&self.actor, target, &client)
                    .await
                    .map(|_| ())
                    .map_err(|error| code(&error.api_error()))
            }
            Op::UnlinkA | Op::UnlinkB => {
                let (target, slug) = if matches!(op, Op::UnlinkA) {
                    (self.a, "pa")
                } else {
                    (self.b, "pb")
                };
                let link = f.link_for(target, slug).await;
                self.external
                    .unlink(UnlinkCommand {
                        actor: &self.actor,
                        target,
                        link: link.parse().unwrap(),
                        scope: UnlinkScope::Admin,
                        client: &client,
                    })
                    .await
                    .map(|_| ())
                    .map_err(|error| error.code().as_str().to_owned())
            }
            Op::DisableProviderA | Op::DisableProviderB => {
                let id = if matches!(op, Op::DisableProviderA) {
                    self.provider_a
                } else {
                    self.provider_b
                };
                let request: UpdateProviderRequest =
                    serde_json::from_value(json!({ "enabled": false })).unwrap();
                let input = UpdateInput::parse(request).unwrap();
                f.stack
                    .providers
                    .update(&self.actor, id, input, &client)
                    .await
                    .map(|_| ())
                    .map_err(|error| code(&error.api_error()))
            }
        }
    }
}

impl Federation {
    async fn link_for(&self, user: UserId, slug: &str) -> String {
        sqlx::query_scalar(
            "SELECT l.id FROM identity_links l JOIN identity_providers p ON p.id = l.provider_id
              WHERE l.user_id = ?1 AND p.key = ?2",
        )
        .bind(user.to_string())
        .bind(slug)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn reseed_race(&self, a: UserId, b: UserId) {
        self.stack
            .execute(&format!(
                "UPDATE users SET role = 'admin', is_active = 1, deactivated_at = NULL,
                        deactivated_by = NULL WHERE id IN ('{a}', '{b}');
                 UPDATE identity_providers SET is_enabled = 1;
                 DELETE FROM identity_links"
            ))
            .await;
        self.link("pa", a, "a-subject", "active").await;
        self.link("pb", b, "b-subject", "active").await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_sso_only_standing_invariant_concurrent_demotion() {
    let f = Federation::start().await;
    let a = f.root_id().await;
    let b = f.admin_member("beta").await;
    let c = f.admin_member("gamma").await;
    let pa = f.oidc("pa", json!({})).await;
    let pb = f.oidc("pb", json!({})).await;
    f.link("pa", a, "a-subject", "active").await;
    f.link("pb", b.id, "b-subject", "active").await;
    f.mark_validated("pa", 0).await;
    f.mark_validated("pb", 0).await;
    let disabled = f.set_password_login(&f.admin, false).await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());
    assert_eq!(f.usable_paths().await, 2);

    let race = Race {
        actor: f.principal(c.id, "gamma"),
        a,
        b: b.id,
        provider_a: pa["id"].as_str().unwrap().parse().unwrap(),
        provider_b: pb["id"].as_str().unwrap().parse().unwrap(),
        external: ExternalLoginService::new(f.stack.providers.clone(), f.stack.auth.clone()),
    };

    let pairs = [
        (Op::DemoteA, Op::DemoteB),
        (Op::DeactivateA, Op::DeactivateB),
        (Op::UnlinkA, Op::UnlinkB),
        (Op::DisableProviderA, Op::DisableProviderB),
        (Op::DemoteA, Op::DeactivateB),
        (Op::DemoteA, Op::UnlinkB),
        (Op::DemoteA, Op::DisableProviderB),
        (Op::DeactivateA, Op::UnlinkB),
        (Op::DeactivateA, Op::DisableProviderB),
        (Op::UnlinkA, Op::DemoteB),
        (Op::UnlinkA, Op::DisableProviderB),
        (Op::DisableProviderA, Op::DeactivateB),
    ];
    let mut rounds = 0_i64;
    for (left, right) in pairs {
        for swapped in [false, true] {
            let (first, second) = if swapped {
                (right, left)
            } else {
                (left, right)
            };
            let (one, two) = tokio::join!(race.run(&f, first), race.run(&f, second));
            let label = format!("{first:?} || {second:?}: {one:?} / {two:?}");
            let outcomes = [one, two];
            assert_eq!(
                outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
                1,
                "{label}"
            );
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|outcome| outcome.as_ref().is_err_and(|code| code == UNSAFE))
                    .count(),
                1,
                "{label}"
            );
            assert!(f.usable_paths().await >= 1, "{label}");
            assert!(!f.snapshot_password_login(), "{label}");
            assert_eq!(f.password_login_row().await, "false", "{label}");
            rounds += 1;
            f.reseed_race(a, b.id).await;
            assert_eq!(f.usable_paths().await, 2, "{label}");
        }
    }
    assert!(rounds >= 24);
    assert_eq!(f.audits("PASSWORD_LOGIN_DISABLED").await, 1);
    assert_eq!(f.audits("PASSWORD_LOGIN_ENABLED").await, 0);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_only_standing_invariant_provider_mutations() {
    let f = Federation::start().await;
    let (root, corp_id) = f.sso_only().await;
    let before = f.fingerprint().await;

    let disable = f
        .patch_provider_body(&corp_id, json!({ "enabled": false }))
        .await;
    f.assert_refused_unchanged(&disable, &before, UNSAFE).await;
    assert_eq!(
        disable.json()["error"]["details"]["blockers"][0]["code"],
        UNSAFE
    );

    let delete = f.delete_provider(&f.admin, &corp_id).await;
    f.assert_refused_unchanged(&delete, &before, UNSAFE).await;

    let issuer = f
        .patch_provider_body(
            &corp_id,
            json!({
                "issuerUrl": "https://other-issuer.example.test",
                "endpoints": {
                    "authorization": format!("{}/authorize", f.idp.issuer()),
                    "token": f.idp.token_endpoint(),
                    "jwks": f.idp.jwks_uri(),
                }
            }),
        )
        .await;
    f.assert_refused_unchanged(&issuer, &before, UNSAFE).await;
    for body in [
        json!({ "clientId": "another-client" }),
        json!({ "clientSecret": "a-rotated-client-secret" }),
    ] {
        let fetched = f.patch_provider_body(&corp_id, body.clone()).await;
        f.assert_refused_unchanged(&fetched, &before, UNSAFE).await;
    }
    assert_eq!(f.audits("IDENTITY_PROVIDER_UPDATED").await, 0);
    assert_eq!(f.audits("IDENTITY_PROVIDER_DISABLED").await, 0);
    assert_eq!(f.audits("IDENTITY_PROVIDER_DELETED").await, 0);
    assert!(f.provider_validation("corp").await.0.is_some());

    let cosmetic = f
        .patch_provider_body(
            &corp_id,
            json!({ "displayName": "Corporate SSO", "sortOrder": 7, "autoProvision": true }),
        )
        .await;
    assert_eq!(cosmetic.status, StatusCode::OK, "{}", cosmetic.text());
    let validation = f.provider_validation("corp").await;
    assert!(validation.0.is_some() && validation.1.is_none());
    assert_eq!(f.usable_paths().await, 1);

    f.oidc("alt", json!({})).await;
    f.link("alt", root, "root-alt", "active").await;
    f.mark_validated("alt", 0).await;
    assert_eq!(f.usable_paths().await, 2);

    let allowed = f
        .patch_provider_body(&corp_id, json!({ "enabled": false }))
        .await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text());
    assert_eq!(f.usable_paths().await, 1);
    let restored = f
        .patch_provider_body(&corp_id, json!({ "enabled": true }))
        .await;
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.text());
    assert_eq!(f.usable_paths().await, 2);

    let invalidated = f
        .patch_provider_body(&corp_id, json!({ "clientId": "another-client" }))
        .await;
    assert_eq!(invalidated.status, StatusCode::OK, "{}", invalidated.text());
    assert_eq!(f.provider_validation("corp").await, (None, None));
    assert_eq!(
        f.usable_paths().await,
        2,
        "the standing path does not depend on validation"
    );

    let alt_id = f.provider_id("alt").await;
    let off = f
        .patch_provider_body(&corp_id, json!({ "enabled": false }))
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text());
    assert_eq!(f.usable_paths().await, 1);
    let last = f.fingerprint().await;
    let blocked = f
        .patch_provider_body(&alt_id, json!({ "enabled": false }))
        .await;
    f.assert_refused_unchanged(&blocked, &last, UNSAFE).await;
    let blocked = f
        .patch_provider_body(&alt_id, json!({ "clientSecret": "yet-another-secret" }))
        .await;
    f.assert_refused_unchanged(&blocked, &last, UNSAFE).await;
    let blocked = f.delete_provider(&f.admin, &alt_id).await;
    f.assert_refused_unchanged(&blocked, &last, UNSAFE).await;

    let idle = f.oidc("idle", json!({})).await;
    let removed = f
        .delete_provider(&f.admin, idle["id"].as_str().unwrap())
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    assert_eq!(f.audits("IDENTITY_PROVIDER_DELETED").await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_only_standing_invariant_global_toggle_and_generic_settings() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;

    for body in [
        json!({ "passwordLoginEnabled": false }),
        json!({ "password_login_enabled": false }),
        json!({ "passwordLoginEnabled": true, "passwordMinLength": 9 }),
    ] {
        let before = f.fingerprint().await;
        let fetched = f
            .call_json(Method::PATCH, SECURITY_SETTINGS, &f.admin, body.clone())
            .await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(fetched.error_code(), "SETTING_UNKNOWN", "{body}");
        assert_eq!(f.fingerprint().await, before);
        assert_eq!(f.password_login_row().await, "true");
        assert!(f.snapshot_password_login());
    }
    let group = f
        .send_as(Method::GET, SECURITY_SETTINGS, &f.admin)
        .await
        .json();
    assert!(group.get("passwordLoginEnabled").is_none(), "{group}");

    let off = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": false }),
        )
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text());
    assert_eq!(f.bootstrap().await["providers"], json!([]));
    let on = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": true }),
        )
        .await;
    assert_eq!(on.status, StatusCode::OK);
    assert_eq!(
        f.bootstrap().await["providers"].as_array().unwrap().len(),
        1
    );

    let root = f.root_id().await;
    f.link("corp", root, "root-corp", "active").await;
    f.mark_validated("corp", 0).await;
    let disabled = f.set_password_login(&f.admin, false).await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());
    let before = f.fingerprint().await;
    let refused = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": false, "passwordMinLength": 12 }),
        )
        .await;
    f.assert_refused_unchanged(&refused, &before, UNSAFE).await;
    assert!(f.stack.settings.current().security.auth_providers_enabled);
    assert_eq!(f.stack.settings.current().security.password_min_length, 8);

    let noop = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": true }),
        )
        .await;
    assert_eq!(noop.status, StatusCode::OK);
    let bypass = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "passwordLoginEnabled": true }),
        )
        .await;
    assert_eq!(bypass.error_code(), "SETTING_UNKNOWN");
    assert_eq!(f.password_login_row().await, "false");

    let enabled = f.set_password_login(&f.admin, true).await;
    assert_eq!(enabled.status, StatusCode::OK);
    let allowed = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": false }),
        )
        .await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text());
    let providers_off = f.set_password_login(&f.admin, true).await;
    assert_eq!(providers_off.status, StatusCode::OK);
    let blocked = f.set_password_login(&f.admin, false).await;
    assert_eq!(blocked.status, StatusCode::CONFLICT);
    assert_eq!(
        blocked.json()["error"]["details"]["blockers"],
        json!([{ "code": PROVIDERS_OFF.0, "detail": PROVIDERS_OFF.1 }])
    );
    assert_eq!(f.password_login_row().await, "true");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_only_standing_invariant_identity_unlink() {
    let f = Federation::start().await;
    let beta = f.admin_member("beta").await;
    let (root, _) = f.sso_only().await;
    let root_link = f.link_for(root, "corp").await;
    let before = f.fingerprint().await;

    let own = f
        .send_as(
            Method::DELETE,
            &format!("/api/v1/identity-links/{root_link}"),
            &f.admin,
        )
        .await;
    f.assert_refused_unchanged(&own, &before, UNSAFE).await;
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::OK);
    let by_admin = f
        .send_as(
            Method::DELETE,
            &format!("{USERS}/{root}/identity-links/{root_link}"),
            &beta.creds,
        )
        .await;
    f.assert_refused_unchanged(&by_admin, &before, UNSAFE).await;
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::OK);
    assert_eq!(f.audits("IDENTITY_LINK_REMOVED").await, 0);

    f.link("corp", beta.id, "beta-corp", "active").await;
    let beta_link = f.link_for(beta.id, "corp").await;
    let allowed = f
        .send_as(
            Method::DELETE,
            &format!("/api/v1/identity-links/{root_link}"),
            &f.admin,
        )
        .await;
    assert_eq!(allowed.status, StatusCode::NO_CONTENT, "{}", allowed.text());
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.audits("IDENTITY_LINK_REMOVED").await, 1);

    let last = f.fingerprint().await;
    let blocked = f
        .send_as(
            Method::DELETE,
            &format!("/api/v1/identity-links/{beta_link}"),
            &beta.creds,
        )
        .await;
    f.assert_refused_unchanged(&blocked, &last, UNSAFE).await;
    assert_eq!(f.me_status_of(&beta.creds).await, StatusCode::OK);

    f.stack
        .execute(&format!(
            "DELETE FROM identity_links WHERE user_id = '{}'",
            beta.id
        ))
        .await;
    let passwordless = f.make_user("sam", "sam@example.test").await;
    f.stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{passwordless}'"
        ))
        .await;
    f.link("corp", passwordless, "sam-corp", "active").await;
    let external = ExternalLoginService::new(f.stack.providers.clone(), f.stack.auth.clone());
    let sam_link: IdentityLinkId = f.link_for(passwordless, "corp").await.parse().unwrap();
    let before = f.fingerprint().await;
    let own = external
        .unlink(UnlinkCommand {
            actor: &f.principal(passwordless, "sam"),
            target: passwordless,
            link: sam_link,
            scope: UnlinkScope::SelfService,
            client: &ClientMetadata::none(),
        })
        .await
        .unwrap_err();
    assert_eq!(own.code().as_str(), "IDENTITY_LINK_LAST_LOGIN_PATH");
    let by_admin = external
        .unlink(UnlinkCommand {
            actor: &f.principal(beta.id, "beta"),
            target: passwordless,
            link: sam_link,
            scope: UnlinkScope::Admin,
            client: &ClientMetadata::none(),
        })
        .await
        .unwrap_err();
    assert_eq!(by_admin.code().as_str(), UNSAFE);
    assert_eq!(f.fingerprint().await, before);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_only_standing_invariant_user_lifecycle() {
    let f = Federation::start().await;
    let beta = f.admin_member("beta").await;
    let gamma = f.admin_member("gamma").await;
    let ada = f.local_member("ada").await;
    let (root, _) = f.sso_only().await;
    let before = f.fingerprint().await;

    let demote = f
        .call_json(
            Method::PUT,
            &format!("{USERS}/{root}/role"),
            &beta.creds,
            json!({ "role": "user" }),
        )
        .await;
    f.assert_refused_unchanged(&demote, &before, UNSAFE).await;
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::OK);
    let deactivate = f
        .send_as(
            Method::POST,
            &format!("{USERS}/{root}/deactivate"),
            &beta.creds,
        )
        .await;
    f.assert_refused_unchanged(&deactivate, &before, UNSAFE)
        .await;
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::OK);
    assert_eq!(f.audits("USER_ROLE_CHANGED").await, 0);
    assert_eq!(f.audits("USER_DEACTIVATED").await, 0);

    let removal = f
        .stack
        .pools
        .write_tx(&f.stack.clock, "test.guard_user_removal", async |tx| {
            assert_safe_sso_after_change(tx, Projection::removing_admin(root)).await?;
            Ok::<(), SsoGuardError>(())
        })
        .await
        .unwrap_err();
    assert!(matches!(removal, SsoGuardError::Unsafe(_)));
    assert_eq!(f.fingerprint().await, before);

    let promoted = f
        .call_json(
            Method::PUT,
            &format!("{USERS}/{}/role", ada.id),
            &beta.creds,
            json!({ "role": "admin" }),
        )
        .await;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.text());
    let before = f.fingerprint().await;
    let still_blocked = f
        .call_json(
            Method::PUT,
            &format!("{USERS}/{root}/role"),
            &beta.creds,
            json!({ "role": "user" }),
        )
        .await;
    f.assert_refused_unchanged(&still_blocked, &before, UNSAFE)
        .await;

    f.link("corp", beta.id, "beta-corp", "active").await;
    let demoted = f
        .call_json(
            Method::PUT,
            &format!("{USERS}/{root}/role"),
            &beta.creds,
            json!({ "role": "user" }),
        )
        .await;
    assert_eq!(demoted.status, StatusCode::OK, "{}", demoted.text());
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.usable_paths().await, 1);

    let last = f.fingerprint().await;
    let refused = f
        .send_as(
            Method::POST,
            &format!("{USERS}/{}/deactivate", beta.id),
            &gamma.creds,
        )
        .await;
    f.assert_refused_unchanged(&refused, &last, UNSAFE).await;
    assert_eq!(f.usable_paths().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_only_last_admin_precedes_the_sso_guard() {
    let f = Federation::start().await;
    let beta = f.admin_member("beta").await;
    let (root, _) = f.sso_only().await;
    f.link("corp", beta.id, "beta-corp", "active").await;
    let actor = f.principal(beta.id, "beta");
    let client = ClientMetadata::none();

    f.stack
        .admin_users
        .change_role(&actor, root, Role::User, &client)
        .await
        .unwrap();
    assert_eq!(f.usable_paths().await, 1);
    for outcome in [
        f.stack
            .admin_users
            .change_role(&actor, beta.id, Role::User, &client)
            .await
            .map(|_| ()),
        f.stack
            .admin_users
            .deactivate(&actor, beta.id, &client)
            .await
            .map(|_| ()),
    ] {
        let error = outcome.unwrap_err();
        assert_eq!(error.api_error().code().as_str(), "LAST_ADMIN_PROTECTED");
    }
    assert_eq!(f.usable_paths().await, 1);

    let promote = f
        .stack
        .admin_users
        .change_role(&actor, root, Role::Admin, &client)
        .await;
    assert!(promote.is_ok());
    f.stack
        .execute(&format!(
            "DELETE FROM identity_links WHERE user_id = '{}'",
            beta.id
        ))
        .await;
    assert_eq!(f.usable_paths().await, 1);
    let error = f
        .stack
        .admin_users
        .change_role(&actor, root, Role::User, &client)
        .await
        .unwrap_err();
    assert_eq!(error.api_error().code().as_str(), UNSAFE);
    assert_eq!(f.usable_paths().await, 1);
    f.stack.stop().await;
}

impl Federation {
    async fn run_provider_test(&self, id: &str) -> Fetched {
        self.send_as(
            Method::POST,
            &format!("{PROVIDER_ADMIN}/{id}/test"),
            &self.admin,
        )
        .await
    }

    async fn break_provider(&self, slug: &str) {
        self.stack
            .execute(&format!(
                "UPDATE identity_providers SET token_endpoint = 'http://127.0.0.1:9/token'
                  WHERE key = '{slug}'"
            ))
            .await;
    }

    async fn repair_provider(&self, slug: &str) {
        let endpoint = self.idp.token_endpoint();
        self.stack
            .execute(&format!(
                "UPDATE identity_providers SET token_endpoint = '{endpoint}' WHERE key = '{slug}'"
            ))
            .await;
    }

    async fn assert_test_cycle(&self, slug: &str, id: &str) {
        self.set_validation(slug, None, None).await;
        let passed = self.run_provider_test(id).await;
        assert_eq!(passed.status, StatusCode::OK, "{}", passed.text());
        let state = self.provider_validation(slug).await;
        assert!(state.0.is_some() && state.1.is_none(), "{state:?}");

        self.break_provider(slug).await;
        let updates = self.audits("IDENTITY_PROVIDER_UPDATED").await;
        let failed = self.run_provider_test(id).await;
        assert_eq!(
            failed.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            failed.text()
        );
        assert_eq!(failed.error_code(), "PROVIDER_VALIDATION_FAILED");
        assert!(failed.json()["error"]["details"]["checks"].is_array());
        let state = self.provider_validation(slug).await;
        assert!(state.0.is_none() && state.1.is_some(), "{state:?}");
        assert_eq!(self.audits("IDENTITY_PROVIDER_UPDATED").await, updates + 1);
        assert_eq!(
            self.audit_metadata("IDENTITY_PROVIDER_UPDATED")
                .await
                .last()
                .unwrap(),
            &json!({ "fields": ["validatedAt", "validationError"] }),
        );
        self.repair_provider(slug).await;
    }
}

#[tokio::test]
async fn it_provider_test_persists_t01_state_in_every_instance_mode() {
    let f = Federation::start().await;
    let corp = f.oidc("corp", json!({})).await;
    let corp_id = corp["id"].as_str().unwrap().to_owned();
    assert!(f.snapshot_password_login());
    f.assert_test_cycle("corp", &corp_id).await;
    f.stack.stop().await;

    let f = Federation::start().await;
    let (_, corp_id) = f.sso_only().await;
    assert!(!f.snapshot_password_login());
    f.assert_test_cycle("corp", &corp_id).await;
    assert_eq!(f.password_login_row().await, "false");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_failure_audit_failure_rolls_the_validation_back() {
    let f = Federation::start().await;
    let (_, corp_id) = f.sso_only().await;
    f.break_provider("corp").await;
    f.stack
        .execute(
            "CREATE TRIGGER fail_provider_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'IDENTITY_PROVIDER_UPDATED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let before = f.fingerprint().await;
    let failed = f.run_provider_test(&corp_id).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(f.fingerprint().await, before);
    let state = f.provider_validation("corp").await;
    assert!(state.0.is_some() && state.1.is_none());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_last_sso_provider_failed_test_keeps_the_structural_standing_guard() {
    let f = Federation::start().await;
    let beta = f.admin_member("beta").await;
    let (root, corp_id) = f.sso_only().await;
    let root_link = f.link_for(root, "corp").await;
    let healthy = f.provider_validation("corp").await;
    assert!(healthy.0.is_some() && healthy.1.is_none());
    f.break_provider("corp").await;

    let failed = f.run_provider_test(&corp_id).await;
    assert_eq!(
        failed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        failed.text()
    );
    assert_eq!(failed.error_code(), "PROVIDER_VALIDATION_FAILED");
    let state = f.provider_validation("corp").await;
    assert!(state.0.is_none() && state.1.is_some(), "{state:?}");
    assert_eq!(f.password_login_row().await, "false");
    assert!(!f.snapshot_password_login());
    assert_eq!(f.provider_enabled("corp").await, 1);
    assert_eq!(f.usable_paths().await, 1);
    assert_eq!(f.me_status_of(&f.admin).await, StatusCode::OK);

    let before = f.fingerprint().await;
    let disable = f
        .patch_provider_body(&corp_id, json!({ "enabled": false }))
        .await;
    f.assert_refused_unchanged(&disable, &before, UNSAFE).await;
    let unlink = f
        .send_as(
            Method::DELETE,
            &format!("/api/v1/identity-links/{root_link}"),
            &f.admin,
        )
        .await;
    f.assert_refused_unchanged(&unlink, &before, UNSAFE).await;
    let demote = f
        .call_json(
            Method::PUT,
            &format!("{USERS}/{root}/role"),
            &beta.creds,
            json!({ "role": "user" }),
        )
        .await;
    f.assert_refused_unchanged(&demote, &before, UNSAFE).await;
    let deactivate = f
        .send_as(
            Method::POST,
            &format!("{USERS}/{root}/deactivate"),
            &beta.creds,
        )
        .await;
    f.assert_refused_unchanged(&deactivate, &before, UNSAFE)
        .await;
    let providers_off = f
        .call_json(
            Method::PATCH,
            SECURITY_SETTINGS,
            &f.admin,
            json!({ "authProvidersEnabled": false }),
        )
        .await;
    f.assert_refused_unchanged(&providers_off, &before, UNSAFE)
        .await;

    let reenabled = f.set_password_login(&f.admin, true).await;
    assert_eq!(reenabled.status, StatusCode::OK, "{}", reenabled.text());
    let refused = f.set_password_login(&f.admin, false).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text());
    assert_eq!(
        refused.json()["error"]["details"]["blockers"],
        json!([{ "code": NO_VALIDATED.0, "detail": NO_VALIDATED.1 }])
    );
    assert_eq!(f.password_login_row().await, "true");

    f.repair_provider("corp").await;
    let passed = f.run_provider_test(&corp_id).await;
    assert_eq!(passed.status, StatusCode::OK, "{}", passed.text());
    let again = f.set_password_login(&f.admin, false).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_password_login_audit_failure_rolls_the_setting_back() {
    let f = Federation::start().await;
    let root = f.root_id().await;
    f.oidc("corp", json!({})).await;
    f.link("corp", root, "root-corp", "active").await;
    f.mark_validated("corp", 0).await;

    f.stack
        .execute(
            "CREATE TRIGGER fail_disable_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'PASSWORD_LOGIN_DISABLED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let before = f.fingerprint().await;
    let failed = f.set_password_login(&f.admin, false).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(f.fingerprint().await, before);
    assert_eq!(f.password_login_row().await, "true");
    assert!(f.snapshot_password_login());
    assert_eq!(f.bootstrap().await["passwordLoginEnabled"], true);
    f.stack.execute("DROP TRIGGER fail_disable_audit").await;

    let disabled = f.set_password_login(&f.admin, false).await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());
    f.stack
        .execute(
            "CREATE TRIGGER fail_enable_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'PASSWORD_LOGIN_ENABLED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let before = f.fingerprint().await;
    let failed = f.set_password_login(&f.admin, true).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(f.fingerprint().await, before);
    assert_eq!(f.password_login_row().await, "false");
    assert!(!f.snapshot_password_login());
    assert_eq!(f.bootstrap().await["passwordLoginEnabled"], false);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_password_login_state_is_secret_free_and_admin_only() {
    let f = Federation::start().await;
    let root = f.root_id().await;
    f.oidc("corp", json!({})).await;
    f.link("corp", root, "root-secret-subject", "active").await;
    f.mark_validated("corp", 0).await;
    let ada = f.local_member("ada").await;

    let state = f.password_login_state(&f.admin).await;
    let mut keys: Vec<&str> = state
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "blockers",
            "canDisable",
            "passwordLoginEnabled",
            "safeAdminLoginPaths"
        ]
    );
    let text = state.to_string();
    for secret in [SECRET, CLIENT_ID, "root-secret-subject"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert_eq!(state["canDisable"], true);
    assert_eq!(state["blockers"], json!([]));

    for creds in [&ada.creds] {
        for method in [Method::GET, Method::PUT] {
            let fetched = f
                .stack
                .call(
                    Call::new(method.clone(), PASSWORD_LOGIN, creds)
                        .json(&json!({ "enabled": false, "confirm": true })),
                    f.next_peer(),
                )
                .await;
            assert_eq!(fetched.status, StatusCode::FORBIDDEN, "{method}");
        }
    }
    assert!(f.snapshot_password_login());
    let bootstrap = f.bootstrap().await;
    let text = bootstrap.to_string();
    for forbidden in [
        "blockers",
        "canDisable",
        "safeAdminLoginPaths",
        "root-secret-subject",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}: {text}");
    }
    f.stack.stop().await;
}
