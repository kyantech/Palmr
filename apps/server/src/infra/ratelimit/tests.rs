use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use http::header::RETRY_AFTER;
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use time::macros::datetime;
use tower::{Service, ServiceExt};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use utoipa_axum::routes;

use super::class::Stage;
use super::key::Subject;
use super::limiter::{Denial, MAX_TRACKED_KEYS, SWEEP_INTERVAL};
use super::{
    MfaPendingToken, NormalizedAccount, PublicScope, RateLimitClass, RateLimitGate,
    RateLimitPrincipal, RateLimiter, RetryAfter, Throttled,
};
use crate::app::auth_class::AuthClass;
use crate::app::router::{
    with_middleware, BytePath, Deadline, HttpEdge, RequestBody, ResponseEncoding, RoutePolicy,
    Routes, Transport,
};
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig, TrustProxy};
use crate::domain::clock::{Clock, TestClock};
use crate::domain::error_code::ErrorCode;
use crate::infra::http::csrf::{with_test_csrf, CsrfGuard};
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;
use crate::infra::telemetry::build_dispatch;

const CLIENT_A: &str = "198.51.100.10";
const CLIENT_B: &str = "198.51.100.20";
const TRUSTED_PROXY: &str = "10.0.0.2";
const SESSION_HEADER: &str = "x-test-session";
const ACCOUNT_HEADER: &str = "x-test-account";
const MFA_HEADER: &str = "x-test-mfa";
const RESPONSE_READ_CAP: usize = 64 * 1024;

const fn policy(auth: AuthClass, class: RateLimitClass) -> RoutePolicy {
    RoutePolicy::new(auth, class, Transport::ControlPlane)
}

#[derive(Clone, Default)]
struct Verifier(Arc<AtomicUsize>);

impl Verifier {
    fn verify(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn header_text<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

#[utoipa::path(get, path = "/test/rl/read", responses((status = 200)))]
async fn read() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(post, path = "/test/rl/login", responses((status = 200)))]
async fn login(Extension(verifier): Extension<Verifier>) -> StatusCode {
    verifier.verify();
    StatusCode::OK
}

#[utoipa::path(post, path = "/test/rl/reset", responses((status = 202)))]
async fn reset(
    Extension(verifier): Extension<Verifier>,
    gate: RateLimitGate,
    request: Request,
) -> Response {
    let Some(account) = header_text(&request, ACCOUNT_HEADER) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Err(rejection) = gate.with_account(NormalizedAccount::new(account)).admit() {
        return rejection.into_response();
    }
    verifier.verify();
    StatusCode::ACCEPTED.into_response()
}

#[utoipa::path(post, path = "/test/rl/totp", responses((status = 200)))]
async fn totp(gate: RateLimitGate, request: Request) -> Response {
    let gate = match header_text(&request, MFA_HEADER) {
        Some(token) => gate.with_mfa_pending(MfaPendingToken::new(token.as_bytes())),
        None => gate,
    };
    match gate.admit() {
        Ok(()) => StatusCode::OK.into_response(),
        Err(rejection) => rejection.into_response(),
    }
}

#[utoipa::path(post, path = "/test/rl/reset-twice", responses((status = 202)))]
async fn reset_twice(gate: RateLimitGate) -> Response {
    let account = NormalizedAccount::new("twice@example.com");
    let first = gate.clone().with_account(account).admit();
    let second = gate.with_account(account).admit();
    match (first, second) {
        (Ok(()), Err(rejection)) => rejection.into_response(),
        _ => StatusCode::OK.into_response(),
    }
}

#[utoipa::path(get, path = "/test/rl/public/{alias}", params(("alias" = String, Path)), responses((status = 200)))]
async fn public_read() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(post, path = "/test/rl/public/{alias}/authorize", params(("alias" = String, Path)), responses((status = 200)))]
async fn public_authorize(Extension(verifier): Extension<Verifier>) -> StatusCode {
    verifier.verify();
    StatusCode::OK
}

