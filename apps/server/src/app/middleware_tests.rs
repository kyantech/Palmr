use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use http::header::{
    HeaderName, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN,
};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use serde_json::{json, Value};
use time::macros::datetime;
use tower::{Layer, Service, ServiceExt};
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::auth_class::AuthClass;
use super::router::{
    with_middleware, BytePath, Deadline, HttpEdge, RateLimitClass, RequestBody, ResponseEncoding,
    RoutePolicy, Routes, Transport,
};
use crate::config::{LogFormat, TrustProxy};
use crate::domain::clock::TestClock;
use crate::infra::http::limits::{BodyLimit, ControlPlaneLimits};
use crate::infra::http::proxy::{ResolvedClient, TrustedProxies};
use crate::infra::http::request_id::RequestId;
use crate::infra::telemetry::build_dispatch;

const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const TRUSTED_PROXY: &str = "10.0.0.2";
const UNTRUSTED_PEER: &str = "203.0.113.9";
const PANIC_SENTINEL: &str = "palmr-panic-sentinel /srv/palmr/src/secret.rs:42";
const TWO_MIB: usize = 2 * 1024 * 1024;
const RESPONSE_READ_CAP: usize = 8 * 1024 * 1024;

const CONTROL_PLANE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const DOWNLOAD: RoutePolicy = RoutePolicy::new(
    AuthClass::PublicGrant,
    RateLimitClass::TransferData,
    Transport::BytePath(BytePath::new(
        RequestBody::GlobalLimit,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
);

const UPLOAD: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::TransferData,
    Transport::BytePath(BytePath::new(
        RequestBody::Streamed,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
);

fn json_response(value: &Value) -> Response {
    ([(CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

#[utoipa::path(get, path = "/test/echo", responses((status = 200)))]
async fn echo(
    Extension(request_id): Extension<RequestId>,
    client: Option<Extension<ResolvedClient>>,
    request: Request,
) -> Response {
    json_response(&json!({
        "requestId": request_id.as_str(),
        "requestIdHeader": request.headers().get(X_REQUEST_ID).map(|value| value.to_str().unwrap()),
        "clientIp": client.map(|Extension(client)| client.ip().to_string()),
        "path": request.uri().path(),
        "query": request.uri().query(),
    }))
}

#[utoipa::path(get, path = "/test/panic", responses((status = 200)))]
async fn panics() -> StatusCode {
    panic!("{PANIC_SENTINEL}");
}

#[utoipa::path(post, path = "/test/body", responses((status = 200)))]
async fn read_body(body: Bytes) -> Response {
    json_response(&json!({ "received": body.len() }))
}

#[utoipa::path(patch, path = "/test/stream", responses((status = 200)))]
async fn stream_body(body: Body) -> Response {
    let mut body = body;
    let mut received = 0_usize;
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.unwrap().into_data() {
            received += data.len();
        }
    }
    json_response(&json!({ "received": received }))
}

#[utoipa::path(get, path = "/test/slow/{seconds}", params(("seconds" = u64, Path)), responses((status = 200)))]
async fn slow(axum::extract::Path(seconds): axum::extract::Path<u64>) -> StatusCode {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    StatusCode::OK
}

#[utoipa::path(get, path = "/test/download/{seconds}", params(("seconds" = u64, Path)), responses((status = 200)))]
async fn slow_download(axum::extract::Path(seconds): axum::extract::Path<u64>) -> Response {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    large_json().await
}

#[utoipa::path(get, path = "/test/large", responses((status = 200)))]
async fn large_json() -> Response {
    let payload = "a".repeat(4096);
    let body = json!({ "payload": payload }).to_string();
    (
        [
            (CONTENT_TYPE, "application/json".to_owned()),
            (CONTENT_LENGTH, body.len().to_string()),
        ],
        body,
    )
        .into_response()
}

#[utoipa::path(get, path = "/test/object", responses((status = 200)))]
async fn object_bytes() -> Response {
    let body = vec![b'x'; 4096];
    (
        [
            (CONTENT_TYPE, "application/octet-stream".to_owned()),
            (CONTENT_LENGTH, body.len().to_string()),
        ],
        body,
    )
        .into_response()
}

fn test_routes(limits: ControlPlaneLimits) -> Routes<()> {
    Routes::with_limits(limits)
        .route(CONTROL_PLANE, routes!(echo))
        .route(CONTROL_PLANE, routes!(panics))
        .route(CONTROL_PLANE, routes!(read_body))
        .route(CONTROL_PLANE, routes!(slow))
        .route(CONTROL_PLANE, routes!(large_json))
        .route(UPLOAD, routes!(stream_body))
        .route(DOWNLOAD, routes!(slow_download))
        .route(DOWNLOAD, routes!(object_bytes))
}

fn edge() -> HttpEdge {
    HttpEdge::new(
        Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC))),
        TrustedProxies::new(&TrustProxy::AllowList(vec!["10.0.0.0/8".parse().unwrap()])),
    )
}

fn app_with(
    limits: ControlPlaneLimits,
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    with_middleware(test_routes(limits).build().unwrap().router, &edge())
}

fn app() -> impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone {
    app_with(ControlPlaneLimits::default())
}

fn request(method: Method, uri: &str, peer: &str) -> http::request::Builder {
    let peer: IpAddr = peer.parse().unwrap();
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(SocketAddr::new(peer, 40_000)))
}

fn get(uri: &str) -> Request {
    request(Method::GET, uri, UNTRUSTED_PEER)
        .body(Body::empty())
        .unwrap()
}

async fn send(request: Request) -> Response {
    app().oneshot(request).await.unwrap()
}

async fn body_bytes(response: Response) -> Bytes {
    Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

fn header_request_id(response: &Response) -> String {
    response.headers()[X_REQUEST_ID]
        .to_str()
        .unwrap()
        .to_owned()
}

fn assert_uuid_v7(value: &str) {
    let uuid = Uuid::parse_str(value).unwrap();
    assert_eq!(uuid.get_version_num(), 7, "{value}");
    assert_eq!(uuid.hyphenated().to_string(), value);
}

async fn assert_error_envelope(response: Response, status: StatusCode, code: &str) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let request_id = header_request_id(&response);
    assert_uuid_v7(&request_id);
    let body = json_body(response).await;
    assert_eq!(body["error"]["code"], code);
    assert_eq!(body["error"]["requestId"], request_id.as_str());
    body
}

async fn gzip(payload: Vec<u8>) -> Bytes {
    let compressor = CompressionLayer::new()
        .compress_when(SizeAbove::new(0))
        .layer(tower::service_fn(move |_request: Request| {
            let payload = payload.clone();
            async move {
                Ok::<_, Infallible>(([(CONTENT_TYPE, "application/json")], payload).into_response())
            }
        }));
    let response = compressor
        .oneshot(
            Request::builder()
                .header(ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[CONTENT_ENCODING], "gzip");
    Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes()
}

#[tokio::test]
async fn svc_request_id_generated_when_untrusted() {
    let response = send(
        request(Method::GET, "/test/echo", UNTRUSTED_PEER)
            .header(X_REQUEST_ID, "attacker-chosen-id")
            .header("x-forwarded-for", TRUSTED_PROXY)
            .header("forwarded", "for=10.0.0.3")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let request_id = header_request_id(&response);
    assert_uuid_v7(&request_id);
    assert_ne!(request_id, "attacker-chosen-id");
    let body = json_body(response).await;
    assert_eq!(body["requestId"], request_id.as_str());
    assert_eq!(body["requestIdHeader"], request_id.as_str());
}

#[tokio::test]
async fn svc_request_id_accepted_from_trusted_proxy() {
    let inbound = "edge-proxy.id:42/abc+=_@-";
    let response = send(
        request(Method::GET, "/test/echo", TRUSTED_PROXY)
            .header(X_REQUEST_ID, inbound)
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_request_id(&response), inbound);
    let body = json_body(response).await;
    assert_eq!(body["requestId"], inbound);
    assert_eq!(body["requestIdHeader"], inbound);
}

#[tokio::test]
async fn svc_request_id_trusted_invalid_or_missing_is_regenerated() {
    let too_long = "a".repeat(129);
    let max_length = "a".repeat(128);
    for (inbound, accepted) in [
        (None, false),
        (Some("has space"), false),
        (Some("semi;colon"), false),
        (Some(too_long.as_str()), false),
        (Some(max_length.as_str()), true),
    ] {
        let mut builder = request(Method::GET, "/test/echo", TRUSTED_PROXY);
        if let Some(value) = inbound {
            builder = builder.header(X_REQUEST_ID, value);
        }
        let response = send(builder.body(Body::empty()).unwrap()).await;
        let request_id = header_request_id(&response);
        if accepted {
            assert_eq!(Some(request_id.as_str()), inbound);
        } else {
            assert_uuid_v7(&request_id);
        }
    }
}

#[tokio::test]
async fn svc_request_id_generated_without_socket_peer() {
    let response = send(
        Request::get("/test/echo")
            .header(X_REQUEST_ID, "edge-7")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_uuid_v7(&header_request_id(&response));
    assert_eq!(json_body(response).await["clientIp"], Value::Null);
}

#[tokio::test]
async fn svc_request_id_echoed_on_unmatched_route() {
    let response = send(get("/test/missing")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_uuid_v7(&header_request_id(&response));
}

#[tokio::test]
async fn svc_client_ip_resolved_once_for_downstream() {
    let response = send(
        request(Method::GET, "/test/echo", TRUSTED_PROXY)
            .header("x-forwarded-for", "198.51.100.200, 203.0.113.7")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(json_body(response).await["clientIp"], "203.0.113.7");

    let response = send(
        request(Method::GET, "/test/echo", UNTRUSTED_PEER)
            .header("x-forwarded-for", "198.51.100.200")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(json_body(response).await["clientIp"], UNTRUSTED_PEER);
}

#[tokio::test]
async fn svc_panic_becomes_internal_error_with_request_id() {
    let response = send(get("/test/panic")).await;
    let body = assert_error_envelope(
        response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
    )
    .await;

    assert_eq!(body["error"]["details"], json!({}));
    let wire = body.to_string();
    for needle in [
        "palmr-panic-sentinel",
        "/srv/palmr",
        "secret.rs",
        "panicked",
        "backtrace",
        "src/",
    ] {
        assert!(!wire.contains(needle), "{needle:?} leaked into {wire}");
    }
}

#[tokio::test]
async fn svc_panic_keeps_trusted_request_id_and_service_alive() {
    let service = app();
    let response = service
        .clone()
        .oneshot(
            request(Method::GET, "/test/panic", TRUSTED_PROXY)
                .header(X_REQUEST_ID, "edge-panic-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(header_request_id(&response), "edge-panic-1");
    assert_eq!(
        json_body(response).await["error"]["requestId"],
        "edge-panic-1"
    );

    let after = service.oneshot(get("/test/echo")).await.unwrap();
    assert_eq!(after.status(), StatusCode::OK);
}

#[tokio::test]
async fn it_body_limit_413_has_request_id() {
    let oversized = vec![b'a'; TWO_MIB + 1];
    let response = send(
        request(Method::POST, "/test/body", UNTRUSTED_PEER)
            .header(CONTENT_LENGTH, oversized.len())
            .body(Body::from(oversized))
            .unwrap(),
    )
    .await;
    let body = assert_error_envelope(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "REQUEST_BODY_TOO_LARGE",
    )
    .await;
    assert_eq!(body["error"]["details"], json!({ "maxBytes": TWO_MIB }));
}

#[tokio::test]
async fn it_body_limit_undeclared_length_413_has_request_id() {
    let response = send(
        request(Method::POST, "/test/body", UNTRUSTED_PEER)
            .body(Body::from(vec![b'a'; TWO_MIB + 1]))
            .unwrap(),
    )
    .await;
    let body = assert_error_envelope(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "REQUEST_BODY_TOO_LARGE",
    )
    .await;
    assert_eq!(body["error"]["details"]["maxBytes"], TWO_MIB);
}

#[tokio::test]
async fn it_body_limit_rejection_is_never_plain_text() {
    let response = send(
        request(Method::POST, "/test/body", TRUSTED_PROXY)
            .header(X_REQUEST_ID, "edge-413")
            .body(Body::from(vec![b'a'; TWO_MIB + 1]))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(header_request_id(&response), "edge-413");
    let bytes = body_bytes(response).await;
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !text.to_ascii_lowercase().contains("length limit"),
        "{text}"
    );
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"]["requestId"], "edge-413");
}

#[tokio::test]
async fn it_body_under_limit_passes() {
    for size in [0, 1024 * 1024, TWO_MIB] {
        let response = send(
            request(Method::POST, "/test/body", UNTRUSTED_PEER)
                .header(CONTENT_LENGTH, size)
                .body(Body::from(vec![b'a'; size]))
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{size}");
        assert_eq!(json_body(response).await["received"], size);
    }
}

#[tokio::test]
async fn it_body_limit_follows_configured_limit() {
    let limits = ControlPlaneLimits {
        body: BodyLimit::new(16),
        ..ControlPlaneLimits::default()
    };
    let response = app_with(limits)
        .oneshot(
            request(Method::POST, "/test/body", UNTRUSTED_PEER)
                .body(Body::from(vec![b'a'; 17]))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = assert_error_envelope(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "REQUEST_BODY_TOO_LARGE",
    )
    .await;
    assert_eq!(body["error"]["details"]["maxBytes"], 16);
}

#[tokio::test]
async fn it_streamed_byte_route_bypasses_global_body_limit() {
    let size = TWO_MIB * 2;
    let response = send(
        request(Method::PATCH, "/test/stream", UNTRUSTED_PEER)
            .header(CONTENT_LENGTH, size)
            .body(Body::from(vec![b'a'; size]))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["received"], size);
}

#[tokio::test]
async fn it_gzip_request_body_is_decoded_for_control_plane() {
    let payload = br#"{"name":"quarterly report"}"#.to_vec();
    let encoded = gzip(payload.clone()).await;
    let response = send(
        request(Method::POST, "/test/body", UNTRUSTED_PEER)
            .header(CONTENT_ENCODING, "gzip")
            .header(CONTENT_TYPE, "application/json")
            .header(CONTENT_LENGTH, encoded.len())
            .body(Body::from(encoded))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["received"], payload.len());
}

#[tokio::test]
async fn it_decoded_request_body_is_bounded() {
    let encoded = gzip(vec![b' '; TWO_MIB * 2]).await;
    assert!(encoded.len() < TWO_MIB);
    let response = send(
        request(Method::POST, "/test/body", UNTRUSTED_PEER)
            .header(CONTENT_ENCODING, "gzip")
            .header(CONTENT_LENGTH, encoded.len())
            .body(Body::from(encoded))
            .unwrap(),
    )
    .await;
    let body = assert_error_envelope(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "REQUEST_BODY_TOO_LARGE",
    )
    .await;
    assert_eq!(body["error"]["details"]["maxBytes"], TWO_MIB);
}

#[tokio::test]
async fn it_undecodable_request_encoding_uses_envelope() {
    let response = send(
        request(Method::POST, "/test/body", UNTRUSTED_PEER)
            .header(CONTENT_ENCODING, "zstd")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_error_envelope(
        response,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    )
    .await;
}

#[tokio::test]
async fn it_streamed_byte_route_body_is_not_decoded() {
    let encoded = gzip(br#"{"a":1}"#.to_vec()).await;
    let response = send(
        request(Method::PATCH, "/test/stream", UNTRUSTED_PEER)
            .header(CONTENT_ENCODING, "gzip")
            .body(Body::from(encoded.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["received"], encoded.len());
}

#[tokio::test(start_paused = true)]
async fn it_control_plane_deadline_is_request_timeout() {
    let response = send(get("/test/slow/31")).await;
    let body =
        assert_error_envelope(response, StatusCode::REQUEST_TIMEOUT, "REQUEST_TIMEOUT").await;
    assert_ne!(body["error"]["code"], "TRANSFER_IDLE_TIMEOUT");
    assert_ne!(body["error"]["code"], "INTERNAL_ERROR");
}

#[tokio::test(start_paused = true)]
async fn it_control_plane_deadline_is_thirty_seconds() {
    assert_eq!(
        ControlPlaneLimits::default().deadline,
        Duration::from_secs(30)
    );
    let response = send(get("/test/slow/29")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test(start_paused = true)]
async fn it_deadline_follows_configured_limit() {
    let limits = ControlPlaneLimits {
        deadline: Duration::from_secs(2),
        ..ControlPlaneLimits::default()
    };
    let response = app_with(limits)
        .oneshot(
            request(Method::GET, "/test/slow/3", TRUSTED_PROXY)
                .header(X_REQUEST_ID, "edge-408")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(header_request_id(&response), "edge-408");
    assert_eq!(json_body(response).await["error"]["requestId"], "edge-408");
}

#[tokio::test(start_paused = true)]
async fn it_idle_only_byte_route_has_no_control_plane_deadline() {
    let response = send(get("/test/download/7200")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn it_compressible_response_is_compressed() {
    for encoding in ["gzip", "br"] {
        let response = send(
            request(Method::GET, "/test/large", UNTRUSTED_PEER)
                .header(ACCEPT_ENCODING, encoding)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_ENCODING], encoding);
        assert!(response.headers().get(CONTENT_LENGTH).is_none());
    }
}

#[tokio::test]
async fn it_identity_byte_route_is_never_compressed() {
    for uri in ["/test/download/0", "/test/object"] {
        let response = send(
            request(Method::GET, uri, UNTRUSTED_PEER)
                .header(ACCEPT_ENCODING, "gzip, br")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert!(response.headers().get(CONTENT_ENCODING).is_none(), "{uri}");
        let declared: usize = response.headers()[CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(body_bytes(response).await.len(), declared, "{uri}");
    }
}

#[tokio::test]
async fn it_path_is_normalized_before_routing() {
    for uri in [
        "/test/echo/",
        "//test/echo",
        "/test//echo",
        "//test///echo//",
    ] {
        let response = send(get(&format!("{uri}?b=2&a=%2F"))).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let body = json_body(response).await;
        assert_eq!(body["path"], "/test/echo", "{uri}");
        assert_eq!(body["query"], "b=2&a=%2F", "{uri}");
    }

    let response = send(get("/TEST/echo")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn it_no_cors_or_security_headers_are_emitted() {
    let preflight = send(
        request(Method::OPTIONS, "/test/echo", UNTRUSTED_PEER)
            .header(ORIGIN, "https://evil.test")
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let simple = send(
        request(Method::GET, "/test/echo", UNTRUSTED_PEER)
            .header(ORIGIN, "https://evil.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    for response in [&preflight, &simple] {
        for (name, _) in response.headers() {
            let name = name.as_str();
            assert!(!name.starts_with("access-control-"), "{name}");
            assert!(
                ![
                    "content-security-policy",
                    "x-content-type-options",
                    "referrer-policy",
                    "cross-origin-resource-policy",
                    "cross-origin-opener-policy",
                    "permissions-policy",
                    "x-frame-options",
                    "strict-transport-security",
                ]
                .contains(&name),
                "{name}"
            );
        }
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn lines(&self) -> Vec<Value> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
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

async fn traced(request: Request) -> (Response, Vec<Value>) {
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("info"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let response = send(request).await;
    (response, capture.lines())
}

fn completion(lines: &[Value]) -> &Value {
    let completed: Vec<&Value> = lines
        .iter()
        .filter(|line| line["message"] == "request completed")
        .collect();
    assert_eq!(completed.len(), 1, "{lines:?}");
    completed[0]
}

#[tokio::test]
async fn it_request_span_records_canonical_fields() {
    let (response, lines) = traced(
        request(Method::GET, "/test/echo?token=query-secret", TRUSTED_PROXY)
            .header("x-forwarded-for", "203.0.113.7")
            .header("authorization", "Bearer header-secret")
            .header("cookie", "palmr_session=cookie-secret")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let request_id = header_request_id(&response);
    let line = completion(&lines);

    assert_eq!(line["level"], "INFO");
    assert_eq!(line["request_id"], request_id.as_str());
    assert_eq!(line["method"], "GET");
    assert_eq!(line["route"], "/test/echo");
    assert_eq!(line["client_ip"], "203.0.113.7");
    assert_eq!(line["status"], 200);
    assert!(line["duration_ms"].is_u64());
    assert!(line.get("error_code").is_none());

    let output = serde_json::to_string(&lines).unwrap();
    for secret in ["query-secret", "header-secret", "cookie-secret", "Bearer"] {
        assert!(!output.contains(secret), "{secret} leaked into {output}");
    }
}

#[tokio::test]
async fn it_request_span_records_route_template_and_error_code() {
    let (response, lines) = traced(get("/test/panic")).await;
    let request_id = header_request_id(&response);

    let panic_line = lines.iter().find(|line| line["level"] == "ERROR").unwrap();
    assert_eq!(panic_line["request_id"], request_id.as_str());
    assert!(!serde_json::to_string(&lines)
        .unwrap()
        .contains("palmr-panic-sentinel"));

    let line = completion(&lines);
    assert_eq!(line["route"], "/test/panic");
    assert_eq!(line["status"], 500);
    assert_eq!(line["error_code"], "INTERNAL_ERROR");

    let (_, lines) = traced(get("/test/slow/0")).await;
    assert_eq!(completion(&lines)["route"], "/test/slow/{seconds}");
}
