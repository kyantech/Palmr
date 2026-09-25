mod assembly;
pub mod client;
pub mod config;
mod copy;
mod list;
mod multipart;
mod object;
pub mod plan;
mod presign;
mod probe_http;
pub mod profile;
mod provider;
mod selftest;
pub mod tls;

use std::error::Error;
use std::fmt;
use std::sync::{Arc, OnceLock};

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use url::Url;

use self::client::S3Clients;
use self::copy::SINGLE_COPY_MAX;
use self::object::SourceBodyError;
use self::probe_http::PublicProbe;
use self::profile::ProfileLimits;
use self::provider::capabilities;
use super::caps::StorageCapabilities;
use super::error::{Retryable, StorageError};
use crate::domain::clock::{Clock, SystemClock};

const MAX_ERROR_CODE_LEN: usize = 64;

pub struct S3Provider {
    clients: S3Clients,
    limits: ProfileLimits,
    caps: StorageCapabilities,
    verified_caps: OnceLock<StorageCapabilities>,
    single_copy_max: u64,
    buffer_bytes: usize,
    clock: Arc<dyn Clock>,
    browser_origin: String,
    public_probe: PublicProbe,
}

impl S3Provider {
    pub fn new(
        clients: S3Clients,
        buffer_bytes: u32,
        base_url: &Url,
    ) -> Result<Self, StorageError> {
        let buffer_bytes = usize::try_from(buffer_bytes)
            .ok()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| {
                StorageError::Config("the upload buffer must hold at least one byte".to_owned())
            })?;
        let limits = clients.shared().profile().limits();
        let public_probe = PublicProbe::new(&clients);
        Ok(Self {
            clients,
            limits,
            caps: capabilities(&limits),
            verified_caps: OnceLock::new(),
            single_copy_max: SINGLE_COPY_MAX,
            buffer_bytes,
            clock: Arc::new(SystemClock),
            browser_origin: base_url.origin().ascii_serialization(),
            public_probe,
        })
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_limits(mut self, limits: ProfileLimits) -> Self {
        self.limits = limits;
        self.caps = capabilities(&limits);
        self.verified_caps = OnceLock::new();
        self
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn with_single_copy_max(mut self, single_copy_max: u64) -> Self {
        self.single_copy_max = single_copy_max;
        self
    }

    fn internal(&self) -> &aws_sdk_s3::Client {
        self.clients.internal_client().client()
    }

    fn bucket(&self) -> &str {
        self.clients.shared().bucket()
    }

    const fn limits(&self) -> ProfileLimits {
        self.limits
    }
}

impl fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Provider")
            .field("clients", &self.clients)
            .field("buffer_bytes", &self.buffer_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    HeadBucket,
    HeadObject,
    GetObject,
    PutObject,
    DeleteObject,
    CopyObject,
    ListObjectsV2,
    CreateMultipartUpload,
    UploadPart,
    UploadPartCopy,
    ListParts,
    CompleteMultipartUpload,
    AbortMultipartUpload,
    ListMultipartUploads,
}

impl Operation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::HeadBucket => "HeadBucket",
            Self::HeadObject => "HeadObject",
            Self::GetObject => "GetObject",
            Self::PutObject => "PutObject",
            Self::DeleteObject => "DeleteObject",
            Self::CopyObject => "CopyObject",
            Self::ListObjectsV2 => "ListObjectsV2",
            Self::CreateMultipartUpload => "CreateMultipartUpload",
            Self::UploadPart => "UploadPart",
            Self::UploadPartCopy => "UploadPartCopy",
            Self::ListParts => "ListParts",
            Self::CompleteMultipartUpload => "CompleteMultipartUpload",
            Self::AbortMultipartUpload => "AbortMultipartUpload",
            Self::ListMultipartUploads => "ListMultipartUploads",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    Construction,
    Timeout,
    Dispatch,
    Response { status: Option<u16> },
    Service { status: u16, code: Option<String> },
    MalformedResponse(&'static str),
    InvalidRequest(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Failure {
    operation: Operation,
    failure: Failure,
}

impl S3Failure {
    pub(crate) const fn new(operation: Operation, failure: Failure) -> Self {
        Self { operation, failure }
    }
}

impl fmt::Display for S3Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = self.operation.as_str();
        match &self.failure {
            Failure::Construction => write!(f, "S3 {operation} request could not be built"),
            Failure::Timeout => write!(f, "S3 {operation} timed out"),
            Failure::Dispatch => write!(f, "S3 {operation} could not reach the provider"),
            Failure::Response {
                status: Some(status),
            } => {
                write!(
                    f,
                    "S3 {operation} returned an unreadable HTTP {status} response"
                )
            }
            Failure::Response { status: None } => {
                write!(f, "S3 {operation} returned an unreadable response")
            }
            Failure::Service {
                status,
                code: Some(code),
            } => write!(f, "S3 {operation} failed with HTTP {status} ({code})"),
            Failure::Service { status, code: None } => {
                write!(f, "S3 {operation} failed with HTTP {status}")
            }
            Failure::MalformedResponse(detail) => {
                write!(f, "S3 {operation} response is malformed: {detail}")
            }
            Failure::InvalidRequest(detail) => {
                write!(f, "S3 {operation} request is invalid: {detail}")
            }
        }
    }
}

impl Error for S3Failure {}

pub(crate) fn classify<E>(operation: Operation, error: SdkError<E, HttpResponse>) -> StorageError
where
    E: ProvideErrorMetadata + Error + Send + Sync + 'static,
{
    if let Some(source) = source_body_error(&error) {
        return StorageError::Io(source);
    }
    let failure = match &error {
        SdkError::ConstructionFailure(_) => Failure::Construction,
        SdkError::TimeoutError(_) => Failure::Timeout,
        SdkError::DispatchFailure(_) => Failure::Dispatch,
        SdkError::ResponseError(response) => Failure::Response {
            status: Some(response.raw().status().as_u16()),
        },
        SdkError::ServiceError(service) => Failure::Service {
            status: service.raw().status().as_u16(),
            code: service.err().code().map(sanitize_code),
        },
        _ => Failure::Response { status: None },
    };
    classify_failure(operation, failure)
}

pub(crate) fn classify_failure(operation: Operation, failure: Failure) -> StorageError {
    match &failure {
        Failure::Timeout | Failure::Dispatch | Failure::Response { .. } => {
            StorageError::ProviderUnavailable(Retryable::new(S3Failure::new(operation, failure)))
        }
        Failure::Service { status, code } => match (*status, code.as_deref()) {
            (_, Some("NoSuchBucket")) => {
                StorageError::S3(Box::new(S3Failure::new(operation, failure)))
            }
            (_, Some("NoSuchKey" | "NoSuchUpload" | "NotFound")) | (404, None) => {
                StorageError::NotFound
            }
            (
                _,
                Some(
                    "AccessDenied"
                    | "AllAccessDisabled"
                    | "InvalidAccessKeyId"
                    | "SignatureDoesNotMatch"
                    | "AccountProblem",
                ),
            )
            | (401 | 403, _) => StorageError::PermissionDenied,
            (
                _,
                Some(
                    "SlowDown"
                    | "RequestTimeout"
                    | "ServiceUnavailable"
                    | "InternalError"
                    | "RequestTimeTooSkewed",
                ),
            )
            | (408 | 429 | 500..=599, _) => StorageError::ProviderUnavailable(Retryable::new(
                S3Failure::new(operation, failure),
            )),
            _ => StorageError::S3(Box::new(S3Failure::new(operation, failure))),
        },
        Failure::Construction | Failure::MalformedResponse(_) | Failure::InvalidRequest(_) => {
            StorageError::S3(Box::new(S3Failure::new(operation, failure)))
        }
    }
}

pub(crate) fn is_range_not_satisfiable<E>(error: &SdkError<E, HttpResponse>) -> bool
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ServiceError(service) => {
            service.raw().status().as_u16() == 416 || service.err().code() == Some("InvalidRange")
        }
        _ => false,
    }
}

pub(crate) fn service_code(error: &StorageError) -> Option<&str> {
    let failure = match error {
        StorageError::S3(source) => source.downcast_ref::<S3Failure>(),
        StorageError::ProviderUnavailable(retryable) => {
            Error::source(retryable).and_then(|source| source.downcast_ref::<S3Failure>())
        }
        _ => None,
    }?;
    match &failure.failure {
        Failure::Service { code, .. } => code.as_deref(),
        _ => None,
    }
}

fn source_body_error(error: &(dyn Error + 'static)) -> Option<std::io::Error> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(body) = error.downcast_ref::<SourceBodyError>() {
            return Some(body.to_io_error());
        }
        current = error.source();
    }
    None
}

fn sanitize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_ERROR_CODE_LEN)
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod object_tests;

#[cfg(test)]
mod plan_tests;

#[cfg(test)]
mod multipart_tests;

#[cfg(test)]
mod presign_tests;

#[cfg(test)]
mod provider_tests;

#[cfg(test)]
pub(crate) mod fake_server;

#[cfg(test)]
mod selftest_tests;
