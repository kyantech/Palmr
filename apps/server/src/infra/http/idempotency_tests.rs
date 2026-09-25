use std::convert::Infallible;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use http::header::{CONTENT_TYPE, LOCATION, RETRY_AFTER, SET_COOKIE};
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use tempfile::TempDir;
use time::macros::datetime;
use tokio::sync::Notify;
use tower::util::BoxCloneSyncService;
use tower::ServiceExt;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use utoipa_axum::routes;

use super::error::ApiError;
use super::headers::SecurityHeaders;
use super::idempotency::{
    replay_aad, request_identity, Admission, IdempotencyRequest, IdempotencyScope,
    IdempotencyService, ReplayEnvelope, IDEMPOTENCY_KEY, IDEMPOTENCY_REPLAYED,
};
use super::proxy::TrustedProxies;
use crate::app::auth_class::AuthClass;
use crate::app::router::{
    with_middleware, HttpEdge, IdempotencyMode, RateLimitClass, RoutePolicy, Routes, Transport,
};
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig, SqliteSynchronous};
use crate::domain::clock::{Clock, TestClock};
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, DATABASE_FILE, MIGRATOR};
use crate::infra::telemetry::build_dispatch;

const START: time::OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
const ITEMS: &str = "/api/v1/test/idempotency/items";
const OTHER: &str = "/api/v1/test/idempotency/other";
const GRANTS: &str = "/api/v1/test/idempotency/grants";
const GRANTS_TEMPLATE: &str = "/api/v1/test/idempotency/grants";
const UNSUPPORTED: &str = "/api/v1/test/idempotency/unsupported";
const USER_HEADER: &str = "x-test-user";
const LINK_HEADER: &str = "x-test-link";
const GRANT_HEADER: &str = "x-test-grant";
const HOLD_HEADER: &str = "x-test-hold";
const KEY: &str = "client-key-0001-aaaaaaaaaaaaaaaa";
const BODY_SENTINEL: &str = "body-secret-sentinel-4c7a91";
const CAPABILITY: &str = "invite-capability-sentinel-b93e0f";
const GRANT_COOKIE_VALUE: &str = "grant-cookie-sentinel-5d21aa";
const PLAINTEXT_COOKIE_VALUE: &str = "plaintext-cookie-sentinel-77e0c3";
const RESPONSE_READ_CAP: usize = 64 * 1024;
const DATABASE_READ_CAP: u64 = 256 * 1024 * 1024;

enum TestPrincipal {}
enum TestEffect {}

const PLAINTEXT: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
)
.with_idempotency(IdempotencyMode::Plaintext);

const SEALED: RoutePolicy = RoutePolicy::new(
    AuthClass::PublicGrant,
    RateLimitClass::None,
    Transport::ControlPlane,
)
.with_idempotency(IdempotencyMode::Sealed);

const UNDECLARED: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

#[derive(Default)]
struct Gate {
    claimed: Notify,
    release: Notify,
}

#[derive(Clone)]
struct TestApp {
    service: IdempotencyService,
    pools: DbPools,
    clock: Arc<dyn Clock>,
    gate: Arc<Gate>,
}

#[utoipa::path(post, path = "/api/v1/test/idempotency/items", responses((status = 201)))]
async fn create_item(
    State(app): State<TestApp>,
    headers: HeaderMap,
    request: IdempotencyRequest,
    body: Bytes,
) -> Response {
    execute(app, &headers, request, body, "items").await
}

#[utoipa::path(
    post,
    path = "/api/v1/test/idempotency/items/{id}/notify",
    params(("id" = String, Path)),
    responses((status = 201))
)]
async fn notify_item(
    State(app): State<TestApp>,
    headers: HeaderMap,
    request: IdempotencyRequest,
    body: Bytes,
) -> Response {
    execute(app, &headers, request, body, "notify").await
}

#[utoipa::path(post, path = "/api/v1/test/idempotency/other", responses((status = 201)))]
async fn create_other(
    State(app): State<TestApp>,
    headers: HeaderMap,
    request: IdempotencyRequest,
    body: Bytes,
) -> Response {
    execute(app, &headers, request, body, "other").await
}

