use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{ACCEPT, CONTENT_LENGTH, LOCATION, USER_AGENT};
use http::{HeaderValue, Method, Request, StatusCode};
use http_body_util::{BodyExt, Empty, LengthLimitError, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rustls::RootCertStore;
use tower::Service;
use url::{Host, Url};

use super::model::URL_MAX_CHARS;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const RESPONSE_LIMIT_BYTES: usize = 256 * 1024;
pub const MAX_REDIRECTS: usize = 2;

const AGENT: HeaderValue = HeaderValue::from_static("Palmr");
const JSON: HeaderValue = HeaderValue::from_static("application/json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFailure {
    InvalidUrl,
    Blocked,
    Unreachable,
    Timeout,
    Tls,
    Status(u16),
    TooLarge,
    TooManyRedirects,
}

impl FetchFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::Blocked => "blocked_address",
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::Tls => "tls_error",
            Self::Status(500..=599) => "upstream_error",
            Self::Status(_) => "unexpected_status",
            Self::TooLarge => "response_too_large",
            Self::TooManyRedirects => "too_many_redirects",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub status: StatusCode,
    pub body: Bytes,
}

type OpenClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;
type GuardedClient = Client<HttpsConnector<HttpConnector<PublicOnlyResolver>>, Empty<Bytes>>;

struct Inner {
    open: OpenClient,
    guarded: GuardedClient,
    tls: Arc<rustls::ClientConfig>,
    root_count: usize,
}

#[derive(Clone)]
pub struct ProviderHttpClient {
    inner: Option<Arc<Inner>>,
    timeout: Duration,
}

impl ProviderHttpClient {
    pub fn new() -> Self {
        Self {
            inner: build_inner().map(Arc::new),
            timeout: REQUEST_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new()
        }
    }

    pub fn tls_config(&self) -> Option<&Arc<rustls::ClientConfig>> {
        self.inner.as_ref().map(|inner| &inner.tls)
    }

    pub fn root_count(&self) -> Option<usize> {
        self.inner.as_ref().map(|inner| inner.root_count)
    }

    pub async fn document(&self, url: &str) -> Result<Document, FetchFailure> {
        let inner = self.inner.as_ref().ok_or(FetchFailure::Tls)?;
        let exchange = inner.follow(url);
        tokio::time::timeout(self.timeout, exchange)
            .await
            .unwrap_or(Err(FetchFailure::Timeout))
    }

    pub async fn probe(&self, url: &str) -> Result<StatusCode, FetchFailure> {
        let inner = self.inner.as_ref().ok_or(FetchFailure::Tls)?;
        let exchange = inner.probe(url);
        tokio::time::timeout(self.timeout, exchange)
            .await
            .unwrap_or(Err(FetchFailure::Timeout))
    }
}

impl Default for ProviderHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProviderHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderHttpClient")
            .field("root_count", &self.root_count())
            .finish()
    }
}

fn build_inner() -> Option<Inner> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let root_count = roots.len();
    let tls = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .ok()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );

    let mut open_http = HttpConnector::new();
    open_http.enforce_http(false);
    open_http.set_connect_timeout(Some(CONNECT_TIMEOUT));
    let mut guarded_http = HttpConnector::new_with_resolver(PublicOnlyResolver);
    guarded_http.enforce_http(false);
    guarded_http.set_connect_timeout(Some(CONNECT_TIMEOUT));

    let open = Client::builder(TokioExecutor::new()).build(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*tls).clone())
            .https_or_http()
            .enable_http1()
            .wrap_connector(open_http),
    );
    let guarded = Client::builder(TokioExecutor::new()).build(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*tls).clone())
            .https_or_http()
            .enable_http1()
            .wrap_connector(guarded_http),
    );
    Some(Inner {
        open,
        guarded,
        tls,
        root_count,
    })
}

