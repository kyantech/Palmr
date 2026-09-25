use std::ops::RangeInclusive;
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Part};
use aws_smithy_types::body::SdkBody;
use time::OffsetDateTime;

use super::copy::copy_source;
use super::object::{malformed, SizedBody};
use super::profile::ProfileLimits;
use super::{classify, classify_failure, Failure, Operation, S3Provider};
use crate::storage::error::{not_found_is_deleted, StorageError};
use crate::storage::health::ProbeKey;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{
    ETag, ListCursor, MultipartHandle, MultipartStorage, MultipartUploadPage, ObjectBody,
    ObjectStat, PartPlanEntry, PendingMultipart, PresignedRequest, PutHint, UploadedPart,
    MAX_LIST_PAGE_SIZE,
};
use crate::storage::stored_key::StoredKey;

const CURSOR_SEPARATOR: char = '\n';

#[async_trait]
impl MultipartStorage for S3Provider {
    async fn create_multipart(
        &self,
        key: &ObjectKey,
        hint: PutHint,
    ) -> Result<MultipartHandle, StorageError> {
        let upload_id = self.create_upload(key, hint).await?;
        Ok(MultipartHandle::new(key.clone(), upload_id))
    }

    async fn sign_part_urls(
        &self,
        handle: &MultipartHandle,
        parts: &[PartPlanEntry],
        ttl: Duration,
    ) -> Result<Vec<PresignedRequest>, StorageError> {
        self.sign_upload_parts(handle, parts, ttl, self.clock.now())
            .await
    }

    async fn list_parts(
        &self,
        handle: &MultipartHandle,
    ) -> Result<Vec<UploadedPart>, StorageError> {
        const OP: Operation = Operation::ListParts;
        let limits = self.limits();
        let mut parts: Vec<UploadedPart> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let output = self
                .internal()
                .list_parts()
                .bucket(self.bucket())
                .key(handle.key().as_str())
                .upload_id(handle.upload_id())
                .max_parts(provider_page())
                .set_part_number_marker(marker.take())
                .send()
                .await
                .map_err(|error| classify(OP, error))?;

            for part in output.parts() {
                let listed = listed_part(part)?;
                if parts
                    .last()
                    .is_some_and(|last| last.part_number >= listed.part_number)
                {
                    return Err(malformed(OP, "parts are not strictly ascending"));
                }
                if listed.part_number > limits.max_parts {
                    return Err(malformed(OP, "a part number exceeds the provider range"));
                }
                parts.push(listed);
            }

            match (output.is_truncated(), output.next_part_number_marker()) {
                (Some(true), _) if output.parts().is_empty() => {
                    return Err(malformed(OP, "a truncated page lists no parts"))
                }
                (Some(true), Some(next)) if !next.is_empty() => marker = Some(next.to_owned()),
                (Some(true), _) => {
                    return Err(malformed(OP, "a truncated page has no part marker"))
                }
                _ => return Ok(parts),
            }
        }
    }

    async fn upload_part_stream(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        len: u64,
        body: ObjectBody,
    ) -> Result<UploadedPart, StorageError> {
        self.upload_part_body(UploadTarget::of(handle), part_number, len, body)
            .await
    }

    async fn upload_part_copy(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        src: &ObjectKey,
        range: RangeInclusive<u64>,
    ) -> Result<UploadedPart, StorageError> {
        const OP: Operation = Operation::UploadPartCopy;
        let limits = self.limits();
        let number = provider_part_number(OP, part_number, &limits)?;
        let (header, len) = copy_source_range(&range)
            .ok_or_else(|| invalid(OP, "the source range is empty or unbounded"))?;
        provider_part_len(OP, len, &limits)?;
        let output = self
            .internal()
            .upload_part_copy()
            .bucket(self.bucket())
            .key(handle.key().as_str())
            .upload_id(handle.upload_id())
            .part_number(number)
            .copy_source(copy_source(self.bucket(), src))
            .copy_source_range(header)
            .send()
            .await
            .map_err(|error| classify(OP, error))?;
        let etag = output.copy_part_result().and_then(|result| result.e_tag());
        Ok(UploadedPart {
            part_number,
            etag: required_etag(OP, etag)?,
            size: len,
        })
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartHandle,
        parts: &[UploadedPart],
    ) -> Result<ObjectStat, StorageError> {
        self.complete_upload(UploadTarget::of(handle), parts).await
    }

    async fn abort_multipart(&self, handle: &MultipartHandle) -> Result<(), StorageError> {
        self.abort_upload(UploadTarget::of(handle)).await
    }

    async fn list_multipart_uploads(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
    ) -> Result<MultipartUploadPage, StorageError> {
        const OP: Operation = Operation::ListMultipartUploads;
        let (key_marker, upload_id_marker) = match cursor {
            Some(cursor) => decode_cursor(&cursor)
                .ok_or_else(|| invalid(OP, "the cursor is not a multipart listing cursor"))?,
            None => (None, None),
        };
        let output = self
            .internal()
            .list_multipart_uploads()
            .bucket(self.bucket())
            .prefix(prefix)
            .max_uploads(provider_page())
            .set_key_marker(key_marker)
            .set_upload_id_marker(upload_id_marker)
            .send()
            .await
            .map_err(|error| classify(OP, error))?;

        let mut uploads = Vec::with_capacity(output.uploads().len());
        for upload in output.uploads() {
            let Some(key) = upload.key().and_then(|key| ObjectKey::parse(key).ok()) else {
                continue;
            };
            let upload_id = upload
                .upload_id()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| malformed(OP, "an upload has no upload id"))?;
            let initiated_at = upload
                .initiated()
                .and_then(|at| OffsetDateTime::from_unix_timestamp_nanos(at.as_nanos()).ok())
                .ok_or_else(|| malformed(OP, "an upload has no initiation time"))?;
            uploads.push(PendingMultipart {
                handle: MultipartHandle::new(key, upload_id),
                initiated_at,
            });
        }

        let next = match output.is_truncated() {
            Some(true) => {
                let key_marker = output
                    .next_key_marker()
                    .filter(|marker| !marker.is_empty())
                    .ok_or_else(|| malformed(OP, "a truncated page has no key marker"))?;
                Some(encode_cursor(key_marker, output.next_upload_id_marker()))
            }
            _ => None,
        };
        Ok(MultipartUploadPage { uploads, next })
    }
}