#[utoipa::path(post, path = "/api/v1/test/idempotency/grants", responses((status = 201)))]
async fn create_grant(
    State(app): State<TestApp>,
    headers: HeaderMap,
    request: IdempotencyRequest,
    body: Bytes,
) -> Response {
    execute(app, &headers, request, body, "grants").await
}

#[utoipa::path(post, path = "/api/v1/test/idempotency/unsupported", responses((status = 201)))]
async fn create_unsupported(
    State(app): State<TestApp>,
    headers: HeaderMap,
    request: IdempotencyRequest,
    body: Bytes,
) -> Response {
    execute(app, &headers, request, body, "unsupported").await
}

async fn execute(
    app: TestApp,
    headers: &HeaderMap,
    request: IdempotencyRequest,
    raw: Bytes,
    route: &'static str,
) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&raw) else {
        return ApiError::new(ErrorCode::ValidationError).into_response();
    };
    let claim = match app.service.claim(request, &body).await {
        Ok(Admission::Execute(claim)) => claim,
        Ok(Admission::Replay(response)) => return response,
        Err(rejection) => return rejection.into_response(),
    };
    if body["fail"] == json!(true) {
        app.service.release(claim).await.unwrap();
        return ApiError::new(ErrorCode::ValidationError).into_response();
    }
    if headers.contains_key(HOLD_HEADER) {
        app.gate.claimed.notify_one();
        app.gate.release.notified().await;
    }

    let name = body["name"].as_str().unwrap_or("unnamed").to_owned();
    let effect_id = Id::<TestEffect>::generate(app.clock.as_ref()).to_string();
    let envelope = if route == "grants" {
        ReplayEnvelope::new(
            StatusCode::CREATED,
            json!({ "inviteUrl": format!("https://palmr.example.test/invite/{CAPABILITY}") }),
        )
        .with_grant_cookie(HeaderValue::from_static(
            "palmr_rs_0193=grant-cookie-sentinel-5d21aa; Path=/; HttpOnly; Secure",
        ))
    } else {
        let envelope = ReplayEnvelope::new(
            StatusCode::CREATED,
            json!({ "id": effect_id, "name": name, "route": route }),
        )
        .with_location(HeaderValue::try_from(format!("{ITEMS}/{effect_id}")).unwrap());
        if body["grantCookie"] == json!(true) {
            envelope.with_grant_cookie(HeaderValue::from_static(
                "palmr_rs_0194=plaintext-cookie-sentinel-77e0c3; Path=/",
            ))
        } else {
            envelope
        }
    };

    let committed = app
        .pools
        .write_tx(app.clock.as_ref(), "test.idempotent_effect", async |tx| {
            sqlx::query("INSERT INTO test_effects (id, route, name) VALUES (?1, ?2, ?3)")
                .bind(&effect_id)
                .bind(route)
                .bind(&name)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
            app.service.complete(tx, &claim, &envelope).await
        })
        .await;
    match committed {
        Ok(()) => envelope.into_response(),
        Err(error) => {
            app.service.release(claim).await.unwrap();
            ApiError::from(error).into_response()
        }
    }
}

fn test_scope(headers: &HeaderMap) -> Option<IdempotencyScope> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| Id::<TestPrincipal>::from_str(value).unwrap())
    };
    header(USER_HEADER)
        .map(IdempotencyScope::user)
        .or_else(|| header(GRANT_HEADER).map(IdempotencyScope::reverse_share_grant))
        .or_else(|| header(LINK_HEADER).map(IdempotencyScope::reverse_share_link))
}

async fn resolve_test_scope(mut request: Request, next: Next) -> Response {
    if let Some(scope) = test_scope(request.headers()) {
        request.extensions_mut().insert(scope);
    }
    next.run(request).await
}

fn test_routes() -> Routes<TestApp> {
    Routes::new()
        .route(PLAINTEXT, routes!(create_item))
        .route(PLAINTEXT, routes!(notify_item))
        .route(PLAINTEXT, routes!(create_other))
        .route(SEALED, routes!(create_grant))
        .route(UNDECLARED, routes!(create_unsupported))
}

type TestService = BoxCloneSyncService<Request, Response, Infallible>;

struct Harness {
    root: TempDir,
    clock: TestClock,
    pools: DbPools,
    keys: Arc<KeyRing>,
    gate: Arc<Gate>,
    service: TestService,
}

