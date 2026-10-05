use std::time::Duration;

use super::*;

const SELF_LINKS: &str = "/api/v1/identity-links";
const ADMIN_USERS: &str = "/api/v1/admin/users";
const UNLINKED: &str = "identity_provider_unlinked";

impl Federation {
    async fn link_id(&self, user: UserId, slug: &str) -> String {
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

    async fn call(&self, method: Method, path: &str, creds: &Credentials) -> Fetched {
        self.stack
            .call(Call::new(method, path, creds), self.next_peer())
            .await
    }

    async fn seed_device(&self, user: UserId, seed: &str) -> String {
        let id = crate::domain::id::Id::<()>::generate(&self.stack.clock).to_string();
        self.stack
            .execute(&format!(
                "INSERT INTO trusted_devices (id, user_id, token_hash, label, created_at, expires_at)
                 VALUES ('{id}', '{user}', '{:0<64}', '{seed}', '2026-09-25T12:00:00.000Z',
                         '2027-09-25T12:00:00.000Z')",
                digest_seed(seed)
            ))
            .await;
        id
    }

    async fn session_states(&self, user: UserId) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT state, revoked_reason FROM sessions WHERE user_id = ?1 ORDER BY created_at, id",
        )
        .bind(user.to_string())
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn revoked_devices(&self, user: UserId) -> Vec<bool> {
        let rows: Vec<(Option<String>,)> =
            sqlx::query_as("SELECT revoked_at FROM trusted_devices WHERE user_id = ?1 ORDER BY id")
                .bind(user.to_string())
                .fetch_all(self.stack.pools.reader().executor())
                .await
                .unwrap();
        rows.into_iter().map(|(at,)| at.is_some()).collect()
    }

    async fn me_status(&self, creds: &Credentials) -> StatusCode {
        self.call(Method::GET, "/api/v1/auth/me", creds)
            .await
            .status
    }

    async fn second_session(&self, username: &str) -> Credentials {
        self.stack.signed_in(username, self.next_peer()).await
    }

    async fn removal_audit(&self) -> Vec<RemovalAudit> {
        let rows: Vec<RemovalRow> = sqlx::query_as(
            "SELECT actor_user_id, actor_label, target_type, target_id, metadata_json
               FROM audit_events WHERE action = 'IDENTITY_LINK_REMOVED' AND result = 'success'
              ORDER BY id",
        )
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(|row| RemovalAudit {
                actor_user_id: row.actor_user_id,
                actor_label: row.actor_label,
                target_type: row.target_type,
                target_id: row.target_id,
                metadata: serde_json::from_str(&row.metadata_json.unwrap()).unwrap(),
            })
            .collect()
    }
}

#[derive(sqlx::FromRow)]
struct RemovalRow {
    actor_user_id: Option<String>,
    actor_label: Option<String>,
    target_type: String,
    target_id: Option<String>,
    metadata_json: Option<String>,
}

struct RemovalAudit {
    actor_user_id: Option<String>,
    actor_label: Option<String>,
    target_type: String,
    target_id: Option<String>,
    metadata: Value,
}

fn digest_seed(seed: &str) -> String {
    sha256_base64url(seed.as_bytes()).chars().take(40).collect()
}

fn clears_credentials(fetched: &Fetched) {
    let cookies = fetched.set_cookies();
    for name in ["palmr_session", "palmr_csrf", "palmr_device"] {
        assert!(
            cookies
                .iter()
                .any(|cookie| cookie.starts_with(&format!("{name}=;"))
                    && cookie.contains("Max-Age=0")),
            "{name} is not expired in {cookies:?}"
        );
    }
}

fn error_of(fetched: &Fetched) -> (StatusCode, String) {
    (fetched.status, fetched.error_code())
}

