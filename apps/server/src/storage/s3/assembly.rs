use std::io;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures_util::stream::{self, StreamExt as _, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _, ReadBuf};

use super::multipart::invalid;
use super::object::SourceBodyError;
use super::plan::{plan_parts_with_limits, PartPlan, PartPlanError};
use super::profile::ProfileLimits;
use super::{Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    MultipartHandle, MultipartStorage as _, ObjectBody, ObjectStat, PutHint, UploadedPart,
};

pub const COPY_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteRoute {
    PutObject,
    Multipart(PartPlan),
}

impl WriteRoute {
    pub fn for_len(len: u64, limits: &ProfileLimits) -> Result<Self, PartPlanError> {
        match plan_parts_with_limits(len, limits)? {
            PartPlan::ZeroByte => Ok(Self::PutObject),
            PartPlan::Multipart { .. } if len <= limits.min_part => Ok(Self::PutObject),
            plan @ PartPlan::Multipart { .. } => Ok(Self::Multipart(plan)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPart {
    pub number: u32,
    pub range: RangeInclusive<u64>,
}

impl PlannedPart {
    pub fn byte_len(&self) -> u64 {
        self.range.end() - self.range.start() + 1
    }
}

pub fn planned_part(
    operation: Operation,
    plan: PartPlan,
    part_number: u64,
    size: u64,
) -> Result<PlannedPart, StorageError> {
    let number = u32::try_from(part_number)
        .map_err(|_| invalid(operation, "the part number is outside the provider range"))?;
    let (start, end) = plan
        .part_range(part_number, size)
        .filter(|(start, end)| start < end)
        .ok_or_else(|| invalid(operation, "the planned part range is empty"))?;
    Ok(PlannedPart {
        number,
        range: start..=end - 1,
    })
}

pub fn plan_rejected(operation: Operation, error: PartPlanError) -> StorageError {
    match error {
        PartPlanError::FileTooLarge { .. } => {
            invalid(operation, "the object exceeds the provider size limits")
        }
        PartPlanError::InvalidProfile | PartPlanError::InvalidPartNumber => {
            invalid(operation, "the provider profile cannot plan this object")
        }
    }
}

impl S3Provider {
    fn multipart_plan(&self, operation: Operation, size: u64) -> Result<PartPlan, StorageError> {
        match plan_parts_with_limits(size, &self.limits()) {
            Ok(plan @ PartPlan::Multipart { .. }) => Ok(plan),
            Ok(PartPlan::ZeroByte) => Err(invalid(
                operation,
                "a zero-byte object is never assembled from parts",
            )),
            Err(error) => Err(plan_rejected(operation, error)),
        }
    }

    pub(super) async fn copy_multipart(
        &self,
        src: &ObjectKey,
        dst: &ObjectKey,
        size: u64,
    ) -> Result<ObjectStat, StorageError> {
        const OP: Operation = Operation::UploadPartCopy;
        let plan = self.multipart_plan(OP, size)?;
        let hint = PutHint {
            declared_len: Some(size),
            content_type: None,
        };
        let handle = self.create_multipart(dst, hint).await?;
        let assembled = async {
            let parts: Vec<UploadedPart> = stream::iter(1..=plan.part_count())
                .map(|part_number| {
                    let handle = &handle;
                    async move {
                        let part = planned_part(OP, plan, part_number, size)?;
                        self.upload_part_copy(handle, part.number, src, part.range)
                            .await
                    }
                })
                .buffered(COPY_CONCURRENCY)
                .try_collect()
                .await?;
            self.complete_multipart(&handle, &parts).await
        }
        .await;
        self.abort_unless_complete(&handle, assembled).await
    }

    pub(super) async fn put_multipart(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        len: u64,
        plan: PartPlan,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        const OP: Operation = Operation::UploadPart;
        let handle = self.create_multipart(key, hint).await?;
        let source = PartSource::new(body);
        let assembled = async {
            let mut parts = Vec::new();
            for part_number in 1..=plan.part_count() {
                let part = planned_part(OP, plan, part_number, len)?;
                let part_len = part.byte_len();
                let uploaded = self
                    .upload_part_stream(&handle, part.number, part_len, source.part(part_len))
                    .await?;
                parts.push(uploaded);
            }
            source.ensure_drained().await?;
            self.complete_multipart(&handle, &parts).await
        }
        .await;
        self.abort_unless_complete(&handle, assembled).await
    }

    async fn abort_unless_complete(
        &self,
        handle: &MultipartHandle,
        assembled: Result<ObjectStat, StorageError>,
    ) -> Result<ObjectStat, StorageError> {
        if assembled.is_err() {
            if let Err(abort) = self.abort_multipart(handle).await {
                tracing::warn!(
                    error = %abort,
                    "aborting an incomplete S3 multipart upload failed; the reconciliation sweep reaps it"
                );
            }
        }
        assembled
    }
}

struct PartSource(Arc<Mutex<ObjectBody>>);

impl PartSource {
    fn new(body: ObjectBody) -> Self {
        Self(Arc::new(Mutex::new(body)))
    }

    fn part(&self, len: u64) -> ObjectBody {
        Box::pin(SharedReader(Arc::clone(&self.0)).take(len))
    }

    async fn ensure_drained(&self) -> Result<(), StorageError> {
        let mut probe = [0_u8; 1];
        let read =
            self.part(1).read(&mut probe).await.map_err(|error| {
                StorageError::Io(SourceBodyError::Read(error.kind()).to_io_error())
            })?;
        if read == 0 {
            Ok(())
        } else {
            Err(StorageError::Io(SourceBodyError::Long.to_io_error()))
        }
    }
}

struct SharedReader(Arc<Mutex<ObjectBody>>);

impl AsyncRead for SharedReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut body = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        body.as_mut().poll_read(cx, buf)
    }
}
