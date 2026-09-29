use std::time::Duration;

use tokio::task::JoinSet;
use tracing_subscriber::filter::EnvFilter;

use super::password_reset::Capture;
use super::profile::assert_code;
use super::*;
use crate::config::LogFormat;
use crate::domain::id::Id;
use crate::infra::crypto::hash::sha256_hex;
use crate::infra::http::idempotency::{request_identity, IdempotencyRecord};
use crate::infra::jobs::prune_tokens::{prune_step, PruneStep};
use crate::infra::telemetry::build_dispatch;

const INVITES: &str = "/api/v1/admin/invites";
const PUBLIC: &str = "/api/v1/public/invites";
const PROFILE: &str = "/api/v1/profile";
const PROFILE_PASSWORD: &str = "/api/v1/profile/password";
const LINK_PREFIX: &str = "https://files.example.test/invite/";
const INVITEE_PASSWORD: &str = "an invited person's passphrase";
const KEY: &str = "invite-key-0001-aaaaaaaaaaaaaaaa";
const HOSTILE: &str = "evil.example";
const TRUST_PROXY: (&str, &str) = ("PALMR_TRUST_PROXY", "198.51.100.0/24");

type ReplayRow = (
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<i64>,
);

type InviteRow = (
    String,
    String,
    String,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    String,
);

struct Adm {
    creds: Credentials,
    id: UserId,
}

fn token_of(url: &str) -> String {
    assert!(url.starts_with(LINK_PREFIX), "{url}");
    url[LINK_PREFIX.len()..].to_owned()
}

fn accept_body(username: &str) -> Value {
    json!({
        "firstName": "Alan",
        "lastName": "Turing",
        "username": username,
        "password": INVITEE_PASSWORD,
        "locale": "pt-BR",
    })
}

fn create_body(email: &str, role: &str, send: bool) -> Value {
    json!({ "email": email, "role": role, "sendEmail": send })
}

