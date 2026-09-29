use utoipa_axum::routes;

use super::profile::Call;
use super::*;
use crate::infra::http::extractors::{Admin, Authenticated};
use crate::infra::ratelimit::class::Stage;
use crate::infra::ratelimit::key::Subject;
use crate::infra::ratelimit::RateLimitPrincipal;

const READ: &str = "/api/v1/test/rl/read";
const WRITE: &str = "/api/v1/test/rl/write";
const ADMIN_WRITE: &str = "/api/v1/test/rl/admin-write";
const ANONYMOUS_READ: &str = "/api/v1/test/rl/anonymous-read";
const PATH_PARAMETER: &str = "0192f3a7-0000-7000-8000-000000000000";

#[utoipa::path(get, path = "/api/v1/test/rl/read", responses((status = 204)))]
async fn read(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/test/rl/write", responses((status = 204)))]
async fn write(Authenticated(_): Authenticated) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/test/rl/admin-write", responses((status = 204)))]
async fn admin_write(Admin(_): Admin) -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(get, path = "/api/v1/test/rl/anonymous-read", responses((status = 204)))]
async fn anonymous_read() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn probes() -> Routes<AppState> {
    let policy = |auth, class| RoutePolicy::new(auth, class, Transport::ControlPlane);
    Routes::new()
        .route(
            policy(AuthClass::Authenticated, RateLimitClass::Read),
            routes!(read),
        )
        .route(
            policy(AuthClass::Authenticated, RateLimitClass::Write),
            routes!(write),
        )
        .route(
            policy(AuthClass::Admin, RateLimitClass::AdminWrite),
            routes!(admin_write),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::Read),
            routes!(anonymous_read),
        )
}

async fn probe_stack(root: &Path) -> Stack {
    Stack::start_with(root, &TestClock::new(START), probes()).await
}

fn assert_throttled(fetched: &Fetched, class: RateLimitClass) {
    assert_eq!(
        fetched.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        fetched.text()
    );
    assert_eq!(fetched.error_code(), "RATE_LIMITED");
    assert_eq!(fetched.json()["error"]["details"]["scope"], class.as_str());
    assert!(fetched.retry_after().is_some());
}

impl Stack {
    async fn probe(
        &self,
        method: Method,
        path: &str,
        credentials: &Credentials,
        host: u8,
    ) -> Fetched {
        self.call(Call::new(method, path, credentials), host).await
    }

    async fn session_principal(&self, credentials: &Credentials) -> RateLimitPrincipal {
        let id: String = sqlx::query_scalar("SELECT id FROM sessions WHERE token_hash = ?1")
            .bind(digest(&credentials.session))
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap();
        RateLimitPrincipal::session(id.as_bytes())
    }

    async fn exhaust_session_bucket(&self, class: RateLimitClass, credentials: &Credentials) {
        let subject = Subject::new(std::net::Ipv4Addr::new(203, 0, 113, 1).into())
            .with_principal(Some(self.session_principal(credentials).await));
        let mut admitted = 0;
        while self.limiter.admit(class, Stage::Session, &subject).is_ok() {
            admitted += 1;
            assert!(admitted <= 1_000, "{class} never throttled");
        }
    }
}

async fn assert_session_isolation(
    stack: &Stack,
    method: &Method,
    path: &str,
    class: RateLimitClass,
    sessions: (&Credentials, &Credentials),
    hosts: (u8, u8),
) {
    let (first, second) = sessions;
    let (home, roaming) = hosts;
    let burst = class.buckets()[0].quota().burst_size().get();
    for attempt in 0..burst {
        let fetched = stack.probe(method.clone(), path, first, home).await;
        assert_eq!(
            fetched.status,
            StatusCode::NO_CONTENT,
            "{class} attempt {attempt}: {}",
            fetched.text()
        );
    }
    assert_throttled(&stack.probe(method.clone(), path, first, home).await, class);
    assert_throttled(
        &stack.probe(method.clone(), path, first, roaming).await,
        class,
    );
    assert_eq!(
        stack.probe(method.clone(), path, second, home).await.status,
        StatusCode::NO_CONTENT
    );

    stack.clock.advance(Duration::from_secs(60));
    assert_eq!(
        stack
            .probe(method.clone(), path, first, roaming)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn svc_rl_read_is_session_keyed() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let first = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;

    assert_session_isolation(
        &stack,
        &Method::GET,
        READ,
        RateLimitClass::Read,
        (&first, &second),
        (20, 21),
    )
    .await;

    let third = stack.signed_in("ada", 12).await;
    for _ in 0..300 {
        assert_eq!(
            stack.get(ME, Some(&third.session), 20).await.status,
            StatusCode::OK
        );
    }
    let me = stack.get(ME, Some(&third.session), 22).await;
    assert_throttled(&me, RateLimitClass::Read);
    assert!(!me.text().contains(&third.session));
    assert_eq!(
        stack.get(ME, Some(&first.session), 20).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_write_is_session_keyed() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let first = stack.signed_in("ada", 10).await;
    let second = stack.signed_in("ada", 11).await;

    assert_session_isolation(
        &stack,
        &Method::POST,
        WRITE,
        RateLimitClass::Write,
        (&first, &second),
        (30, 31),
    )
    .await;
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_admin_write_is_session_keyed() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    let root_admin = stack
        .user(UserSpec::local("root", "root@example.test", &hash))
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{root_admin}'"
        ))
        .await;
    let first = stack.signed_in("root", 10).await;
    let second = stack.signed_in("root", 11).await;

    assert_session_isolation(
        &stack,
        &Method::POST,
        ADMIN_WRITE,
        RateLimitClass::AdminWrite,
        (&first, &second),
        (40, 41),
    )
    .await;
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_read_falls_back_to_ip_without_a_session() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;

    for _ in 0..300 {
        assert_eq!(
            stack.get(ANONYMOUS_READ, None, 50).await.status,
            StatusCode::NO_CONTENT
        );
    }
    assert_throttled(
        &stack.get(ANONYMOUS_READ, None, 50).await,
        RateLimitClass::Read,
    );
    assert_eq!(
        stack.get(ANONYMOUS_READ, None, 51).await.status,
        StatusCode::NO_CONTENT
    );

    let forged = "palmr_session_not_a_live_session";
    for _ in 0..300 {
        let fetched = stack.get(READ, Some(forged), 52).await;
        assert_eq!(fetched.status, StatusCode::UNAUTHORIZED);
        assert_eq!(fetched.error_code(), "AUTH_REQUIRED");
    }
    assert_throttled(
        &stack.get(READ, Some(forged), 52).await,
        RateLimitClass::Read,
    );
    assert_throttled(&stack.get(READ, None, 52).await, RateLimitClass::Read);
    assert_eq!(
        stack.get(READ, None, 53).await.error_code(),
        "AUTH_REQUIRED"
    );

    let signed_in = stack.signed_in("ada", 52).await;
    assert_eq!(
        stack.get(READ, Some(&signed_in.session), 52).await.status,
        StatusCode::NO_CONTENT
    );
    stack.stop().await;
}

