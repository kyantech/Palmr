use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::header::{HeaderMap, HeaderName, HeaderValue};

use super::error::ApiError;
use super::proxy::{socket_peer, TrustedProxies};
use crate::domain::clock::Clock;
use crate::domain::id::Id;

pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

const MAX_LEN: usize = 128;

enum HttpRequest {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(String);

impl RequestId {
    fn generate(clock: &dyn Clock) -> Self {
        Self(Id::<HttpRequest>::generate(clock).to_string())
    }

    fn accept(value: &HeaderValue) -> Option<Self> {
        let bytes = value.as_bytes();
        let valid = (1..=MAX_LEN).contains(&bytes.len()) && bytes.iter().copied().all(is_id_byte);
        valid
            .then(|| value.to_str().ok())
            .flatten()
            .map(|text| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn header_value(&self) -> Option<HeaderValue> {
        HeaderValue::from_str(&self.0).ok()
    }

    pub fn of(request: &Request) -> Option<Self> {
        request.extensions().get::<Self>().cloned()
    }
}

const fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'.' | b'_' | b'@' | b':' | b'/' | b'+' | b'=' | b'-')
}

#[derive(Clone)]
pub struct RequestIdSource {
    proxies: Arc<TrustedProxies>,
    clock: Arc<dyn Clock>,
}

impl RequestIdSource {
    pub fn new(proxies: Arc<TrustedProxies>, clock: Arc<dyn Clock>) -> Self {
        Self { proxies, clock }
    }

    fn select(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> RequestId {
        peer.filter(|peer| self.proxies.is_trusted(*peer))
            .and_then(|_| single_inbound(headers))
            .and_then(RequestId::accept)
            .unwrap_or_else(|| RequestId::generate(self.clock.as_ref()))
    }
}

fn single_inbound(headers: &HeaderMap) -> Option<&HeaderValue> {
    let mut values = headers.get_all(X_REQUEST_ID).iter();
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

pub async fn assign_request_id(
    State(source): State<RequestIdSource>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = source.select(socket_peer(request.extensions()), request.headers());
    let header = request_id.header_value();
    match &header {
        Some(value) => request.headers_mut().insert(X_REQUEST_ID, value.clone()),
        None => request.headers_mut().remove(X_REQUEST_ID),
    };
    request.extensions_mut().insert(request_id);

    let mut response = next.run(request).await;
    if let Some(value) = header {
        response.headers_mut().insert(X_REQUEST_ID, value);
    }
    response
}

pub fn tag_error(error: ApiError, request_id: Option<&RequestId>) -> ApiError {
    match request_id {
        Some(request_id) => error.with_request_id(request_id.as_str()),
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::sync::Arc;

    use http::header::{HeaderMap, HeaderValue};
    use rstest::rstest;
    use time::macros::datetime;
    use uuid::Uuid;

    use super::{RequestId, RequestIdSource, X_REQUEST_ID};
    use crate::config::TrustProxy;
    use crate::domain::clock::TestClock;
    use crate::infra::http::proxy::TrustedProxies;

    fn source(trust: TrustProxy) -> RequestIdSource {
        RequestIdSource::new(
            Arc::new(TrustedProxies::new(&trust)),
            Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC))),
        )
    }

    fn trusting_ten_slash_eight() -> RequestIdSource {
        source(TrustProxy::AllowList(vec!["10.0.0.0/8".parse().unwrap()]))
    }

    fn inbound(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(X_REQUEST_ID, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn peer(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    fn assert_generated_v7(request_id: &RequestId) {
        let uuid = Uuid::parse_str(request_id.as_str()).unwrap();
        assert_eq!(uuid.get_version_num(), 7);
        assert_eq!(uuid.hyphenated().to_string(), request_id.as_str());
    }

    #[rstest]
    #[case::uuid("0192f3a7-5c4b-7e21-9a02-3f8c1d6e4b90")]
    #[case::every_allowed_symbol("Az09._@:/+=-")]
    #[case::single_char("x")]
    #[case::max_length(&"a".repeat(128))]
    fn unit_request_id_accepts_valid_syntax(#[case] value: &str) {
        let accepted = RequestId::accept(&HeaderValue::from_str(value).unwrap()).unwrap();
        assert_eq!(accepted.as_str(), value);
    }

    #[rstest]
    #[case::empty("")]
    #[case::too_long(&"a".repeat(129))]
    #[case::space("edge id")]
    #[case::tab("edge\tid")]
    #[case::semicolon("edge;id")]
    #[case::comma("edge,id")]
    #[case::quote("edge\"id")]
    #[case::brace("{edge}")]
    #[case::percent("edge%0aid")]
    fn unit_request_id_rejects_invalid_syntax(#[case] value: &str) {
        assert_eq!(
            RequestId::accept(&HeaderValue::from_str(value).unwrap()),
            None
        );
    }

    #[test]
    fn unit_request_id_rejects_non_ascii_bytes() {
        let value = HeaderValue::from_bytes(b"edge\xffid").unwrap();
        assert_eq!(RequestId::accept(&value), None);
    }

    #[test]
    fn unit_request_id_trusted_peer_valid_value_kept() {
        let selected = trusting_ten_slash_eight().select(peer("10.1.2.3"), &inbound(&["edge-7"]));
        assert_eq!(selected.as_str(), "edge-7");
    }

    #[test]
    fn unit_request_id_ipv4_mapped_trusted_peer_kept() {
        let selected =
            trusting_ten_slash_eight().select(peer("::ffff:10.1.2.3"), &inbound(&["edge-7"]));
        assert_eq!(selected.as_str(), "edge-7");
    }

    #[rstest]
    #[case::untrusted_peer(peer("203.0.113.9"), &["edge-7"])]
    #[case::unknown_peer(None, &["edge-7"])]
    #[case::trusted_peer_without_header(peer("10.1.2.3"), &[])]
    #[case::trusted_peer_invalid_value(peer("10.1.2.3"), &["edge 7"])]
    #[case::trusted_peer_duplicate_values(peer("10.1.2.3"), &["edge-7", "edge-8"])]
    fn unit_request_id_generated_otherwise(#[case] peer: Option<IpAddr>, #[case] values: &[&str]) {
        let selected = trusting_ten_slash_eight().select(peer, &inbound(values));
        assert_generated_v7(&selected);
    }

    #[test]
    fn unit_request_id_trust_off_never_accepts() {
        for peer_ip in ["127.0.0.1", "::1", "10.1.2.3", "172.17.0.1"] {
            let selected = source(TrustProxy::Off).select(peer(peer_ip), &inbound(&["edge-7"]));
            assert_generated_v7(&selected);
        }
    }

    #[test]
    fn unit_request_id_generated_values_are_unique() {
        let source = source(TrustProxy::Off);
        let first = source.select(None, &HeaderMap::new());
        let second = source.select(None, &HeaderMap::new());
        assert_ne!(first, second);
    }
}
