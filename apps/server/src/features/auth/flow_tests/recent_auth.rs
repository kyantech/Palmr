use utoipa_axum::routes;

use super::*;
use crate::features::auth::routes::REAUTHENTICATE_ROUTE;
use crate::features::auth::sessions::{AuthMethod, NewSession};
use crate::infra::http::extractors::{AdminRecentAuth, AuthenticatedRecentAuth};

const REAUTH: &str = "/api/v1/auth/reauthenticate";
const RECENT: &str = "/api/v1/test/recent";
const ADMIN_RECENT: &str = "/api/v1/test/admin-recent";
const EFFECTIVE: &str = "/api/v1/settings/effective";

#[utoipa::path(get, path = "/api/v1/test/recent", responses((status = 204)))]
async fn recent(AuthenticatedRecentAuth(_): AuthenticatedRecentAuth) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/test/admin-recent", responses((status = 204)))]
async fn admin_recent(AdminRecentAuth(_): AdminRecentAuth) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn probes() -> Routes<AppState> {
    let policy = |class| RoutePolicy::new(class, RateLimitClass::None, Transport::ControlPlane);
    Routes::new()
        .route(policy(AuthClass::AuthenticatedRecentAuth), routes!(recent))
        .route(policy(AuthClass::AdminRecentAuth), routes!(admin_recent))
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct SessionRow {
    id: String,
    token_hash: String,
    csrf_token_hash: String,
    created_at: String,
    last_seen_at: String,
    last_auth_at: String,
    absolute_expires_at: String,
}

async fn session_row(stack: &Stack, raw: &str) -> SessionRow {
    sqlx::query_as(
        "SELECT id, token_hash, csrf_token_hash, created_at, last_seen_at, last_auth_at,
                absolute_expires_at
           FROM sessions WHERE token_hash = ?1",
    )
    .bind(digest(raw))
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap()
}

fn stamp(at: OffsetDateTime) -> String {
    Timestamp::try_from(at).unwrap().to_string()
}

impl Stack {
    async fn reauth(&self, credentials: &Credentials, body: &str, host: u8) -> Fetched {
        let request = Request::builder()
            .method(Method::POST)
            .uri(REAUTH)
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, BASE_URL)
            .header(
                COOKIE,
                format!(
                    "palmr_session={}; palmr_csrf={}",
                    credentials.session, credentials.csrf
                ),
            )
            .header(CSRF_HEADER, &credentials.csrf)
            .body(Body::from(body.to_owned()))
            .unwrap();
        self.send(with_peer(request, host)).await
    }

    async fn reauth_password(
        &self,
        credentials: &Credentials,
        password: &str,
        host: u8,
    ) -> Fetched {
        let body = json!({ "password": password, "totpCode": null });
        self.reauth(credentials, &body.to_string(), host).await
    }

    async fn minted(&self, user: UserId) -> Credentials {
        let minted = self
            .sessions
            .mint(NewSession {
                user_id: user,
                auth_method: AuthMethod::External,
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
}

#[tokio::test]
async fn it_recent_auth_window() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start_with(root.path(), &clock, probes()).await;
    stack.setting("recent_auth_minutes", "integer", "7").await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{ada}'"
        ))
        .await;
    let current = stack.signed_in("ada", 10).await;
    let other = stack.signed_in("ada@example.test", 11).await;
    let signed_in = session_row(&stack, &current.session).await;
    let other_before = session_row(&stack, &other.session).await;
    assert_eq!(signed_in.last_auth_at, stamp(START));

    for path in [RECENT, ADMIN_RECENT] {
        assert_eq!(
            stack.get(path, Some(&current.session), 12).await.status,
            StatusCode::NO_CONTENT
        );
    }
    let me = stack.get(ME, Some(&current.session), 12).await;
    assert_eq!(
        me.json()["session"]["recentAuthUntil"],
        stamp(START + Duration::from_secs(7 * 60))
    );

    clock.advance(Duration::from_secs(7 * 60));
    for path in [RECENT, ADMIN_RECENT] {
        assert_eq!(
            stack.get(path, Some(&current.session), 13).await.status,
            StatusCode::NO_CONTENT,
            "the window boundary is inclusive"
        );
    }
    clock.advance(Duration::from_millis(1));
    for path in [RECENT, ADMIN_RECENT] {
        let expired = stack.get(path, Some(&current.session), 14).await;
        assert_eq!(expired.status, StatusCode::FORBIDDEN);
        assert_eq!(expired.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
        assert_eq!(expired.json()["error"]["details"]["method"], "password");
    }

    clock.advance(Duration::from_secs(30));
    let wrong = stack.reauth_password(&current, WRONG, 15).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.error_code(), "AUTH_INVALID_CREDENTIALS");
    assert!(wrong.set_cookies().is_empty());
    assert_eq!(
        session_row(&stack, &current.session).await.last_auth_at,
        stamp(START)
    );
    assert_eq!(
        stack.get(ME, Some(&current.session), 15).await.json()["session"]["recentAuthUntil"],
        stamp(START + Duration::from_secs(7 * 60))
    );

    clock.advance(Duration::from_secs(5));
    let reauthenticated_at = clock_now(&clock);
    let proven = stack.reauth_password(&current, PASSWORD, 16).await;
    assert_eq!(proven.status, StatusCode::NO_CONTENT, "{}", proven.text());
    assert!(proven.body.is_empty());
    assert!(proven.set_cookies().is_empty());

    let after = session_row(&stack, &current.session).await;
    assert_eq!(after.last_auth_at, stamp(reauthenticated_at));
    assert_eq!(
        SessionRow {
            last_auth_at: signed_in.last_auth_at.clone(),
            last_seen_at: signed_in.last_seen_at.clone(),
            ..after.clone()
        },
        signed_in,
        "id, token, CSRF digest, creation and absolute expiry are unchanged"
    );
    assert_eq!(session_row(&stack, &other.session).await, other_before);

    for path in [RECENT, ADMIN_RECENT] {
        assert_eq!(
            stack.get(path, Some(&current.session), 17).await.status,
            StatusCode::NO_CONTENT
        );
    }
    let me = stack.get(ME, Some(&current.session), 17).await;
    assert_eq!(me.json()["session"]["id"], signed_in.id);
    assert_eq!(
        me.json()["session"]["recentAuthUntil"],
        stamp(reauthenticated_at + Duration::from_secs(7 * 60))
    );
    let expired_other = stack.get(RECENT, Some(&other.session), 18).await;
    assert_eq!(expired_other.error_code(), "AUTH_RECENT_AUTH_REQUIRED");

    stack.setting("recent_auth_minutes", "integer", "1").await;
    clock.advance(Duration::from_secs(61));
    assert_eq!(
        stack
            .get(RECENT, Some(&current.session), 19)
            .await
            .error_code(),
        "AUTH_RECENT_AUTH_REQUIRED",
        "the extractor follows the effective setting"
    );
    assert_eq!(
        stack.get(ME, Some(&current.session), 19).await.json()["session"]["recentAuthUntil"],
        stamp(reauthenticated_at + Duration::from_secs(60))
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_recent_auth_not_refreshed_by_activity() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start_with(root.path(), &clock, probes()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    let until = stamp(START + Duration::from_secs(5 * 60));

    let mut last_seen = session_row(&stack, &current.session).await.last_seen_at;
    for step in 1..=3_u8 {
        clock.advance(Duration::from_secs(90));
        let me = stack.get(ME, Some(&current.session), 10 + step).await;
        assert_eq!(me.status, StatusCode::OK);
        assert_eq!(me.json()["session"]["recentAuthUntil"], until);
        assert_eq!(
            stack
                .get(EFFECTIVE, Some(&current.session), 10 + step)
                .await
                .status,
            StatusCode::OK
        );
        assert_eq!(
            stack
                .get(RECENT, Some(&current.session), 10 + step)
                .await
                .status,
            StatusCode::NO_CONTENT
        );
        let row = session_row(&stack, &current.session).await;
        assert!(
            row.last_seen_at > last_seen,
            "activity touches last_seen_at"
        );
        assert_eq!(row.last_auth_at, stamp(START));
        last_seen = row.last_seen_at;
    }

    clock.advance(Duration::from_secs(90));
    let me = stack.get(ME, Some(&current.session), 20).await;
    assert_eq!(me.json()["session"]["recentAuthUntil"], until);
    let expired = stack.get(RECENT, Some(&current.session), 20).await;
    assert_eq!(expired.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    let invalid = stack
        .reauth(&current, &json!({ "password": "" }).to_string(), 21)
        .await;
    assert_eq!(invalid.error_code(), "VALIDATION_ERROR");
    let row = session_row(&stack, &current.session).await;
    assert!(row.last_seen_at > last_seen);
    assert_eq!(row.last_auth_at, stamp(START));
    stack.stop().await;
}

#[tokio::test]
async fn it_reauthenticate_request_contract() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("temp", "temp@example.test", &hash)
        })
        .await;
    let current = stack.signed_in("ada", 10).await;

    let malformed = stack.reauth(&current, "{\"password\":", 11).await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed.error_code(), "INVALID_JSON");

    for (body, fields) in [
        (json!({}), json!(["password"])),
        (json!({ "password": null }), json!(["password"])),
        (json!({ "password": "" }), json!(["password"])),
        (json!({ "password": 7 }), json!(["password"])),
        (
            json!({ "password": PASSWORD, "totpCode": 492_013 }),
            json!(["totpCode"]),
        ),
        (
            json!({ "password": PASSWORD, "userId": "x" }),
            json!(["body"]),
        ),
        (
            json!({ "password": PASSWORD, "identifier": "ada" }),
            json!(["body"]),
        ),
    ] {
        let fetched = stack.reauth(&current, &body.to_string(), 12).await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(fetched.error_code(), "VALIDATION_ERROR", "{body}");
        assert_eq!(
            fetched.json()["error"]["details"]["fields"],
            fields,
            "{body}"
        );
    }
    clock.advance(Duration::from_secs(61));

    let body = json!({ "password": PASSWORD }).to_string();
    let form = stack
        .send(with_peer(
            Request::builder()
                .method(Method::POST)
                .uri(REAUTH)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(ORIGIN, BASE_URL)
                .header(
                    COOKIE,
                    format!(
                        "palmr_session={}; palmr_csrf={}",
                        current.session, current.csrf
                    ),
                )
                .header(CSRF_HEADER, &current.csrf)
                .body(Body::from("password=x"))
                .unwrap(),
            13,
        ))
        .await;
    assert_eq!(form.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let without_header = stack
        .post_json(
            REAUTH,
            &body,
            14,
            Some(&format!(
                "palmr_session={}; palmr_csrf={}",
                current.session, current.csrf
            )),
        )
        .await;
    assert_eq!(without_header.status, StatusCode::FORBIDDEN);
    assert_eq!(without_header.error_code(), "CSRF_TOKEN_MISSING");

    let other = stack.signed_in("ada", 15).await;
    let foreign_csrf = stack
        .reauth(
            &Credentials {
                session: current.session.clone(),
                csrf: other.csrf.clone(),
            },
            &body,
            16,
        )
        .await;
    assert_eq!(foreign_csrf.status, StatusCode::FORBIDDEN);
    assert_eq!(foreign_csrf.error_code(), "CSRF_TOKEN_INVALID");

    let cross_origin = stack
        .send(with_peer(
            Request::builder()
                .method(Method::POST)
                .uri(REAUTH)
                .header(CONTENT_TYPE, "application/json")
                .header(ORIGIN, "https://evil.example")
                .header(
                    COOKIE,
                    format!(
                        "palmr_session={}; palmr_csrf={}",
                        current.session, current.csrf
                    ),
                )
                .header(CSRF_HEADER, &current.csrf)
                .body(Body::from(body.clone()))
                .unwrap(),
            17,
        ))
        .await;
    assert_eq!(cross_origin.error_code(), "ORIGIN_NOT_ALLOWED");

    let anonymous = stack.post_json(REAUTH, &body, 18, None).await;
    assert_eq!(anonymous.status, StatusCode::FORBIDDEN);
    assert_eq!(anonymous.error_code(), "CSRF_TOKEN_MISSING");
    let unknown_session = stack
        .reauth(
            &Credentials {
                session: Token::mint().unwrap().encode().expose_secret().clone(),
                csrf: current.csrf.clone(),
            },
            &body,
            18,
        )
        .await;
    assert_eq!(unknown_session.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_session.error_code(), "AUTH_REQUIRED");

    let restricted = stack.signed_in("temp", 19).await;
    let refused = stack.reauth_password(&restricted, PASSWORD, 19).await;
    assert_eq!(refused.error_code(), "AUTH_PASSWORD_CHANGE_REQUIRED");

    assert_eq!(stack.auth.verifications_performed(), 3);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts WHERE result <> 'success'")
            .await,
        0
    );
    let entry = application_routes()
        .build()
        .unwrap()
        .inventory
        .get(&Method::POST, REAUTH)
        .map(|entry| entry.policy())
        .unwrap();
    assert_eq!(entry, REAUTHENTICATE_ROUTE);
    assert_eq!(entry.auth(), AuthClass::Authenticated);
    assert_eq!(entry.rate_limit().as_str(), "rl.auth.totp");
    stack.stop().await;
}

