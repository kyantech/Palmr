use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::header::{
    HeaderName, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN, TRANSFER_ENCODING,
};
use http::{HeaderMap, Method};
use url::Url;

use super::cookies::{self, CookiePolicy, CSRF_COOKIE};
use super::error::ApiError;
use super::request_id::{tag_error, RequestId};
use crate::app::auth_class::AuthClass;
use crate::config::PublicBaseUrl;
use crate::domain::error_code::ErrorCode;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;

pub const CSRF_HEADER: HeaderName = HeaderName::from_static("x-palmr-csrf");

const FORM_URLENCODED: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";
const MULTIPART: &str = "multipart/form-data";
const OFFSET_OCTET_STREAM: &str = "application/offset+octet-stream";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestContent {
    Json,
    Multipart,
    OffsetOctetStream,
    JsonOrOffsetOctetStream,
}

impl RequestContent {
    fn accepts(self, media: &MediaType<'_>) -> bool {
        match self {
            Self::Json => media.is_json(),
            Self::Multipart => media.essence_is(MULTIPART),
            Self::OffsetOctetStream => media.is_bare(OFFSET_OCTET_STREAM),
            Self::JsonOrOffsetOctetStream => media.is_json() || media.is_bare(OFFSET_OCTET_STREAM),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfAuthority {
    None,
    Cookie,
}

impl CsrfAuthority {
    pub const fn of(class: AuthClass) -> Self {
        match class {
            AuthClass::Public | AuthClass::Setup => Self::None,
            AuthClass::PublicGrant
            | AuthClass::Authenticated
            | AuthClass::AuthenticatedRecentAuth
            | AuthClass::Admin
            | AuthClass::AdminRecentAuth => Self::Cookie,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnonymousCsrf {
    None,
    Issue,
}

impl AnonymousCsrf {
    pub const fn permitted_for(self, class: AuthClass) -> bool {
        match self {
            Self::None => true,
            Self::Issue => matches!(
                class,
                AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestGate {
    authority: CsrfAuthority,
    content: RequestContent,
    anonymous_csrf: AnonymousCsrf,
}

impl RequestGate {
    pub const fn new(
        class: AuthClass,
        content: RequestContent,
        anonymous_csrf: AnonymousCsrf,
    ) -> Self {
        Self {
            authority: CsrfAuthority::of(class),
            content,
            anonymous_csrf,
        }
    }

    pub const fn authority(&self) -> CsrfAuthority {
        self.authority
    }
}

#[derive(Debug, Clone)]
pub struct CsrfGuard {
    origin: url::Origin,
    cookies: CookiePolicy,
}

impl CsrfGuard {
    pub fn new(base_url: &PublicBaseUrl) -> Self {
        Self {
            origin: base_url.url().origin(),
            cookies: CookiePolicy::from_base_url(base_url),
        }
    }

    fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(ORIGIN).iter();
        let Some(value) = values.next() else {
            return true;
        };
        if values.next().is_some() {
            return false;
        }
        value
            .to_str()
            .ok()
            .and_then(|text| Url::parse(text).ok())
            .filter(is_serialized_origin)
            .is_some_and(|url| url.origin() == self.origin)
    }

    fn issue_anonymous(&self, headers: &mut HeaderMap) -> Result<(), ApiError> {
        let token = Token::mint().map_err(|_| ApiError::internal())?;
        self.cookies
            .append_anonymous_csrf(headers, &token.encode())
            .map_err(|_| ApiError::internal())
    }
}

fn is_serialized_origin(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

#[derive(Debug, Clone)]
pub struct CsrfProof(TokenDigest);

impl CsrfProof {
    pub const fn digest(&self) -> &TokenDigest {
        &self.0
    }
}

pub fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

pub fn apply<S>(gate: RequestGate, handler: MethodRouter<S>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    handler.route_layer(from_fn_with_state(gate, enforce_request_gate))
}

async fn enforce_request_gate(
    State(gate): State<RequestGate>,
    mut request: Request,
    next: Next,
) -> Response {
    let guard = request.extensions().get::<Arc<CsrfGuard>>().cloned();
    if is_state_changing(request.method()) {
        let request_id = RequestId::of(&request);
        let checked = match &guard {
            Some(guard) => check_state_change(guard, gate, request.headers()),
            None => {
                tracing::error!("request gate is not configured; refusing the state change");
                Err(ApiError::internal())
            }
        };
        match checked {
            Ok(Some(proof)) => {
                request.extensions_mut().insert(proof);
            }
            Ok(None) => {}
            Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
        }
    }

    let issue =
        gate.anonymous_csrf == AnonymousCsrf::Issue && !carries_usable_csrf(request.headers());
    let mut response = next.run(request).await;
    if let (true, Some(guard)) = (issue, guard) {
        if !cookies::sets(response.headers(), CSRF_COOKIE) {
            if let Err(_error) = guard.issue_anonymous(response.headers_mut()) {
                tracing::error!("anonymous CSRF cookie could not be issued");
            }
        }
    }
    response
}

fn check_state_change(
    guard: &CsrfGuard,
    gate: RequestGate,
    headers: &HeaderMap,
) -> Result<Option<CsrfProof>, ApiError> {
    check_content_type(gate.content, headers)?;
    if !guard.origin_allowed(headers) {
        return Err(ApiError::new(ErrorCode::OriginNotAllowed));
    }
    match gate.authority {
        CsrfAuthority::None => Ok(None),
        CsrfAuthority::Cookie => double_submit(headers).map(Some),
    }
}

fn check_content_type(content: RequestContent, headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return if declares_body(headers) {
            Err(unsupported())
        } else {
            Ok(())
        };
    };
    if values.next().is_some() {
        return Err(unsupported());
    }
    let media = value
        .to_str()
        .ok()
        .and_then(MediaType::parse)
        .ok_or_else(unsupported)?;
    if media.essence_is(FORM_URLENCODED) || !content.accepts(&media) {
        return Err(unsupported());
    }
    Ok(())
}

fn unsupported() -> ApiError {
    ApiError::new(ErrorCode::UnsupportedMediaType)
}

fn declares_body(headers: &HeaderMap) -> bool {
    let declared_length = headers
        .get(CONTENT_LENGTH)
        .is_some_and(|value| value.as_bytes() != b"0");
    declared_length
        || headers.contains_key(TRANSFER_ENCODING)
        || headers.contains_key(CONTENT_ENCODING)
}

fn double_submit(headers: &HeaderMap) -> Result<CsrfProof, ApiError> {
    let cookie = cookies::read(headers, CSRF_COOKIE);
    let mut header_values = headers.get_all(CSRF_HEADER).iter();
    let header = header_values.next();
    let repeated_header = header_values.next().is_some();

    let cookie = match cookie {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => return Err(ApiError::new(ErrorCode::CsrfTokenMissing)),
        Err(_) => return Err(ApiError::new(ErrorCode::CsrfTokenInvalid)),
    };
    let header = match header {
        Some(value) if !value.is_empty() => value,
        _ => return Err(ApiError::new(ErrorCode::CsrfTokenMissing)),
    };
    if repeated_header {
        return Err(ApiError::new(ErrorCode::CsrfTokenInvalid));
    }

    let invalid = || ApiError::new(ErrorCode::CsrfTokenInvalid);
    let from_cookie = Token::decode(&cookie).map_err(|_| invalid())?.digest();
    let from_header = header
        .to_str()
        .ok()
        .and_then(|text| Token::decode(text).ok())
        .ok_or_else(invalid)?
        .digest();
    if from_cookie.verify(&from_header) {
        Ok(CsrfProof(from_cookie))
    } else {
        Err(invalid())
    }
}

fn carries_usable_csrf(headers: &HeaderMap) -> bool {
    matches!(
        cookies::read(headers, CSRF_COOKIE),
        Ok(Some(value)) if Token::decode(&value).is_ok()
    )
}

struct MediaType<'a> {
    essence: &'a str,
    parameters: Vec<(&'a str, &'a str)>,
}

impl<'a> MediaType<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        let mut parts = value.split(';');
        let essence = parts.next()?.trim();
        let (kind, subtype) = essence.split_once('/')?;
        if !is_token(kind) || !is_token(subtype) {
            return None;
        }
        let mut parameters = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, raw) = part.split_once('=')?;
            let name = name.trim();
            let raw = raw.trim();
            let unquoted = raw
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or(raw);
            if !is_token(name) {
                return None;
            }
            parameters.push((name, unquoted));
        }
        Some(Self {
            essence,
            parameters,
        })
    }

    fn essence_is(&self, expected: &str) -> bool {
        self.essence.eq_ignore_ascii_case(expected)
    }

    fn is_bare(&self, expected: &str) -> bool {
        self.essence_is(expected) && self.parameters.is_empty()
    }

    fn is_json(&self) -> bool {
        self.essence_is(JSON)
            && self.parameters.iter().all(|(name, value)| {
                name.eq_ignore_ascii_case("charset") && value.eq_ignore_ascii_case("utf-8")
            })
    }
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

#[cfg(test)]
pub(crate) fn with_test_csrf(builder: http::request::Builder) -> http::request::Builder {
    let token = Token::mint().unwrap().encode();
    let token = token.expose_secret();
    builder
        .header(http::header::COOKIE, format!("{CSRF_COOKIE}={token}"))
        .header(CSRF_HEADER, token.as_str())
}

#[cfg(test)]
mod tests;