impl Inner {
    async fn follow(&self, url: &str) -> Result<Document, FetchFailure> {
        let mut current = acceptable_url(url).ok_or(FetchFailure::InvalidUrl)?;
        let origin = current.origin();
        for _ in 0..=MAX_REDIRECTS {
            let guarded = current.origin() != origin;
            if guarded && !publicly_routable_host(&current) {
                return Err(FetchFailure::Blocked);
            }
            let response = self.send(&current, guarded).await?;
            let status = response.status();
            if is_redirect(status) {
                let target = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|location| current.join(location).ok())
                    .ok_or(FetchFailure::InvalidUrl)?;
                if !url_is_acceptable(&target) {
                    return Err(FetchFailure::InvalidUrl);
                }
                current = target;
                continue;
            }
            if !status.is_success() {
                return Err(FetchFailure::Status(status.as_u16()));
            }
            let declared = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok());
            if declared.is_some_and(|length| length > RESPONSE_LIMIT_BYTES) {
                return Err(FetchFailure::TooLarge);
            }
            let body = Limited::new(response.into_body(), RESPONSE_LIMIT_BYTES)
                .collect()
                .await
                .map_err(|error| {
                    if error.downcast_ref::<LengthLimitError>().is_some() {
                        FetchFailure::TooLarge
                    } else {
                        FetchFailure::Unreachable
                    }
                })?
                .to_bytes();
            return Ok(Document { status, body });
        }
        Err(FetchFailure::TooManyRedirects)
    }

    async fn probe(&self, url: &str) -> Result<StatusCode, FetchFailure> {
        let url = acceptable_url(url).ok_or(FetchFailure::InvalidUrl)?;
        let response = self.send(&url, false).await?;
        Ok(response.status())
    }

    async fn send(
        &self,
        url: &Url,
        guarded: bool,
    ) -> Result<http::Response<hyper::body::Incoming>, FetchFailure> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(url.as_str())
            .header(ACCEPT, JSON)
            .header(USER_AGENT, AGENT)
            .body(Empty::<Bytes>::new())
            .map_err(|_| FetchFailure::InvalidUrl)?;
        let result = if guarded {
            self.guarded.request(request).await
        } else {
            self.open.request(request).await
        };
        result.map_err(|error| classify(&error))
    }
}

const fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

fn classify(error: &(dyn std::error::Error + 'static)) -> FetchFailure {
    if chain_contains::<BlockedAddress>(error) {
        FetchFailure::Blocked
    } else if chain_contains::<rustls::Error>(error) {
        FetchFailure::Tls
    } else {
        FetchFailure::Unreachable
    }
}

fn chain_contains<T: std::error::Error + 'static>(
    error: &(dyn std::error::Error + 'static),
) -> bool {
    let mut current = Some(error);
    while let Some(node) = current {
        if node.is::<T>() {
            return true;
        }
        if let Some(inner) = node
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
        {
            if chain_contains::<T>(inner) {
                return true;
            }
        }
        current = node.source();
    }
    false
}

#[derive(Debug)]
struct BlockedAddress;

impl std::fmt::Display for BlockedAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the resolved address is not publicly routable")
    }
}

impl std::error::Error for BlockedAddress {}

#[derive(Clone, Copy)]
pub struct PublicOnlyResolver;

impl Service<Name> for PublicOnlyResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        Box::pin(async move {
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| is_denied_address(address.ip()))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    BlockedAddress,
                ));
            }
            Ok(addresses.into_iter())
        })
    }
}

pub fn acceptable_url(text: &str) -> Option<Url> {
    if text.is_empty() || text.len() > URL_MAX_CHARS || text.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(text).ok()?;
    url_is_acceptable(&url).then_some(url)
}

fn url_is_acceptable(url: &Url) -> bool {
    let scheme_ok = match url.scheme() {
        "https" => true,
        "http" => is_loopback_host(url),
        _ => false,
    };
    scheme_ok
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn publicly_routable_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => !is_denied_address(IpAddr::V4(address)),
        Some(Host::Ipv6(address)) => !is_denied_address(IpAddr::V6(address)),
        Some(Host::Domain(domain)) => !domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

pub fn is_denied_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => denied_v4(v4),
        IpAddr::V6(v6) => denied_v6(v6),
    }
}