async fn dispatch(service: BoxedService, request: Request) -> Fetched {
    let response = service.0.oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let body = Limited::new(body, BODY_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    Fetched {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

impl Stack {
    async fn admin(&self, username: &str, host: u8) -> Adm {
        let hash = password_hash();
        let id = self
            .user(UserSpec::local(
                username,
                &format!("{username}@example.test"),
                &hash,
            ))
            .await;
        self.execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{id}'"
        ))
        .await;
        Adm {
            creds: self.signed_in(username, host).await,
            id,
        }
    }

    fn admin_request(
        method: Method,
        path: &str,
        creds: &Credentials,
        body: Option<&Value>,
        headers: &[(&str, &str)],
    ) -> Request {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(ORIGIN, BASE_URL)
            .header(CSRF_HEADER, &creds.csrf)
            .header(
                COOKIE,
                format!("palmr_session={}; palmr_csrf={}", creds.session, creds.csrf),
            );
        if body.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        builder.body(body).unwrap()
    }

    async fn admin_call(
        &self,
        method: Method,
        path: &str,
        creds: &Credentials,
        body: Option<&Value>,
        host: u8,
    ) -> Fetched {
        self.send(with_peer(
            Self::admin_request(method, path, creds, body, &[]),
            host,
        ))
        .await
    }

    async fn create_invite(&self, admin: &Adm, email: &str, role: &str, send: bool) -> Fetched {
        self.admin_call(
            Method::POST,
            INVITES,
            &admin.creds,
            Some(&create_body(email, role, send)),
            20,
        )
        .await
    }

    async fn invited(&self, admin: &Adm, email: &str, role: &str) -> (String, String) {
        let created = self.create_invite(admin, email, role, false).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        let json = created.json();
        (
            json["id"].as_str().unwrap().to_owned(),
            token_of(json["inviteUrl"].as_str().unwrap()),
        )
    }

    async fn lookup_invite(&self, token: &str, host: u8) -> Fetched {
        self.get(&format!("{PUBLIC}/{token}"), None, host).await
    }

    async fn accept_invite(&self, token: &str, body: &Value, host: u8) -> Fetched {
        self.post_json(
            &format!("{PUBLIC}/{token}/accept"),
            &body.to_string(),
            host,
            None,
        )
        .await
    }

    async fn invite_row(&self, id: &str) -> InviteRow {
        sqlx::query_as(
            "SELECT state, token_hash, expires_at, token_ciphertext, accepted_user_id,
                    revoked_by, created_at
               FROM invites WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn invite_audit(&self) -> Vec<(String, String, Option<String>, String)> {
        sqlx::query_as(
            "SELECT action, actor_type, actor_label, metadata_json
               FROM audit_events WHERE action LIKE 'INVITE_%' ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn users_named(&self, email: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email_normalized = ?1")
            .bind(email)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn assert_text_free_of(&self, needle: &str) {
        for (table, text) in self.every_stored_text().await {
            assert!(
                !text.contains(needle),
                "{table} stores the raw invite token"
            );
        }
    }
}

#[tokio::test]
async fn it_invite_token_hashed() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let created = stack
        .create_invite(&admin, "Alice@Corp.Example", "user", false)
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let json = created.json();
    let url = json["inviteUrl"].as_str().unwrap();
    let token = token_of(url);
    assert_eq!(token.len(), 43);
    assert!(token
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
    assert_eq!(json["expiresAt"], "2026-09-26T12:00:00.000Z");
    assert_eq!(created.headers.get("cache-control").unwrap(), "no-store");

    let row = stack.invite_row(json["id"].as_str().unwrap()).await;
    assert_eq!(row.0, "pending");
    assert_eq!(row.1.len(), 64);
    assert_eq!(row.1, digest(&token));
    let sealed = row
        .3
        .as_ref()
        .expect("a pending invite keeps a sealed copy");
    assert!(!String::from_utf8_lossy(sealed).contains(&token));
    let (email, normalized): (String, String) =
        sqlx::query_as("SELECT email, email_normalized FROM invites")
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        (email.as_str(), normalized.as_str()),
        ("Alice@Corp.Example", "alice@corp.example")
    );

    let looked_up = stack.lookup_invite(&token, 30).await;
    assert_eq!(looked_up.status, StatusCode::OK, "{}", looked_up.text());
    let accepted = stack.accept_invite(&token, &accept_body("alice"), 31).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());

    stack.assert_text_free_of(&token).await;
    let after = stack.invite_row(json["id"].as_str().unwrap()).await;
    assert_eq!(after.1, digest(&token));
    assert!(after.3.is_none());
    for (_, _, label, metadata) in stack.invite_audit().await {
        assert!(!metadata.contains(&token) && !metadata.contains(&digest(&token)));
        assert!(!label.unwrap_or_default().contains(&token));
    }
    let logs = capture.text();
    assert!(
        !logs.contains(&token),
        "the raw invite token reached the logs"
    );
    assert!(!logs.contains(&digest(&token)));
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_url_uses_base_url() {
    for env in [&[][..], &[TRUST_PROXY][..]] {
        let root = TempDir::new().unwrap();
        let stack =
            Stack::start_configured(root.path(), &TestClock::new(START), Routes::new(), env).await;
        stack.enable_smtp().await;
        let admin = stack.admin("root", 10).await;
        let hostile: &[(&str, &str)] = &[
            ("host", HOSTILE),
            ("x-forwarded-host", HOSTILE),
            ("x-forwarded-proto", "http"),
            ("forwarded", "host=evil.example;proto=http"),
            ("referer", "https://evil.example/admin"),
        ];
        let body = create_body("guest@example.test", "user", true);
        let created = stack
            .send(with_peer(
                Stack::admin_request(Method::POST, INVITES, &admin.creds, Some(&body), hostile),
                10,
            ))
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        let url = created.json()["inviteUrl"].as_str().unwrap().to_owned();
        let token = token_of(&url);
        assert!(!url.contains(HOSTILE) && !url.contains("?token="), "{url}");

        let foreign = Request::builder()
            .method(Method::POST)
            .uri(INVITES)
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, format!("https://{HOSTILE}"))
            .header(CSRF_HEADER, &admin.creds.csrf)
            .header(
                COOKIE,
                format!(
                    "palmr_session={}; palmr_csrf={}",
                    admin.creds.session, admin.creds.csrf
                ),
            )
            .body(Body::from(
                create_body("other@example.test", "user", false).to_string(),
            ))
            .unwrap();
        let rejected = stack.send(with_peer(foreign, 10)).await;
        assert_code(&rejected, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED");
        assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 1);

        stack.deliver_mail().await;
        let captured = stack.mail.captured();
        assert_eq!(captured.len(), 1);
        let message = &captured[0].message;
        let link = format!("{LINK_PREFIX}{token}");
        assert!(message.text.contains(&link), "{}", message.text);
        let html = message.html.replace("&#x2f;", "/").replace("&#47;", "/");
        assert!(html.contains(&link), "{html}");
        for text in [&message.text, &message.html, &message.subject] {
            assert!(!text.contains(HOSTILE), "{text}");
            assert!(!text.contains("http://"), "{text}");
            assert!(!text.contains("?token="), "{text}");
        }
        stack.stop().await;
    }
}

#[tokio::test]
async fn it_invite_create_contract() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let admin = stack.admin("root", 10).await;

    let default = stack
        .create_invite(&admin, "one@example.test", "user", false)
        .await;
    assert_eq!(default.status, StatusCode::CREATED, "{}", default.text());
    assert_eq!(default.json()["expiresAt"], "2026-09-26T12:00:00.000Z");

    stack
        .setting("invite_validity_hours", "integer", "48")
        .await;
    let configured = stack
        .create_invite(&admin, "two@example.test", "admin", false)
        .await;
    assert_eq!(configured.json()["expiresAt"], "2026-09-27T12:00:00.000Z");
    let explicit = stack
        .admin_call(
            Method::POST,
            INVITES,
            &admin.creds,
            Some(&json!({ "email": "three@example.test", "role": "user", "expiresInHours": 2, "sendEmail": false })),
            20,
        )
        .await;
    assert_eq!(explicit.json()["expiresAt"], "2026-09-25T14:00:00.000Z");

    let smtp_off = stack
        .create_invite(&admin, "four@example.test", "user", true)
        .await;
    assert_code(&smtp_off, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
    assert!(!smtp_off.text().contains("/invite/"));
    assert_eq!(stack.users_named("four@example.test").await, 0);
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 3);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM email_outbox").await,
        0
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CREATED'")
            .await,
        3
    );

    let taken = stack
        .create_invite(&admin, "ROOT@example.test", "user", false)
        .await;
    assert_code(&taken, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
    let duplicate = stack
        .create_invite(&admin, "ONE@example.test", "user", false)
        .await;
    assert_code(&duplicate, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");

    for (body, fields) in [
        (
            json!({ "email": "bad", "role": "user", "sendEmail": false }),
            json!(["email"]),
        ),
        (
            json!({ "email": "a@example.test", "role": "owner", "sendEmail": false }),
            json!(["role"]),
        ),
        (
            json!({ "email": "a@example.test", "role": "user", "expiresInHours": 0, "sendEmail": false }),
            json!(["expiresInHours"]),
        ),
        (
            json!({ "email": "a@example.test", "role": "user", "expiresInHours": 721, "sendEmail": false }),
            json!(["expiresInHours"]),
        ),
        (
            json!({ "email": "a@example.test", "role": "user", "sendEmail": false, "inviteUrl": "x" }),
            json!(["body"]),
        ),
        (
            json!({ "email": "a@example.test", "role": "user" }),
            json!(["sendEmail"]),
        ),
    ] {
        let rejected = stack
            .admin_call(Method::POST, INVITES, &admin.creds, Some(&body), 20)
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            rejected.json()["error"]["details"]["fields"],
            fields,
            "{body}"
        );
    }

    let signed_out = stack.get(INVITES, None, 21).await;
    assert_eq!(signed_out.status, StatusCode::UNAUTHORIZED);
    let hash = password_hash();
    stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let bob = stack.signed_in("bob", 22).await;
    for (method, path) in [
        (Method::GET, INVITES.to_owned()),
        (Method::POST, INVITES.to_owned()),
        (
            Method::DELETE,
            format!("{INVITES}/{}", default.json()["id"].as_str().unwrap()),
        ),
        (
            Method::POST,
            format!(
                "{INVITES}/{}/resend",
                default.json()["id"].as_str().unwrap()
            ),
        ),
    ] {
        let body = create_body("z@example.test", "user", false);
        let denied = stack.admin_call(method, &path, &bob, Some(&body), 22).await;
        assert_code(&denied, StatusCode::FORBIDDEN, "FORBIDDEN");
    }
    let audit = stack.invite_audit().await;
    assert_eq!(audit.len(), 3);
    assert_eq!(
        audit[1].3,
        r#"{"email_queued":false,"role":"admin","validity_hours":48}"#
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_invite_consumption_atomic_under_concurrency() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let (id, token) = stack.invited(&admin, "race@example.test", "user").await;

    let mut attempts = JoinSet::new();
    for index in 0..6_u8 {
        let service = stack.service.clone();
        let body = accept_body(&format!("racer{index}"));
        let request = with_peer(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{PUBLIC}/{token}/accept"))
                .header(CONTENT_TYPE, "application/json")
                .header(ORIGIN, BASE_URL)
                .body(Body::from(body.to_string()))
                .unwrap(),
            40 + index,
        );
        attempts.spawn(dispatch(service, request));
    }
    let mut created = Vec::new();
    let mut lost = Vec::new();
    while let Some(fetched) = attempts.join_next().await {
        let fetched = fetched.unwrap();
        match fetched.status {
            StatusCode::CREATED => created.push(fetched),
            _ => lost.push(fetched),
        }
    }
    assert_eq!(created.len(), 1, "exactly one acceptance may succeed");
    assert_eq!(lost.len(), 5);
    for loser in &lost {
        assert_code(loser, StatusCode::GONE, "INVITE_ALREADY_USED");
        assert!(loser.set_cookies().is_empty());
    }
    assert_eq!(created[0].set_cookies().len(), 2);

    assert_eq!(stack.users_named("race@example.test").await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM users WHERE username LIKE 'racer%'")
            .await,
        1
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM sessions WHERE auth_method = 'invite'")
            .await,
        1
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CONSUMED'")
            .await,
        1
    );
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM user_preferences WHERE user_id IN (SELECT id FROM users WHERE username LIKE 'racer%')").await,
        1
    );
    let row = stack.invite_row(&id).await;
    assert_eq!(row.0, "accepted");
    assert!(row.3.is_none());
    let winner: String = sqlx::query_scalar("SELECT id FROM users WHERE username LIKE 'racer%'")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(row.4.as_deref(), Some(winner.as_str()));
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_revoked_and_expired_410() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let admin = stack.admin("root", 10).await;

    let (revoked_id, revoked) = stack.invited(&admin, "revoked@example.test", "user").await;
    let (expired_id, expired) = stack.invited(&admin, "expired@example.test", "user").await;
    let (used_id, used) = stack.invited(&admin, "used@example.test", "user").await;
    let (_, live) = stack.invited(&admin, "live@example.test", "user").await;

    let accepted = stack.accept_invite(&used, &accept_body("used"), 30).await;
    assert_eq!(accepted.status, StatusCode::CREATED);
    let revoke = stack
        .admin_call(
            Method::DELETE,
            &format!("{INVITES}/{revoked_id}"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_eq!(revoke.status, StatusCode::NO_CONTENT);
    stack
        .execute(&format!(
            "UPDATE invites SET expires_at = '2026-09-25T12:00:01.000Z' WHERE id = '{expired_id}'"
        ))
        .await;
    clock.advance(Duration::from_secs(2));

    for (token, code) in [
        (&revoked, "INVITE_REVOKED"),
        (&expired, "INVITE_EXPIRED"),
        (&used, "INVITE_ALREADY_USED"),
    ] {
        let lookup = stack.lookup_invite(token, 31).await;
        assert_code(&lookup, StatusCode::GONE, code);
        let accept = stack.accept_invite(token, &accept_body("late"), 32).await;
        assert_code(&accept, StatusCode::GONE, code);
    }
    for (id, code) in [
        (&revoked_id, "INVITE_REVOKED"),
        (&expired_id, "INVITE_EXPIRED"),
        (&used_id, "INVITE_ALREADY_USED"),
    ] {
        let resend = stack
            .admin_call(
                Method::POST,
                &format!("{INVITES}/{id}/resend"),
                &admin.creds,
                None,
                20,
            )
            .await;
        assert_code(&resend, StatusCode::GONE, code);
    }
    for token in [&"A".repeat(43), "short", &"A".repeat(44)] {
        assert_code(
            &stack.lookup_invite(token, 33).await,
            StatusCode::NOT_FOUND,
            "INVITE_NOT_FOUND",
        );
        assert_code(
            &stack.accept_invite(token, &accept_body("ghost"), 34).await,
            StatusCode::NOT_FOUND,
            "INVITE_NOT_FOUND",
        );
    }
    let missing = stack
        .admin_call(
            Method::POST,
            &format!(
                "{INVITES}/{}/resend",
                "019300aa-0000-7000-8000-000000000000"
            ),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_code(&missing, StatusCode::NOT_FOUND, "INVITE_NOT_FOUND");
    assert_eq!(stack.users_named("late@example.test").await, 0);
    assert_eq!(stack.lookup_invite(&live, 35).await.status, StatusCode::OK);
    assert_eq!(stack.invite_row(&expired_id).await.0, "expired");
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_lookup_discloses_only_the_bound_email() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    stack.setting("password_min_length", "integer", "12").await;
    let (_, token) = stack.invited(&admin, "new@example.test", "admin").await;
    let found = stack.lookup_invite(&token, 30).await;
    assert_eq!(found.status, StatusCode::OK, "{}", found.text());
    assert_eq!(
        found.json(),
        json!({
            "valid": true,
            "email": "new@example.test",
            "passwordMinLength": 12,
            "expiresAt": "2026-09-26T12:00:00.000Z",
        })
    );
    assert_eq!(found.headers.get("cache-control").unwrap(), "no-store");
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_public_routes_are_rate_limited() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let missing = "A".repeat(43);
    for _ in 0..20 {
        let lookup = stack.lookup_invite(&missing, 60).await;
        assert_eq!(lookup.status, StatusCode::NOT_FOUND);
    }
    let limited = stack.lookup_invite(&missing, 60).await;
    assert_code(&limited, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert!(limited.retry_after().is_some());
    let accept = stack
        .accept_invite(&missing, &accept_body("nobody"), 60)
        .await;
    assert_code(&accept, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert_eq!(
        stack.lookup_invite(&missing, 61).await.status,
        StatusCode::NOT_FOUND
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_email_binding_is_authoritative() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let (id, token) = stack.invited(&admin, "Alice@Corp.Example", "user").await;

    for extra in [
        json!({ "email": "attacker@evil.example" }),
        json!({ "emailNormalized": "attacker@evil.example" }),
        json!({ "token": "x" }),
        json!({ "userId": admin.id.to_string() }),
    ] {
        let mut body = accept_body("alice");
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let rejected = stack.accept_invite(&token, &body, 30).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            rejected.json()["error"]["details"]["fields"],
            json!(["body"])
        );
    }
    assert_eq!(stack.invite_row(&id).await.0, "pending");
    assert_eq!(stack.users_named("attacker@evil.example").await, 0);

    let accepted = stack.accept_invite(&token, &accept_body("alice"), 31).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    assert_eq!(accepted.json()["user"]["email"], "Alice@Corp.Example");
    assert_eq!(stack.users_named("alice@corp.example").await, 1);
    assert_eq!(stack.users_named("attacker@evil.example").await, 0);
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_role_cannot_be_escalated() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let (_, user_token) = stack.invited(&admin, "member@example.test", "user").await;
    let (_, admin_token) = stack.invited(&admin, "second@example.test", "admin").await;

    for role in ["admin", "user"] {
        let mut body = accept_body("member");
        body["role"] = json!(role);
        let rejected = stack.accept_invite(&user_token, &body, 30).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    let member = stack
        .accept_invite(&user_token, &accept_body("member"), 31)
        .await;
    assert_eq!(member.status, StatusCode::CREATED, "{}", member.text());
    assert_eq!(member.json()["user"]["role"], "user");
    let member_creds = Credentials::from(&member);
    let denied = stack
        .admin_call(Method::GET, INVITES, &member_creds, None, 31)
        .await;
    assert_code(&denied, StatusCode::FORBIDDEN, "FORBIDDEN");

    let second = stack
        .accept_invite(&admin_token, &accept_body("second"), 32)
        .await;
    assert_eq!(second.status, StatusCode::CREATED, "{}", second.text());
    assert_eq!(second.json()["user"]["role"], "admin");
    let second_creds = Credentials::from(&second);
    let listed = stack
        .admin_call(Method::GET, INVITES, &second_creds, None, 32)
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let roles: Vec<(String, String)> = sqlx::query_as(
        "SELECT username, role FROM users WHERE username IN ('member', 'second') ORDER BY username",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        roles,
        [
            ("member".to_owned(), "user".to_owned()),
            ("second".to_owned(), "admin".to_owned())
        ]
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_collision_does_not_consume() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("taken", "taken@example.test", &hash))
        .await;
    let (id, token) = stack.invited(&admin, "fresh@example.test", "user").await;
    let sessions_before = stack.scalar_i64("SELECT COUNT(*) FROM sessions").await;
    let users_before = stack.scalar_i64("SELECT COUNT(*) FROM users").await;

    for username in ["taken", "TAKEN"] {
        let collided = stack
            .accept_invite(&token, &accept_body(username), 30)
            .await;
        assert_code(&collided, StatusCode::CONFLICT, "USER_USERNAME_TAKEN");
        assert!(collided.set_cookies().is_empty());
        let row = stack.invite_row(&id).await;
        assert_eq!(row.0, "pending");
        assert!(
            row.3.is_some(),
            "the sealed copy survives a rolled-back accept"
        );
        assert!(row.4.is_none());
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM users").await,
            users_before
        );
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM sessions").await,
            sessions_before
        );
        assert_eq!(
            stack
                .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CONSUMED'")
                .await,
            0
        );
        assert_eq!(stack.lookup_invite(&token, 31).await.status, StatusCode::OK);
    }

    let (email_id, email_token) = stack.invited(&admin, "later@example.test", "user").await;
    stack
        .user(UserSpec::local("someone", "later@example.test", &hash))
        .await;
    let email_users = stack.scalar_i64("SELECT COUNT(*) FROM users").await;
    let clash = stack
        .accept_invite(&email_token, &accept_body("someone_else"), 32)
        .await;
    assert_code(&clash, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
    assert_eq!(stack.invite_row(&email_id).await.0, "pending");
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM users").await,
        email_users
    );

    let retried = stack.accept_invite(&token, &accept_body("fresh"), 33).await;
    assert_eq!(retried.status, StatusCode::CREATED, "{}", retried.text());
    assert_eq!(stack.invite_row(&id).await.0, "accepted");
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CONSUMED'")
            .await,
        1
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_accept_establishes_session_and_applies_password_policy() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    stack.setting("password_min_length", "integer", "14").await;
    let (id, token) = stack.invited(&admin, "alan@example.test", "user").await;

    let mut short = accept_body("alan");
    short["password"] = json!("only-13-chars");
    let refused = stack.accept_invite(&token, &short, 30).await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_eq!(refused.json()["error"]["details"]["minLength"], 14);
    assert_eq!(stack.invite_row(&id).await.0, "pending");
    assert_eq!(stack.users_named("alan@example.test").await, 0);

    for (field, value) in [
        ("username", json!("no")),
        ("firstName", json!("   ")),
        ("locale", json!("xx-XX")),
    ] {
        let mut body = accept_body("alan");
        body[field] = value;
        let invalid = stack.accept_invite(&token, &body, 31).await;
        assert_code(
            &invalid,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(invalid.json()["error"]["details"]["fields"], json!([field]));
    }

    let accepted = stack.accept_invite(&token, &accept_body("alan"), 32).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    let json = accepted.json();
    assert_eq!(json["mustChangePassword"], false);
    assert_eq!(json["mfaEnrollmentRequired"], false);
    assert_eq!(json["user"]["role"], "user");
    assert_eq!(json["user"]["locale"], "pt-BR");
    assert_eq!(json["user"]["email"], "alan@example.test");
    assert_eq!(accepted.headers.get("cache-control").unwrap(), "no-store");
    let creds = Credentials::from(&accepted);
    let me = stack.get(ME, Some(&creds.session), 32).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.json()["restriction"], Value::Null);

    let (must_change, active, totp, hash_set, created_by): (bool, bool, bool, bool, Option<String>) =
        sqlx::query_as(
            "SELECT must_change_password, is_active, totp_enabled, password_hash IS NOT NULL, created_by
               FROM users WHERE username = 'alan'",
        )
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        (must_change, active, totp, hash_set),
        (false, true, false, true)
    );
    assert_eq!(created_by.as_deref(), Some(admin.id.to_string().as_str()));
    assert_eq!(
        stack.login("alan", INVITEE_PASSWORD, 33).await.status,
        StatusCode::OK
    );

    let audit = stack.invite_audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|row| row.0 == "INVITE_CONSUMED")
            .count(),
        1
    );
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CONSUMED' AND actor_type = 'user'").await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_mandatory_two_factor_is_still_evaluated() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let (_, token) = stack.invited(&admin, "mfa@example.test", "user").await;
    stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let accepted = stack.accept_invite(&token, &accept_body("mfa"), 30).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    assert_eq!(accepted.json()["mfaEnrollmentRequired"], true);
    assert_eq!(accepted.json()["mustChangePassword"], false);
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_resend_reuses_original_token() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let admin = stack.admin("root", 10).await;

    let unavailable = stack
        .create_invite(&admin, "mail@example.test", "user", true)
        .await;
    assert_code(
        &unavailable,
        StatusCode::CONFLICT,
        "FEATURE_UNAVAILABLE_SMTP",
    );

    stack.enable_smtp().await;
    let created = stack
        .create_invite(&admin, "mail@example.test", "user", true)
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let id = created.json()["id"].as_str().unwrap().to_owned();
    let token = token_of(created.json()["inviteUrl"].as_str().unwrap());
    let original = stack.invite_row(&id).await;

    let (params, sealed, kind): (String, Option<Vec<u8>>, String) =
        sqlx::query_as("SELECT params_json, token_ciphertext, kind FROM email_outbox")
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(kind, "invite");
    assert!(
        !params.contains(&token) && !params.contains("token"),
        "{params}"
    );
    assert!(!String::from_utf8_lossy(&sealed.unwrap()).contains(&token));
    let jobs: Vec<String> = sqlx::query_scalar("SELECT payload_json FROM jobs")
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap();
    assert!(jobs.iter().all(|payload| !payload.contains(&token)));

    stack.deliver_mail().await;
    clock.advance(Duration::from_secs(3 * 3600));
    let resend = stack
        .admin_call(
            Method::POST,
            &format!("{INVITES}/{id}/resend"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_eq!(resend.status, StatusCode::ACCEPTED, "{}", resend.text());
    stack.deliver_mail().await;

    let captured = stack.mail.captured();
    assert_eq!(captured.len(), 2);
    let link = format!("{LINK_PREFIX}{token}");
    for mail in &captured {
        assert!(mail.message.text.contains(&link), "{}", mail.message.text);
        assert_eq!(mail.message.to.email.as_str(), "mail@example.test");
    }
    let after = stack.invite_row(&id).await;
    assert_eq!(after.0, "pending");
    assert_eq!(after.1, original.1, "resend must not mint a second token");
    assert_eq!(after.2, original.2, "resend must not extend the expiry");
    assert_eq!(after.6, original.6);
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM email_outbox WHERE kind = 'invite'")
            .await,
        2
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM email_outbox WHERE token_ciphertext IS NOT NULL")
            .await,
        0
    );
    assert_eq!(stack.invite_audit().await.len(), 1);

    stack
        .setting_in("smtp", "smtp_enabled", "boolean", "false")
        .await;
    stack.settings.reload().await.unwrap();
    let no_smtp = stack
        .admin_call(
            Method::POST,
            &format!("{INVITES}/{id}/resend"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_code(&no_smtp, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");

    assert_eq!(stack.lookup_invite(&token, 30).await.status, StatusCode::OK);
    let listed = stack
        .admin_call(Method::GET, INVITES, &admin.creds, None, 20)
        .await;
    assert_eq!(
        listed.json()["items"][0]["lastSentAt"],
        "2026-09-25T15:00:00.000Z"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_revoke_wipes_sealed_copy_and_audits() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let admin = stack.admin("root", 10).await;
    let created = stack
        .create_invite(&admin, "gone@example.test", "admin", true)
        .await;
    let id = created.json()["id"].as_str().unwrap().to_owned();
    let token = token_of(created.json()["inviteUrl"].as_str().unwrap());
    assert!(stack.invite_row(&id).await.3.is_some());

    let revoke = stack
        .admin_call(
            Method::DELETE,
            &format!("{INVITES}/{id}"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_eq!(revoke.status, StatusCode::NO_CONTENT);
    let row = stack.invite_row(&id).await;
    assert_eq!(row.0, "revoked");
    assert!(row.3.is_none());
    assert_eq!(row.5.as_deref(), Some(admin.id.to_string().as_str()));
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM email_outbox WHERE state = 'canceled' AND token_ciphertext IS NULL").await,
        1
    );
    stack.deliver_mail().await;
    assert!(
        stack.mail.captured().is_empty(),
        "a revoked invite must not be e-mailed"
    );
    assert_code(
        &stack.lookup_invite(&token, 30).await,
        StatusCode::GONE,
        "INVITE_REVOKED",
    );
    assert_code(
        &stack.accept_invite(&token, &accept_body("gone"), 31).await,
        StatusCode::GONE,
        "INVITE_REVOKED",
    );
    stack.assert_text_free_of(&token).await;

    let again = stack
        .admin_call(
            Method::DELETE,
            &format!("{INVITES}/{id}"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
    let missing = stack
        .admin_call(
            Method::DELETE,
            &format!("{INVITES}/019300aa-0000-7000-8000-000000000000"),
            &admin.creds,
            None,
            20,
        )
        .await;
    assert_code(&missing, StatusCode::NOT_FOUND, "INVITE_NOT_FOUND");
    let audit = stack.invite_audit().await;
    assert_eq!(
        audit.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
        ["INVITE_CREATED", "INVITE_REVOKED"]
    );
    assert_eq!(audit[1].1, "user");
    assert_eq!(audit[1].3, r#"{"role":"admin"}"#);
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_admin_list_reports_status_without_secrets() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let admin = stack.admin("root", 10).await;
    let (_, pending) = stack.invited(&admin, "pending@example.test", "user").await;
    let (accepted_id, accepted) = stack
        .invited(&admin, "accepted@example.test", "admin")
        .await;
    let (revoked_id, _) = stack.invited(&admin, "revoked@example.test", "user").await;
    let short = stack
        .admin_call(
            Method::POST,
            INVITES,
            &admin.creds,
            Some(&json!({ "email": "short@example.test", "role": "user", "expiresInHours": 1, "sendEmail": false })),
            20,
        )
        .await;
    let short_id = short.json()["id"].as_str().unwrap().to_owned();
    let short_token = token_of(short.json()["inviteUrl"].as_str().unwrap());
    assert_eq!(
        stack
            .accept_invite(&accepted, &accept_body("accepted"), 30)
            .await
            .status,
        StatusCode::CREATED
    );
    stack
        .admin_call(
            Method::DELETE,
            &format!("{INVITES}/{revoked_id}"),
            &admin.creds,
            None,
            20,
        )
        .await;
    clock.advance(Duration::from_secs(2 * 3600));

    let fetch = |query: &str| {
        let path = format!("{INVITES}{query}");
        let stack = &stack;
        let creds = &admin.creds;
        async move { stack.admin_call(Method::GET, &path, creds, None, 20).await }
    };
    let all = fetch("").await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text());
    assert_eq!(all.json()["totalCount"], 4);
    let text = all.text();
    for secret in [
        pending.as_str(),
        accepted.as_str(),
        short_token.as_str(),
        "tokenHash",
        "token_hash",
        "tokenCiphertext",
        "inviteUrl",
        "ciphertext",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let by_status = |body: Value, wanted: &str| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["status"] == wanted)
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        by_status(all.json(), "expired"),
        std::slice::from_ref(&short_id)
    );
    for (query, status, ids) in [
        ("?status=pending", "pending", 1),
        ("?status=accepted", "accepted", 1),
        ("?status=revoked", "revoked", 1),
        ("?status=expired", "expired", 1),
    ] {
        let filtered = fetch(query).await;
        assert_eq!(filtered.status, StatusCode::OK, "{}", filtered.text());
        assert_eq!(filtered.json()["totalCount"], ids, "{query}");
        assert_eq!(by_status(filtered.json(), status).len(), ids, "{query}");
    }
    let accepted_item = fetch("?status=accepted").await.json()["items"][0].clone();
    assert_eq!(accepted_item["id"], accepted_id);
    assert_eq!(accepted_item["role"], "admin");
    assert_eq!(accepted_item["email"], "accepted@example.test");
    assert_eq!(accepted_item["createdBy"]["username"], "root");
    assert_eq!(accepted_item["createdBy"]["id"], admin.id.to_string());
    assert_eq!(accepted_item["lastSentAt"], Value::Null);
    assert!(accepted_item["acceptedAt"].is_string() && accepted_item["acceptedUserId"].is_string());
    let bad = fetch("?status=bogus").await;
    assert_code(&bad, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let first = fetch("?limit=2").await.json();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = fetch(&format!("?limit=2&cursor={cursor}")).await.json();
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["nextCursor"], Value::Null);
    assert_eq!(
        stack.lookup_invite(&short_token, 31).await.status,
        StatusCode::GONE
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_invite_tokens_pruned_after_retention() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let admin = stack.admin("root", 10).await;
    let (lapsed_id, lapsed) = stack.invited(&admin, "lapsed@example.test", "user").await;
    let (live_id, _) = stack.invited(&admin, "live@example.test", "user").await;
    stack
        .execute(&format!(
            "UPDATE invites SET expires_at = '2026-09-25T13:00:00.000Z' WHERE id = '{lapsed_id}'"
        ))
        .await;
    let (used_id, used) = stack.invited(&admin, "used@example.test", "user").await;
    assert_eq!(
        stack
            .accept_invite(&used, &accept_body("used"), 30)
            .await
            .status,
        StatusCode::CREATED
    );

    clock.advance(Duration::from_secs(2 * 3600));
    let step = prune_step(&stack.pools, &clock, PruneStep::ExpiredInvites)
        .await
        .unwrap();
    assert_eq!(step.deleted, 1);
    let lapsed_row = stack.invite_row(&lapsed_id).await;
    assert_eq!(lapsed_row.0, "expired");
    assert!(lapsed_row.3.is_none(), "expiry wipes the sealed copy");
    assert_eq!(stack.invite_row(&live_id).await.0, "pending");
    assert_code(
        &stack.lookup_invite(&lapsed, 31).await,
        StatusCode::GONE,
        "INVITE_EXPIRED",
    );

    let early = prune_step(&stack.pools, &clock, PruneStep::TerminalInvites)
        .await
        .unwrap();
    assert_eq!(early.deleted, 0);
    clock.advance(Duration::from_secs(29 * 24 * 3600));
    let grace = prune_step(&stack.pools, &clock, PruneStep::TerminalInvites)
        .await
        .unwrap();
    assert_eq!(
        grace.deleted, 0,
        "terminal rows outlive their state by 30 days"
    );
    clock.advance(Duration::from_secs(24 * 3600));
    let swept = prune_step(&stack.pools, &clock, PruneStep::TerminalInvites)
        .await
        .unwrap();
    assert_eq!(swept.deleted, 2);
    let remaining: Vec<String> = sqlx::query_scalar("SELECT id FROM invites")
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(remaining, [live_id]);
    let _ = used_id;
    stack.stop().await;
}

#[tokio::test]
async fn it_idempotency_sealed_payload_never_plaintext() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    stack.enable_smtp().await;
    let admin = stack.admin("root", 10).await;
    let body = create_body("keyed@example.test", "user", true);
    let keyed = |body: &Value, key: &str| {
        Stack::admin_request(
            Method::POST,
            INVITES,
            &admin.creds,
            Some(body),
            &[("idempotency-key", key)],
        )
    };

    let first = stack.send(with_peer(keyed(&body, KEY), 20)).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text());
    let url = first.json()["inviteUrl"].as_str().unwrap().to_owned();
    let token = token_of(&url);
    let replay = stack.send(with_peer(keyed(&body, KEY), 20)).await;
    assert_eq!(replay.status, StatusCode::CREATED, "{}", replay.text());
    assert_eq!(replay.json(), first.json());
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(replay.headers.get("cache-control").unwrap(), "no-store");

    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 1);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM email_outbox").await,
        1
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'INVITE_CREATED'")
            .await,
        1
    );
    stack.deliver_mail().await;
    assert_eq!(stack.mail.captured().len(), 1);

    let (json, ciphertext, nonce, version): ReplayRow = sqlx::query_as(
        "SELECT response_json, response_ciphertext, response_nonce, key_version
               FROM idempotency_records WHERE route_template = '/api/v1/admin/invites'",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(
        json.is_none(),
        "an invite replay must never be stored as plaintext JSON"
    );
    let ciphertext = ciphertext.unwrap();
    assert_eq!(nonce.unwrap().len(), 24);
    assert_eq!(version, Some(1));
    assert!(!String::from_utf8_lossy(&ciphertext).contains(&token));
    assert!(!String::from_utf8_lossy(&ciphertext).contains("inviteUrl"));
    stack.assert_text_free_of(&token).await;
    stack.assert_text_free_of(&url).await;

    let other = create_body("different@example.test", "user", true);
    let conflict = stack.send(with_peer(keyed(&other, KEY), 20)).await;
    assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 1);

    let claimed = create_body("racing@example.test", "user", false);
    let identity = request_identity(&stack.settings.keys(), &Method::POST, INVITES, &claimed);
    let now = Timestamp::try_from(stack.clock_now()).unwrap();
    let lease = Timestamp::try_from(stack.clock_now() + time::Duration::hours(1)).unwrap();
    let record = Id::<IdempotencyRecord>::generate(&clock);
    stack
        .execute(&format!(
            "INSERT INTO idempotency_records
               (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
                state, lease_expires_at, created_at, expires_at)
             VALUES ('{record}', 'user', '{}', 'POST', '/api/v1/admin/invites', '{}', '{}',
                     'in_progress', '{lease}', '{now}', '2026-09-26T12:00:00.000Z')",
            admin.id,
            sha256_hex(b"invite-key-0002-bbbbbbbbbbbbbbbb").as_str(),
            identity.as_str(),
        ))
        .await;
    let in_progress = stack
        .send(with_peer(
            keyed(&claimed, "invite-key-0002-bbbbbbbbbbbbbbbb"),
            20,
        ))
        .await;
    assert_code(
        &in_progress,
        StatusCode::CONFLICT,
        "IDEMPOTENCY_REQUEST_IN_PROGRESS",
    );
    assert_eq!(in_progress.headers.get("retry-after").unwrap(), "1");
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM invites").await, 1);

    let released = stack
        .send(with_peer(
            keyed(
                &create_body("smtp@example.test", "user", true),
                "invite-key-0003-cccccccccccccccc",
            ),
            20,
        ))
        .await;
    assert_eq!(released.status, StatusCode::CREATED);
    stack
        .setting_in("smtp", "smtp_enabled", "boolean", "false")
        .await;
    stack.settings.reload().await.unwrap();
    let failing = create_body("nosmtp@example.test", "user", true);
    for _ in 0..2 {
        let refused = stack
            .send(with_peer(
                keyed(&failing, "invite-key-0004-dddddddddddddddd"),
                20,
            ))
            .await;
        assert_code(&refused, StatusCode::CONFLICT, "FEATURE_UNAVAILABLE_SMTP");
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM idempotency_records WHERE state = 'completed'")
            .await,
        2,
        "an expected failure must not consume its key"
    );
    stack.stop().await;
}

impl Stack {
    fn clock_now(&self) -> OffsetDateTime {
        use crate::domain::clock::Clock;
        self.clock.now()
    }
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-068")]
#[tokio::test]
async fn regression_R068_invite_created_user_self_service_profile_and_password() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.admin("root", 10).await;
    let (_, token) = stack.invited(&admin, "grace@example.test", "user").await;
    assert_eq!(stack.lookup_invite(&token, 30).await.status, StatusCode::OK);
    let accepted = stack.accept_invite(&token, &accept_body("grace"), 31).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    let creds = Credentials::from(&accepted);
    let call = |method, path: &'static str, body: Value| {
        let stack = &stack;
        let creds = &creds;
        async move { stack.admin_call(method, path, creds, Some(&body), 31).await }
    };

    let profile = call(
        Method::PATCH,
        PROFILE,
        json!({ "firstName": "Grace", "lastName": "Hopper" }),
    )
    .await;
    assert_eq!(profile.status, StatusCode::OK, "{}", profile.text());
    assert_eq!(profile.json()["firstName"], "Grace");
    for forbidden in [
        json!({ "role": "admin" }),
        json!({ "email": "x@example.test" }),
        json!({ "username": "root" }),
    ] {
        let rejected = call(Method::PATCH, PROFILE, forbidden).await;
        assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let wrong = call(
        Method::POST,
        PROFILE_PASSWORD,
        json!({ "currentPassword": "not the password", "newPassword": "a replacement passphrase" }),
    )
    .await;
    assert_code(&wrong, StatusCode::FORBIDDEN, "PASSWORD_CURRENT_INVALID");
    let changed = call(
        Method::POST,
        PROFILE_PASSWORD,
        json!({ "currentPassword": INVITEE_PASSWORD, "newPassword": "a replacement passphrase" }),
    )
    .await;
    assert!(changed.status.is_success(), "{}", changed.text());
    let relogin = stack.login("grace", "a replacement passphrase", 32).await;
    assert_eq!(relogin.status, StatusCode::OK);
    let denied = stack
        .admin_call(Method::GET, INVITES, &Credentials::from(&relogin), None, 32)
        .await;
    assert_code(&denied, StatusCode::FORBIDDEN, "FORBIDDEN");
    stack.stop().await;
}