#[tokio::test]
async fn it_identity_link_list_returns_only_the_callers_safe_fields() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    f.oauth2("plain", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    f.link("plain", ada.id, "ada-plain", "active").await;
    f.link("corp", bea.id, "bea-corp", "active").await;
    f.stack
        .execute(&format!(
            "UPDATE identity_links SET created_at = '2026-09-25T12:00:01.000Z',
                    email_at_link = 'ada@example.test', last_login_at = '2026-09-25T12:30:00.000Z'
              WHERE user_id = '{}' AND subject = 'ada-plain'",
            ada.id
        ))
        .await;

    let anonymous = f
        .stack
        .call(
            Call {
                session: None,
                ..Call::new(Method::GET, SELF_LINKS, &ada.creds)
            },
            f.next_peer(),
        )
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    let listed = f.call(Method::GET, SELF_LINKS, &ada.creds).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let body = listed.json();
    assert_eq!(body["totalCount"], 2);
    assert!(body["nextCursor"].is_null());
    let items = body["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["externalSubject"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["ada-corp", "ada-plain"]
    );
    let mut keys: Vec<&str> = items[1]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "emailAtLink",
            "externalSubject",
            "id",
            "lastUsedAt",
            "linkedAt",
            "providerDisplayName",
            "providerSlug"
        ]
    );
    assert_eq!(items[1]["providerSlug"], "plain");
    assert_eq!(items[1]["providerDisplayName"], "OAuth plain");
    assert_eq!(items[1]["emailAtLink"], "ada@example.test");
    assert_eq!(items[1]["lastUsedAt"], "2026-09-25T12:30:00.000Z");
    assert!(items[0]["lastUsedAt"].is_null());
    let text = listed.text();
    for secret in [CLIENT_ID, SECRET, "bea-corp", "state", "token", "verifier"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }

    let first = f
        .call(Method::GET, &format!("{SELF_LINKS}?limit=1"), &ada.creds)
        .await
        .json();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["totalCount"], 2);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = f
        .call(
            Method::GET,
            &format!("{SELF_LINKS}?limit=1&cursor={cursor}"),
            &ada.creds,
        )
        .await
        .json();
    assert_eq!(second["items"][0]["externalSubject"], "ada-plain");
    assert!(second["nextCursor"].is_null());

    let others = f.call(Method::GET, SELF_LINKS, &bea.creds).await.json();
    assert_eq!(others["totalCount"], 1);
    assert_eq!(others["items"][0]["externalSubject"], "bea-corp");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_unlink_requires_recent_auth_and_hides_foreign_links() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    f.link("corp", bea.id, "bea-corp", "active").await;
    let mine = f.link_id(ada.id, "corp").await;
    let theirs = f.link_id(bea.id, "corp").await;

    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let stale = f
        .call(Method::DELETE, &format!("{SELF_LINKS}/{mine}"), &ada.creds)
        .await;
    assert_eq!(
        error_of(&stale),
        (
            StatusCode::FORBIDDEN,
            "AUTH_RECENT_AUTH_REQUIRED".to_owned()
        )
    );
    assert_eq!(f.links().await.len(), 2);

    let reauthenticated = f
        .stack
        .call(
            Call::new(Method::POST, "/api/v1/auth/reauthenticate", &ada.creds)
                .json(&json!({ "password": PASSWORD })),
            f.next_peer(),
        )
        .await;
    assert_eq!(reauthenticated.status, StatusCode::NO_CONTENT);

    let unknown = crate::domain::id::Id::<()>::generate(&f.stack.clock).to_string();
    let responses = [
        f.call(
            Method::DELETE,
            &format!("{SELF_LINKS}/not-a-uuid"),
            &ada.creds,
        )
        .await,
        f.call(
            Method::DELETE,
            &format!("{SELF_LINKS}/{unknown}"),
            &ada.creds,
        )
        .await,
        f.call(
            Method::DELETE,
            &format!("{SELF_LINKS}/{theirs}"),
            &ada.creds,
        )
        .await,
    ];
    for refused in &responses {
        assert_eq!(
            error_of(refused),
            (StatusCode::NOT_FOUND, "PROVIDER_LINK_NOT_FOUND".to_owned())
        );
        assert_eq!(
            refused.error_without_request_id(),
            responses[0].error_without_request_id()
        );
        assert!(refused.set_cookies().is_empty());
    }
    assert_eq!(f.links().await.len(), 2);
    assert_eq!(f.me_status(&bea.creds).await, StatusCode::OK);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_unlink_protects_the_only_login_path_of_a_passwordless_account() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    f.oidc("plain", json!({})).await;
    let sso = f
        .external_member("corp", "sso-subject", "sso@example.test")
        .await;
    let device = f.seed_device(sso.id, "sso-device").await;
    let corp = f.link_id(sso.id, "corp").await;
    let before = f.session_states(sso.id).await;

    let refused = f
        .call(Method::DELETE, &format!("{SELF_LINKS}/{corp}"), &sso.creds)
        .await;
    assert_eq!(
        error_of(&refused),
        (
            StatusCode::CONFLICT,
            "IDENTITY_LINK_LAST_LOGIN_PATH".to_owned()
        )
    );
    assert!(refused.set_cookies().is_empty());
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.session_states(sso.id).await, before);
    assert_eq!(f.revoked_devices(sso.id).await, [false]);
    assert!(f.removal_audit().await.is_empty());
    assert_eq!(f.me_status(&sso.creds).await, StatusCode::OK);

    f.link("plain", sso.id, "sso-plain", "active").await;
    let removed = f
        .call(Method::DELETE, &format!("{SELF_LINKS}/{corp}"), &sso.creds)
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.links().await[0].1, "sso-plain");
    assert_eq!(f.revoked_devices(sso.id).await, [true]);
    let _ = device;
    f.stack.stop().await;
}