#[derive(Clone, Copy)]
pub(super) struct UploadTarget<'a> {
    key: &'a dyn StoredKey,
    upload_id: &'a str,
}

impl<'a> UploadTarget<'a> {
    pub(super) fn of(handle: &'a MultipartHandle) -> Self {
        Self {
            key: handle.key(),
            upload_id: handle.upload_id(),
        }
    }

    pub(super) fn probe(key: &'a ProbeKey, upload_id: &'a str) -> Self {
        Self { key, upload_id }
    }

    pub(super) fn key(&self) -> &'a dyn StoredKey {
        self.key
    }

    pub(super) const fn upload_id(&self) -> &'a str {
        self.upload_id
    }
}

impl S3Provider {
    pub(super) async fn create_upload(
        &self,
        key: &(impl StoredKey + ?Sized),
        hint: PutHint,
    ) -> Result<String, StorageError> {
        const OP: Operation = Operation::CreateMultipartUpload;
        if hint
            .declared_len
            .is_some_and(|len| len > self.limits().max_object)
        {
            return Err(invalid(
                OP,
                "the declared length exceeds the provider object ceiling",
            ));
        }
        let output = self
            .internal()
            .create_multipart_upload()
            .bucket(self.bucket())
            .key(key.stored_key())
            .set_content_type(hint.content_type)
            .send()
            .await
            .map_err(|error| classify(OP, error))?;
        output
            .upload_id()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| malformed(OP, "the response carries no upload id"))
    }

    pub(super) async fn upload_part_body(
        &self,
        target: UploadTarget<'_>,
        part_number: u32,
        len: u64,
        body: ObjectBody,
    ) -> Result<UploadedPart, StorageError> {
        const OP: Operation = Operation::UploadPart;
        let limits = self.limits();
        let number = provider_part_number(OP, part_number, &limits)?;
        let content_length = provider_part_len(OP, len, &limits)?;
        let sized = SizedBody::new(body, len, self.buffer_bytes);
        let output = self
            .internal()
            .upload_part()
            .bucket(self.bucket())
            .key(target.key().stored_key())
            .upload_id(target.upload_id())
            .part_number(number)
            .content_length(content_length)
            .body(ByteStream::new(SdkBody::from_body_1_x(sized)))
            .send()
            .await
            .map_err(|error| classify(OP, error))?;
        Ok(UploadedPart {
            part_number,
            etag: required_etag(OP, output.e_tag())?,
            size: len,
        })
    }

    pub(super) async fn complete_upload(
        &self,
        target: UploadTarget<'_>,
        parts: &[UploadedPart],
    ) -> Result<ObjectStat, StorageError> {
        const OP: Operation = Operation::CompleteMultipartUpload;
        let manifest = completion_manifest(parts, &self.limits())?;
        let output = self
            .internal()
            .complete_multipart_upload()
            .bucket(self.bucket())
            .key(target.key().stored_key())
            .upload_id(target.upload_id())
            .multipart_upload(manifest)
            .send()
            .await
            .map_err(|error| classify(OP, error))?;
        required_etag(OP, output.e_tag())?;
        self.head_object(target.key()).await
    }

    pub(super) async fn abort_upload(&self, target: UploadTarget<'_>) -> Result<(), StorageError> {
        let outcome = self
            .internal()
            .abort_multipart_upload()
            .bucket(self.bucket())
            .key(target.key().stored_key())
            .upload_id(target.upload_id())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| classify(Operation::AbortMultipartUpload, error));
        not_found_is_deleted(outcome)
    }
}

