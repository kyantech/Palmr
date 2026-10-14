use std::borrow::Cow;
use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use aws_sdk_s3::config::{
    BehaviorVersion, Config, Credentials, Region, RequestChecksumCalculation,
    StalledStreamProtectionConfig,
};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_smithy_runtime_api::client::connector_metadata::ConnectorMetadata;
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
    SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::body::SdkBody;
use tower::Service;
use url::Url;

use crate::config::StorageConfig;
use crate::domain::secret::{Secret, REDACTED};

use super::config::{public_origin, ResolvedS3Config, S3SetupError};
use super::profile::ProviderProfile;
use super::tls::{self, S3TlsConfig, S3TlsPolicy};

const CREDENTIALS_PROVIDER_NAME: &str = "palmr-static";
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const STALL_GRACE: Duration = Duration::from_secs(60);
pub(super) const SEND_BUFFER_BYTES: u32 = 256 * 1024;

pub struct S3Shared {
    resolved: ResolvedS3Config,
}

impl S3Shared {
    pub const fn profile(&self) -> ProviderProfile {
        self.resolved.profile
    }

    pub fn region(&self) -> &str {
        &self.resolved.region
    }

    pub fn bucket(&self) -> &str {
        &self.resolved.bucket
    }

    pub const fn force_path_style(&self) -> bool {
        self.resolved.force_path_style
    }

    pub const fn tls_policy(&self) -> S3TlsPolicy {
        self.resolved.tls_policy()
    }

    pub const fn multipart_ttl(&self) -> Duration {
        self.resolved.multipart_ttl
    }

    pub const fn part_checksums_override(&self) -> Option<bool> {
        self.resolved.require_part_checksums
    }

    pub(crate) fn access_key(&self) -> &Secret<String> {
        &self.resolved.access_key
    }

    pub(crate) fn secret_key(&self) -> &Secret<String> {
        &self.resolved.secret_key
    }

    fn credentials(&self) -> Credentials {
        Credentials::new(
            self.resolved.access_key.expose_secret().clone(),
            self.resolved.secret_key.expose_secret().clone(),
            None,
            None,
            CREDENTIALS_PROVIDER_NAME,
        )
    }
}

impl fmt::Debug for S3Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Shared")
            .field("profile", &self.resolved.profile)
            .field("region", &self.resolved.region)
            .field("bucket", &self.resolved.bucket)
            .field("force_path_style", &self.resolved.force_path_style)
            .field("tls_policy", &self.resolved.tls_policy())
            .field("access_key", &REDACTED)
            .field("secret_key", &REDACTED)
            .finish()
    }
}

pub struct InternalClient {
    client: aws_sdk_s3::Client,
    endpoint: Url,
    shared: Arc<S3Shared>,
}

impl InternalClient {
    pub fn client(&self) -> &aws_sdk_s3::Client {
        &self.client
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub(crate) fn shared(&self) -> &Arc<S3Shared> {
        &self.shared
    }
}

impl fmt::Debug for InternalClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InternalClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

pub struct PublicSigner {
    client: aws_sdk_s3::Client,
    endpoint: Url,
    shared: Arc<S3Shared>,
}

impl PublicSigner {
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub(super) fn signing_client(&self) -> &aws_sdk_s3::Client {
        &self.client
    }

    pub fn presigning_config(&self, ttl: Duration) -> Result<PresigningConfig, S3SetupError> {
        PresigningConfig::expires_in(ttl).map_err(|_| S3SetupError::PresigningConfiguration)
    }

    pub(crate) fn shared(&self) -> &Arc<S3Shared> {
        &self.shared
    }
}

impl fmt::Debug for PublicSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicSigner")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

pub struct S3Clients {
    shared: Arc<S3Shared>,
    base: Config,
    internal: InternalClient,
    public: PublicSigner,
    internal_tls: S3TlsConfig,
    public_tls: S3TlsConfig,
}

impl S3Clients {
    pub fn build(storage: &StorageConfig) -> Result<Option<Self>, S3SetupError> {
        let StorageConfig::S3(s3) = storage else {
            return Ok(None);
        };

        let resolved = ResolvedS3Config::from_config(s3);
        if !resolved.verify_certificates {
            tracing::warn!(
                endpoint = %resolved.endpoint,
                "TLS certificate verification is DISABLED for the S3 client; S3 credentials and every uploaded byte can be read and modified in transit. Use PALMR_S3_CA_FILE to trust your CA instead. SMTP and identity-provider connections are UNAFFECTED and still verify certificates."
            );
        }

        let ca_file = resolved.ca_file.as_deref();
        let internal_tls = tls::build(ca_file, resolved.verify_certificates)?;
        let public_tls = tls::build(ca_file, resolved.verify_certificates)?;

        let shared = Arc::new(S3Shared { resolved });
        let base = Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(shared.region().to_owned()))
            .credentials_provider(shared.credentials())
            .force_path_style(shared.force_path_style())
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .stalled_stream_protection(
                StalledStreamProtectionConfig::enabled()
                    .grace_period(STALL_GRACE)
                    .build(),
            )
            .build();

        let internal = InternalClient {
            client: client_for(&base, &shared.resolved.endpoint, &internal_tls),
            endpoint: shared.resolved.endpoint.clone(),
            shared: Arc::clone(&shared),
        };
        let public = PublicSigner {
            client: client_for(&base, &shared.resolved.public_endpoint, &public_tls),
            endpoint: shared.resolved.public_endpoint.clone(),
            shared: Arc::clone(&shared),
        };

        Ok(Some(Self {
            shared,
            base,
            internal,
            public,
            internal_tls,
            public_tls,
        }))
    }

    pub fn internal_client(&self) -> &InternalClient {
        &self.internal
    }