#[utoipa::path(get, path = "/test/rl/embed/{token}", params(("token" = String, Path)), responses((status = 200)))]
async fn embed_read() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(get, path = "/test/rl/none", responses((status = 200)))]
async fn unlimited() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(patch, path = "/test/rl/data", responses((status = 204)))]
async fn data_frame() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/test/rl/smtp-test", responses((status = 200)))]
async fn smtp_test() -> StatusCode {
    StatusCode::OK
}

#[utoipa::path(post, path = "/test/rl/provider-test", responses((status = 200)))]
async fn provider_test() -> StatusCode {
    StatusCode::OK
}

fn test_routes() -> Routes<()> {
    Routes::new()
        .route(
            policy(AuthClass::Public, RateLimitClass::Read),
            routes!(read),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::AuthLogin),
            routes!(login),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::AuthReset),
            routes!(reset),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::AuthReset),
            routes!(reset_twice),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::AuthTotp),
            routes!(totp),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::PublicRead),
            routes!(public_read),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::PublicPassword),
            routes!(public_authorize),
        )
        .route(
            policy(AuthClass::PublicGrant, RateLimitClass::PublicRead),
            routes!(embed_read),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::None),
            routes!(unlimited),
        )
        .route(
            RoutePolicy::new(
                AuthClass::PublicGrant,
                RateLimitClass::TransferData,
                Transport::BytePath(BytePath::new(
                    RequestBody::Streamed,
                    ResponseEncoding::Identity,
                    Deadline::IdleOnly,
                )),
            ),
            routes!(data_frame),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::EmailTest),
            routes!(smtp_test),
        )
        .route(
            policy(AuthClass::Public, RateLimitClass::ProviderTest),
            routes!(provider_test),
        )
}

async fn resolve_test_session(mut request: Request, next: Next) -> Response {
    if let Some(session) = header_text(&request, SESSION_HEADER).map(str::to_owned) {
        request
            .extensions_mut()
            .insert(RateLimitPrincipal::session(session.as_bytes()));
    }
    next.run(request).await
}

struct Harness<S> {
    clock: TestClock,
    limiter: Arc<RateLimiter>,
    verifier: Verifier,
    service: S,
}

fn harness(
) -> Harness<impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone> {
    let clock = TestClock::new(datetime!(2026-09-24 09:00 UTC));
    let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
        &str,
        &str,
    )>()))
    .unwrap()
    .config;
    let edge = HttpEdge::new(
        Arc::new(clock.clone()),
        TrustedProxies::new(&TrustProxy::AllowList(vec!["10.0.0.0/8".parse().unwrap()])),
        SecurityHeaders::new(&config),
        CsrfGuard::new(&config.base_url),
    );
    let verifier = Verifier::default();
    let router = test_routes()
        .build()
        .unwrap()
        .router
        .layer(Extension(verifier.clone()))
        .layer(from_fn(resolve_test_session));
    Harness {
        clock,
        limiter: Arc::clone(edge.rate_limits()),
        verifier,
        service: with_middleware(router, &edge),
    }
}

impl<S> Harness<S>
where
    S: Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone,
{
    async fn send(&self, request: Request) -> Response {
        self.service.clone().oneshot(request).await.unwrap()
    }

    async fn status(&self, request: Request) -> StatusCode {
        self.send(request).await.status()
    }
}

fn request(method: Method, uri: &str, peer: &str) -> http::request::Builder {
    let peer: IpAddr = peer.parse().unwrap();
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(SocketAddr::new(peer, 40_000)))
}

fn empty(builder: http::request::Builder) -> Request {
    builder.body(Body::empty()).unwrap()
}

fn get(uri: &str, peer: &str) -> Request {
    empty(request(Method::GET, uri, peer))
}

fn post(uri: &str, peer: &str) -> Request {
    empty(request(Method::POST, uri, peer))
}

fn reset_request(peer: &str, account: &str) -> Request {
    empty(request(Method::POST, "/test/rl/reset", peer).header(ACCOUNT_HEADER, account))
}

fn session_read(peer: &str, session: &str) -> Request {
    empty(request(Method::GET, "/test/rl/read", peer).header(SESSION_HEADER, session))
}

