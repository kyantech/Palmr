use std::fmt;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG};
use http::{HeaderValue, Method, StatusCode};
use serde_json::{json, Value};
use utoipa::openapi::info::{Info, LicenseBuilder};
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::schema::{Components, Schema};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{OpenApi, OpenApiVersion, RefOr};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::routes;
use utoipa_scalar::Scalar;

use super::auth_class::AuthClass;
use super::health::VERSION;
use super::router::{
    application_routes, RateLimitClass, RouteBuildError, RoutePolicy, Routes, Transport,
};
use super::state::AppState;
use crate::config::PublicBaseUrl;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::etag::{matches_validator, weak_etag, weak_etag_from_sha256};
use crate::infra::http::headers::CspNonce;
use crate::infra::http::request_id::{tag_error, RequestId};

pub const OPENAPI_PATH: &str = "/openapi.json";
pub const DOCS_PATH: &str = "/docs";
pub const SCALAR_RUNTIME_PATH: &str = "/docs/scalar.js";

pub const DOCS_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::None,
    Transport::ControlPlane,
);

pub const OPENAPI_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-cache, max-age=60");
const REVALIDATE: HeaderValue = HeaderValue::from_static("no-cache");
const HTML_CONTENT_TYPE: HeaderValue = HeaderValue::from_static("text/html; charset=utf-8");
const JAVASCRIPT_CONTENT_TYPE: HeaderValue =
    HeaderValue::from_static("text/javascript; charset=utf-8");

const API_TITLE: &str = "Palmr";
const API_LICENSE: &str = "AGPL-3.0-only";

pub const SCALAR_RUNTIME: &[u8] =
    include_bytes!("../../vendor/scalar-api-reference-1.71.0/standalone.js");
pub const SCALAR_RUNTIME_SHA256: [u8; 32] = [
    0x08, 0xef, 0x75, 0xa0, 0xb4, 0x50, 0x3c, 0x16, 0x30, 0x31, 0x6f, 0x8f, 0xfc, 0xe1, 0xde, 0x95,
    0xfd, 0x4a, 0x90, 0xfd, 0x78, 0x20, 0x1a, 0x3d, 0x81, 0x1b, 0x51, 0xcb, 0x90, 0xac, 0x20, 0x85,
];

const PAGE_HEAD: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Palmr API reference</title><meta property=\"csp-nonce\" content=\"";
// The runtime auto-mounts a second, unconfigured instance onto any element
// with id "api-reference", so the page mounts under a different id.
const PAGE_RUNTIME_OPEN: &str =
    "\"></head><body><div id=\"palmr-api-reference\"></div><script src=\"";
const PAGE_RUNTIME_CLOSE: &str = "\"></script><script nonce=\"";
const PAGE_INIT: &str =
    "\">Scalar.createApiReference(\"#palmr-api-reference\", $spec);</script></body></html>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Credential {
    Session,
    CsrfCookie,
    CsrfHeader,
    ShareGrant,
    ReverseShareGrant,
}

impl Credential {
    pub const ALL: [Self; 5] = [
        Self::Session,
        Self::CsrfCookie,
        Self::CsrfHeader,
        Self::ShareGrant,
        Self::ReverseShareGrant,
    ];

    pub const fn scheme_name(self) -> &'static str {
        match self {
            Self::Session => "palmrSession",
            Self::CsrfCookie => "palmrCsrfCookie",
            Self::CsrfHeader => "palmrCsrfHeader",
            Self::ShareGrant => "shareGrant",
            Self::ReverseShareGrant => "reverseShareGrant",
        }
    }

    fn scheme(self) -> SecurityScheme {
        let (location, name, description): (fn(ApiKeyValue) -> ApiKey, &str, &str) = match self {
            Self::Session => (
                ApiKey::Cookie,
                "palmr_session",
                "Opaque server-side session issued at login and revocable at any time. \
                 Authorization is always decided by the server from the session row.",
            ),
            Self::CsrfCookie => (
                ApiKey::Cookie,
                "palmr_csrf",
                "Double-submit token that accompanies the session. It grants nothing on its own; \
                 state-changing requests must echo it in the X-Palmr-CSRF header.",
            ),
            Self::CsrfHeader => (
                ApiKey::Header,
                "X-Palmr-CSRF",
                "The palmr_csrf cookie value, required on POST, PUT, PATCH and DELETE requests \
                 authorized by a cookie. Never required on GET or HEAD.",
            ),
            Self::ShareGrant => (
                ApiKey::Cookie,
                "palmr_share_{sharePublicId}",
                "Grant for exactly one Share, issued by that Share's authorize operation. \
                 {sharePublicId} stands for the Share's public identifier: the browser sends one \
                 cookie per granted Share and the server reads only the one for the addressed alias.",
            ),
            Self::ReverseShareGrant => (
                ApiKey::Cookie,
                "palmr_rs_{reverseSharePublicId}",
                "Grant for exactly one Reverse Share, issued by that Reverse Share's authorize \
                 operation. {reverseSharePublicId} stands for the Reverse Share's public identifier.",
            ),
        };
        SecurityScheme::ApiKey(location(ApiKeyValue::with_description(name, description)))
    }
}

