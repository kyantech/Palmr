use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use http::header::{
    HeaderValue, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, SET_COOKIE,
};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use rstest::rstest;
use serde_json::Value;
use time::macros::datetime;
use tower::{Service, ServiceExt};
use utoipa_axum::routes;

use super::{
    check_content_type, with_test_csrf, AnonymousCsrf, CsrfAuthority, CsrfGuard, RequestContent,
    CSRF_HEADER,
};
use crate::app::auth_class::AuthClass;
use crate::app::router::{
    with_middleware, BytePath, Deadline, HttpEdge, RateLimitClass, RequestBody, ResponseEncoding,
    RouteError, RoutePolicy, Routes, Transport,
};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::crypto::token::{Token, ENCODED_TOKEN_LEN};
use crate::infra::http::cookies::CookiePolicy;
use crate::infra::http::headers::SecurityHeaders;
use crate::infra::http::proxy::TrustedProxies;

const BASE_URL: &str = "https://palmr.example.com";
const RESPONSE_READ_CAP: usize = 64 * 1024;
const OFFSET_OCTET_STREAM: &str = "application/offset+octet-stream";
const FORM: &str = "application/x-www-form-urlencoded";

const PUBLIC_WRITE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

const GRANT_WRITE: RoutePolicy = RoutePolicy::new(
    AuthClass::PublicGrant,
    RateLimitClass::None,
    Transport::ControlPlane,
);