async fn body_json(response: Response) -> Value {
    let bytes = Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn retry_after_seconds(response: &Response) -> u64 {
    response
        .headers()
        .get(RETRY_AFTER)
        .expect("a throttled response carries Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn assert_throttled(response: Response, scope: RateLimitClass) -> u64 {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = retry_after_seconds(&response);
    assert!(retry_after > 0);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "RATE_LIMITED");
    assert_eq!(body["error"]["details"], json!({ "scope": scope.as_str() }));
    assert!(ErrorCode::RateLimited.retryable());
    retry_after
}

async fn exhaust<S>(harness: &Harness<S>, count: usize, build: impl Fn() -> Request)
where
    S: Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone,
{
    for attempt in 0..count {
        let status = harness.status(build()).await;
        assert!(status.is_success(), "attempt {attempt} returned {status}");
    }
}

#[tokio::test]
async fn svc_rate_limit_class_enforced() {
    let app = harness();

    exhaust(&app, 10, || {
        post("/test/rl/public/summer/authorize", CLIENT_A)
    })
    .await;
    assert_eq!(app.verifier.calls(), 10);
    assert_throttled(
        app.send(post("/test/rl/public/summer/authorize", CLIENT_A))
            .await,
        RateLimitClass::PublicPassword,
    )
    .await;
    assert_throttled(
        app.send(post("/test/rl/public/SUMMER/authorize", CLIENT_A))
            .await,
        RateLimitClass::PublicPassword,
    )
    .await;
    assert_eq!(app.verifier.calls(), 10);
    assert_eq!(
        app.status(post("/test/rl/public/winter/authorize", CLIENT_A))
            .await,
        StatusCode::OK
    );
    assert_eq!(
        app.status(post("/test/rl/public/summer/authorize", CLIENT_B))
            .await,
        StatusCode::OK
    );
    assert_eq!(app.verifier.calls(), 12);

    exhaust(&app, 300, || session_read(CLIENT_A, "session-one")).await;
    assert_throttled(
        app.send(session_read(CLIENT_A, "session-one")).await,
        RateLimitClass::Read,
    )
    .await;
    assert_eq!(
        app.status(session_read(CLIENT_A, "session-two")).await,
        StatusCode::OK
    );
    assert_eq!(
        app.status(get("/test/rl/read", CLIENT_A)).await,
        StatusCode::OK
    );

    let before_reset = app.verifier.calls();
    exhaust(&app, 3, || reset_request(CLIENT_A, "alice@example.com")).await;
    assert_throttled(
        app.send(reset_request(CLIENT_A, "bob@example.com")).await,
        RateLimitClass::AuthReset,
    )
    .await;
    assert_throttled(
        app.send(reset_request(CLIENT_B, "Alice@Example.com")).await,
        RateLimitClass::AuthReset,
    )
    .await;
    assert_eq!(
        app.status(reset_request(CLIENT_B, "carol@example.com"))
            .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(app.verifier.calls(), before_reset + 4);

    let before_login = app.verifier.calls();
    exhaust(&app, 10, || post("/test/rl/login", CLIENT_B)).await;
    for _ in 0..5 {
        assert_throttled(
            app.send(post("/test/rl/login", CLIENT_B)).await,
            RateLimitClass::AuthLogin,
        )
        .await;
    }
    assert_eq!(app.verifier.calls(), before_login + 10);

    exhaust(&app, 1_000, || get("/test/rl/none", CLIENT_A)).await;
    assert_eq!(app.limiter.tracked_keys(RateLimitClass::None), 0);

    exhaust(&app, 1_000, || {
        empty(with_test_csrf(request(
            Method::PATCH,
            "/test/rl/data",
            CLIENT_A,
        )))
    })
    .await;
    assert_eq!(app.limiter.tracked_keys(RateLimitClass::TransferData), 0);
    assert_eq!(app.limiter.tracked_keys(RateLimitClass::TransferControl), 0);
}

#[tokio::test]
async fn svc_rate_limit_totp_keys_on_mfa_challenge() {
    let app = harness();
    let challenge = |token: &'static str| {
        move || empty(request(Method::POST, "/test/rl/totp", CLIENT_A).header(MFA_HEADER, token))
    };

    exhaust(&app, 10, challenge("challenge-one")).await;
    assert_throttled(
        app.send(challenge("challenge-one")()).await,
        RateLimitClass::AuthTotp,
    )
    .await;
    assert_eq!(
        app.status(challenge("challenge-two")()).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn svc_rate_limit_deferred_gate_is_single_use() {
    let app = harness();
    let response = app.send(post("/test/rl/reset-twice", CLIENT_A)).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body_json(response).await["error"]["code"], "INTERNAL_ERROR");
}

#[tokio::test]
async fn svc_rate_limit_fails_closed_without_the_edge() {
    let router = test_routes()
        .build()
        .unwrap()
        .router
        .layer(Extension(Verifier::default()));
    let response = router
        .oneshot(post("/test/rl/login", CLIENT_A))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let unlimited = test_routes()
        .build()
        .unwrap()
        .router
        .oneshot(get("/test/rl/none", CLIENT_A))
        .await
        .unwrap();
    assert_eq!(unlimited.status(), StatusCode::OK);
}

#[tokio::test]
async fn svc_rate_limit_ignores_raw_forwarded_headers() {
    let app = harness();
    for attempt in 0..10_u8 {
        let spoofed = format!("192.0.2.{attempt}");
        let response = app
            .send(empty(
                request(Method::POST, "/test/rl/login", CLIENT_A)
                    .header("x-forwarded-for", spoofed.as_str())
                    .header("forwarded", format!("for={spoofed}"))
                    .header("x-real-ip", spoofed.as_str()),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_throttled(
        app.send(empty(
            request(Method::POST, "/test/rl/login", CLIENT_A)
                .header("x-forwarded-for", "192.0.2.200")
                .header("x-real-ip", "192.0.2.201"),
        ))
        .await,
        RateLimitClass::AuthLogin,
    )
    .await;
}

#[tokio::test]
async fn svc_rate_limit_keys_on_trusted_proxy_resolved_ip() {
    let app = harness();
    let via_proxy = |client: &'static str| {
        move || {
            empty(
                request(Method::POST, "/test/rl/login", TRUSTED_PROXY)
                    .header("x-forwarded-for", format!("192.0.2.99, {client}")),
            )
        }
    };

    exhaust(&app, 10, via_proxy(CLIENT_A)).await;
    assert_throttled(
        app.send(via_proxy(CLIENT_A)()).await,
        RateLimitClass::AuthLogin,
    )
    .await;
    assert_eq!(app.status(via_proxy(CLIENT_B)()).await, StatusCode::OK);
    assert_eq!(
        app.status(post("/test/rl/login", CLIENT_A)).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.status(post("/test/rl/login", TRUSTED_PROXY)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn svc_rate_limit_admin_test_classes_are_instance_wide() {
    let app = harness();

    for attempt in 0..5_u8 {
        let peer = format!("198.51.100.{}", 100 + attempt);
        assert_eq!(
            app.status(post("/test/rl/smtp-test", &peer)).await,
            StatusCode::OK
        );
    }
    let retry = assert_throttled(
        app.send(post("/test/rl/smtp-test", CLIENT_B)).await,
        RateLimitClass::EmailTest,
    )
    .await;
    assert_eq!(retry, 720);

    for attempt in 0..30_u8 {
        let peer = format!("198.51.100.{}", 100 + attempt);
        assert_eq!(
            app.status(post("/test/rl/provider-test", &peer)).await,
            StatusCode::OK
        );
    }
    let retry = assert_throttled(
        app.send(post("/test/rl/provider-test", CLIENT_B)).await,
        RateLimitClass::ProviderTest,
    )
    .await;
    assert_eq!(retry, 120);

    app.clock.advance(Duration::from_secs(120));
    assert_eq!(
        app.status(post("/test/rl/provider-test", CLIENT_A)).await,
        StatusCode::OK
    );
    assert_eq!(
        app.status(post("/test/rl/provider-test", CLIENT_A)).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(app.limiter.tracked_keys(RateLimitClass::EmailTest), 1);
    assert_eq!(app.limiter.tracked_keys(RateLimitClass::ProviderTest), 1);
}

fn flood_subject(index: u32) -> Subject {
    Subject::new(IpAddr::V4(Ipv4Addr::from(0x6440_0000 | (index & 0xFFFF))))
        .with_scope(Some(PublicScope::alias(&format!("flood-{index}"))))
}

#[tokio::test]
async fn it_rate_limit_keyspace_bounded() {
    let app = harness();
    let class = RateLimitClass::PublicRead;
    let flood = u32::try_from(MAX_TRACKED_KEYS * 3).unwrap();

    let mut peak = 0;
    for index in 0..flood {
        app.limiter
            .admit(class, Stage::Edge, &flood_subject(index))
            .unwrap();
        peak = peak.max(app.limiter.tracked_keys(class));
    }
    assert!(peak <= MAX_TRACKED_KEYS, "peak {peak}");
    assert!(app.limiter.tracked_keys(class) <= MAX_TRACKED_KEYS);
    assert!(app.limiter.tracked_keys(class) > 0);

    app.clock.advance(Duration::from_secs(60) + SWEEP_INTERVAL);
    app.limiter
        .admit(class, Stage::Edge, &flood_subject(flood))
        .unwrap();
    assert_eq!(app.limiter.tracked_keys(class), 1);

    for index in 0..2_000_u32 {
        let uri = format!("/test/rl/public/http-flood-{index}");
        assert_eq!(app.status(get(&uri, CLIENT_A)).await, StatusCode::OK);
    }
    assert_eq!(app.limiter.tracked_keys(class), 2_001);

    app.clock.advance(Duration::from_secs(30));
    assert_eq!(
        app.status(get("/test/rl/public/after-flood", CLIENT_B))
            .await,
        StatusCode::OK
    );
    assert_eq!(app.limiter.tracked_keys(class), 2_002);

    app.clock.advance(Duration::from_secs(60) + SWEEP_INTERVAL);
    assert_eq!(
        app.status(get("/test/rl/public/settled", CLIENT_B)).await,
        StatusCode::OK
    );
    assert_eq!(app.limiter.tracked_keys(class), 1);
}

#[tokio::test]
async fn it_rate_limit_eviction_prefers_idle_keys_over_live_ones() {
    let app = harness();
    let class = RateLimitClass::AuthToken;
    let victim = Subject::new(CLIENT_A.parse().unwrap());
    for _ in 0..20 {
        app.limiter.admit(class, Stage::Edge, &victim).unwrap();
    }
    assert!(matches!(
        app.limiter.admit(class, Stage::Edge, &victim),
        Err(Denial::Throttled(_))
    ));

    for index in 0..u32::try_from(MAX_TRACKED_KEYS * 2).unwrap() {
        app.limiter
            .admit(class, Stage::Edge, &flood_subject(index))
            .unwrap();
    }
    assert!(app.limiter.tracked_keys(class) <= MAX_TRACKED_KEYS);
    assert!(matches!(
        app.limiter.admit(class, Stage::Edge, &victim),
        Err(Denial::Throttled(_))
    ));
}

fn deterministic_limiter() -> (TestClock, RateLimiter) {
    let clock = TestClock::new(datetime!(2026-09-24 09:00 UTC));
    let limiter = RateLimiter::new(Arc::new(clock.clone()) as Arc<dyn Clock>);
    (clock, limiter)
}

fn throttled(denial: Result<(), Denial>) -> Throttled {
    match denial {
        Err(Denial::Throttled(throttled)) => throttled,
        other => panic!("expected a throttled denial, got {other:?}"),
    }
}

#[tokio::test]
async fn unit_retry_after_header() {
    assert_eq!(RetryAfter::covering(Duration::ZERO).seconds(), 1);
    assert_eq!(RetryAfter::covering(Duration::from_nanos(1)).seconds(), 1);
    assert_eq!(RetryAfter::covering(Duration::from_secs(1)).seconds(), 1);
    assert_eq!(
        RetryAfter::covering(Duration::from_nanos(1_000_000_001)).seconds(),
        2
    );
    assert_eq!(
        RetryAfter::covering(Duration::from_millis(59_500)).seconds(),
        60
    );

    let (clock, limiter) = deterministic_limiter();
    let class = RateLimitClass::ProviderTest;
    let subject = Subject::new(CLIENT_A.parse().unwrap());
    for _ in 0..30 {
        limiter.admit(class, Stage::Edge, &subject).unwrap();
    }
    let first = throttled(limiter.admit(class, Stage::Edge, &subject));
    assert_eq!(first.scope(), class);
    assert_eq!(first.retry_after().seconds(), 120);

    clock.advance(Duration::from_millis(500));
    let fractional = throttled(limiter.admit(class, Stage::Edge, &subject));
    assert_eq!(fractional.retry_after().seconds(), 120);
    clock.advance(Duration::from_secs(fractional.retry_after().seconds() - 1));
    assert!(limiter.admit(class, Stage::Edge, &subject).is_err());
    clock.advance(Duration::from_secs(1));
    limiter.admit(class, Stage::Edge, &subject).unwrap();

    let email = RateLimitClass::EmailTest;
    for _ in 0..5 {
        limiter.admit(email, Stage::Edge, &subject).unwrap();
    }
    let denied = throttled(limiter.admit(email, Stage::Edge, &subject));
    let wait = denied.retry_after().seconds();
    assert_eq!(wait, 720);
    clock.advance(Duration::from_secs(wait) - Duration::from_nanos(1));
    assert!(limiter.admit(email, Stage::Edge, &subject).is_err());
    clock.advance(Duration::from_nanos(1));
    limiter.admit(email, Stage::Edge, &subject).unwrap();

    let response = denied.into_response(None);
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let header = response.headers().get(RETRY_AFTER).unwrap().clone();
    assert_eq!(header.to_str().unwrap(), "720");
    let body = body_json(response).await;
    assert_eq!(
        body,
        json!({
            "error": {
                "code": "RATE_LIMITED",
                "message": ErrorCode::RateLimited.default_message(),
                "requestId": "",
                "details": { "scope": "rl.email.test" },
            }
        })
    );
    assert!(!body.to_string().contains(CLIENT_A));
}

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

#[tokio::test]
async fn svc_rate_limit_rejection_leaks_no_key_material() {
    const CAPABILITY: &str = "cap-sentinel-Zx81Qp0vYt3LmN7aB2cD4eF6gH8iJ0kL";
    const ACCOUNT: &str = "account-sentinel@example.com";
    const SESSION: &str = "session-sentinel-7c1f0d9e";

    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let app = harness();

    let embed = format!("/test/rl/embed/{CAPABILITY}");
    exhaust(&app, 120, || get(&embed, CLIENT_A)).await;
    let mut bodies = Vec::new();
    let response = app.send(get(&embed, CLIENT_A)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    bodies.push(body_json(response).await.to_string());

    exhaust(&app, 3, || reset_request(CLIENT_A, ACCOUNT)).await;
    let response = app.send(reset_request(CLIENT_B, ACCOUNT)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    bodies.push(body_json(response).await.to_string());

    exhaust(&app, 300, || session_read(CLIENT_A, SESSION)).await;
    let response = app.send(session_read(CLIENT_A, SESSION)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    bodies.push(body_json(response).await.to_string());

    let logs = capture.text();
    assert!(!logs.is_empty());
    for body in &bodies {
        for leaked in [CAPABILITY, ACCOUNT, SESSION, CLIENT_A, CLIENT_B] {
            assert!(!body.contains(leaked), "{leaked} in {body}");
        }
    }
    for leaked in [CAPABILITY, ACCOUNT, SESSION, "IdentityDigest"] {
        assert!(!logs.contains(leaked), "{leaked} reached the logs");
    }
}

#[test]
fn unit_rate_limit_state_is_never_persisted() {
    let schema = include_str!("../../../migrations/0001_initial_schema.sql").to_ascii_lowercase();
    let tables: Vec<&str> = schema
        .split("create table")
        .skip(1)
        .filter_map(|definition| definition.split_whitespace().next())
        .collect();
    assert!(tables.contains(&"account_lockouts"), "{tables:?}");
    for table in &tables {
        for term in ["rate", "limit", "gcra", "throttle"] {
            assert!(!table.contains(term), "{table} persists limiter state");
        }
    }
}
