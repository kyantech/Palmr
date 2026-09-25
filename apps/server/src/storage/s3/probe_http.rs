use std::io;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::header::{ACCESS_CONTROL_REQUEST_METHOD, CONTENT_LENGTH, ORIGIN};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Limited, StreamBody};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio_util::io::ReaderStream;
use url::Origin;

use super::client::S3Clients;
use super::presign::signed_origin;
use crate::storage::provider::{ObjectBody, PresignedRequest};

const RESPONSE_LIMIT: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const BODY_CHUNK: usize = 64 * 1024;

type ProbeBody = UnsyncBoxBody<Bytes, io::Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeFailure {
    Refused,
    Unreachable,
}

#[derive(Debug, Clone)]
pub(super) struct ProbeResponse {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Bytes,
}

pub(super) struct PublicProbe {
    client: Client<HttpsConnector<HttpConnector>, ProbeBody>,
    origin: Origin,
}

impl PublicProbe {
    pub(super) fn new(clients: &S3Clients) -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(CONNECT_TIMEOUT));
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((**clients.public_tls().config()).clone())
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        Self {
            client: Client::builder(TokioExecutor::new()).build(https),
            origin: signed_origin(clients),
        }
    }

    pub(super) async fn execute(
        &self,
        signed: &PresignedRequest,
        browser_origin: &str,
        body: Option<(ObjectBody, u64)>,
    ) -> Result<ProbeResponse, ProbeFailure> {
        let builder = self
            .request(signed, signed.method().clone())?
            .header(ORIGIN, browser_value(browser_origin)?);
        let request = match body {
            Some((body, len)) => {
                let frames = ReaderStream::with_capacity(body, BODY_CHUNK)
                    .map(|chunk| chunk.map(Frame::data));
                builder
                    .header(CONTENT_LENGTH, len)
                    .body(BodyExt::boxed_unsync(StreamBody::new(frames)))
            }
            None => builder.body(empty()),
        };
        self.send(request.map_err(|_| ProbeFailure::Refused)?).await
    }

    pub(super) async fn preflight(
        &self,
        signed: &PresignedRequest,
        browser_origin: &str,
        method: &Method,
    ) -> Result<ProbeResponse, ProbeFailure> {
        let request = self
            .request(signed, Method::OPTIONS)?
            .header(ORIGIN, browser_value(browser_origin)?)
            .header(
                ACCESS_CONTROL_REQUEST_METHOD,
                HeaderValue::from_str(method.as_str()).map_err(|_| ProbeFailure::Refused)?,
            )
            .body(empty())
            .map_err(|_| ProbeFailure::Refused)?;
        self.send(request).await
    }

    fn request(
        &self,
        signed: &PresignedRequest,
        method: Method,
    ) -> Result<http::request::Builder, ProbeFailure> {
        if signed.url().origin() != self.origin {
            return Err(ProbeFailure::Refused);
        }
        Ok(Request::builder().method(method).uri(signed.url().as_str()))
    }

    async fn send(&self, request: Request<ProbeBody>) -> Result<ProbeResponse, ProbeFailure> {
        let exchange = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|_| ProbeFailure::Unreachable)?;
            let (parts, body) = response.into_parts();
            let body = match Limited::new(body, RESPONSE_LIMIT).collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(_) => Bytes::new(),
            };
            Ok(ProbeResponse {
                status: parts.status,
                headers: parts.headers,
                body,
            })
        };
        tokio::time::timeout(REQUEST_TIMEOUT, exchange)
            .await
            .unwrap_or(Err(ProbeFailure::Unreachable))
    }
}

fn empty() -> ProbeBody {
    BodyExt::boxed_unsync(Empty::<Bytes>::new().map_err(|never| match never {}))
}

fn browser_value(origin: &str) -> Result<HeaderValue, ProbeFailure> {
    HeaderValue::from_str(origin).map_err(|_| ProbeFailure::Refused)
}