    pub fn public_signer(&self) -> &PublicSigner {
        &self.public
    }

    pub fn internal_tls(&self) -> &S3TlsConfig {
        &self.internal_tls
    }

    pub fn public_tls(&self) -> &S3TlsConfig {
        &self.public_tls
    }

    pub fn public_origin(&self) -> Option<String> {
        public_origin(&self.public.endpoint)
    }

    pub(crate) fn shared(&self) -> &Arc<S3Shared> {
        &self.shared
    }

    pub(crate) fn base_config(&self) -> &Config {
        &self.base
    }

    #[cfg(test)]
    pub(crate) fn with_internal_client(mut self, client: aws_sdk_s3::Client) -> Self {
        self.internal.client = client;
        self
    }
}

impl fmt::Debug for S3Clients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Clients")
            .field("shared", &self.shared)
            .field("internal", &self.internal)
            .field("public", &self.public)
            .field("internal_tls", &self.internal_tls)
            .field("public_tls", &self.public_tls)
            .finish_non_exhaustive()
    }
}

fn client_for(base: &Config, endpoint: &Url, tls: &S3TlsConfig) -> aws_sdk_s3::Client {
    let config = base
        .to_builder()
        .endpoint_url(endpoint.as_str())
        .http_client(http_client(tls))
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

pub(super) fn http_client(tls: &S3TlsConfig) -> SharedHttpClient {
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(CONNECT_TIMEOUT));
    http.set_send_buffer_size(Some(SEND_BUFFER_BYTES as usize));
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config((**tls.config()).clone())
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(http);
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(https);
    SharedHttpClient::new(S3HttpClient {
        connector: S3HttpConnector {
            client,
            response_wait: None,
        },
    })
}

type HyperS3Client = hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    SdkBody,
>;

#[derive(Clone)]
struct S3HttpConnector {
    client: HyperS3Client,
    response_wait: Option<Duration>,
}

impl fmt::Debug for S3HttpConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("S3HttpConnector")
    }
}

impl HttpConnector for S3HttpConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let request = match request.try_into_http1x() {
            Ok(request) => request,
            Err(error) => {
                return HttpConnectorFuture::ready(Err(ConnectorError::user(error.into())))
            }
        };
        let sent = Arc::new(SentSignal::default());
        let request = request.map(|body| {
            SdkBody::from_body_1_x(SentBody {
                remaining: http_body::Body::size_hint(&body).exact(),
                inner: body,
                signal: Arc::clone(&sent),
            })
        });
        sent.mark_if_empty(&request);
        let mut client = self.client.clone();
        let future = client.call(request);
        let response_wait = self.response_wait;
        HttpConnectorFuture::new(async move {
            let response = match response_wait {
                None => future.await,
                Some(limit) => {
                    tokio::pin!(future);
                    tokio::select! {
                        outcome = &mut future => outcome,
                        () = sent.elapsed_since_sent(limit) => {
                            return Err(ConnectorError::timeout(
                                std::io::Error::from(std::io::ErrorKind::TimedOut).into(),
                            ));
                        }
                    }
                }
            }
            .map_err(connector_error)?;
            let response = response.map(SdkBody::from_body_1_x);
            HttpResponse::try_from(response)
                .map_err(|error| ConnectorError::other(error.into(), None))
        })
    }
}

struct SentSignal {
    started: tokio::time::Instant,
    sent: AtomicBool,
    sent_after: std::sync::Mutex<Duration>,
    notify: tokio::sync::Notify,
}

impl Default for SentSignal {
    fn default() -> Self {
        Self {
            started: tokio::time::Instant::now(),
            sent: AtomicBool::new(false),
            sent_after: std::sync::Mutex::new(Duration::ZERO),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl SentSignal {
    fn mark(&self) {
        if !self.sent.swap(true, Ordering::SeqCst) {
            *self
                .sent_after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self.started.elapsed();
        }
        self.notify.notify_waiters();
    }

    fn mark_if_empty(&self, request: &http::Request<SdkBody>) {
        if http_body::Body::is_end_stream(request.body()) {
            self.mark();
        }
    }

    async fn elapsed_since_sent(&self, limit: Duration) {
        loop {
            let notified = self.notify.notified();
            if self.sent.load(Ordering::SeqCst) {
                break;
            }
            notified.await;
        }
        let uploading = *self
            .sent_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokio::time::sleep(limit.saturating_add(uploading)).await;
    }
}

struct SentBody {
    inner: SdkBody,
    remaining: Option<u64>,
    signal: Arc<SentSignal>,
}

impl http_body::Body for SentBody {
    type Data = bytes::Bytes;
    type Error = <SdkBody as http_body::Body>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(None) => this.signal.mark(),
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(remaining), Some(data)) = (this.remaining.as_mut(), frame.data_ref()) {
                    *remaining = remaining.saturating_sub(data.len() as u64);
                    if *remaining == 0 {
                        this.signal.mark();
                    }
                }
            }
            Poll::Ready(Some(Err(_))) | Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[derive(Clone)]
struct S3HttpClient {
    connector: S3HttpConnector,
}

impl fmt::Debug for S3HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("S3HttpClient")
    }
}

impl HttpClient for S3HttpClient {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(S3HttpConnector {
            response_wait: settings.read_timeout(),
            ..self.connector.clone()
        })
    }

    fn connector_metadata(&self) -> Option<ConnectorMetadata> {
        Some(ConnectorMetadata::new("hyper", Some(Cow::Borrowed("1.x"))))
    }
}

fn connector_error(error: hyper_util::client::legacy::Error) -> ConnectorError {
    let is_connect = error.is_connect();
    let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(error);
    if is_connect {
        ConnectorError::io(boxed)
    } else {
        ConnectorError::other(boxed, None)
    }
}