pub(super) fn invalid(operation: Operation, detail: &'static str) -> StorageError {
    classify_failure(operation, Failure::InvalidRequest(detail))
}

pub(super) fn provider_page() -> i32 {
    i32::try_from(MAX_LIST_PAGE_SIZE).unwrap_or(i32::MAX)
}

pub(super) fn provider_part_number(
    operation: Operation,
    part_number: u32,
    limits: &ProfileLimits,
) -> Result<i32, StorageError> {
    i32::try_from(part_number)
        .ok()
        .filter(|_| (1..=limits.max_parts).contains(&part_number))
        .ok_or_else(|| invalid(operation, "the part number is outside the provider range"))
}

pub(super) fn provider_part_len(
    operation: Operation,
    len: u64,
    limits: &ProfileLimits,
) -> Result<i64, StorageError> {
    i64::try_from(len)
        .ok()
        .filter(|_| (1..=limits.max_part).contains(&len))
        .ok_or_else(|| invalid(operation, "the part length is outside the provider range"))
}

pub(super) fn copy_source_range(range: &RangeInclusive<u64>) -> Option<(String, u64)> {
    let (start, end) = (*range.start(), *range.end());
    let len = end.checked_sub(start)?.checked_add(1)?;
    Some((format!("bytes={start}-{end}"), len))
}

pub(super) fn completion_manifest(
    parts: &[UploadedPart],
    limits: &ProfileLimits,
) -> Result<CompletedMultipartUpload, StorageError> {
    const OP: Operation = Operation::CompleteMultipartUpload;
    if parts.is_empty() {
        return Err(invalid(OP, "the completion manifest is empty"));
    }
    let mut completed = Vec::with_capacity(parts.len());
    let mut previous = 0_u32;
    for part in parts {
        if part.part_number <= previous {
            return Err(invalid(
                OP,
                "the completion manifest is not strictly ascending",
            ));
        }
        previous = part.part_number;
        let number = provider_part_number(OP, part.part_number, limits)?;
        if !valid_etag(part.etag.as_str()) {
            return Err(invalid(OP, "a completion part has no valid ETag"));
        }
        completed.push(
            CompletedPart::builder()
                .part_number(number)
                .e_tag(part.etag.as_str())
                .build(),
        );
    }
    Ok(CompletedMultipartUpload::builder()
        .set_parts(Some(completed))
        .build())
}

fn valid_etag(etag: &str) -> bool {
    !etag.is_empty() && !etag.chars().any(char::is_control)
}

fn required_etag(operation: Operation, etag: Option<&str>) -> Result<ETag, StorageError> {
    etag.filter(|etag| valid_etag(etag))
        .map(ETag::new)
        .ok_or_else(|| malformed(operation, "the response carries no ETag"))
}

fn listed_part(part: &Part) -> Result<UploadedPart, StorageError> {
    const OP: Operation = Operation::ListParts;
    let part_number = part
        .part_number()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0)
        .ok_or_else(|| malformed(OP, "a part has no valid part number"))?;
    let etag = required_etag(OP, part.e_tag())?;
    let size = part
        .size()
        .and_then(|size| u64::try_from(size).ok())
        .ok_or_else(|| malformed(OP, "a part has no valid size"))?;
    Ok(UploadedPart {
        part_number,
        etag,
        size,
    })
}

pub(super) fn encode_cursor(key_marker: &str, upload_id_marker: Option<&str>) -> ListCursor {
    ListCursor::new(format!(
        "{key_marker}{CURSOR_SEPARATOR}{}",
        upload_id_marker.unwrap_or_default()
    ))
}

pub(super) fn decode_cursor(cursor: &ListCursor) -> Option<(Option<String>, Option<String>)> {
    let (key_marker, upload_id_marker) = cursor.as_str().split_once(CURSOR_SEPARATOR)?;
    if key_marker.is_empty() {
        return None;
    }
    let upload_id_marker = (!upload_id_marker.is_empty()).then(|| upload_id_marker.to_owned());
    Some((Some(key_marker.to_owned()), upload_id_marker))
}