// The auth-class tag and the session requirements are written here, from the
// policy the route was registered with, so a handler annotation can never
// disagree with what the router enforces.
pub(super) fn declare_route_policy(
    operation: &mut Operation,
    auth: AuthClass,
    method: &Method,
) -> Result<(), AuthClassTagConflict> {
    if !declared_auth_classes(operation).is_empty() {
        return Err(AuthClassTagConflict);
    }
    operation
        .tags
        .get_or_insert_with(Vec::new)
        .push(auth.as_str().to_owned());
    if let Some(requirement) = session_requirement(auth, method) {
        operation.security = Some(vec![requirement]);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthClassTagConflict;

pub fn declared_auth_classes(operation: &Operation) -> Vec<AuthClass> {
    operation
        .tags
        .iter()
        .flatten()
        .filter_map(|tag| {
            AuthClass::ALL
                .into_iter()
                .find(|class| class.as_str() == tag)
        })
        .collect()
}

fn session_requirement(auth: AuthClass, method: &Method) -> Option<SecurityRequirement> {
    match auth {
        AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup => None,
        AuthClass::Authenticated
        | AuthClass::AuthenticatedRecentAuth
        | AuthClass::Admin
        | AuthClass::AdminRecentAuth => {
            let session = SecurityRequirement::new(Credential::Session.scheme_name(), NO_SCOPES);
            Some(if is_state_changing(method) {
                session
                    .add(Credential::CsrfCookie.scheme_name(), NO_SCOPES)
                    .add(Credential::CsrfHeader.scheme_name(), NO_SCOPES)
            } else {
                session
            })
        }
    }
}

const NO_SCOPES: [&str; 0] = [];

fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

pub fn operations(item: &PathItem) -> impl Iterator<Item = (Method, &Operation)> {
    [
        (Method::GET, &item.get),
        (Method::HEAD, &item.head),
        (Method::POST, &item.post),
        (Method::PUT, &item.put),
        (Method::PATCH, &item.patch),
        (Method::DELETE, &item.delete),
        (Method::OPTIONS, &item.options),
        (Method::TRACE, &item.trace),
    ]
    .into_iter()
    .filter_map(|(method, operation)| operation.as_ref().map(|operation| (method, operation)))
}

pub(super) fn operations_mut(
    item: &mut PathItem,
) -> impl Iterator<Item = (Method, &mut Operation)> {
    [
        (Method::GET, &mut item.get),
        (Method::HEAD, &mut item.head),
        (Method::POST, &mut item.post),
        (Method::PUT, &mut item.put),
        (Method::PATCH, &mut item.patch),
        (Method::DELETE, &mut item.delete),
        (Method::OPTIONS, &mut item.options),
        (Method::TRACE, &mut item.trace),
    ]
    .into_iter()
    .filter_map(|(method, operation)| operation.as_mut().map(|operation| (method, operation)))
}

pub fn describe(mut openapi: OpenApi) -> OpenApi {
    openapi.openapi = OpenApiVersion::Version31;
    let mut info = Info::new(API_TITLE, VERSION);
    info.license = Some(
        LicenseBuilder::new()
            .name(API_LICENSE)
            .identifier(Some(API_LICENSE))
            .build(),
    );
    openapi.info = info;
    openapi.servers = None;

    let components = openapi.components.get_or_insert_with(Components::new);
    for (name, schema) in envelope_schemas() {
        components.schemas.insert(name, schema);
    }
    for credential in Credential::ALL {
        components
            .security_schemes
            .insert(credential.scheme_name().to_owned(), credential.scheme());
    }
    openapi
}

fn envelope_schemas() -> Vec<(String, RefOr<Schema>)> {
    let mut schemas = vec![(ApiErrorBody::name().into_owned(), ApiErrorBody::schema())];
    ApiErrorBody::schemas(&mut schemas);
    schemas
}

pub fn render_document(openapi: OpenApi) -> Result<Vec<u8>, ApiDocsError> {
    serde_json::to_vec(&describe(openapi)).map_err(ApiDocsError::Serialize)
}

pub fn export_document() -> Result<Vec<u8>, ExportError> {
    let assembled = application_routes().build().map_err(ExportError::Router)?;
    render_document(assembled.openapi).map_err(ExportError::Docs)
}

#[derive(Debug)]
pub enum ExportError {
    Router(RouteBuildError),
    Docs(ApiDocsError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Router(error) => error.fmt(f),
            Self::Docs(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Router(error) => Some(error),
            Self::Docs(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub enum ApiDocsError {
    Serialize(serde_json::Error),
}

impl fmt::Display for ApiDocsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => {
                write!(f, "the OpenAPI document could not be serialized: {error}")
            }
        }
    }
}

impl std::error::Error for ApiDocsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApiDocs(Arc<Rendered>);

#[derive(Debug)]
struct Rendered {
    document: Bytes,
    etag: HeaderValue,
    page: DocsPage,
}

impl ApiDocs {
    pub fn new(openapi: OpenApi, base_url: &PublicBaseUrl) -> Result<Self, ApiDocsError> {
        let document = render_document(openapi)?;
        let etag = weak_etag(&document);
        Ok(Self(Arc::new(Rendered {
            document: Bytes::from(document),
            etag,
            page: DocsPage::new(base_url),
        })))
    }

    pub fn document(&self) -> &Bytes {
        &self.0.document
    }

    pub fn etag(&self) -> &HeaderValue {
        &self.0.etag
    }

    pub fn render_page(&self, nonce: &CspNonce) -> String {
        self.0.page.render(nonce)
    }
}

#[derive(Debug)]
struct DocsPage {
    segments: Vec<String>,
}

// Each dynamic value is concatenated exactly once, after every template
// substitution, so a base path can never be reinterpreted as a placeholder.
impl DocsPage {
    fn new(base_url: &PublicBaseUrl) -> Self {
        let base = base_path(base_url);
        let runtime_src = escape_attribute(&format!("{base}{}", &SCALAR_RUNTIME_PATH[1..]));
        let init = Scalar::new(scalar_configuration(&base))
            .custom_html(PAGE_INIT)
            .to_html();
        Self {
            segments: vec![
                PAGE_HEAD.to_owned(),
                format!("{PAGE_RUNTIME_OPEN}{runtime_src}{PAGE_RUNTIME_CLOSE}"),
                init,
            ],
        }
    }

    fn render(&self, nonce: &CspNonce) -> String {
        self.segments.join(nonce.as_str())
    }
}

fn base_path(base_url: &PublicBaseUrl) -> String {
    let path = base_url.url().path();
    if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    }
}

// Scalar's API client joins this server with each operation path, so a sub-path
// deployment must name its prefix or "Test request" would leave the instance.
fn scalar_configuration(base: &str) -> Value {
    let mut config = json!({
        "url": format!("{base}{}", &OPENAPI_PATH[1..]),
        "withDefaultFonts": false,
        "telemetry": false,
        "showDeveloperTools": "never",
        "agent": { "disabled": true },
        "mcp": { "disabled": true },
    });
    let prefix = base.trim_end_matches('/');
    if !prefix.is_empty() {
        config["servers"] = json!([{ "url": prefix }]);
    }
    config
}

fn escape_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(DOCS_ROUTE, routes!(openapi_document))
        .route(DOCS_ROUTE, routes!(docs))
        .route(DOCS_ROUTE, routes!(scalar_runtime))
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    tag = "documentation",
    responses(
        (status = 200, description = "The OpenAPI 3.1 description of this instance's HTTP API.", content_type = "application/json"),
        (status = 304, description = "The cached document is still current."),
    )
)]
async fn openapi_document(State(state): State<AppState>, request: Request) -> Response {
    let docs = state.api_docs();
    let etag = docs.etag().clone();
    if matches_validator(request.headers(), &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(CACHE_CONTROL, OPENAPI_CACHE_CONTROL), (ETAG, etag)],
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE)),
            (CACHE_CONTROL, OPENAPI_CACHE_CONTROL),
            (ETAG, etag),
        ],
        docs.document().clone(),
    )
        .into_response()
}