#[tokio::test]
async fn it_reauth_failures_feed_durable_lockout() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let mut stack = Stack::start_with(root.path(), &clock, probes()).await;
    stack.setting("max_login_attempts", "integer", "3").await;
    stack.setting("login_lockout_minutes", "integer", "7").await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    clock.advance(Duration::from_secs(10 * 60));

    let first = stack.reauth_password(&current, WRONG, 11).await;
    assert_eq!(first.error_code(), "AUTH_INVALID_CREDENTIALS");
    let (failed, until, lock_count) = stack.lock_row(ada).await;
    assert_eq!((failed, until, lock_count), (1, None, 0));

    let concurrent = (0..4_u8).map(|host| stack.reauth_password(&current, WRONG, 20 + host));
    for fetched in futures_util::future::join_all(concurrent).await {
        assert_eq!(fetched.error_code(), "AUTH_INVALID_CREDENTIALS");
    }
    let results = stack.attempt_results().await;
    let counted = results
        .iter()
        .filter(|(_, user, result)| {
            user.as_deref() == Some(&*ada.to_string()) && result == "bad_credentials"
        })
        .count();
    let refused = results
        .iter()
        .filter(|(_, _, result)| result == "locked_out")
        .count();
    assert_eq!((counted, refused), (3, 2));
    assert!(results
        .iter()
        .filter(|(_, _, result)| result != "success")
        .all(|(identifier, _, _)| identifier == "ada"));
    let (failed, until, lock_count) = stack.lock_row(ada).await;
    assert_eq!((failed, until.is_some(), lock_count), (3, true, 1));
    let locked_row = stack.lock_row(ada).await;

    let proven_while_locked = stack.reauth_password(&current, PASSWORD, 30).await;
    assert_eq!(proven_while_locked.status, StatusCode::UNAUTHORIZED);
    assert_eq!(proven_while_locked.error_code(), "AUTH_INVALID_CREDENTIALS");
    assert_eq!(stack.lock_row(ada).await, locked_row);
    assert_eq!(
        session_row(&stack, &current.session).await.last_auth_at,
        stamp(START)
    );
    stack.flush_audit().await;
    let actions: Vec<String> = stack
        .audit_actions()
        .await
        .into_iter()
        .map(|(action, ..)| action)
        .collect();
    assert!(actions.iter().any(|action| action == "LOGIN_LOCKED_OUT"));
    stack.stop().await;

    let restarted = Stack::start_with(root.path(), &clock, probes()).await;
    assert_eq!(restarted.lock_row(ada).await, locked_row);
    let after_restart = restarted.reauth_password(&current, PASSWORD, 31).await;
    assert_eq!(after_restart.error_code(), "AUTH_INVALID_CREDENTIALS");
    assert_eq!(
        restarted.login("ada", PASSWORD, 32).await.error_code(),
        "AUTH_LOCKED",
        "reauthentication failures lock password login too"
    );

    clock.advance(Duration::from_secs(7 * 60));
    let unlocked_at = clock_now(&clock);
    let proven = restarted.reauth_password(&current, PASSWORD, 33).await;
    assert_eq!(proven.status, StatusCode::NO_CONTENT, "{}", proven.text());
    assert_eq!(restarted.lock_row(ada).await, (0, None, 1));
    assert_eq!(
        session_row(&restarted, &current.session).await.last_auth_at,
        stamp(unlocked_at)
    );
    assert_eq!(
        restarted
            .get(RECENT, Some(&current.session), 34)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    restarted.stop().await;
}