#[tokio::test]
async fn svc_rl_session_admission_follows_csrf_and_precedes_role() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let ada = stack.signed_in("ada", 10).await;
    stack
        .exhaust_session_bucket(RateLimitClass::Write, &ada)
        .await;
    stack
        .exhaust_session_bucket(RateLimitClass::AdminWrite, &ada)
        .await;

    let cross_site = stack
        .call(
            Call {
                origin: Some("https://evil.example.test"),
                ..Call::new(Method::POST, WRITE, &ada)
            },
            60,
        )
        .await;
    assert_eq!(cross_site.error_code(), "ORIGIN_NOT_ALLOWED");
    let missing_proof = stack
        .call(
            Call {
                csrf_header: None,
                ..Call::new(Method::POST, WRITE, &ada)
            },
            60,
        )
        .await;
    assert_eq!(missing_proof.status, StatusCode::FORBIDDEN);
    assert_ne!(missing_proof.error_code(), "RATE_LIMITED");

    assert_throttled(
        &stack.probe(Method::POST, WRITE, &ada, 60).await,
        RateLimitClass::Write,
    );
    assert_throttled(
        &stack.probe(Method::POST, ADMIN_WRITE, &ada, 60).await,
        RateLimitClass::AdminWrite,
    );
    stack.stop().await;
}

#[tokio::test]
async fn svc_rate_limit_class_enforced_on_every_session_keyed_route() {
    let root = TempDir::new().unwrap();
    let stack = probe_stack(root.path()).await;
    let hash = password_hash();
    stack
        .user(UserSpec::local("ada", "ada@example.test", &hash))
        .await;
    let exhausted = stack.signed_in("ada", 10).await;

    let inventory = application_routes().build().unwrap().inventory;
    let swept: Vec<(Method, String, RateLimitClass)> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.policy().rate_limit().has_buckets_at(Stage::Session))
        .map(|entry| {
            (
                entry.method().clone(),
                entry.path().to_owned(),
                entry.policy().rate_limit(),
            )
        })
        .collect();
    for (method, path) in [
        (Method::GET, ME),
        (Method::POST, LOGOUT),
        (Method::GET, "/api/v1/sessions"),
        (Method::DELETE, "/api/v1/sessions/{id}"),
        (Method::GET, "/api/v1/settings/effective"),
    ] {
        assert!(
            swept.iter().any(|(m, p, _)| *m == method && p == path),
            "{method} {path} is not session-keyed"
        );
    }

    let mut exhausted_classes = Vec::new();
    for (index, (method, path, class)) in swept.iter().enumerate() {
        if !exhausted_classes.contains(class) {
            stack.exhaust_session_bucket(*class, &exhausted).await;
            exhausted_classes.push(*class);
        }
        let concrete = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    PATH_PARAMETER
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let host = 100 + u8::try_from(index).unwrap();
        let fetched = stack
            .call(Call::new(method.clone(), &concrete, &exhausted), host)
            .await;
        assert_eq!(
            fetched.status,
            StatusCode::TOO_MANY_REQUESTS,
            "{method} {path} did not admit its {class} session bucket: {}",
            fetched.text()
        );
        assert_eq!(
            fetched.json()["error"]["details"]["scope"],
            class.as_str(),
            "{method} {path}"
        );
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM sessions WHERE state = 'active'")
            .await,
        1
    );
    stack.stop().await;
}
