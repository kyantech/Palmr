use tracing_subscriber::filter::EnvFilter;

use super::password_reset::Capture;
use super::profile::assert_code;
use super::*;
use crate::config::LogFormat;
use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::infra::crypto::hash::sha256_hex;
use crate::infra::http::idempotency::{canonical_json, request_identity, IdempotencyRecord};
use crate::infra::telemetry::build_dispatch;

const USERS: &str = "/api/v1/admin/users";
const SENTINEL: &str = "Sentinel-Temporary-Password-0123456789-AbCdEfGhIjKlMnOpQrSt";
const KEY: &str = "admin-user-key-0001-aaaaaaaaaaaaa";
const LIVE_KEY: &str = "admin-user-key-0002-bbbbbbbbbbbbb";
const LAPSED_KEY: &str = "admin-user-key-0003-ccccccccccccc";

fn body(username: &str, password: Option<&str>) -> Value {
    let mut body = json!({
        "firstName": "Grace",
        "lastName": "Hopper",
        "username": username,
        "email": format!("{username}@example.test"),
        "role": "user",
        "locale": "en-US",
    });
    if let Some(password) = password {
        body["password"] = json!(password);
    }
    body
}

fn users_request(
    method: Method,
    path: &str,
    creds: &Credentials,
    payload: Option<&Value>,
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
    if payload.is_some() {
        builder = builder.header(CONTENT_TYPE, "application/json");
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let payload = payload.map_or_else(Body::empty, |payload| Body::from(payload.to_string()));
    builder.body(payload).unwrap()
}

impl Stack {
    pub(super) async fn operator(&self, host: u8) -> Credentials {
        let hash = password_hash();
        let id = self
            .user(UserSpec::local("root", "root@example.test", &hash))
            .await;
        self.execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{id}'"
        ))
        .await;
        self.signed_in("root", host).await
    }

    async fn write(
        &self,
        method: Method,
        path: &str,
        creds: &Credentials,
        payload: &Value,
        headers: &[(&str, &str)],
        host: u8,
    ) -> Fetched {
        self.send(with_peer(
            users_request(method, path, creds, Some(payload), headers),
            host,
        ))
        .await
    }

    pub(super) async fn read_users(&self, creds: &Credentials, host: u8) -> Fetched {
        self.send(with_peer(
            users_request(Method::GET, USERS, creds, None, &[]),
            host,
        ))
        .await
    }

    async fn plant_claim(&self, operator_id: &str, key: &str, payload: &Value, lease_offset: i64) {
        let identity = request_identity(&self.settings.keys(), &Method::POST, USERS, payload);
        let now = Timestamp::try_from(self.clock.now()).unwrap();
        let lease =
            Timestamp::try_from(self.clock.now() + time::Duration::seconds(lease_offset)).unwrap();
        let record = Id::<IdempotencyRecord>::generate(&self.clock);
        self.execute(&format!(
            "INSERT INTO idempotency_records
               (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
                state, lease_expires_at, created_at, expires_at)
             VALUES ('{record}', 'user', '{operator_id}', 'POST', '{USERS}', '{}', '{}',
                     'in_progress', '{lease}', '{now}', '2026-09-26T12:00:00.000Z')",
            sha256_hex(key.as_bytes()).as_str(),
            identity.as_str(),
        ))
        .await;
    }

    pub(super) async fn operator_id(&self, username: &str) -> String {
        sqlx::query_scalar("SELECT id FROM users WHERE username = ?1")
            .bind(username)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn created_users(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM users WHERE created_by IS NOT NULL")
            .await
    }
}

fn keyed(key: &str) -> [(&str, &str); 1] {
    [("idempotency-key", key)]
}

