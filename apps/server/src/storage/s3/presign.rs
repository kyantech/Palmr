use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use aws_sdk_s3::presigning::{PresignedRequest as SignedByProvider, PresigningConfig};
use http::{HeaderMap, HeaderValue, Method};
use time::OffsetDateTime;
use url::Url;

use super::multipart::{invalid, provider_part_len, provider_part_number};
use super::object::malformed;
use super::{classify, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    GrantContext, MultipartHandle, PartPlanEntry, PresignStorage, PresignedRequest,
};

pub const MAX_PART_URLS_PER_CALL: usize = 16;
pub const MAX_PART_URL_TTL: Duration = Duration::from_secs(15 * 60);
pub const MAX_GET_URL_TTL: Duration = Duration::from_secs(5 * 60);

const SIGNED_HEADERS_PARAM: &str = "x-amz-signedheaders";
const BROWSER_SIGNED_HEADERS: &str = "host";
const CHECKSUM_PREFIX: &str = "x-amz-checksum-";
const SDK_CHECKSUM_ALGORITHM: &str = "x-amz-sdk-checksum-algorithm";

#[async_trait]
impl PresignStorage for S3Provider {
    async fn presign_get(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        disposition: &HeaderValue,
        content_type: &str,
        _authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError> {
        self.sign_get(key, ttl, disposition, content_type, self.clock.now())
            .await
    }

    async fn presign_put(
        &self,
        _key: &ObjectKey,
        _ttl: Duration,
        _authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError> {
        Err(invalid(
            Operation::PutObject,
            "no presigned PutObject capability is issued",
        ))
    }
}

impl S3Provider {
    pub(super) async fn sign_upload_parts(
        &self,
        handle: &MultipartHandle,
        parts: &[PartPlanEntry],
        ttl: Duration,
        now: OffsetDateTime,
    ) -> Result<Vec<PresignedRequest>, StorageError> {
        const OP: Operation = Operation::UploadPart;
        let limits = self.limits();
        if limits.requires_part_checksums {
            return Err(invalid(
                OP,
                "the provider profile requires server-proxied parts",
            ));
        }
        if parts.len() > MAX_PART_URLS_PER_CALL {
            return Err(invalid(
                OP,
                "more part URLs were requested than one batch allows",
            ));
        }
        let window = SigningWindow::new(OP, ttl, MAX_PART_URL_TTL, now)?;
        let signer = self.clients.public_signer().signing_client();
        let mut signed = Vec::with_capacity(parts.len());
        for part in parts {
            let number = provider_part_number(OP, part.part_number, &limits)?;
            provider_part_len(OP, part.len, &limits)?;
            let request = signer
                .upload_part()
                .bucket(self.bucket())
                .key(handle.key().as_str())
                .upload_id(handle.upload_id())
                .part_number(number)
                .presigned(window.config(OP)?)
                .await
                .map_err(|error| classify(OP, error))?;
            signed.push(self.browser_request(OP, &request, window.expires_at)?);
        }
        Ok(signed)
    }

    pub(super) async fn sign_get(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        disposition: &HeaderValue,
        content_type: &str,
        now: OffsetDateTime,
    ) -> Result<PresignedRequest, StorageError> {
        const OP: Operation = Operation::GetObject;
        let window = SigningWindow::new(OP, ttl, MAX_GET_URL_TTL, now)?;
        let disposition = disposition
            .to_str()
            .ok()
            .filter(|value| pinnable(value))
            .ok_or_else(|| invalid(OP, "the content disposition cannot be pinned"))?;
        if !pinnable(content_type) {
            return Err(invalid(OP, "the content type cannot be pinned"));
        }
        let request = self
            .clients
            .public_signer()
            .signing_client()
            .get_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .response_content_disposition(disposition)
            .response_content_type(content_type)
            .presigned(window.config(OP)?)
            .await
            .map_err(|error| classify(OP, error))?;
        self.browser_request(OP, &request, window.expires_at)
    }

    fn browser_request(
        &self,
        operation: Operation,
        signed: &SignedByProvider,
        expires_at: OffsetDateTime,
    ) -> Result<PresignedRequest, StorageError> {
        let method = Method::from_bytes(signed.method().as_bytes())
            .map_err(|_| malformed(operation, "the signed method is not an HTTP method"))?;
        let url = Url::parse(signed.uri())
            .map_err(|_| malformed(operation, "the signed URL does not parse"))?;
        if !self.addressed_as_configured(&url) {
            return Err(malformed(
                operation,
                "the signed URL does not follow the configured addressing style",
            ));
        }

        let mut signed_headers = None;
        for (name, value) in url.query_pairs() {
            let name = name.to_ascii_lowercase();
            if name.starts_with(CHECKSUM_PREFIX) || name == SDK_CHECKSUM_ALGORITHM {
                return Err(malformed(operation, "the signed URL carries a checksum"));
            }
            if name == SIGNED_HEADERS_PARAM {
                signed_headers = Some(value.into_owned());
            }
        }
        if signed_headers.as_deref() != Some(BROWSER_SIGNED_HEADERS) {
            return Err(malformed(
                operation,
                "the signature covers headers a browser does not send",
            ));
        }
        if signed
            .headers()
            .any(|(name, _)| !name.eq_ignore_ascii_case(BROWSER_SIGNED_HEADERS))
        {
            return Err(malformed(
                operation,
                "the signed request requires extra headers",
            ));
        }
        Ok(PresignedRequest::new(
            method,
            url,
            HeaderMap::new(),
            expires_at,
        ))
    }

    fn addressed_as_configured(&self, url: &Url) -> bool {
        let endpoint = self.clients.public_signer().endpoint();
        let (Some(host), Some(endpoint_host)) = (url.host_str(), endpoint.host_str()) else {
            return false;
        };
        if url.scheme() != endpoint.scheme()
            || url.port_or_known_default() != endpoint.port_or_known_default()
        {
            return false;
        }
        let base = endpoint.path().trim_end_matches('/');
        let bucket = self.bucket();
        if self.clients.shared().force_path_style() {
            host == endpoint_host && url.path().starts_with(&format!("{base}/{bucket}/"))
        } else {
            host.strip_suffix(endpoint_host)
                .and_then(|label| label.strip_suffix('.'))
                == Some(bucket)
                && url.path().starts_with(&format!("{base}/"))
        }
    }
}

struct SigningWindow {
    start: OffsetDateTime,
    lifetime: Duration,
    expires_at: OffsetDateTime,
}

impl SigningWindow {
    fn new(
        operation: Operation,
        ttl: Duration,
        ceiling: Duration,
        now: OffsetDateTime,
    ) -> Result<Self, StorageError> {
        if ttl > ceiling {
            return Err(invalid(
                operation,
                "the requested lifetime exceeds the ceiling",
            ));
        }
        if ttl.as_secs() == 0 {
            return Err(invalid(
                operation,
                "the requested lifetime is under one second",
            ));
        }
        let lifetime = Duration::from_secs(ttl.as_secs());
        let start = now.replace_nanosecond(0).unwrap_or(now);
        Ok(Self {
            start,
            lifetime,
            expires_at: start + lifetime,
        })
    }

    fn config(&self, operation: Operation) -> Result<PresigningConfig, StorageError> {
        PresigningConfig::builder()
            .start_time(SystemTime::from(self.start))
            .expires_in(self.lifetime)
            .build()
            .map_err(|_| invalid(operation, "the signing window is rejected by the signer"))
    }
}

fn pinnable(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| (b' '..=b'~').contains(&byte))
}