#[tokio::test]
async fn it_reauth_second_factor_account_fails_closed() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start_with(root.path(), &clock, probes()).await;
    let hash = password_hash();
    let ada = stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let current = stack.signed_in("ada", 10).await;
    stack
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{ada}'"
        ))
        .await;
    clock.advance(Duration::from_secs(10 * 60));
    let before = session_row(&stack, &current.session).await;

    for body in [
        json!({ "password": PASSWORD }),
        json!({ "password": PASSWORD, "totpCode": "492013" }),
    ] {
        let refused = stack.reauth(&current, &body.to_string(), 11).await;
        assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(refused.error_code(), "INTERNAL_ERROR");
        assert!(refused.set_cookies().is_empty());
        let after = session_row(&stack, &current.session).await;
        assert_eq!(
            SessionRow {
                last_seen_at: before.last_seen_at.clone(),
                ..after
            },
            before
        );
    }
    assert_eq!(
        stack
            .get(RECENT, Some(&current.session), 12)
            .await
            .error_code(),
        "AUTH_RECENT_AUTH_REQUIRED"
    );
    assert_eq!(
        stack
            .reauth_password(&current, WRONG, 13)
            .await
            .error_code(),
        "AUTH_INVALID_CREDENTIALS"
    );
    assert_eq!(stack.lock_row(ada).await.0, 1);
    assert_eq!(
        session_row(&stack, &current.session).await.last_auth_at,
        before.last_auth_at
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_reauth_sso_only_account_fails_closed_until_external_reauth() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start_with(root.path(), &clock, probes()).await;
    let hash = password_hash();
    let sso = stack
        .user(UserSpec {
            hash: None,
            ..UserSpec::local("sso", "sso@example.test", &hash)
        })
        .await;
    let current = stack.minted(sso).await;
    clock.advance(Duration::from_secs(10 * 60));
    let before = session_row(&stack, &current.session).await;

    for body in [json!({}), json!({ "password": PASSWORD })] {
        let refused = stack.reauth(&current, &body.to_string(), 11).await;
        assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(refused.set_cookies().is_empty());
        assert_eq!(refused.error_code(), "INTERNAL_ERROR");
        assert!(!refused.text().contains("externalReauthUrl"));
    }
    assert_eq!(
        session_row(&stack, &current.session).await.last_auth_at,
        before.last_auth_at
    );
    let expired = stack.get(RECENT, Some(&current.session), 12).await;
    assert_eq!(expired.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(expired.json()["error"]["details"]["method"], "external");
    assert_eq!(stack.auth.verifications_performed(), 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM login_attempts")
            .await,
        0
    );
    stack.stop().await;
}
