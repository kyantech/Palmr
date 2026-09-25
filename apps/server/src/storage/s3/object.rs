use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{ready, Context, Poll};

use aws_sdk_s3::primitives::{ByteStream, DateTime};
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use futures_core::Stream;
use http_body::{Body, Frame, SizeHint};
use time::OffsetDateTime;
use tokio::io::AsyncReadExt as _;
use tokio_util::io::ReaderStream;

use super::{classify, classify_failure, is_range_not_satisfiable, Failure, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{ETag, ObjectBody, ObjectStat};

const MAX_RANGE_END: u64 = i64::MAX.unsigned_abs();

impl S3Provider {
    pub(crate) async fn head_object(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        let output = self
            .internal()
            .head_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| classify(Operation::HeadObject, error))?;
        let size = measured_size(Operation::HeadObject, output.content_length())?;
        object_stat(
            Operation::HeadObject,
            size,
            output.last_modified(),
            output.e_tag(),
        )
    }

    pub(crate) async fn object_exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        match self.head_object(key).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn get_object(
        &self,
        key: &ObjectKey,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        let output = self
            .internal()
            .get_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| classify(Operation::GetObject, error))?;
        let size = measured_size(Operation::GetObject, output.content_length())?;
        let stat = object_stat(
            Operation::GetObject,
            size,
            output.last_modified(),
            output.e_tag(),
        )?;
        Ok((stat, streamed(output.body)))
    }

    pub(crate) async fn get_object_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        if len == 0 || start > MAX_RANGE_END {
            let stat = self.head_object(key).await?;
            if start >= stat.size {
                return Err(StorageError::RangeNotSatisfiable { size: stat.size });
            }
            if len == 0 {
                return Ok((stat, Box::pin(tokio::io::empty())));
            }
        }

        let requested = RequestedRange::new(start, len);
        let output = match self
            .internal()
            .get_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .range(requested.header())
            .send()
            .await
        {
            Ok(output) => output,
            Err(error) if is_range_not_satisfiable(&error) => {
                let stat = self.head_object(key).await?;
                return Err(StorageError::RangeNotSatisfiable { size: stat.size });
            }
            Err(error) => return Err(classify(Operation::GetObject, error)),
        };

        let served = output
            .content_range()
            .and_then(ContentRange::parse)
            .ok_or_else(|| {
                malformed(
                    Operation::GetObject,
                    "the provider ignored the Range request",
                )
            })?;
        if !requested.is_served_by(&served) {
            return Err(malformed(
                Operation::GetObject,
                "the provider served a different byte range",
            ));
        }
        if measured_size(Operation::GetObject, output.content_length())? != served.len() {
            return Err(malformed(
                Operation::GetObject,
                "the range body length disagrees with Content-Range",
            ));
        }
        let stat = object_stat(
            Operation::GetObject,
            served.total,
            output.last_modified(),
            output.e_tag(),
        )?;
        Ok((stat, streamed(output.body)))
    }

    pub(crate) async fn put_object_single(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        len: u64,
        content_type: Option<&str>,
    ) -> Result<ObjectStat, StorageError> {
        let content_length = i64::try_from(len).map_err(|_| {
            classify_failure(
                Operation::PutObject,
                Failure::InvalidRequest("the declared length exceeds the protocol range"),
            )
        })?;
        let sized = SizedBody::new(body, len, self.buffer_bytes);
        self.internal()
            .put_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .content_length(content_length)
            .set_content_type(content_type.map(str::to_owned))
            .body(ByteStream::new(SdkBody::from_body_1_x(sized)))
            .send()
            .await
            .map_err(|error| classify(Operation::PutObject, error))?;
        self.head_object(key).await
    }

    pub(crate) async fn delete_object(&self, key: &ObjectKey) -> Result<(), StorageError> {
        match self
            .internal()
            .delete_object()
            .bucket(self.bucket())
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| classify(Operation::DeleteObject, error))
        {
            Ok(_) | Err(StorageError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn measured_size(
    operation: Operation,
    length: Option<i64>,
) -> Result<u64, StorageError> {
    length
        .and_then(|length| u64::try_from(length).ok())
        .ok_or_else(|| malformed(operation, "Content-Length is missing or negative"))
}

pub(super) fn object_stat(
    operation: Operation,
    size: u64,
    last_modified: Option<&DateTime>,
    etag: Option<&str>,
) -> Result<ObjectStat, StorageError> {
    let modified_at = last_modified
        .and_then(|value| OffsetDateTime::from_unix_timestamp_nanos(value.as_nanos()).ok())
        .ok_or_else(|| malformed(operation, "Last-Modified is missing or out of range"))?;
    Ok(ObjectStat {
        size,
        modified_at,
        etag: etag.map(ETag::new),
    })
}

pub(super) fn malformed(operation: Operation, detail: &'static str) -> StorageError {
    classify_failure(operation, Failure::MalformedResponse(detail))
}

fn streamed(body: ByteStream) -> ObjectBody {
    Box::pin(body.into_async_read())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestedRange {
    start: u64,
    last: u64,
}

impl RequestedRange {
    pub(super) fn new(start: u64, len: u64) -> Self {
        Self {
            start,
            last: start.saturating_add(len.saturating_sub(1)),
        }
    }

    pub(super) fn header(&self) -> String {
        if self.last > MAX_RANGE_END {
            format!("bytes={}-", self.start)
        } else {
            format!("bytes={}-{}", self.start, self.last)
        }
    }

    fn is_served_by(&self, served: &ContentRange) -> bool {
        served.start == self.start
            && served.total > self.start
            && served.last == self.last.min(served.total - 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContentRange {
    start: u64,
    last: u64,
    total: u64,
}

impl ContentRange {
    pub(super) fn parse(value: &str) -> Option<Self> {
        let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
        let (start, last) = range.split_once('-')?;
        let parsed = Self {
            start: start.parse().ok()?,
            last: last.parse().ok()?,
            total: total.parse().ok()?,
        };
        (parsed.start <= parsed.last && parsed.last < parsed.total).then_some(parsed)
    }

    const fn len(&self) -> u64 {
        self.last - self.start + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceBodyError {
    Short,
    Long,
    Read(io::ErrorKind),
}

impl SourceBodyError {
    pub(super) fn to_io_error(self) -> io::Error {
        match self {
            Self::Short => io::Error::new(io::ErrorKind::UnexpectedEof, self),
            Self::Long => io::Error::new(io::ErrorKind::InvalidData, self),
            Self::Read(kind) => io::Error::new(kind, self),
        }
    }
}

impl fmt::Display for SourceBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Short => f.write_str("the source stream ended before the declared length"),
            Self::Long => f.write_str("the source stream exceeds the declared length"),
            Self::Read(kind) => write!(f, "the source stream failed: {kind}"),
        }
    }
}

impl std::error::Error for SourceBodyError {}

type SourceStream = ReaderStream<tokio::io::Take<ObjectBody>>;

pub(super) struct SizedBody {
    stream: Mutex<SourceStream>,
    declared: u64,
    sent: u64,
    finished: bool,
}

impl SizedBody {
    pub(super) fn new(body: ObjectBody, declared: u64, buffer_bytes: usize) -> Self {
        let reader = body.take(declared.saturating_add(1));
        Self {
            stream: Mutex::new(ReaderStream::with_capacity(reader, buffer_bytes)),
            declared,
            sent: 0,
            finished: false,
        }
    }

    fn stream(&mut self) -> Pin<&mut SourceStream> {
        let stream = self
            .stream
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Pin::new(stream)
    }
}

impl Body for SizedBody {
    type Data = Bytes;
    type Error = SourceBodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, SourceBodyError>>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        let next = ready!(this.stream().poll_next(cx));
        let outcome = match next {
            Some(Ok(chunk)) => {
                this.sent = this.sent.saturating_add(chunk.len() as u64);
                if this.sent > this.declared {
                    Some(Err(SourceBodyError::Long))
                } else {
                    Some(Ok(Frame::data(chunk)))
                }
            }
            Some(Err(error)) => Some(Err(SourceBodyError::Read(error.kind()))),
            None if this.sent < this.declared => Some(Err(SourceBodyError::Short)),
            None => None,
        };
        if !matches!(outcome, Some(Ok(_))) {
            this.finished = true;
        }
        Poll::Ready(outcome)
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.declared.saturating_sub(self.sent))
    }
}