fn denied_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    matches!(
        (a, b, c),
        (0 | 10 | 127, _, _)
            | (100, 64..=127, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 168, _)
            | (192, 0, 0 | 2)
            | (198, 18 | 19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

fn denied_v6(address: Ipv6Addr) -> bool {
    if let Some(embedded) = address.to_ipv4_mapped() {
        return denied_v4(embedded);
    }
    let segments = address.segments();
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let [high, low] = [segments[6].to_be_bytes(), segments[7].to_be_bytes()];
        return denied_v4(Ipv4Addr::new(high[0], high[1], low[0], low[1]));
    }
    address.is_unspecified()
        || address.is_loopback()
        || segments[0] & 0xfe00 == 0xfc00
        || segments[0] & 0xffc0 == 0xfe80
        || segments[0] & 0xff00 == 0xff00
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn unit_provider_url_policy_is_https_or_loopback_http() {
        for accepted in [
            "https://sso.example.com/application/o/palmr/",
            "https://accounts.google.com",
            "https://10.0.0.5:8443/realms/main",
            "http://localhost:8080/realms/main",
            "http://127.0.0.1:9000/",
            "http://[::1]:9000/",
        ] {
            assert!(acceptable_url(accepted).is_some(), "{accepted}");
        }
        let long = format!("https://example.com/{}", "a".repeat(600));
        for rejected in [
            "",
            "http://sso.example.com/",
            "http://192.168.1.10/",
            "ftp://sso.example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pw@sso.example.com/",
            "https://sso.example.com/#fragment",
            "https://sso.example.com/\nHost: evil",
            "not a url",
            long.as_str(),
        ] {
            assert!(acceptable_url(rejected).is_none(), "{rejected:?}");
        }
    }

    #[test]
    fn unit_provider_deny_list_covers_the_documented_ranges() {
        let denied_v4 = [
            "0.0.0.0",
            "0.255.255.255",
            "10.1.2.3",
            "127.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.10",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ];
        for text in denied_v4 {
            assert!(is_denied_address(text.parse().unwrap()), "{text}");
        }
        for text in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "172.32.0.1",
            "193.0.0.1",
            "198.20.0.1",
        ] {
            assert!(!is_denied_address(text.parse().unwrap()), "{text}");
        }
        for text in [
            "::",
            "::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "64:ff9b::7f00:1",
        ] {
            assert!(is_denied_address(text.parse().unwrap()), "{text}");
        }
        for text in ["2606:4700:4700::1111", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
            assert!(!is_denied_address(text.parse().unwrap()), "{text}");
        }
        assert!(is_denied_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_denied_address(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn unit_provider_redirect_hosts_are_judged_before_dialling() {
        for denied in [
            "http://127.0.0.1/",
            "http://localhost/",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/",
            "https://[fd00:ec2::254]/",
        ] {
            let url = Url::parse(denied).unwrap();
            assert!(!publicly_routable_host(&url), "{denied}");
        }
        assert!(publicly_routable_host(
            &Url::parse("https://8.8.8.8/").unwrap()
        ));
        assert!(publicly_routable_host(
            &Url::parse("https://sso.example.com/").unwrap()
        ));
    }

    #[test]
    fn unit_provider_http_client_owns_its_tls_configuration() {
        let default_before = rustls::crypto::CryptoProvider::get_default()
            .map(|provider| Arc::as_ptr(provider) as usize);
        let first = ProviderHttpClient::new();
        let second = ProviderHttpClient::new();
        let first_config = first.tls_config().unwrap();
        let second_config = second.tls_config().unwrap();
        assert!(!Arc::ptr_eq(first_config, second_config));
        assert_eq!(
            first.root_count(),
            Some(webpki_roots::TLS_SERVER_ROOTS.len())
        );
        let default_after = rustls::crypto::CryptoProvider::get_default()
            .map(|provider| Arc::as_ptr(provider) as usize);
        assert_eq!(default_before, default_after);
    }

    #[test]
    fn unit_provider_fetch_failure_codes_are_stable() {
        assert_eq!(FetchFailure::Status(503).code(), "upstream_error");
        assert_eq!(FetchFailure::Status(404).code(), "unexpected_status");
        assert_eq!(FetchFailure::TooLarge.code(), "response_too_large");
        assert_eq!(FetchFailure::Blocked.code(), "blocked_address");
    }
}
