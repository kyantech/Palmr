use super::*;
use crate::app::auth_class::RecentAuthWaiver;
use crate::app::router::application_routes as inventory_routes;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::{AuthMethod, NewSession, SessionError};
use crate::features::users::preferences::{Accent, Theme};
use crate::features::users::profile::{ProfileError, VerifiedChange};
use crate::features::users::routes::PASSWORD_CHANGE_ROUTE;

const PROFILE: &str = "/api/v1/profile";
const PREFERENCES: &str = "/api/v1/profile/preferences";
const PASSWORD_PATH: &str = "/api/v1/profile/password";
const USAGE: &str = "/api/v1/profile/usage";
const REAUTH: &str = "/api/v1/auth/reauthenticate";
const SESSIONS: &str = "/api/v1/sessions";
const NEW_PASSWORD: &str = "a brand new passphrase";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SessionRow {
    id: String,
    state: String,
    revoked_reason: Option<String>,
    token_hash: String,
    csrf_token_hash: String,
    created_at: String,
    last_auth_at: String,
    absolute_expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SecurityState {
    password_hash: Option<String>,
    password_updated_at: Option<String>,
    must_change_password: bool,
    first_name: String,
    last_name: String,
    email: String,
    username: String,
    role: String,
    is_active: bool,
}

struct Call<'a> {
    method: Method,
    path: &'a str,
    body: Option<String>,
    content_type: Option<&'a str>,
    session: Option<&'a str>,
    csrf_cookie: Option<&'a str>,
    csrf_header: Option<&'a str>,
    origin: Option<&'a str>,
}

impl<'a> Call<'a> {
    fn new(method: Method, path: &'a str, credentials: &'a Credentials) -> Self {
        Self {
            method,
            path,
            body: None,
            content_type: Some("application/json"),
            session: Some(&credentials.session),
            csrf_cookie: Some(&credentials.csrf),
            csrf_header: Some(&credentials.csrf),
            origin: Some(BASE_URL),
        }
    }

    fn json(mut self, body: &Value) -> Self {
        self.body = Some(body.to_string());
        self
    }

    fn raw(mut self, body: &str) -> Self {
        self.body = Some(body.to_owned());
        self
    }
}

impl Stack {
    async fn call(&self, call: Call<'_>, host: u8) -> Fetched {
        let mut builder = Request::builder().method(call.method).uri(call.path);
        if let Some(content_type) = call.content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        if let Some(origin) = call.origin {
            builder = builder.header(ORIGIN, origin);
        }
        let mut cookies = Vec::new();
        if let Some(session) = call.session {
            cookies.push(format!("palmr_session={session}"));
        }
        if let Some(csrf) = call.csrf_cookie {
            cookies.push(format!("palmr_csrf={csrf}"));
        }
        if !cookies.is_empty() {
            builder = builder.header(COOKIE, cookies.join("; "));
        }
        if let Some(header) = call.csrf_header {
            builder = builder.header(CSRF_HEADER, header);
        }
        let body = call.body.map_or_else(Body::empty, Body::from);
        self.send(with_peer(builder.body(body).unwrap(), host))
            .await
    }

    async fn patch(
        &self,
        path: &str,
        credentials: &Credentials,
        body: &Value,
        host: u8,
    ) -> Fetched {
        self.call(Call::new(Method::PATCH, path, credentials).json(body), host)
            .await
    }

    async fn change_password(
        &self,
        credentials: &Credentials,
        current: &str,
        new: &str,
        host: u8,
    ) -> Fetched {
        let body = json!({ "currentPassword": current, "newPassword": new });
        self.call(
            Call::new(Method::POST, PASSWORD_PATH, credentials).json(&body),
            host,
        )
        .await
    }