#[tokio::test]
async fn it_unlink_by_a_password_user_revokes_every_session_and_device_and_audits() {
    let mut f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    let second = f.second_session("ada").await;
    f.seed_device(ada.id, "laptop").await;
    f.seed_device(ada.id, "phone").await;
    f.seed_device(bea.id, "tablet").await;
    let link = f.link_id(ada.id, "corp").await;
    let bea_sessions = f.session_states(bea.id).await;

    let removed = f
        .call(Method::DELETE, &format!("{SELF_LINKS}/{link}"), &ada.creds)
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    assert!(removed.body.is_empty());
    clears_credentials(&removed);

    assert_eq!(f.links().await.len(), 0);
    assert_eq!(
        f.session_states(ada.id).await,
        vec![
            ("revoked".to_owned(), Some(UNLINKED.to_owned())),
            ("revoked".to_owned(), Some(UNLINKED.to_owned())),
        ]
    );
    assert_eq!(f.revoked_devices(ada.id).await, [true, true]);
    assert_eq!(f.session_states(bea.id).await, bea_sessions);
    assert_eq!(f.revoked_devices(bea.id).await, [false]);
    assert_eq!(f.me_status(&ada.creds).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.me_status(&second).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.me_status(&bea.creds).await, StatusCode::OK);

    f.stack.flush_audit().await;
    let audit = f.removal_audit().await;
    assert_eq!(audit.len(), 1);
    let RemovalAudit {
        actor_user_id: actor,
        actor_label: label,
        target_type: kind,
        target_id: target,
        metadata,
    } = &audit[0];
    assert_eq!(actor.as_deref(), Some(ada.id.to_string().as_str()));
    assert_eq!(label.as_deref(), Some("ada"));
    assert_eq!(kind, "identity_link");
    assert_eq!(target.as_deref(), Some(link.as_str()));
    assert_eq!(metadata["by"], "self");
    assert_eq!(metadata["provider_id"], f.provider_id("corp").await);
    assert_eq!(metadata["user_id"], ada.id.to_string());
    assert_eq!(metadata["sessions_revoked"], 2);
    assert_eq!(metadata["trusted_devices_revoked"], 2);
    let rendered = metadata.to_string();
    for secret in ["ada-corp", "token", "password"] {
        assert!(!rendered.contains(secret), "{rendered}");
    }
    f.stack.stop().await;
}

