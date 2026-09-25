pub mod client;
pub mod config;
mod copy;
mod list;
mod object;
pub mod profile;
pub mod tls;

use std::error::Error;
use std::fmt;

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;

use self::client::S3Clients;
use self::object::SourceBodyError;
use super::error::{Retryable, StorageError};

const MAX_ERROR_CODE_LEN: usize = 64;

pub struct S3Provider {
    clients: S3Clients,
    buffer_bytes: usize,
}

impl S3Provider {
    pub fn new(clients: S3Clients, buffer_bytes: u32) -> Result<Self, StorageError> {
        let buffer_bytes = usize::try_from(buffer_bytes)
            .ok()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| {
                StorageError::Config("the upload buffer must hold at least one byte".to_owned())
            })?;
        Ok(Self {
            clients,
            buffer_bytes,
        })
    }

    fn internal(&self) -> &aws_sdk_s3::Client {
        self.clients.internal_client().client()
    }

    fn bucket(&self) -> &str {
        self.clients.shared().bucket()
    }
}

impl fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Provider")
            .field("clients", &self.clients)
            .field("buffer_bytes", &self.buffer_bytes)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    HeadObject,
    GetObject,
    PutObject,
    DeleteObject,
    CopyObject,
    ListObjectsV2,
}

impl Operation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::HeadObject => "HeadObject",
            Self::GetObject => "GetObject",
            Self::PutObject => "PutObject",
            Self::DeleteObject => "DeleteObject",
            Self::CopyObject => "CopyObject",
            Self::ListObjectsV2 => "ListObjectsV2",
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
            (_, Some("NoSuchKey" | "NotFound")) | (404, None) => StorageError::NotFound,
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