#[tokio::test]
async fn it_admin_create_user_in_progress_claim_and_lease_takeover() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let operator_id = stack.operator_id("root").await;

    let live = body("liveclaim", Some("a live claim passphrase"));
    stack
        .plant_claim(&operator_id, LIVE_KEY, &live, 3_600)
        .await;
    let refused = stack
        .write(Method::POST, USERS, &operator, &live, &keyed(LIVE_KEY), 20)
        .await;
    assert_code(
        &refused,
        StatusCode::CONFLICT,
        "IDEMPOTENCY_REQUEST_IN_PROGRESS",
    );
    assert_eq!(refused.headers.get(RETRY_AFTER).unwrap(), "1");
    assert_eq!(stack.created_users().await, 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'USER_CREATED'")
            .await,
        0
    );

    let other = body("otherbody", Some("a live claim passphrase"));
    let conflict = stack
        .write(Method::POST, USERS, &operator, &other, &keyed(LIVE_KEY), 20)
        .await;
    assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    assert_eq!(stack.created_users().await, 0);

    let lapsed = body("lapsedclaim", Some("a lapsed claim passphrase"));
    stack
        .plant_claim(&operator_id, LAPSED_KEY, &lapsed, -60)
        .await;
    let taken_over = stack
        .write(
            Method::POST,
            USERS,
            &operator,
            &lapsed,
            &keyed(LAPSED_KEY),
            20,
        )
        .await;
    assert_eq!(
        taken_over.status,
        StatusCode::CREATED,
        "{}",
        taken_over.text()
    );
    assert!(taken_over.headers.get("idempotency-replayed").is_none());
    let replay = stack
        .write(
            Method::POST,
            USERS,
            &operator,
            &lapsed,
            &keyed(LAPSED_KEY),
            20,
        )
        .await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(replay.json(), taken_over.json());
    assert_eq!(stack.created_users().await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'USER_CREATED'")
            .await,
        1
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_admin_create_user_password_is_write_only_everywhere() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let operator = stack.operator(10).await;
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    stack.setting("password_min_length", "integer", "64").await;
    let short = body("shortpw", Some(SENTINEL));
    let refused = stack
        .write(Method::POST, USERS, &operator, &short, &keyed(KEY), 20)
        .await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_eq!(refused.json()["error"]["details"]["minLength"], 64);
    assert!(!refused.text().contains(SENTINEL));
    assert_eq!(stack.created_users().await, 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM idempotency_records")
            .await,
        0,
        "a rejected request releases its claim"
    );

    stack.setting("password_min_length", "integer", "8").await;
    let created = stack
        .write(Method::POST, USERS, &operator, &short, &keyed(KEY), 20)
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let replay = stack
        .write(Method::POST, USERS, &operator, &short, &keyed(KEY), 20)
        .await;
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    let listed = stack.read_users(&operator, 20).await;
    for text in [created.text(), replay.text(), listed.text()] {
        assert!(!text.contains(SENTINEL));
        assert!(!text.to_lowercase().contains("password_hash"));
        assert!(!text.contains("$argon2"));
    }

    let collision = stack
        .write(
            Method::POST,
            USERS,
            &operator,
            &body("ShortPw", Some(SENTINEL)),
            &[],
            20,
        )
        .await;
    assert_code(&collision, StatusCode::CONFLICT, "USER_EMAIL_TAKEN");
    assert!(!collision.text().contains(SENTINEL));

    for (table, text) in stack.every_stored_text().await {
        assert!(!text.contains(SENTINEL), "{table} stores the password");
        assert!(!text.contains(KEY), "{table} stores the raw key");
    }
    let logs = capture.text();
    assert!(!logs.contains(SENTINEL), "the logs carry the password");
    assert!(!logs.contains(KEY), "the logs carry the idempotency key");

    let (request_hash, response_json, ciphertext): (String, Option<String>, Option<Vec<u8>>) =
        sqlx::query_as(
            "SELECT request_hash, response_json, response_ciphertext
               FROM idempotency_records WHERE route_template = '/api/v1/admin/users'",
        )
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(request_hash.len(), 64);
    let canonical = canonical_json(&short);
    let unkeyed = sha256_hex(format!("POST\0{USERS}\0{}", canonical.as_str()).as_bytes());
    assert_ne!(
        request_hash,
        unkeyed.as_str(),
        "the request identity is keyed, not a bare digest of the body"
    );
    assert!(
        ciphertext.is_none(),
        "a user replay is stored as plaintext JSON"
    );
    let envelope: Value = serde_json::from_str(&response_json.unwrap()).unwrap();
    assert_eq!(envelope["body"]["username"], "shortpw");
    assert_eq!(envelope["body"]["hasLocalPassword"], true);
    assert!(envelope["body"].get("password").is_none());
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_admin_user_write_routes_share_a_session_keyed_bucket() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let first = stack.operator(10).await;
    let second = stack.signed_in("root", 11).await;
    let absent = "0192f3a1-0000-7000-8000-00000000abcd";
    let rejected_create = json!({});
    let unknown_patch = json!({ "firstName": "Nobody" });
    for round in 0..60 {
        let reply = if round % 2 == 0 {
            stack
                .write(Method::POST, USERS, &first, &rejected_create, &[], 30)
                .await
        } else {
            stack
                .write(
                    Method::PATCH,
                    &format!("{USERS}/{absent}"),
                    &first,
                    &unknown_patch,
                    &[],
                    30,
                )
                .await
        };
        assert_ne!(reply.status, StatusCode::TOO_MANY_REQUESTS, "round {round}");
    }
    for (method, path, payload) in [
        (Method::POST, USERS.to_owned(), &rejected_create),
        (Method::PATCH, format!("{USERS}/{absent}"), &unknown_patch),
    ] {
        let throttled = stack
            .write(method.clone(), &path, &first, payload, &[], 30)
            .await;
        assert_code(&throttled, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
        let independent = stack.write(method, &path, &second, payload, &[], 30).await;
        assert_ne!(independent.status, StatusCode::TOO_MANY_REQUESTS);
    }
    stack.stop().await;
}