impl Harness {
    async fn open() -> Self {
        Self::start(TempDir::new().unwrap(), TestClock::new(START)).await
    }

    async fn start(root: TempDir, clock: TestClock) -> Self {
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        pools
            .write_tx(&clock, "test.effects_table", async |tx| {
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS test_effects (
                        id TEXT NOT NULL PRIMARY KEY, route TEXT NOT NULL, name TEXT NOT NULL)",
                )
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(root.path()).unwrap();
        let keys = Arc::new(KeyRing::new(&instance_key));
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        let gate = Arc::new(Gate::default());
        let app = TestApp {
            service: IdempotencyService::new(pools.clone(), Arc::clone(&shared), Arc::clone(&keys)),
            pools: pools.clone(),
            clock: Arc::clone(&shared),
            gate: Arc::clone(&gate),
        };
        let router = test_routes()
            .build()
            .unwrap()
            .router
            .layer(from_fn(resolve_test_scope))
            .with_state(app);
        let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
            &str,
            &str,
        )>()))
        .unwrap()
        .config;
        let edge = HttpEdge::new(
            shared,
            TrustedProxies::new(&config.trust_proxy),
            SecurityHeaders::new(&config),
        );
        Self {
            root,
            clock,
            pools,
            keys,
            gate,
            service: BoxCloneSyncService::new(with_middleware(router, &edge)),
        }
    }

    async fn restart(self) -> Self {
        let Self {
            root, clock, pools, ..
        } = self;
        let _ = pools.shutdown().await;
        Self::start(root, clock).await
    }

    async fn send(&self, request: Request) -> Response {
        self.service.clone().oneshot(request).await.unwrap()
    }

    async fn effects(&self, route: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM test_effects WHERE route = ?1")
            .bind(route)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn records(&self) -> Vec<(String, String, String, String)> {
        sqlx::query_as(
            "SELECT scope_kind, scope_id, route_template, state
               FROM idempotency_records ORDER BY created_at, id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn execute(&self, sql: &str, binds: &[&str]) -> u64 {
        self.pools
            .write_tx(&self.clock, "test.mutate", async |tx| {
                let mut query = sqlx::query(sql);
                for bind in binds {
                    query = query.bind(*bind);
                }
                let done = query.execute(tx.executor()).await.map_err(DbError::from)?;
                Ok::<u64, DbError>(done.rows_affected())
            })
            .await
            .unwrap()
    }

    fn database_bytes(&self) -> Vec<u8> {
        database_bytes(self.root.path())
    }
}

fn database_bytes(root: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-journal"] {
        if let Ok(file) = File::open(root.join(format!("{DATABASE_FILE}{suffix}"))) {
            std::io::copy(&mut file.take(DATABASE_READ_CAP), &mut bytes).unwrap();
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn principal(clock: &TestClock) -> String {
    Id::<TestPrincipal>::generate(clock).to_string()
}

struct Call<'a> {
    path: &'a str,
    scope: (&'a str, &'a str),
    key: Option<&'a str>,
    body: String,
    hold: bool,
}

impl<'a> Call<'a> {
    fn user(path: &'a str, user: &'a str, key: Option<&'a str>, body: &Value) -> Self {
        Self {
            path,
            scope: (USER_HEADER, user),
            key,
            body: body.to_string(),
            hold: false,
        }
    }

    fn scoped(path: &'a str, scope: (&'a str, &'a str), key: &'a str, body: &Value) -> Self {
        Self {
            path,
            scope,
            key: Some(key),
            body: body.to_string(),
            hold: false,
        }
    }

    fn raw_body(mut self, body: &str) -> Self {
        self.body = body.to_owned();
        self
    }

    const fn held(mut self) -> Self {
        self.hold = true;
        self
    }

    fn request(&self) -> Request {
        let mut builder = Request::post(self.path)
            .header(CONTENT_TYPE, "application/json")
            .header(self.scope.0, self.scope.1)
            .extension(ConnectInfo(SocketAddr::from(([198, 51, 100, 7], 40_000))));
        if let Some(key) = self.key {
            builder = builder.header(IDEMPOTENCY_KEY, key);
        }
        if self.hold {
            builder = builder.header(HOLD_HEADER, "1");
        }
        builder.body(Body::from(self.body.clone())).unwrap()
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
    text: String,
}

impl Reply {
    fn replayed(&self) -> bool {
        self.headers.get(IDEMPOTENCY_REPLAYED) == Some(&HeaderValue::from_static("true"))
    }

    fn header(&self, name: http::HeaderName) -> Option<&str> {
        self.headers.get(name).map(|value| value.to_str().unwrap())
    }

    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }
}

async fn read(response: Response) -> Reply {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let body = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap()
    };
    Reply {
        status,
        headers,
        body,
        text,
    }
}

async fn call(harness: &Harness, call: &Call<'_>) -> Reply {
    read(harness.send(call.request()).await).await
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
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
async fn it_idempotency_replay_returns_original_result() {
    let mut harness = Harness::open().await;
    let user = principal(&harness.clock);
    let body = json!({ "name": "alpha", "secret": BODY_SENTINEL });
    let first_call = Call::user(ITEMS, &user, Some(KEY), &body);

    let first = call(&harness, &first_call).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text);
    assert!(!first.replayed());
    let location = first.header(LOCATION).unwrap().to_owned();
    assert_eq!(
        location,
        format!("{ITEMS}/{}", first.body["id"].as_str().unwrap())
    );
    assert_eq!(first.body["name"], "alpha");
    assert_eq!(harness.effects("items").await, 1);
    assert_eq!(
        harness.records().await,
        [(
            "user".to_owned(),
            user.clone(),
            ITEMS.to_owned(),
            "completed".to_owned()
        )]
    );

    let reordered = Call::user(ITEMS, &user, Some(KEY), &Value::Null).raw_body(&format!(
        "{{ \"secret\" : \"{BODY_SENTINEL}\",\n  \"name\" : \"alpha\" }}"
    ));
    for retry in [&first_call, &reordered] {
        let replay = call(&harness, retry).await;
        assert_eq!(replay.status, first.status);
        assert_eq!(replay.body, first.body);
        assert_eq!(replay.text, first.text);
        assert_eq!(replay.header(LOCATION), Some(location.as_str()));
        assert_eq!(replay.header(CONTENT_TYPE), first.header(CONTENT_TYPE));
        assert!(replay.replayed());
        assert!(replay.header(SET_COOKIE).is_none());
        assert_ne!(
            replay.header(http::HeaderName::from_static("x-request-id")),
            first.header(http::HeaderName::from_static("x-request-id"))
        );
    }
    assert_eq!(harness.effects("items").await, 1);

    harness = harness.restart().await;
    let after_restart = call(&harness, &first_call).await;
    assert_eq!(after_restart.status, StatusCode::CREATED);
    assert_eq!(after_restart.text, first.text);
    assert_eq!(after_restart.header(LOCATION), Some(location.as_str()));
    assert!(after_restart.replayed());
    assert_eq!(harness.effects("items").await, 1);

    let failing_key = "client-key-0002-failing-attempt";
    let failing = Call::user(ITEMS, &user, Some(failing_key), &json!({ "fail": true }));
    for _ in 0..2 {
        let rejected = call(&harness, &failing).await;
        assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!rejected.replayed());
        assert_eq!(harness.records().await.len(), 1);
    }
    let recovered = call(
        &harness,
        &Call::user(ITEMS, &user, Some(failing_key), &json!({ "name": "beta" })),
    )
    .await;
    assert_eq!(recovered.status, StatusCode::CREATED);
    assert!(!recovered.replayed());
    assert_eq!(harness.effects("items").await, 2);

    harness
        .clock
        .advance(Duration::from_secs(24 * 60 * 60) - Duration::from_millis(1));
    let last_replay = call(&harness, &first_call).await;
    assert!(last_replay.replayed());
    assert_eq!(last_replay.text, first.text);
    assert_eq!(harness.effects("items").await, 2);

    harness.clock.advance(Duration::from_millis(1));
    let after_window = call(&harness, &first_call).await;
    assert_eq!(after_window.status, StatusCode::CREATED);
    assert!(!after_window.replayed());
    assert_ne!(after_window.body["id"], first.body["id"]);
    assert_eq!(harness.effects("items").await, 3);
}

#[tokio::test]
async fn it_idempotency_key_conflict_on_different_body() {
    let harness = Harness::open().await;
    let user = principal(&harness.clock);

    let first = call(
        &harness,
        &Call::user(ITEMS, &user, Some(KEY), &json!({ "name": "alpha" })),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    for body in [
        json!({ "name": "omega" }),
        json!({ "name": "alpha", "extra": 1 }),
        json!({}),
    ] {
        let conflict = call(&harness, &Call::user(ITEMS, &user, Some(KEY), &body)).await;
        assert_eq!(conflict.status, StatusCode::CONFLICT);
        assert_eq!(conflict.code(), "IDEMPOTENCY_KEY_CONFLICT");
        assert!(!conflict.replayed());
        assert!(!conflict.text.contains("alpha"), "{}", conflict.text);
        assert!(conflict.header(LOCATION).is_none());
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM test_effects")
        .fetch_all(harness.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(names, ["alpha"]);

    let replay = call(
        &harness,
        &Call::user(ITEMS, &user, Some(KEY), &json!({ "name": "alpha" })),
    )
    .await;
    assert!(replay.replayed());
    assert_eq!(replay.body, first.body);

    let item_a = principal(&harness.clock);
    let item_b = principal(&harness.clock);
    let notify_body = json!({ "name": "notify" });
    let notify_a = format!("{ITEMS}/{item_a}/notify");
    let notify_b = format!("{ITEMS}/{item_b}/notify");
    let notified = call(
        &harness,
        &Call::user(&notify_a, &user, Some(KEY), &notify_body),
    )
    .await;
    assert_eq!(notified.status, StatusCode::CREATED);
    let other_resource = call(
        &harness,
        &Call::user(&notify_b, &user, Some(KEY), &notify_body),
    )
    .await;
    assert_eq!(other_resource.status, StatusCode::CONFLICT);
    assert_eq!(other_resource.code(), "IDEMPOTENCY_KEY_CONFLICT");
    assert_eq!(harness.effects("notify").await, 1);
    let templates: Vec<String> = harness
        .records()
        .await
        .into_iter()
        .map(|(_, _, template, _)| template)
        .collect();
    assert_eq!(
        templates,
        [
            ITEMS.to_owned(),
            "/api/v1/test/idempotency/items/{id}/notify".to_owned()
        ]
    );
}

#[tokio::test]
async fn it_idempotency_concurrent_request_in_progress() {
    let harness = Arc::new(Harness::open().await);
    let user = principal(&harness.clock);
    let body = json!({ "name": "held" });

    let held = {
        let harness = Arc::clone(&harness);
        let request = Call::user(ITEMS, &user, Some(KEY), &body).held().request();
        tokio::spawn(async move { read(harness.send(request).await).await })
    };
    harness.gate.claimed.notified().await;

    let busy = call(&harness, &Call::user(ITEMS, &user, Some(KEY), &body)).await;
    assert_eq!(busy.status, StatusCode::CONFLICT);
    assert_eq!(busy.code(), "IDEMPOTENCY_REQUEST_IN_PROGRESS");
    assert_eq!(busy.header(RETRY_AFTER), Some("1"));
    let different = call(
        &harness,
        &Call::user(ITEMS, &user, Some(KEY), &json!({ "name": "other" })),
    )
    .await;
    assert_eq!(different.code(), "IDEMPOTENCY_KEY_CONFLICT");
    assert!(different.header(RETRY_AFTER).is_none());
    assert_eq!(harness.effects("items").await, 0);

    harness.gate.release.notify_one();
    let finished = held.await.unwrap();
    assert_eq!(finished.status, StatusCode::CREATED);
    assert!(!finished.replayed());
    let replay = call(&harness, &Call::user(ITEMS, &user, Some(KEY), &body)).await;
    assert!(replay.replayed());
    assert_eq!(replay.text, finished.text);
    assert_eq!(harness.effects("items").await, 1);

    let abandoned_key = "client-key-0003-abandoned-claim";
    let abandoned = {
        let harness = Arc::clone(&harness);
        let request = Call::user(OTHER, &user, Some(abandoned_key), &body)
            .held()
            .request();
        tokio::spawn(async move { harness.send(request).await })
    };
    harness.gate.claimed.notified().await;
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    let retry = Call::user(OTHER, &user, Some(abandoned_key), &body);
    let still_leased = call(&harness, &retry).await;
    assert_eq!(still_leased.code(), "IDEMPOTENCY_REQUEST_IN_PROGRESS");

    harness
        .clock
        .advance(Duration::from_secs(30) - Duration::from_millis(1));
    assert_eq!(
        call(&harness, &retry).await.code(),
        "IDEMPOTENCY_REQUEST_IN_PROGRESS"
    );
    harness.clock.advance(Duration::from_millis(1));
    let taken_over = call(&harness, &retry).await;
    assert_eq!(
        taken_over.status,
        StatusCode::CREATED,
        "{}",
        taken_over.text
    );
    assert!(!taken_over.replayed());
    assert_eq!(harness.effects("other").await, 1);
    assert!(call(&harness, &retry).await.replayed());

    let stale_key = "client-key-0004-stale-attempt--";
    let stale = {
        let harness = Arc::clone(&harness);
        let request = Call::user(ITEMS, &user, Some(stale_key), &body)
            .held()
            .request();
        tokio::spawn(async move { read(harness.send(request).await).await })
    };
    harness.gate.claimed.notified().await;
    harness.clock.advance(Duration::from_secs(31));
    let successor = call(&harness, &Call::user(ITEMS, &user, Some(stale_key), &body)).await;
    assert_eq!(successor.status, StatusCode::CREATED);
    harness.gate.release.notify_one();
    let stale = stale.await.unwrap();
    assert_eq!(stale.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(stale.code(), "INTERNAL_ERROR");
    assert_eq!(harness.effects("items").await, 2);
    let replay = call(&harness, &Call::user(ITEMS, &user, Some(stale_key), &body)).await;
    assert!(replay.replayed());
    assert_eq!(replay.text, successor.text);
    assert_eq!(harness.effects("items").await, 2);
    assert!(harness
        .records()
        .await
        .iter()
        .all(|(_, _, _, state)| state == "completed"));
}

#[tokio::test]
async fn it_idempotency_sealed_payload_never_plaintext() {
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let harness = Harness::open().await;
    let link = principal(&harness.clock);
    let other_link = principal(&harness.clock);
    let grant = principal(&harness.clock);
    let sealed_key = "sealed-client-key-0005-sentinel";
    let body = json!({ "name": "guest", "secret": BODY_SENTINEL });
    let first_call = Call::scoped(GRANTS, (LINK_HEADER, &link), sealed_key, &body);
    let mut error_bodies = Vec::new();

    let first = call(&harness, &first_call).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text);
    assert!(first.text.contains(CAPABILITY));
    let cookie = first.header(SET_COOKIE).unwrap().to_owned();
    assert!(cookie.contains(GRANT_COOKIE_VALUE));

    let replay = call(&harness, &first_call).await;
    assert!(replay.replayed());
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.text, first.text);
    assert_eq!(replay.header(SET_COOKIE), Some(cookie.as_str()));
    assert_eq!(harness.effects("grants").await, 1);

    let (id, json_column, ciphertext, nonce, key_version): (
        String,
        Option<String>,
        Vec<u8>,
        Vec<u8>,
        i64,
    ) = sqlx::query_as(
        "SELECT id, response_json, response_ciphertext, response_nonce, key_version
           FROM idempotency_records WHERE scope_id = ?1",
    )
    .bind(&link)
    .fetch_one(harness.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(json_column, None);
    assert_eq!(nonce.len(), 24);
    assert_eq!(key_version, 1);
    assert!(!contains(&ciphertext, CAPABILITY));
    assert!(!contains(&ciphertext, GRANT_COOKIE_VALUE));

    let link_scope =
        IdempotencyScope::reverse_share_link(Id::<TestPrincipal>::from_str(&link).unwrap());
    let sealed = SealedSecret::from_parts(ciphertext.clone(), &nonce, key_version).unwrap();
    let opened = harness
        .keys
        .open(
            SealPurpose::IdempotencyReplay,
            &replay_aad(&id, &link_scope, GRANTS_TEMPLATE),
            &sealed,
        )
        .unwrap();
    assert!(contains(opened.expose_secret(), CAPABILITY));
    let other_id = principal(&harness.clock);
    let other_scope =
        IdempotencyScope::reverse_share_link(Id::<TestPrincipal>::from_str(&other_link).unwrap());
    let grant_scope =
        IdempotencyScope::reverse_share_grant(Id::<TestPrincipal>::from_str(&link).unwrap());
    for aad in [
        replay_aad(&other_id, &link_scope, GRANTS_TEMPLATE),
        replay_aad(&id, &other_scope, GRANTS_TEMPLATE),
        replay_aad(&id, &grant_scope, GRANTS_TEMPLATE),
        replay_aad(&id, &link_scope, "/api/v1/admin/invites"),
    ] {
        assert_eq!(
            harness
                .keys
                .open(SealPurpose::IdempotencyReplay, &aad, &sealed)
                .unwrap_err(),
            CryptoError::AuthenticationFailed
        );
    }

    let moved_call = Call::scoped(GRANTS, (LINK_HEADER, &other_link), sealed_key, &body);
    assert_eq!(
        call(&harness, &moved_call).await.status,
        StatusCode::CREATED
    );
    let moved = harness
        .pools
        .write_tx(&harness.clock, "test.move_ciphertext", async |tx| {
            let done = sqlx::query(
                "UPDATE idempotency_records SET response_ciphertext = ?1, response_nonce = ?2
                  WHERE scope_id = ?3",
            )
            .bind(&ciphertext)
            .bind(&nonce)
            .bind(&other_link)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
            Ok::<u64, DbError>(done.rows_affected())
        })
        .await
        .unwrap();
    assert_eq!(moved, 1);
    let refused = call(&harness, &moved_call).await;
    assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(refused.code(), "INTERNAL_ERROR");
    assert!(refused.header(SET_COOKIE).is_none());
    error_bodies.push(refused.text);

    let granted = call(
        &harness,
        &Call::scoped(GRANTS, (GRANT_HEADER, &grant), sealed_key, &body),
    )
    .await;
    assert_eq!(granted.status, StatusCode::CREATED);
    assert!(!granted.replayed());

    let user = principal(&harness.clock);
    let plaintext_cookie = call(
        &harness,
        &Call::user(
            ITEMS,
            &user,
            Some("plaintext-cookie-key-0006"),
            &json!({ "name": "cookie", "grantCookie": true }),
        ),
    )
    .await;
    assert_eq!(plaintext_cookie.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(plaintext_cookie.header(SET_COOKIE).is_none());
    assert_eq!(harness.effects("items").await, 0);
    error_bodies.push(plaintext_cookie.text);

    let plain_call = Call::user(
        ITEMS,
        &user,
        Some("plaintext-tamper-key-0007"),
        &json!({ "name": "plain" }),
    );
    assert_eq!(
        call(&harness, &plain_call).await.status,
        StatusCode::CREATED
    );
    let tampered = harness
        .execute(
            "UPDATE idempotency_records
                SET response_json = json_set(response_json, '$.headers.\"Set-Cookie\"', ?1)
              WHERE scope_id = ?2 AND route_template = ?3",
            &["palmr_rs_tamper=injected", &user, ITEMS],
        )
        .await;
    assert_eq!(tampered, 1);
    let tampered_replay = call(&harness, &plain_call).await;
    assert_eq!(tampered_replay.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(tampered_replay.header(SET_COOKIE).is_none());
    error_bodies.push(tampered_replay.text);

    let busy = call(
        &harness,
        &Call::scoped(
            GRANTS,
            (LINK_HEADER, &link),
            sealed_key,
            &json!({ "name": "x" }),
        ),
    )
    .await;
    assert_eq!(busy.code(), "IDEMPOTENCY_KEY_CONFLICT");
    error_bodies.push(busy.text);

    let audit_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(harness.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(audit_rows, 0);

    let identity = request_identity(&harness.keys, &http::Method::POST, GRANTS, &body);
    let database = harness.database_bytes();
    assert!(contains(&database, identity.as_str()));
    for secret in [
        CAPABILITY,
        GRANT_COOKIE_VALUE,
        PLAINTEXT_COOKIE_VALUE,
        BODY_SENTINEL,
        sealed_key,
    ] {
        assert!(
            !contains(&database, secret),
            "{secret} is stored in plaintext"
        );
    }

    let logs = capture.text();
    assert!(logs.contains("idempotency record could not be claimed or replayed"));
    for secret in [
        CAPABILITY,
        GRANT_COOKIE_VALUE,
        PLAINTEXT_COOKIE_VALUE,
        BODY_SENTINEL,
        sealed_key,
        identity.as_str(),
    ] {
        assert!(!logs.contains(secret), "{secret} reached the logs");
        for body in &error_bodies {
            assert!(!body.contains(secret), "{secret} in {body}");
        }
    }
}

#[tokio::test]
async fn it_idempotency_key_bounds_and_scope_isolation() {
    let harness = Harness::open().await;
    let user = principal(&harness.clock);
    let other_user = principal(&harness.clock);
    let body = json!({ "name": "bounded" });

    for key in [
        "k".repeat(15),
        "k".repeat(129),
        "ключ-не-ascii-значение".to_owned(),
    ] {
        let rejected = call(&harness, &Call::user(ITEMS, &user, Some(&key), &body)).await;
        assert_eq!(rejected.status, StatusCode::UNPROCESSABLE_ENTITY, "{key}");
        assert_eq!(rejected.code(), "VALIDATION_ERROR");
        assert_eq!(
            rejected.body["error"]["details"],
            json!({ "field": "Idempotency-Key" })
        );
        assert!(!rejected.text.contains(&key));
    }
    let mut repeated = Call::user(ITEMS, &user, Some(&"a".repeat(16)), &body).request();
    repeated.headers_mut().append(
        IDEMPOTENCY_KEY,
        HeaderValue::from_static("bbbbbbbbbbbbbbbb"),
    );
    let repeated = read(harness.send(repeated).await).await;
    assert_eq!(repeated.code(), "VALIDATION_ERROR");
    assert_eq!(harness.effects("items").await, 0);
    assert!(harness.records().await.is_empty());

    for key in ["k".repeat(16), "k".repeat(128)] {
        let accepted = call(&harness, &Call::user(ITEMS, &user, Some(&key), &body)).await;
        assert_eq!(accepted.status, StatusCode::CREATED, "{}", key.len());
        assert!(call(&harness, &Call::user(ITEMS, &user, Some(&key), &body))
            .await
            .replayed());
    }
    assert_eq!(harness.effects("items").await, 2);

    for key in ["x", "k", &"k".repeat(129)] {
        for _ in 0..2 {
            let ignored = call(&harness, &Call::user(UNSUPPORTED, &user, Some(key), &body)).await;
            assert_eq!(ignored.status, StatusCode::CREATED);
            assert!(!ignored.replayed());
        }
    }
    assert_eq!(harness.effects("unsupported").await, 6);

    for _ in 0..2 {
        let unkeyed = call(&harness, &Call::user(ITEMS, &user, None, &body)).await;
        assert_eq!(unkeyed.status, StatusCode::CREATED);
        assert!(!unkeyed.replayed());
    }
    assert_eq!(harness.effects("items").await, 4);
    assert_eq!(harness.records().await.len(), 2);

    let shared_key = "shared-client-key-0008";
    let mine = call(&harness, &Call::user(ITEMS, &user, Some(shared_key), &body)).await;
    let theirs = call(
        &harness,
        &Call::user(ITEMS, &other_user, Some(shared_key), &body),
    )
    .await;
    let elsewhere = call(&harness, &Call::user(OTHER, &user, Some(shared_key), &body)).await;
    for reply in [&mine, &theirs, &elsewhere] {
        assert_eq!(reply.status, StatusCode::CREATED);
        assert!(!reply.replayed());
    }
    assert_ne!(mine.body["id"], theirs.body["id"]);
    assert_eq!(harness.effects("items").await, 6);
    assert_eq!(harness.effects("other").await, 1);
    let scopes: Vec<(String, String, String)> = harness
        .records()
        .await
        .into_iter()
        .skip(2)
        .map(|(kind, id, template, _)| (kind, id, template))
        .collect();
    assert_eq!(
        scopes,
        [
            ("user".to_owned(), user.clone(), ITEMS.to_owned()),
            ("user".to_owned(), other_user.clone(), ITEMS.to_owned()),
            ("user".to_owned(), user.clone(), OTHER.to_owned()),
        ]
    );
    assert!(call(
        &harness,
        &Call::user(ITEMS, &other_user, Some(shared_key), &body)
    )
    .await
    .replayed());
}