#[tokio::test]
async fn it_unlink_audit_failure_rolls_everything_back() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let ada = f.local_member("ada").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    let second = f.second_session("ada").await;
    f.seed_device(ada.id, "laptop").await;
    let link = f.link_id(ada.id, "corp").await;
    let sessions = f.session_states(ada.id).await;
    f.stack
        .execute(
            "CREATE TRIGGER fail_unlink_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'IDENTITY_LINK_REMOVED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;

    let failed = f
        .call(Method::DELETE, &format!("{SELF_LINKS}/{link}"), &ada.creds)
        .await;
    assert_eq!(
        error_of(&failed),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR".to_owned()
        )
    );
    assert!(failed.set_cookies().is_empty());
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.session_states(ada.id).await, sessions);
    assert_eq!(f.revoked_devices(ada.id).await, [false]);
    assert_eq!(f.me_status(&ada.creds).await, StatusCode::OK);
    assert_eq!(f.me_status(&second).await, StatusCode::OK);
    assert!(f.removal_audit().await.is_empty());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_admin_identity_link_list_is_scoped_and_secret_free() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    f.oauth2("plain", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    f.link("plain", ada.id, "ada-plain", "active").await;
    f.link("corp", bea.id, "bea-corp", "active").await;

    let listed = f
        .call(
            Method::GET,
            &format!("{ADMIN_USERS}/{}/identity-links", ada.id),
            &f.admin,
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let body = listed.json();
    assert_eq!(body["totalCount"], 2);
    let subjects: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["externalSubject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects.len(), 2);
    assert!(subjects.iter().all(|subject| subject.starts_with("ada-")));
    let text = listed.text();
    for secret in [CLIENT_ID, SECRET, "bea-corp", "token", "verifier"] {
        assert!(!text.contains(secret), "{secret} leaked");
    }

    let unknown = crate::domain::id::Id::<()>::generate(&f.stack.clock).to_string();
    for id in [unknown.as_str(), "not-a-uuid"] {
        let missing = f
            .call(
                Method::GET,
                &format!("{ADMIN_USERS}/{id}/identity-links"),
                &f.admin,
            )
            .await;
        assert_eq!(
            error_of(&missing),
            (StatusCode::NOT_FOUND, "USER_NOT_FOUND".to_owned())
        );
    }
    let forbidden = f
        .call(
            Method::GET,
            &format!("{ADMIN_USERS}/{}/identity-links", bea.id),
            &ada.creds,
        )
        .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_admin_unlink_enforces_auth_and_the_user_link_ownership_predicate() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;
    f.link("corp", ada.id, "ada-corp", "active").await;
    f.link("corp", bea.id, "bea-corp", "active").await;
    let adas = f.link_id(ada.id, "corp").await;
    let beas = f.link_id(bea.id, "corp").await;
    let path = |user: &dyn std::fmt::Display, link: &str| {
        format!("{ADMIN_USERS}/{user}/identity-links/{link}")
    };

    let by_user = f
        .call(Method::DELETE, &path(&ada.id, &adas), &bea.creds)
        .await;
    assert_eq!(by_user.status, StatusCode::FORBIDDEN);

    let mismatched = f
        .call(Method::DELETE, &path(&ada.id, &beas), &f.admin)
        .await;
    assert_eq!(
        error_of(&mismatched),
        (StatusCode::NOT_FOUND, "PROVIDER_LINK_NOT_FOUND".to_owned())
    );
    let unknown_user = crate::domain::id::Id::<()>::generate(&f.stack.clock).to_string();
    let missing_user = f
        .call(Method::DELETE, &path(&unknown_user, &adas), &f.admin)
        .await;
    assert_eq!(
        error_of(&missing_user),
        (StatusCode::NOT_FOUND, "USER_NOT_FOUND".to_owned())
    );
    let malformed = f
        .call(Method::DELETE, &path(&ada.id, "not-a-uuid"), &f.admin)
        .await;
    assert_eq!(error_of(&malformed).1, "PROVIDER_LINK_NOT_FOUND");
    assert_eq!(f.links().await.len(), 2);
    assert_eq!(f.me_status(&ada.creds).await, StatusCode::OK);
    assert_eq!(f.me_status(&bea.creds).await, StatusCode::OK);

    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let stale = f
        .call(Method::DELETE, &path(&ada.id, &adas), &f.admin)
        .await;
    assert_eq!(
        error_of(&stale),
        (
            StatusCode::FORBIDDEN,
            "AUTH_RECENT_AUTH_REQUIRED".to_owned()
        )
    );
    assert_eq!(f.links().await.len(), 2);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_admin_unlink_revokes_the_target_but_not_the_acting_admin() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let sso = f
        .external_member("corp", "sso-subject", "sso@example.test")
        .await;
    let second = f
        .login_oidc(
            "corp",
            json!({ "sub": "sso-subject", "email": "sso@example.test", "email_verified": true }),
            &[],
        )
        .await;
    let second = assert_signed_in(&second, "/overview");
    f.seed_device(sso.id, "phone").await;
    let link = f.link_id(sso.id, "corp").await;
    let admin_sessions = f
        .session_states(f.user_by_email("root@example.test").await)
        .await;

    let removed = f
        .call(
            Method::DELETE,
            &format!("{ADMIN_USERS}/{}/identity-links/{link}", sso.id),
            &f.admin,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    assert!(
        removed.set_cookies().is_empty(),
        "the acting admin's credentials are untouched"
    );
    assert_eq!(f.links().await.len(), 0);
    assert_eq!(
        f.session_states(sso.id).await,
        vec![
            ("revoked".to_owned(), Some(UNLINKED.to_owned())),
            ("revoked".to_owned(), Some(UNLINKED.to_owned())),
        ]
    );
    assert_eq!(f.revoked_devices(sso.id).await, [true]);
    assert_eq!(f.me_status(&sso.creds).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.me_status(&second).await, StatusCode::UNAUTHORIZED);
    assert_eq!(f.me_status(&f.admin).await, StatusCode::OK);
    assert_eq!(
        f.session_states(f.user_by_email("root@example.test").await)
            .await,
        admin_sessions
    );

    let audit = f.removal_audit().await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_label.as_deref(), Some("root"));
    assert_eq!(audit[0].metadata["by"], "admin");
    assert_eq!(audit[0].metadata["user_id"], sso.id.to_string());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_admin_unlink_of_the_admins_own_identity_clears_their_credentials() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let admin_id = f.user_by_email("root@example.test").await;
    f.link("corp", admin_id, "root-corp", "active").await;
    let link = f.link_id(admin_id, "corp").await;

    let removed = f
        .call(
            Method::DELETE,
            &format!("{ADMIN_USERS}/{admin_id}/identity-links/{link}"),
            &f.admin,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    clears_credentials(&removed);
    assert_eq!(f.me_status(&f.admin).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.session_states(admin_id).await,
        vec![("revoked".to_owned(), Some(UNLINKED.to_owned()))]
    );
    f.stack.stop().await;
}