const TUS_PATCH: RoutePolicy = RoutePolicy::new(
    AuthClass::PublicGrant,
    RateLimitClass::None,
    Transport::BytePath(BytePath::new(
        RequestBody::Streamed,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
)
.with_request_content(RequestContent::OffsetOctetStream);

const TUS_CREATE: RoutePolicy = RoutePolicy::new(
    AuthClass::PublicGrant,
    RateLimitClass::None,
    Transport::BytePath(BytePath::new(
        RequestBody::Streamed,
        ResponseEncoding::Identity,
        Deadline::IdleOnly,
    )),
)
.with_request_content(RequestContent::JsonOrOffsetOctetStream);

const MULTIPART_WRITE: RoutePolicy = PUBLIC_WRITE.with_request_content(RequestContent::Multipart);

#[derive(Clone, Default)]
struct Reached(Arc<AtomicUsize>);

impl Reached {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn reached(Extension(reached): Extension<Reached>) -> StatusCode {
    reached.0.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/test/csrf/public", responses((status = 204)))]
async fn public_write(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(post, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_write(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(get, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_read(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(options, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_options(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(put, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_put(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(delete, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_delete(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(patch, path = "/test/csrf/grant", responses((status = 204)))]
async fn grant_patch(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(patch, path = "/test/csrf/tus/{id}", params(("id" = String, Path)), responses((status = 204)))]
async fn tus_patch(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(post, path = "/test/csrf/tus", responses((status = 204)))]
async fn tus_create(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(post, path = "/test/csrf/multipart", responses((status = 204)))]
async fn multipart_write(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(get, path = "/test/csrf/bootstrap", responses((status = 204)))]
async fn bootstrap(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(get, path = "/test/csrf/plain", responses((status = 204)))]
async fn plain(extension: Extension<Reached>) -> StatusCode {
    reached(extension)
}

#[utoipa::path(post, path = "/test/csrf/login", responses((status = 204)))]
async fn login(extension: Extension<Reached>) -> Response {
    reached(extension);
    let mut headers = HeaderMap::new();
    CookiePolicy::from_base_url(&config(BASE_URL).base_url)
        .append_session_pair(
            &mut headers,
            &Token::mint().unwrap().encode(),
            &Token::mint().unwrap().encode(),
            60,
        )
        .unwrap();
    (StatusCode::NO_CONTENT, headers).into_response()
}

fn test_routes() -> Routes<()> {
    Routes::new()
        .route(PUBLIC_WRITE, routes!(public_write))
        .route(GRANT_WRITE, routes!(grant_write))
        .route(GRANT_WRITE, routes!(grant_read))
        .route(GRANT_WRITE, routes!(grant_options))
        .route(GRANT_WRITE, routes!(grant_put))
        .route(GRANT_WRITE, routes!(grant_delete))
        .route(GRANT_WRITE, routes!(grant_patch))
        .route(TUS_PATCH, routes!(tus_patch))
        .route(TUS_CREATE, routes!(tus_create))
        .route(MULTIPART_WRITE, routes!(multipart_write))
        .route(PUBLIC_WRITE.with_anonymous_csrf(), routes!(bootstrap))
        .route(PUBLIC_WRITE, routes!(plain))
        .route(PUBLIC_WRITE.with_anonymous_csrf(), routes!(login))
}

fn config(base_url: &str) -> OperatorConfig {
    let vars: Vec<(&str, &str)> = if base_url.is_empty() {
        Vec::new()
    } else {
        vec![("PALMR_BASE_URL", base_url)]
    };
    OperatorConfig::load(&EnvironmentSource::from_vars(vars))
        .unwrap()
        .config
}

struct App<S> {
    service: S,
    reached: Reached,
}

fn app_for(
    base_url: &str,
) -> App<impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone> {
    let config = config(base_url);
    let reached = Reached::default();
    let router = test_routes()
        .build()
        .unwrap()
        .router
        .layer(Extension(reached.clone()));
    let edge = HttpEdge::new(
        Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))),
        TrustedProxies::new(&config.trust_proxy),
        SecurityHeaders::new(&config),
        CsrfGuard::new(&config.base_url),
    );
    App {
        service: with_middleware(router, &edge),
        reached,
    }
}

fn app() -> App<impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone>
{
    app_for(BASE_URL)
}

impl<S> App<S>
where
    S: Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone,
{
    async fn send(&self, request: http::request::Builder) -> Response {
        self.send_body(request, Body::empty()).await
    }

    async fn send_body(&self, request: http::request::Builder, body: Body) -> Response {
        let request = request
            .extension(ConnectInfo(SocketAddr::from(([198, 51, 100, 7], 40_000))))
            .body(body)
            .unwrap();
        self.service.clone().oneshot(request).await.unwrap()
    }
}

fn request(method: Method, uri: &str) -> http::request::Builder {
    Request::builder().method(method).uri(uri)
}

fn pair(cookie: &str, header: &str) -> impl Fn(http::request::Builder) -> http::request::Builder {
    let cookie = format!("palmr_csrf={cookie}");
    let header = header.to_owned();
    move |builder| {
        builder
            .header(COOKIE, cookie.as_str())
            .header(CSRF_HEADER, header.as_str())
    }
}

fn fresh() -> String {
    Token::mint().unwrap().encode().expose_secret().clone()
}

async fn body_text(response: Response) -> String {
    let bytes = Limited::new(response.into_body(), RESPONSE_READ_CAP)
        .collect()
        .await
        .unwrap()
        .to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn assert_rejected(response: Response, status: StatusCode, code: &str) -> String {
    assert_eq!(response.status(), status);
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let text = body_text(response).await;
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"]["code"], code, "{text}");
    assert_eq!(body["error"]["requestId"], request_id.as_str());
    text
}

fn csrf_set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .filter(|value| value.starts_with("palmr_csrf="))
        .collect()
}

#[test]
fn unit_csrf_authority_follows_auth_class() {
    for class in AuthClass::ALL {
        let expected = match class {
            AuthClass::Public | AuthClass::Setup => CsrfAuthority::None,
            _ => CsrfAuthority::Cookie,
        };
        assert_eq!(CsrfAuthority::of(class), expected, "{class}");
    }
    assert_eq!(
        CsrfAuthority::of(AuthClass::PublicGrant),
        CsrfAuthority::Cookie
    );
    assert_eq!(TUS_PATCH.request_gate().authority(), CsrfAuthority::Cookie);
    assert_eq!(TUS_CREATE.request_gate().authority(), CsrfAuthority::Cookie);
}

#[rstest]
#[case::json(RequestContent::Json, "application/json", true)]
#[case::json_charset(RequestContent::Json, "application/json; charset=utf-8", true)]
#[case::json_charset_upper(RequestContent::Json, "Application/JSON; Charset=\"UTF-8\"", true)]
#[case::json_other_charset(RequestContent::Json, "application/json; charset=iso-8859-1", false)]
#[case::json_other_parameter(RequestContent::Json, "application/json; profile=x", false)]
#[case::json_suffix(RequestContent::Json, "application/merge-patch+json", false)]
#[case::json_text(RequestContent::Json, "text/plain;charset=UTF-8", false)]
#[case::json_form(RequestContent::Json, FORM, false)]
#[case::json_multipart(RequestContent::Json, "multipart/form-data; boundary=x", false)]
#[case::json_octets(RequestContent::Json, OFFSET_OCTET_STREAM, false)]
#[case::json_garbage(RequestContent::Json, "json", false)]
#[case::json_empty(RequestContent::Json, "", false)]
#[case::multipart(RequestContent::Multipart, "multipart/form-data; boundary=abc", true)]
#[case::multipart_form(RequestContent::Multipart, FORM, false)]
#[case::multipart_json(RequestContent::Multipart, "application/json", false)]
#[case::tus_patch(RequestContent::OffsetOctetStream, OFFSET_OCTET_STREAM, true)]
#[case::tus_patch_json(RequestContent::OffsetOctetStream, "application/json", false)]
#[case::tus_patch_form(RequestContent::OffsetOctetStream, FORM, false)]
#[case::tus_create_json(RequestContent::JsonOrOffsetOctetStream, "application/json", true)]
#[case::tus_create_octets(RequestContent::JsonOrOffsetOctetStream, OFFSET_OCTET_STREAM, true)]
#[case::tus_create_form(RequestContent::JsonOrOffsetOctetStream, FORM, false)]
#[case::form_with_charset(
    RequestContent::Json,
    "application/x-www-form-urlencoded; charset=utf-8",
    false
)]
fn unit_request_content_type_policy(
    #[case] content: RequestContent,
    #[case] content_type: &str,
    #[case] accepted: bool,
) {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
    assert_eq!(
        check_content_type(content, &headers).is_ok(),
        accepted,
        "{content:?} {content_type}"
    );
}

#[test]
fn unit_content_type_required_only_when_a_body_is_declared() {
    let mut headers = HeaderMap::new();
    assert!(check_content_type(RequestContent::Json, &headers).is_ok());
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    assert!(check_content_type(RequestContent::Json, &headers).is_ok());
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("2"));
    assert!(check_content_type(RequestContent::Json, &headers).is_err());

    let mut chunked = HeaderMap::new();
    chunked.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    assert!(check_content_type(RequestContent::Json, &chunked).is_err());

    let mut repeated = HeaderMap::new();
    repeated.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    repeated.append(CONTENT_TYPE, HeaderValue::from_static(FORM));
    assert!(check_content_type(RequestContent::Json, &repeated).is_err());
}

#[rstest]
#[case::exact("https://palmr.example.com", true)]
#[case::explicit_default_port("https://palmr.example.com:443", true)]
#[case::host_case("https://PALMR.example.com", true)]
#[case::suffix_lookalike("https://palmr.example.com.evil.test", false)]
#[case::prefix_lookalike("https://evil-palmr.example.com", false)]
#[case::subdomain("https://files.palmr.example.com", false)]
#[case::scheme("http://palmr.example.com", false)]
#[case::port("https://palmr.example.com:8443", false)]
#[case::opaque("null", false)]
#[case::path("https://palmr.example.com/evil", false)]
#[case::userinfo("https://user@palmr.example.com", false)]
#[case::garbage("palmr.example.com", false)]
fn unit_origin_compares_the_origin_tuple(#[case] origin: &str, #[case] allowed: bool) {
    let guard = CsrfGuard::new(&config(BASE_URL).base_url);
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
    assert_eq!(guard.origin_allowed(&headers), allowed, "{origin}");
}

#[test]
fn unit_origin_absent_passes_and_repeated_fails() {
    let guard = CsrfGuard::new(&config(BASE_URL).base_url);
    assert!(guard.origin_allowed(&HeaderMap::new()));
    let mut repeated = HeaderMap::new();
    repeated.append(ORIGIN, HeaderValue::from_static(BASE_URL));
    repeated.append(ORIGIN, HeaderValue::from_static(BASE_URL));
    assert!(!guard.origin_allowed(&repeated));
}

#[test]
fn unit_origin_ignores_base_url_path() {
    let guard = CsrfGuard::new(&config("https://example.com/palmr").base_url);
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, HeaderValue::from_static("https://example.com"));
    assert!(guard.origin_allowed(&headers));
}

#[test]
fn unit_anonymous_csrf_is_declared_only_on_the_public_surface() {
    for class in AuthClass::ALL {
        let anonymous = matches!(
            class,
            AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup
        );
        assert_eq!(AnonymousCsrf::Issue.permitted_for(class), anonymous);
        assert!(AnonymousCsrf::None.permitted_for(class));
    }

    let error = Routes::<()>::new()
        .route(
            RoutePolicy::new(
                AuthClass::Authenticated,
                RateLimitClass::None,
                Transport::ControlPlane,
            )
            .with_anonymous_csrf(),
            routes!(plain),
        )
        .build()
        .err()
        .unwrap();
    assert_eq!(
        error.errors(),
        [RouteError::AnonymousCsrfOutsidePublicSurface {
            method: Method::GET,
            path: "/test/csrf/plain".to_owned(),
        }]
    );
}

#[tokio::test]
async fn svc_safe_methods_bypass_csrf() {
    let app = app();
    for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
        let response = app
            .send(request(method.clone(), "/test/csrf/grant").header(ORIGIN, "https://evil.test"))
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{method}");
    }
    assert_eq!(app.reached.count(), 3);
}

#[tokio::test]
async fn svc_cookie_authority_writes_require_double_submit() {
    let app = app();
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        let response = app.send(request(method.clone(), "/test/csrf/grant")).await;
        assert_rejected(response, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;

        let token = fresh();
        let response = app
            .send(pair(&token, &token)(request(
                method.clone(),
                "/test/csrf/grant",
            )))
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{method}");
    }
    assert_eq!(app.reached.count(), 4);
}

#[tokio::test]
async fn svc_csrf_missing_and_mismatched() {
    let app = app();
    let token = fresh();
    let other = fresh();
    let grant = || request(Method::POST, "/test/csrf/grant");

    let cookie_only = app
        .send(grant().header(COOKIE, format!("palmr_csrf={token}")))
        .await;
    assert_rejected(cookie_only, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;

    let header_only = app.send(grant().header(CSRF_HEADER, &token)).await;
    assert_rejected(header_only, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;

    let empty_header = app.send(pair(&token, "")(grant())).await;
    assert_rejected(empty_header, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;

    let mismatched = app.send(pair(&token, &other)(grant())).await;
    let text = assert_rejected(mismatched, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID").await;
    assert!(!text.contains(&token) && !text.contains(&other), "{text}");

    let malformed = app.send(pair("forged", "forged")(grant())).await;
    assert_rejected(malformed, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID").await;

    let repeated_header = app
        .send(pair(&token, &token)(grant()).header(CSRF_HEADER, &token))
        .await;
    assert_rejected(repeated_header, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID").await;

    let repeated_cookie = app
        .send(pair(&token, &token)(grant()).header(COOKIE, format!("palmr_csrf={other}")))
        .await;
    assert_rejected(repeated_cookie, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID").await;

    assert_eq!(app.reached.count(), 0);
    let valid = app.send(pair(&token, &token)(grant())).await;
    assert_eq!(valid.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.reached.count(), 1);
}

#[tokio::test]
async fn svc_public_cookieless_authority_is_exempt() {
    let app = app();
    let bare = app.send(request(Method::POST, "/test/csrf/public")).await;
    assert_eq!(bare.status(), StatusCode::NO_CONTENT);

    let token = fresh();
    let with_cookie_only = app
        .send(
            request(Method::POST, "/test/csrf/public")
                .header(COOKIE, format!("palmr_csrf={token}")),
        )
        .await;
    assert_eq!(with_cookie_only.status(), StatusCode::NO_CONTENT);

    let mismatched = app
        .send(pair(&token, &fresh())(request(
            Method::POST,
            "/test/csrf/public",
        )))
        .await;
    assert_eq!(mismatched.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.reached.count(), 3);
}

#[tokio::test]
async fn svc_tus_and_streaming_writes_are_not_exempt() {
    let app = app();
    let patch = || {
        request(Method::PATCH, "/test/csrf/tus/0192f3a1")
            .header(CONTENT_TYPE, OFFSET_OCTET_STREAM)
            .header(CONTENT_LENGTH, "4")
    };

    let missing = app.send_body(patch(), Body::from("abcd")).await;
    assert_rejected(missing, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;

    let token = fresh();
    let mismatched = app
        .send_body(pair(&token, &fresh())(patch()), Body::from("abcd"))
        .await;
    assert_rejected(mismatched, StatusCode::FORBIDDEN, "CSRF_TOKEN_INVALID").await;

    let accepted = app
        .send_body(pair(&token, &token)(patch()), Body::from("abcd"))
        .await;
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let create = app
        .send(request(Method::POST, "/test/csrf/tus").header(CONTENT_TYPE, OFFSET_OCTET_STREAM))
        .await;
    assert_rejected(create, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING").await;
    assert_eq!(app.reached.count(), 1);
}

#[tokio::test]
async fn svc_streaming_routes_are_not_forced_to_json() {
    let app = app();
    let token = fresh();

    let tus_json = app
        .send_body(
            pair(&token, &token)(request(Method::PATCH, "/test/csrf/tus/0192f3a1"))
                .header(CONTENT_TYPE, "application/json")
                .header(CONTENT_LENGTH, "2"),
            Body::from("{}"),
        )
        .await;
    assert_rejected(
        tus_json,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    )
    .await;

    for content_type in ["application/json", OFFSET_OCTET_STREAM] {
        let create = app
            .send_body(
                pair(&token, &token)(request(Method::POST, "/test/csrf/tus"))
                    .header(CONTENT_TYPE, content_type)
                    .header(CONTENT_LENGTH, "2"),
                Body::from("{}"),
            )
            .await;
        assert_eq!(create.status(), StatusCode::NO_CONTENT, "{content_type}");
    }

    let multipart = app
        .send_body(
            request(Method::POST, "/test/csrf/multipart")
                .header(CONTENT_TYPE, "multipart/form-data; boundary=palmr")
                .header(CONTENT_LENGTH, "2"),
            Body::from("--"),
        )
        .await;
    assert_eq!(multipart.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.reached.count(), 3);
}

#[tokio::test]
async fn svc_json_content_type_enforced_before_the_handler() {
    let app = app();
    let post = |content_type: &str| {
        request(Method::POST, "/test/csrf/public")
            .header(CONTENT_TYPE, content_type)
            .header(CONTENT_LENGTH, "2")
    };

    for accepted in ["application/json", "application/json; charset=utf-8"] {
        let response = app.send_body(post(accepted), Body::from("{}")).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{accepted}");
    }
    for refused in [
        "text/plain;charset=UTF-8",
        "multipart/form-data; boundary=x",
        OFFSET_OCTET_STREAM,
        "application/xml",
    ] {
        let response = app.send_body(post(refused), Body::from("{}")).await;
        assert_rejected(
            response,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
        )
        .await;
    }
    let undeclared = app
        .send_body(
            request(Method::POST, "/test/csrf/public").header(CONTENT_LENGTH, "2"),
            Body::from("{}"),
        )
        .await;
    assert_rejected(
        undeclared,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    )
    .await;
    assert_eq!(app.reached.count(), 2);
}

#[tokio::test]
async fn it_form_urlencoded_rejected_415() {
    let app = app();
    let token = fresh();
    let form = |builder: http::request::Builder| {
        builder
            .header(CONTENT_TYPE, FORM)
            .header(CONTENT_LENGTH, "11")
    };

    for builder in [
        form(request(Method::POST, "/test/csrf/public")),
        form(request(Method::POST, "/test/csrf/public").header(ORIGIN, BASE_URL)),
        form(pair(&token, &token)(request(
            Method::POST,
            "/test/csrf/grant",
        ))),
        form(pair(&token, &token)(request(
            Method::DELETE,
            "/test/csrf/grant",
        ))),
        form(pair(&token, &token)(request(
            Method::POST,
            "/test/csrf/tus",
        ))),
        form(pair(&token, &token)(request(
            Method::PATCH,
            "/test/csrf/tus/0192f3a1",
        ))),
        form(request(Method::POST, "/test/csrf/multipart")),
        request(Method::POST, "/test/csrf/public").header(
            CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=UTF-8",
        ),
    ] {
        let response = app.send_body(builder, Body::from("name=guest&")).await;
        let text = assert_rejected(
            response,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
        )
        .await;
        assert!(!text.contains("VALIDATION_ERROR"), "{text}");
    }
    assert_eq!(app.reached.count(), 0);
}

#[tokio::test]
async fn svc_origin_checked_against_base_url() {
    let app = app();
    for origin in [
        "https://palmr.example.com.evil.test",
        "http://palmr.example.com",
        "https://evil.test",
        "null",
    ] {
        let public = app
            .send(request(Method::POST, "/test/csrf/public").header(ORIGIN, origin))
            .await;
        assert_rejected(public, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED").await;

        let token = fresh();
        let grant = app
            .send(
                pair(&token, &token)(request(Method::POST, "/test/csrf/grant"))
                    .header(ORIGIN, origin),
            )
            .await;
        assert_rejected(grant, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED").await;
    }
    assert_eq!(app.reached.count(), 0);

    let matching = app
        .send(request(Method::POST, "/test/csrf/public").header(ORIGIN, BASE_URL))
        .await;
    assert_eq!(matching.status(), StatusCode::NO_CONTENT);
    let token = fresh();
    let matching_grant = app
        .send(
            pair(&token, &token)(request(Method::POST, "/test/csrf/grant"))
                .header(ORIGIN, BASE_URL),
        )
        .await;
    assert_eq!(matching_grant.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn svc_origin_never_derived_from_request_headers() {
    let app = app();
    let spoofed = app
        .send(
            request(Method::POST, "/test/csrf/public")
                .header(ORIGIN, "https://evil.test")
                .header("host", "evil.test")
                .header("x-forwarded-host", "evil.test")
                .header("x-forwarded-proto", "https")
                .header("forwarded", "host=evil.test;proto=https"),
        )
        .await;
    assert_rejected(spoofed, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED").await;
    assert_eq!(app.reached.count(), 0);
}

#[tokio::test]
async fn svc_referer_is_not_an_origin_fallback() {
    let app = app();
    let hostile_referer = app
        .send(request(Method::POST, "/test/csrf/public").header(REFERER, "https://evil.test/page"))
        .await;
    assert_eq!(hostile_referer.status(), StatusCode::NO_CONTENT);

    let token = fresh();
    let grant = app
        .send(
            pair(&token, &token)(request(Method::POST, "/test/csrf/grant"))
                .header(REFERER, "https://evil.test/page"),
        )
        .await;
    assert_eq!(grant.status(), StatusCode::NO_CONTENT);

    let matching_referer = app
        .send(
            request(Method::POST, "/test/csrf/public")
                .header(ORIGIN, "https://evil.test")
                .header(REFERER, format!("{BASE_URL}/files")),
        )
        .await;
    assert_rejected(
        matching_referer,
        StatusCode::FORBIDDEN,
        "ORIGIN_NOT_ALLOWED",
    )
    .await;
    assert_eq!(app.reached.count(), 2);
}

#[tokio::test]
async fn svc_unconfigured_base_url_refuses_foreign_origins() {
    let app = app_for("");
    let local = app
        .send(request(Method::POST, "/test/csrf/public").header(ORIGIN, "http://localhost:5487"))
        .await;
    assert_eq!(local.status(), StatusCode::NO_CONTENT);
    for origin in [
        "http://127.0.0.1:5487",
        "http://localhost:3000",
        "https://localhost:5487",
        "https://palmr.example.com",
    ] {
        let response = app
            .send(request(Method::POST, "/test/csrf/public").header(ORIGIN, origin))
            .await;
        assert_rejected(response, StatusCode::FORBIDDEN, "ORIGIN_NOT_ALLOWED").await;
    }
    assert_eq!(app.reached.count(), 1);
}

#[tokio::test]
async fn it_anonymous_public_response_establishes_csrf_cookie() {
    let app = app();
    let response = app.send(request(Method::GET, "/test/csrf/bootstrap")).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let issued = csrf_set_cookies(&response);
    assert_eq!(issued.len(), 1, "{issued:?}");
    let cookie = &issued[0];
    let value = cookie
        .strip_prefix("palmr_csrf=")
        .and_then(|rest| rest.split(';').next())
        .unwrap();
    assert_eq!(value.len(), ENCODED_TOKEN_LEN);
    assert!(Token::decode(value).is_ok());
    assert!(cookie.contains("; Path=/"));
    assert!(cookie.contains("; SameSite=Lax"));
    assert!(cookie.contains("; Secure"));
    assert!(!cookie.contains("HttpOnly"));
    assert!(!cookie.contains("Max-Age"));

    let used = app
        .send(pair(value, value)(request(
            Method::POST,
            "/test/csrf/grant",
        )))
        .await;
    assert_eq!(used.status(), StatusCode::NO_CONTENT);

    let second = app.send(request(Method::GET, "/test/csrf/bootstrap")).await;
    let reissued = csrf_set_cookies(&second);
    assert_eq!(reissued.len(), 1);
    assert!(!reissued[0].contains(value));
}

#[tokio::test]
async fn it_anonymous_csrf_cookie_is_not_secure_on_http_base_url() {
    let app = app_for("http://localhost:5487");
    let response = app.send(request(Method::GET, "/test/csrf/bootstrap")).await;
    let issued = csrf_set_cookies(&response);
    assert_eq!(issued.len(), 1);
    assert!(!issued[0].contains("Secure"));
}

#[tokio::test]
async fn it_existing_anonymous_csrf_is_not_rotated() {
    let app = app();
    let token = fresh();
    for method in [Method::GET, Method::HEAD] {
        let response = app
            .send(
                request(method.clone(), "/test/csrf/bootstrap")
                    .header(COOKIE, format!("other=1; palmr_csrf={token}")),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(response.headers().get(SET_COOKIE).is_none(), "{method}");
    }

    for unusable in [
        "palmr_csrf=forged",
        "palmr_csrf=",
        "palmr_csrf=a; palmr_csrf=b",
    ] {
        let response = app
            .send(request(Method::GET, "/test/csrf/bootstrap").header(COOKIE, unusable))
            .await;
        assert_eq!(csrf_set_cookies(&response).len(), 1, "{unusable}");
    }
}

#[tokio::test]
async fn it_anonymous_csrf_only_where_declared() {
    let app = app();
    let token = fresh();
    for builder in [
        request(Method::GET, "/test/csrf/plain"),
        request(Method::POST, "/test/csrf/public"),
        request(Method::GET, "/test/csrf/grant"),
        pair(&token, &token)(request(Method::POST, "/test/csrf/grant")),
    ] {
        let response = app.send(builder).await;
        assert!(response.headers().get(SET_COOKIE).is_none());
    }
}

#[tokio::test]
async fn it_anonymous_csrf_never_duplicates_a_session_pair() {
    let app = app();
    let response = app.send(request(Method::POST, "/test/csrf/login")).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookies: Vec<&str> = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(cookies.len(), 2, "{cookies:?}");
    assert_eq!(csrf_set_cookies(&response).len(), 1);
    assert!(csrf_set_cookies(&response)[0].contains("Max-Age=60"));
}

#[tokio::test]
async fn svc_request_gate_fails_closed_without_the_edge() {
    let reached = Reached::default();
    let router = test_routes()
        .build()
        .unwrap()
        .router
        .layer(Extension(reached.clone()));
    let token = fresh();
    let write = router
        .clone()
        .oneshot(
            pair(&token, &token)(request(Method::POST, "/test/csrf/grant"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_rejected_without_request_id(write, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
        .await;
    let public_write = router
        .clone()
        .oneshot(
            request(Method::POST, "/test/csrf/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public_write.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let read = router
        .oneshot(
            request(Method::GET, "/test/csrf/bootstrap")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::NO_CONTENT);
    assert!(read.headers().get(SET_COOKIE).is_none());
    assert_eq!(reached.count(), 1);
}

async fn assert_rejected_without_request_id(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
    assert_eq!(body["error"]["code"], code);
}

#[tokio::test]
async fn svc_state_change_precedence_is_content_type_then_origin_then_csrf() {
    let app = app();
    let everything_wrong = app
        .send(
            request(Method::POST, "/test/csrf/grant")
                .header(CONTENT_TYPE, FORM)
                .header(ORIGIN, "https://evil.test"),
        )
        .await;
    assert_rejected(
        everything_wrong,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    )
    .await;

    let foreign_without_csrf = app
        .send(request(Method::POST, "/test/csrf/grant").header(ORIGIN, "https://evil.test"))
        .await;
    assert_rejected(
        foreign_without_csrf,
        StatusCode::FORBIDDEN,
        "ORIGIN_NOT_ALLOWED",
    )
    .await;

    let same_origin_without_csrf = app
        .send(request(Method::POST, "/test/csrf/grant").header(ORIGIN, BASE_URL))
        .await;
    assert_rejected(
        same_origin_without_csrf,
        StatusCode::FORBIDDEN,
        "CSRF_TOKEN_MISSING",
    )
    .await;
}

#[tokio::test]
async fn svc_with_test_csrf_helper_passes_the_gate() {
    let app = app();
    let response = app
        .send(with_test_csrf(request(Method::POST, "/test/csrf/grant")))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