    async fn security_state(&self, user: UserId) -> SecurityState {
        sqlx::query_as(
            "SELECT password_hash, password_updated_at, must_change_password, first_name,
                    last_name, email, username, role, is_active
               FROM users WHERE id = ?1",
        )
        .bind(user.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn sessions_of(&self, user: UserId) -> Vec<SessionRow> {
        sqlx::query_as(
            "SELECT id, state, revoked_reason, token_hash, csrf_token_hash, created_at,
                    last_auth_at, absolute_expires_at
               FROM sessions WHERE user_id = ?1 ORDER BY id",
        )
        .bind(user.to_string())
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn session_by_token(&self, raw: &str) -> SessionRow {
        sqlx::query_as(
            "SELECT id, state, revoked_reason, token_hash, csrf_token_hash, created_at,
                    last_auth_at, absolute_expires_at
               FROM sessions WHERE token_hash = ?1",
        )
        .bind(digest(raw))
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn devices_of(&self, user: UserId) -> Vec<(String, Option<String>)> {
        sqlx::query_as("SELECT id, revoked_at FROM trusted_devices WHERE user_id = ?1 ORDER BY id")
            .bind(user.to_string())
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn trusted_device(&self, user: UserId, id: &str, revoked_at: Option<&str>) {
        let revoked = revoked_at.map_or_else(|| "NULL".to_owned(), |at| format!("'{at}'"));
        self.execute(&format!(
            "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at, revoked_at)
             VALUES ('{id}', '{user}', '{id:x<64}', '2026-09-25T12:00:00.000Z',
                     '2026-10-25T12:00:00.000Z', {revoked})"
        ))
        .await;
    }

    async fn mfa_pending(&self, user: UserId, id: &str) {
        self.execute(&format!(
            "INSERT INTO sessions (id, user_id, token_hash, csrf_token_hash, state, auth_method,
                                   mfa_token_hash, mfa_expires_at, created_at, last_seen_at,
                                   last_auth_at, idle_expires_at, absolute_expires_at)
             VALUES ('{id}', '{user}', '{t:a>64}', '{c:b>64}', 'mfa_pending', 'password',
                     '{m:c>64}', '2026-09-25T12:05:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2026-10-02T12:00:00.000Z', '2026-10-25T12:00:00.000Z')",
            t = "",
            c = "",
            m = "",
        ))
        .await;
    }

    async fn audit_rows(&self, action: &str) -> Vec<(String, Option<String>, String)> {
        sqlx::query_as(
            "SELECT actor_label, target_id, metadata_json FROM audit_events
              WHERE action = ?1 ORDER BY id",
        )
        .bind(action)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn principal(
        &self,
        raw: &str,
    ) -> crate::features::auth::sessions::AuthenticatedPrincipal {
        self.sessions
            .authenticate(&Secret::new(raw.to_owned()))
            .await
            .unwrap()
    }
}

fn validation_fields(fetched: &Fetched) -> Value {
    assert_eq!(
        fetched.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        fetched.text()
    );
    assert_eq!(fetched.error_code(), "VALIDATION_ERROR");
    fetched.json()["error"]["details"]["fields"].clone()
}

fn assert_code(fetched: &Fetched, status: StatusCode, code: &str) {
    assert_eq!(fetched.status, status, "{}", fetched.text());
    assert_eq!(fetched.error_code(), code, "{}", fetched.text());
}

#[tokio::test]
async fn it_profile_user_cannot_change_email_username_role() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let ada_creds = stack.signed_in("ada", 10).await;

    let me = stack.get(ME, Some(&ada_creds.session), 10).await;
    let profile = stack.get(PROFILE, Some(&ada_creds.session), 10).await;
    assert_eq!(profile.status, StatusCode::OK, "{}", profile.text());
    assert_eq!(profile.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(profile.json(), me.json()["user"]);
    let serialized = profile.text();
    for secret in [
        "passwordHash",
        "password",
        "emailNormalized",
        "usernameNormalized",
        "quota",
    ] {
        assert!(
            !serialized.contains(secret),
            "{secret} leaked: {serialized}"
        );
    }

    let first = stack
        .patch(PROFILE, &ada_creds, &json!({ "firstName": "  Grace " }), 10)
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    assert_eq!(first.json()["firstName"], "Grace");
    assert_eq!(first.json()["lastName"], "Lovelace");
    let last = stack
        .patch(PROFILE, &ada_creds, &json!({ "lastName": "Hopper" }), 10)
        .await;
    assert_eq!(last.status, StatusCode::OK, "{}", last.text());
    assert_eq!(last.json()["firstName"], "Grace");
    assert_eq!(last.json()["lastName"], "Hopper");
    let both = stack
        .patch(
            PROFILE,
            &ada_creds,
            &json!({ "firstName": "Ada", "lastName": "King" }),
            10,
        )
        .await;
    assert_eq!(both.status, StatusCode::OK, "{}", both.text());
    assert_eq!(
        stack.get(ME, Some(&ada_creds.session), 10).await.json()["user"],
        both.json()
    );
    let accepted = stack.security_state(ada).await;
    assert_eq!(
        (accepted.first_name.as_str(), accepted.last_name.as_str()),
        ("Ada", "King")
    );

    for forbidden in [
        json!({ "firstName": "Mallory", "email": "mallory@example.test" }),
        json!({ "firstName": "Mallory", "username": "mallory" }),
        json!({ "firstName": "Mallory", "role": "admin" }),
        json!({ "firstName": "Mallory", "isActive": false }),
        json!({ "lastName": "Mallory", "pendingEmail": "mallory@example.test" }),
        json!({ "lastName": "Mallory", "quotaBytes": 0 }),
        json!({ "lastName": "Mallory", "password": "hunter2hunter2" }),
        json!({ "lastName": "Mallory", "userId": ada.to_string() }),
        json!({ "role": "admin" }),
        json!({ "email": "mallory@example.test" }),
        json!({ "username": "mallory" }),
    ] {
        let rejected = stack.patch(PROFILE, &ada_creds, &forbidden, 10).await;
        assert_eq!(validation_fields(&rejected), json!(["body"]), "{forbidden}");
        assert_eq!(stack.security_state(ada).await, accepted, "{forbidden}");
    }

    for (invalid, fields) in [
        (json!({ "firstName": "   " }), json!(["firstName"])),
        (json!({ "firstName": "" }), json!(["firstName"])),
        (json!({ "lastName": "x".repeat(101) }), json!(["lastName"])),
        (json!({ "firstName": "Ada\u{0007}" }), json!(["firstName"])),
        (
            json!({ "firstName": 7, "lastName": "King" }),
            json!(["firstName"]),
        ),
        (
            json!({ "firstName": "Grace", "lastName": "   " }),
            json!(["lastName"]),
        ),
        (json!([]), json!(["body"])),
    ] {
        let rejected = stack.patch(PROFILE, &ada_creds, &invalid, 10).await;
        assert_eq!(validation_fields(&rejected), fields, "{invalid}");
        assert_eq!(stack.security_state(ada).await, accepted, "{invalid}");
    }
    let hundred = stack
        .patch(
            PROFILE,
            &ada_creds,
            &json!({ "lastName": "x".repeat(100) }),
            10,
        )
        .await;
    assert_eq!(hundred.status, StatusCode::OK, "{}", hundred.text());

    let malformed = stack
        .call(
            Call::new(Method::PATCH, PROFILE, &ada_creds).raw("{\"firstName\":"),
            10,
        )
        .await;
    assert_code(&malformed, StatusCode::BAD_REQUEST, "INVALID_JSON");
    let wrong_type = stack
        .call(
            Call {
                content_type: Some("text/plain"),
                ..Call::new(Method::PATCH, PROFILE, &ada_creds).json(&json!({ "firstName": "X" }))
            },
            10,
        )
        .await;
    assert_code(
        &wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );
    let no_csrf = stack
        .call(
            Call {
                csrf_header: None,
                ..Call::new(Method::PATCH, PROFILE, &ada_creds).json(&json!({ "firstName": "X" }))
            },
            10,
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN, "{}", no_csrf.text());
    assert!(
        no_csrf.error_code().starts_with("CSRF_"),
        "{}",
        no_csrf.text()
    );
    let foreign_origin = stack
        .call(
            Call {
                origin: Some("https://evil.example.test"),
                ..Call::new(Method::PATCH, PROFILE, &ada_creds).json(&json!({ "firstName": "X" }))
            },
            10,
        )
        .await;
    assert_eq!(
        foreign_origin.status,
        StatusCode::FORBIDDEN,
        "{}",
        foreign_origin.text()
    );
    let anonymous = stack.get(PROFILE, None, 10).await;
    assert_code(&anonymous, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    assert_eq!(stack.security_state(ada).await.first_name, "Ada");

    let temp = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("temp", "temp@example.test", &hash)
        })
        .await;
    let restricted = stack.signed_in("temp", 11).await;
    for fetched in [
        stack.get(PROFILE, Some(&restricted.session), 11).await,
        stack.get(PREFERENCES, Some(&restricted.session), 11).await,
        stack.get(USAGE, Some(&restricted.session), 11).await,
        stack
            .patch(PROFILE, &restricted, &json!({ "firstName": "Temp" }), 11)
            .await,
        stack
            .patch(PREFERENCES, &restricted, &json!({ "theme": "dark" }), 11)
            .await,
    ] {
        assert_code(
            &fetched,
            StatusCode::FORBIDDEN,
            "AUTH_PASSWORD_CHANGE_REQUIRED",
        );
    }
    assert_eq!(stack.security_state(temp).await.first_name, "Ada");
    stack.stop().await;
}

#[tokio::test]
async fn it_preferences_curated_accent_only() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let creds = stack.signed_in("ada", 10).await;
    let other = stack.signed_in("ada", 11).await;

    let initial = stack.get(PREFERENCES, Some(&creds.session), 10).await;
    assert_eq!(initial.status, StatusCode::OK, "{}", initial.text());
    assert_eq!(
        initial.json(),
        json!({ "locale": "en-US", "theme": "system", "accent": "default" })
    );

    let mut expected = json!({ "locale": "en-US", "theme": "system", "accent": "default" });
    for &theme in Theme::ALL {
        let saved = stack
            .patch(PREFERENCES, &creds, &json!({ "theme": theme.as_str() }), 10)
            .await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
        expected["theme"] = json!(theme.as_str());
        assert_eq!(saved.json(), expected);
        let me = stack.get(ME, Some(&creds.session), 10).await.json();
        assert_eq!(me["user"]["theme"], theme.as_str());
    }
    let stored: String =
        sqlx::query_scalar("SELECT theme FROM user_preferences WHERE user_id = ?1")
            .bind(ada.to_string())
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(stored, "system");

    for &accent in Accent::ALL {
        let saved = stack
            .patch(
                PREFERENCES,
                &creds,
                &json!({ "accent": accent.as_str() }),
                10,
            )
            .await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
        expected["accent"] = json!(accent.as_str());
        assert_eq!(saved.json(), expected);
        let me = stack.get(ME, Some(&creds.session), 10).await.json();
        assert_eq!(me["user"]["accent"], accent.as_str());
    }

    for &locale in LocaleCode::ALL {
        let saved = stack
            .patch(
                PREFERENCES,
                &creds,
                &json!({ "locale": locale.as_str() }),
                10,
            )
            .await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
        expected["locale"] = json!(locale.as_str());
        assert_eq!(saved.json(), expected);
        let me = stack.get(ME, Some(&other.session), 11).await.json();
        assert_eq!(me["user"]["locale"], locale.as_str());
    }

    let combined = stack
        .patch(
            PREFERENCES,
            &creds,
            &json!({ "locale": "pt-BR", "theme": "dark", "accent": "blue" }),
            10,
        )
        .await;
    assert_eq!(combined.status, StatusCode::OK, "{}", combined.text());
    let settled = json!({ "locale": "pt-BR", "theme": "dark", "accent": "blue" });
    assert_eq!(combined.json(), settled);

    for (invalid, fields) in [
        (json!({ "accent": "#1668dc" }), json!(["accent"])),
        (json!({ "accent": "1668dc" }), json!(["accent"])),
        (json!({ "accent": "rgb(22, 104, 220)" }), json!(["accent"])),
        (json!({ "accent": "hsl(215, 82%, 48%)" }), json!(["accent"])),
        (json!({ "accent": "teal" }), json!(["accent"])),
        (json!({ "accent": "Blue" }), json!(["accent"])),
        (json!({ "accent": "custom" }), json!(["accent"])),
        (json!({ "accent": 3 }), json!(["accent"])),
        (json!({ "theme": "auto" }), json!(["theme"])),
        (json!({ "theme": "Dark" }), json!(["theme"])),
        (json!({ "locale": "pt-PT" }), json!(["locale"])),
        (json!({ "locale": "en" }), json!(["locale"])),
        (json!({ "locale": "en-us" }), json!(["locale"])),
        (
            json!({ "theme": "light", "accent": "#ff0000" }),
            json!(["accent"]),
        ),
        (
            json!({ "locale": "xx-XX", "theme": "sepia", "accent": "#000" }),
            json!(["locale", "theme", "accent"]),
        ),
        (
            json!({ "accent": "blue", "font": "Comic Sans" }),
            json!(["body"]),
        ),
        (json!({ "fontFamily": "serif" }), json!(["body"])),
        (json!({ "radius": 4 }), json!(["body"])),
        (json!({ "borderRadius": 4 }), json!(["body"])),
        (json!({ "customCss": "body{}" }), json!(["body"])),
        (json!({ "background": "#000" }), json!(["body"])),
        (json!({ "primaryColor": "#1668dc" }), json!(["body"])),
        (json!({ "filesViewMode": "grid" }), json!(["body"])),
    ] {
        let rejected = stack.patch(PREFERENCES, &creds, &invalid, 10).await;
        assert_eq!(validation_fields(&rejected), fields, "{invalid}");
        let current = stack.get(PREFERENCES, Some(&creds.session), 10).await;
        assert_eq!(current.json(), settled, "{invalid}");
    }
    let malformed = stack
        .call(Call::new(Method::PATCH, PREFERENCES, &creds).raw("{"), 10)
        .await;
    assert_code(&malformed, StatusCode::BAD_REQUEST, "INVALID_JSON");
    let wrong_type = stack
        .call(
            Call {
                content_type: Some("application/x-www-form-urlencoded"),
                ..Call::new(Method::PATCH, PREFERENCES, &creds).raw("theme=dark")
            },
            10,
        )
        .await;
    assert_code(
        &wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );

    for session in [&creds.session, &other.session] {
        let me = stack.get(ME, Some(session), 12).await;
        assert_eq!(
            me.status,
            StatusCode::OK,
            "presentation changes never revoke sessions"
        );
    }
    assert!(stack
        .sessions_of(ada)
        .await
        .iter()
        .all(|row| row.state == "active"));
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_revokes_other_sessions() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let bob = stack
        .user(UserSpec::local("bob", "bob@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada@example.test", 11).await;
    stack
        .mfa_pending(ada, "0192f4aa-0000-7000-8000-000000000001")
        .await;
    let bob_creds = stack.signed_in("bob", 12).await;
    stack.trusted_device(ada, "device-ada-1", None).await;
    stack
        .trusted_device(ada, "device-ada-2", Some("2026-09-20T00:00:00.000Z"))
        .await;
    stack.trusted_device(bob, "device-bob-1", None).await;
    let before = stack.session_by_token(&current.session).await;
    let before_state = stack.security_state(ada).await;
    clock.advance(Duration::from_secs(60));

    let changed = stack
        .change_password(&current, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    assert!(changed.body.is_empty());
    assert_eq!(changed.headers.get("cache-control").unwrap(), "no-store");
    let set_cookies = changed.set_cookies();
    assert_eq!(set_cookies.len(), 2, "{set_cookies:?}");
    let rotated = Credentials::from(&changed);
    assert_ne!(rotated.session, current.session);
    assert_ne!(rotated.csrf, current.csrf);

    let old = stack.get(ME, Some(&current.session), 10).await;
    assert_code(&old, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    let fresh = stack.get(ME, Some(&rotated.session), 10).await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.text());
    assert_eq!(fresh.json()["restriction"], Value::Null);
    let revoked = stack.get(ME, Some(&second.session), 11).await;
    assert_code(&revoked, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    let bob_me = stack.get(ME, Some(&bob_creds.session), 12).await;
    assert_eq!(
        bob_me.status,
        StatusCode::OK,
        "other accounts are untouched"
    );

    let after = stack.session_by_token(&rotated.session).await;
    assert_eq!(after.id, before.id, "the current row is rotated in place");
    assert_eq!(after.state, "active");
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.absolute_expires_at, before.absolute_expires_at);
    assert_eq!(after.last_auth_at, before.last_auth_at);
    assert_eq!(after.token_hash, digest(&rotated.session));
    assert_eq!(after.csrf_token_hash, digest(&rotated.csrf));
    for row in stack.sessions_of(ada).await {
        for raw in [
            &rotated.session,
            &rotated.csrf,
            &current.session,
            &current.csrf,
        ] {
            assert_ne!(&row.token_hash, raw);
            assert_ne!(&row.csrf_token_hash, raw);
        }
        if row.id != before.id {
            assert_eq!(row.state, "revoked", "{row:?}");
            assert_eq!(
                row.revoked_reason.as_deref(),
                Some("password_changed"),
                "{row:?}"
            );
        }
    }
    assert_eq!(stack.sessions_of(ada).await.len(), 3);
    assert_eq!(stack.sessions_of(bob).await[0].state, "active");

    let stale_csrf = stack
        .call(
            Call {
                csrf_cookie: Some(&current.csrf),
                csrf_header: Some(&current.csrf),
                ..Call::new(Method::PATCH, PROFILE, &rotated).json(&json!({ "firstName": "X" }))
            },
            10,
        )
        .await;
    assert_code(&stale_csrf, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID");
    let with_rotated = stack
        .patch(PROFILE, &rotated, &json!({ "firstName": "Ada" }), 10)
        .await;
    assert_eq!(
        with_rotated.status,
        StatusCode::OK,
        "{}",
        with_rotated.text()
    );

    let after_state = stack.security_state(ada).await;
    assert!(!after_state.must_change_password);
    assert_ne!(after_state.password_hash, before_state.password_hash);
    assert_ne!(
        after_state.password_updated_at,
        before_state.password_updated_at
    );
    let stored = after_state.password_hash.clone().unwrap();
    assert!(stored.starts_with("$argon2id$"));
    assert!(matches!(
        verify_password(NEW_PASSWORD.as_bytes(), &stored).unwrap(),
        PasswordVerification::Verified { .. }
    ));

    let devices = stack.devices_of(ada).await;
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().all(|(_, revoked_at)| revoked_at.is_some()));
    assert_eq!(
        devices[1].1.as_deref(),
        Some("2026-09-20T00:00:00.000Z"),
        "an already revoked device keeps its original revocation time"
    );
    assert_eq!(
        stack.devices_of(bob).await,
        vec![("device-bob-1".to_owned(), None)]
    );

    let audit = stack.audit_rows("PASSWORD_CHANGED").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    let (actor, target, metadata) = &audit[0];
    assert_eq!(actor, "ada");
    assert_eq!(target.as_deref(), Some(ada.to_string().as_str()));
    assert_eq!(
        serde_json::from_str::<Value>(metadata).unwrap(),
        json!({ "forced": false, "sessions_revoked": 2, "trusted_devices_revoked": 1 })
    );
    let everything: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(actor_label, '') || COALESCE(target_label, '') || metadata_json
           FROM audit_events",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    for text in everything {
        for secret in [
            PASSWORD,
            NEW_PASSWORD,
            stored.as_str(),
            rotated.session.as_str(),
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
    stack.flush_audit().await;
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM account_lockouts")
            .await,
        0,
        "password change never touches the login lockout"
    );

    let old_login = stack.login("ada", PASSWORD, 13).await;
    assert_code(
        &old_login,
        StatusCode::UNAUTHORIZED,
        "AUTH_INVALID_CREDENTIALS",
    );
    let new_login = stack.login("ada", NEW_PASSWORD, 14).await;
    assert_eq!(new_login.status, StatusCode::OK, "{}", new_login.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_completes_forced_change() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let temp = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("temp", "temp@example.test", &hash)
        })
        .await;
    let restricted = stack.signed_in("temp", 10).await;
    let elsewhere = stack.signed_in("temp", 11).await;
    stack.trusted_device(temp, "device-temp", None).await;

    for path in [PROFILE, SESSIONS, USAGE] {
        let blocked = stack.get(path, Some(&restricted.session), 10).await;
        assert_code(
            &blocked,
            StatusCode::FORBIDDEN,
            "AUTH_PASSWORD_CHANGE_REQUIRED",
        );
    }
    let me = stack.get(ME, Some(&restricted.session), 10).await;
    assert_eq!(me.json()["restriction"], "must_change_password");

    clock.advance(Duration::from_secs(30 * 60));
    let recent: bool = stack.principal(&restricted.session).await.recent_auth;
    assert!(
        !recent,
        "the forced change must not depend on a recent-auth window"
    );

    let wrong = stack
        .change_password(&restricted, WRONG, NEW_PASSWORD, 10)
        .await;
    assert_code(&wrong, StatusCode::FORBIDDEN, "PASSWORD_CURRENT_INVALID");
    assert!(stack.security_state(temp).await.must_change_password);

    let changed = stack
        .change_password(&restricted, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let rotated = Credentials::from(&changed);
    assert_code(
        &stack.get(ME, Some(&restricted.session), 10).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    assert_code(
        &stack.get(ME, Some(&elsewhere.session), 11).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    let me = stack.get(ME, Some(&rotated.session), 10).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    assert_eq!(me.json()["restriction"], Value::Null);
    for path in [PROFILE, SESSIONS, USAGE, PREFERENCES] {
        let allowed = stack.get(path, Some(&rotated.session), 10).await;
        assert_eq!(allowed.status, StatusCode::OK, "{path}: {}", allowed.text());
    }
    let state = stack.security_state(temp).await;
    assert!(!state.must_change_password);
    assert!(stack
        .devices_of(temp)
        .await
        .iter()
        .all(|(_, revoked_at)| revoked_at.is_some()));
    let audit = stack.audit_rows("PASSWORD_CHANGED").await;
    assert_eq!(
        serde_json::from_str::<Value>(&audit[0].2).unwrap(),
        json!({ "forced": true, "sessions_revoked": 1, "trusted_devices_revoked": 1 })
    );

    let again = stack
        .change_password(&rotated, NEW_PASSWORD, "yet another passphrase", 10)
        .await;
    assert_eq!(
        (again.status, again.error_code()),
        (
            StatusCode::FORBIDDEN,
            "AUTH_RECENT_AUTH_REQUIRED".to_owned()
        ),
        "once the restriction is lifted, the waiver no longer applies to a stale session"
    );
    assert_eq!(again.json()["error"]["details"]["method"], "password");
    stack.flush_audit().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_waiver_is_narrow() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let enrolling = stack.signed_in("ada", 10).await;
    let me = stack.get(ME, Some(&enrolling.session), 10).await;
    assert_eq!(me.json()["restriction"], "mfa_enrollment_required");
    let fresh = stack
        .change_password(&enrolling, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_code(
        &fresh,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );
    clock.advance(Duration::from_secs(10 * 60));
    let stale = stack
        .change_password(&enrolling, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_code(&stale, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");

    let waivers: Vec<(Method, String)> = inventory_routes()
        .build()
        .unwrap()
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.policy().recent_auth_waiver() != RecentAuthWaiver::None)
        .map(|entry| (entry.method().clone(), entry.path().to_owned()))
        .collect();
    assert_eq!(waivers, vec![(Method::POST, PASSWORD_PATH.to_owned())]);
    assert_eq!(
        PASSWORD_CHANGE_ROUTE.auth(),
        AuthClass::AuthenticatedRecentAuth
    );
    assert_eq!(
        PASSWORD_CHANGE_ROUTE.recent_auth_waiver(),
        RecentAuthWaiver::ForcedPasswordChange
    );

    #[utoipa::path(post, path = "/api/v1/profile/other", responses((status = 204)))]
    async fn elsewhere() -> StatusCode {
        StatusCode::NO_CONTENT
    }
    #[utoipa::path(get, path = "/api/v1/profile/password", responses((status = 204)))]
    async fn wrong_method() -> StatusCode {
        StatusCode::NO_CONTENT
    }
    for (class, routes) in [
        (
            AuthClass::AuthenticatedRecentAuth,
            utoipa_axum::routes!(elsewhere),
        ),
        (
            AuthClass::AuthenticatedRecentAuth,
            utoipa_axum::routes!(wrong_method),
        ),
        (AuthClass::Authenticated, utoipa_axum::routes!(elsewhere)),
    ] {
        let policy = RoutePolicy::new(class, RateLimitClass::Write, Transport::ControlPlane)
            .with_forced_password_change_waiver();
        let Err(error) = Routes::<AppState>::new().route(policy, routes).build() else {
            panic!("a misplaced forced-password-change waiver must not build");
        };
        assert!(
            matches!(
                error.errors(),
                [RouteError::RecentAuthWaiverOutsidePasswordChange { .. }]
            ),
            "{error}"
        );
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_requires_recent_auth() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let creds = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;
    let before = stack.security_state(ada).await;
    let sessions_before = stack.sessions_of(ada).await;

    clock.advance(Duration::from_secs(6 * 60));
    let refused = stack
        .change_password(&creds, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_code(&refused, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(refused.json()["error"]["details"]["method"], "password");
    assert!(refused.set_cookies().is_empty());
    assert_eq!(stack.security_state(ada).await, before);
    assert_eq!(stack.sessions_of(ada).await, sessions_before);

    let body = json!({ "password": PASSWORD });
    let reauth = stack
        .call(Call::new(Method::POST, REAUTH, &creds).json(&body), 10)
        .await;
    assert_eq!(reauth.status, StatusCode::NO_CONTENT, "{}", reauth.text());
    let changed = stack
        .change_password(&creds, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let rotated = Credentials::from(&changed);
    assert_code(
        &stack.get(ME, Some(&second.session), 11).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    let row = stack.session_by_token(&rotated.session).await;
    assert_eq!(
        row.last_auth_at,
        Timestamp::try_from(START + time::Duration::minutes(6))
            .unwrap()
            .to_string(),
        "rotation keeps the re-authentication stamp"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_failures_change_nothing() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let creds = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;
    stack.trusted_device(ada, "device-ada", None).await;
    let before = stack.security_state(ada).await;
    let sessions_before = stack.sessions_of(ada).await;
    let devices_before = stack.devices_of(ada).await;

    let assert_unchanged = async |label: &str| {
        assert_eq!(stack.security_state(ada).await, before, "{label}");
        assert_eq!(stack.sessions_of(ada).await, sessions_before, "{label}");
        assert_eq!(stack.devices_of(ada).await, devices_before, "{label}");
        assert_eq!(
            stack.get(ME, Some(&second.session), 11).await.status,
            StatusCode::OK
        );
        assert!(
            stack.audit_rows("PASSWORD_CHANGED").await.is_empty(),
            "{label}"
        );
    };

    let wrong = stack.change_password(&creds, WRONG, NEW_PASSWORD, 10).await;
    assert_code(&wrong, StatusCode::FORBIDDEN, "PASSWORD_CURRENT_INVALID");
    assert!(wrong.set_cookies().is_empty());
    assert_unchanged("wrong current password").await;
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM account_lockouts")
            .await,
        0
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts WHERE result <> 'success'")
            .await,
        0,
        "a profile confirmation is not a login attempt"
    );

    let short = stack.change_password(&creds, PASSWORD, "short", 10).await;
    assert_code(
        &short,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_eq!(short.json()["error"]["details"]["minLength"], 8);
    assert_unchanged("policy floor").await;
    stack.setting("password_min_length", "integer", "24").await;
    let configured = stack
        .change_password(&creds, PASSWORD, NEW_PASSWORD, 10)
        .await;
    assert_code(
        &configured,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_eq!(configured.json()["error"]["details"]["minLength"], 24);
    assert_unchanged("configured policy").await;
    stack.setting("password_min_length", "integer", "8").await;

    for (body, fields) in [
        (
            json!({ "newPassword": NEW_PASSWORD }),
            json!(["currentPassword"]),
        ),
        (
            json!({ "currentPassword": PASSWORD }),
            json!(["newPassword"]),
        ),
        (json!({}), json!(["currentPassword", "newPassword"])),
        (
            json!({ "currentPassword": "", "newPassword": NEW_PASSWORD }),
            json!(["currentPassword"]),
        ),
        (
            json!({ "currentPassword": PASSWORD, "newPassword": NEW_PASSWORD, "userId": ada.to_string() }),
            json!(["body"]),
        ),
    ] {
        let rejected = stack
            .call(
                Call::new(Method::POST, PASSWORD_PATH, &creds).json(&body),
                10,
            )
            .await;
        assert_eq!(validation_fields(&rejected), fields, "{body}");
    }
    let malformed = stack
        .call(
            Call::new(Method::POST, PASSWORD_PATH, &creds).raw("{\"currentPassword\""),
            10,
        )
        .await;
    assert_code(&malformed, StatusCode::BAD_REQUEST, "INVALID_JSON");
    let wrong_type = stack
        .call(
            Call {
                content_type: Some("text/plain"),
                ..Call::new(Method::POST, PASSWORD_PATH, &creds)
                    .json(&json!({ "currentPassword": PASSWORD, "newPassword": NEW_PASSWORD }))
            },
            10,
        )
        .await;
    assert_code(
        &wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );
    let no_csrf = stack
        .call(
            Call {
                csrf_header: None,
                ..Call::new(Method::POST, PASSWORD_PATH, &creds)
                    .json(&json!({ "currentPassword": PASSWORD, "newPassword": NEW_PASSWORD }))
            },
            10,
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    assert!(
        no_csrf.error_code().starts_with("CSRF_"),
        "{}",
        no_csrf.text()
    );
    assert_unchanged("shape and transport failures").await;

    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    let minted = stack
        .sessions
        .mint(NewSession {
            user_id: sso,
            auth_method: AuthMethod::External,
            ip_address: None,
            user_agent: None,
        })
        .await
        .unwrap();
    let sso_creds = Credentials {
        session: minted.session_token.expose_secret().clone(),
        csrf: minted.csrf_token.expose_secret().clone(),
    };
    let refused = stack
        .change_password(&sso_creds, "anything at all", NEW_PASSWORD, 12)
        .await;
    assert_code(&refused, StatusCode::FORBIDDEN, "PASSWORD_CURRENT_INVALID");
    assert_eq!(stack.security_state(sso).await.password_hash, None);
    assert_eq!(
        stack.get(ME, Some(&sso_creds.session), 12).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_password_change_transaction_is_all_or_nothing() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("ada", "ada@example.test", &hash)
        })
        .await;
    let current = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;
    stack.trusted_device(ada, "device-ada", None).await;
    let principal = stack.principal(&current.session).await;
    let replacement = hash_password(NEW_PASSWORD.as_bytes()).unwrap();
    let stale = hash_password(WRONG.as_bytes()).unwrap();
    let credentials = stack.sessions.prepare_credentials().unwrap();
    let client = ClientMetadata::none();
    let before = stack.security_state(ada).await;
    let sessions_before = stack.sessions_of(ada).await;
    let devices_before = stack.devices_of(ada).await;

    let commit = async |verified: &Secret<String>| {
        stack
            .pools
            .write_tx(&stack.clock, "profile.test_commit", async |tx| {
                stack
                    .profile
                    .commit_password_change(
                        tx,
                        VerifiedChange {
                            principal: &principal,
                            verified,
                            replacement: &replacement,
                            credentials: &credentials,
                            client: &client,
                        },
                    )
                    .await
            })
            .await
    };
    let assert_unchanged = async |label: &str, sessions: &[SessionRow]| {
        assert_eq!(stack.security_state(ada).await, before, "{label}");
        assert_eq!(stack.sessions_of(ada).await, sessions, "{label}");
        assert_eq!(stack.devices_of(ada).await, devices_before, "{label}");
        assert!(
            stack.audit_rows("PASSWORD_CHANGED").await.is_empty(),
            "{label}"
        );
    };

    let raced = commit(&stale).await;
    assert!(
        matches!(raced, Err(ProfileError::CurrentPasswordInvalid)),
        "{:?}",
        raced.as_ref().err()
    );
    assert_unchanged("credential changed after verification", &sessions_before).await;
    assert_eq!(
        stack.get(ME, Some(&second.session), 11).await.status,
        StatusCode::OK
    );

    stack
        .execute(&format!(
            "UPDATE sessions SET state = 'revoked', revoked_at = '2026-09-25T12:00:00.000Z',
                                 revoked_reason = 'logout'
              WHERE token_hash = '{}'",
            digest(&current.session)
        ))
        .await;
    let sessions_revoked_current = stack.sessions_of(ada).await;
    let verified = Secret::new(before.password_hash.clone().unwrap());
    let late_failure = commit(&verified).await;
    assert!(
        matches!(
            late_failure,
            Err(ProfileError::Session(SessionError::AuthRequired))
        ),
        "{:?}",
        late_failure.as_ref().err()
    );
    assert_unchanged(
        "rotation failed after the password, sessions and devices were written",
        &sessions_revoked_current,
    )
    .await;
    assert_eq!(
        stack.get(ME, Some(&second.session), 11).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_profile_usage_accounting() {
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
    let root_admin = stack
        .user(UserSpec::local("root", "root@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{root_admin}'"
        ))
        .await;
    let ada_creds = stack.signed_in("ada", 10).await;
    let bob_creds = stack.signed_in("bob", 11).await;
    let admin_creds = stack.signed_in("root", 12).await;
    let usage = async |creds: &Credentials| {
        let fetched = stack.get(USAGE, Some(&creds.session), 13).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    };

    assert_eq!(
        usage(&ada_creds).await,
        json!({
            "usedBytes": 0, "myFilesBytes": 0, "receivedBytes": 0, "reservedBytes": 0,
            "quotaBytes": null, "effectiveMaxFileSizeBytes": null, "usedBytesExact": true
        })
    );

    let mut seed = 0_u32;
    let mut object = async |owner: UserId| {
        seed += 1;
        let id = format!("obj-{owner}-{seed}");
        stack
            .execute(&format!(
                "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                                              created_at, updated_at, finalized_at)
                 VALUES ('{id}', 'objects/00/00/{seed:032x}', 'local', 0, 'active', 1,
                         '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z',
                         '2026-09-25T12:00:00.000Z')"
            ))
            .await;
        id
    };
    for (owner, name, size) in [
        (ada, "a.bin", 3_000_i64),
        (ada, "b.bin", 1_000),
        (bob, "c.bin", 7),
    ] {
        let storage = object(owner).await;
        stack
            .execute(&format!(
                "INSERT INTO files (id, owner_id, storage_object_id, name, name_normalized,
                                    size_bytes, created_at, updated_at)
                 VALUES ('file-{storage}', '{owner}', '{storage}', '{name}', '{name}', {size},
                         '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')"
            ))
            .await;
    }
    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 4000 WHERE id = '{ada}'"
        ))
        .await;
    let my_files_only = usage(&ada_creds).await;
    assert_eq!(my_files_only["usedBytes"], 4000);
    assert_eq!(my_files_only["myFilesBytes"], 4000);
    assert_eq!(my_files_only["receivedBytes"], 0);

    for (owner, alias) in [
        (ada, "ada-inbox"),
        (bob, "bob-inbox"),
        (root_admin, "root-inbox"),
    ] {
        stack
            .execute(&format!(
                "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
                 VALUES ('rs-{alias}', '{owner}', 'public-{alias:0>16}', '{alias}',
                         '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')"
            ))
            .await;
    }
    for (owner, alias, size) in [
        (bob, "bob-inbox", 500_i64),
        (bob, "bob-inbox", 250),
        (ada, "ada-inbox", 600),
        (root_admin, "root-inbox", 900),
    ] {
        let storage = object(owner).await;
        stack
            .execute(&format!(
                "INSERT INTO received_files (id, owner_id, reverse_share_id, storage_object_id,
                                             name, name_normalized, size_bytes, received_at,
                                             updated_at)
                 VALUES ('rcv-{storage}', '{owner}', 'rs-{alias}', '{storage}', '{storage}.bin',
                         '{storage}.bin',
                         {size}, '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')"
            ))
            .await;
    }
    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 757 WHERE id = '{bob}'"
        ))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 4600 WHERE id = '{ada}'"
        ))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 900 WHERE id = '{root_admin}'"
        ))
        .await;
    let bob_usage = usage(&bob_creds).await;
    assert_eq!(bob_usage["myFilesBytes"], 7);
    assert_eq!(bob_usage["receivedBytes"], 750);
    assert_eq!(bob_usage["usedBytes"], 757);
    let admin_usage = usage(&admin_creds).await;
    assert_eq!(
        admin_usage,
        json!({
            "usedBytes": 900, "myFilesBytes": 0, "receivedBytes": 900, "reservedBytes": 0,
            "quotaBytes": null, "effectiveMaxFileSizeBytes": null, "usedBytesExact": true
        }),
        "received-only accounting; an admin is accounted like any user"
    );
    let combined = usage(&ada_creds).await;
    assert_eq!(combined["myFilesBytes"], 4000);
    assert_eq!(combined["receivedBytes"], 600);
    assert_eq!(combined["usedBytes"], 4600);

    for (id, state, bytes, extra) in [
        ("res-held-1", "held", 1_000_i64, "NULL, NULL, NULL"),
        ("res-held-2", "held", 24, "NULL, NULL, NULL"),
        (
            "res-committed",
            "committed",
            5_000,
            "5000, '2026-09-25T12:00:00.000Z', NULL",
        ),
        (
            "res-released",
            "released",
            7_000,
            "NULL, '2026-09-25T12:00:00.000Z', 'canceled'",
        ),
    ] {
        stack
            .execute(&format!(
                "INSERT INTO transfer_sessions (id, context, user_id, provider, created_at,
                                                updated_at, expires_at)
                 VALUES ('ts-{id}', 'my_files', '{ada}', 'local', '2026-09-25T12:00:00.000Z',
                         '2026-09-25T12:00:00.000Z', '2026-09-26T12:00:00.000Z')"
            ))
            .await;
        stack
            .execute(&format!(
                "INSERT INTO quota_reservations (id, user_id, transfer_session_id, context,
                                                 reserved_bytes, committed_bytes, settled_at,
                                                 release_reason, state, created_at, expires_at)
                 VALUES ('{id}', '{ada}', 'ts-{id}', 'my_files', {bytes}, {extra}, '{state}',
                         '2026-09-25T12:00:00.000Z', '2026-09-26T12:00:00.000Z')"
            ))
            .await;
    }
    let reserved = usage(&ada_creds).await;
    assert_eq!(
        reserved["reservedBytes"], 1024,
        "only held reservations count"
    );
    assert_eq!(reserved["usedBytes"], 4600);
    assert_eq!(usage(&bob_creds).await["reservedBytes"], 0);

    stack
        .setting_in("quotas", "default_user_quota_bytes", "integer", "10000")
        .await;
    stack
        .setting_in("quotas", "max_file_size_bytes", "integer", "2048")
        .await;
    let inherit = usage(&ada_creds).await;
    assert_eq!(inherit["quotaBytes"], 10000);
    assert_eq!(inherit["effectiveMaxFileSizeBytes"], 2048);
    assert_eq!(
        usage(&admin_creds).await["quotaBytes"],
        10000,
        "no role-based bypass"
    );
    stack
        .execute(&format!(
            "UPDATE users SET quota_override_mode = 'unlimited' WHERE id = '{ada}'"
        ))
        .await;
    assert_eq!(usage(&ada_creds).await["quotaBytes"], Value::Null);
    stack
        .execute(&format!(
            "UPDATE users SET quota_override_mode = 'bytes', quota_bytes = 0 WHERE id = '{ada}'"
        ))
        .await;
    let zero = usage(&ada_creds).await;
    assert_eq!(zero["quotaBytes"], 0, "an explicit zero is not Unlimited");
    assert_eq!(zero["usedBytesExact"], true);
    stack
        .execute(&format!(
            "UPDATE users SET quota_override_mode = 'bytes', quota_bytes = 123 WHERE id = '{bob}'"
        ))
        .await;
    assert_eq!(usage(&bob_creds).await["quotaBytes"], 123);

    stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 9007199254740993 WHERE id = '{bob}'"
        ))
        .await;
    let clamped = usage(&bob_creds).await;
    assert_eq!(clamped["usedBytes"], 9_007_199_254_740_991_u64);
    assert_eq!(clamped["usedBytesExact"], false);
    stack.stop().await;
}

impl Stack {
    async fn setting_in(&self, group: &str, key: &str, value_type: &str, value_json: &str) {
        self.execute(&format!(
            "INSERT INTO app_settings (key, group_name, value_type, value_json, is_secret, updated_at)
             VALUES ('{key}', '{group}', '{value_type}', '{value_json}', 0, '2026-09-25T12:00:00.000Z')
             ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json"
        ))
        .await;
        self.settings.reload().await.unwrap();
    }
}