// The page carries this response's CSP nonce, so it has no validator: a 304
// would revive a cached page whose nonce no longer matches the policy header.
#[utoipa::path(
    get,
    path = "/docs",
    tag = "documentation",
    responses(
        (status = 200, description = "The Scalar API reference for this instance.", content_type = "text/html"),
        (status = 500, description = "The page could not be rendered.", body = ApiErrorBody),
    )
)]
async fn docs(State(state): State<AppState>, request: Request) -> Response {
    let Some(nonce) = request.extensions().get::<CspNonce>() else {
        tracing::error!("the API reference was requested without a CSP nonce");
        return tag_error(ApiError::internal(), RequestId::of(&request).as_ref()).into_response();
    };
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, HTML_CONTENT_TYPE),
            (CACHE_CONTROL, REVALIDATE),
        ],
        state.api_docs().render_page(nonce),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/docs/scalar.js",
    tag = "documentation",
    responses(
        (status = 200, description = "The Scalar API reference runtime served by this instance.", content_type = "text/javascript"),
        (status = 304, description = "The cached runtime is still current."),
    )
)]
async fn scalar_runtime(request: Request) -> Response {
    let etag = weak_etag_from_sha256(SCALAR_RUNTIME_SHA256);
    if matches_validator(request.headers(), &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(CACHE_CONTROL, REVALIDATE), (ETAG, etag)],
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, JAVASCRIPT_CONTENT_TYPE),
            (CACHE_CONTROL, REVALIDATE),
            (ETAG, etag),
        ],
        Bytes::from_static(SCALAR_RUNTIME),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
