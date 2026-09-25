use std::fmt;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use http::{HeaderMap, HeaderValue, Method};
use time::OffsetDateTime;
use tokio::io::AsyncRead;
use url::Url;

use super::caps::StorageCapabilities;
use super::error::StorageError;
use super::health::SelfTestReport;
use super::key::ObjectKey;
use super::ProviderKind;

pub type ObjectBody = Pin<Box<dyn AsyncRead + Send>>;

pub const MAX_LIST_PAGE_SIZE: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ETag(String);

impl ETag {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStat {
    pub size: u64,
    pub modified_at: OffsetDateTime,
    pub etag: Option<ETag>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PutHint {
    pub declared_len: Option<u64>,
    pub content_type: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ListCursor(String);

impl fmt::Debug for ListCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ListCursor(<opaque>)")
    }
}

impl ListCursor {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub key: String,
    pub size: u64,
    pub modified_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub entries: Vec<ListEntry>,
    pub next: Option<ListCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capacity {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityReport {
    Available(Capacity),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStorageDescriptor {
    pub capacity: CapacityReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageDescriptor {
    pub provider: ProviderKind,
    pub local: Option<LocalStorageDescriptor>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MultipartHandle {
    key: ObjectKey,
    upload_id: String,
}

impl MultipartHandle {
    pub fn new(key: ObjectKey, upload_id: impl Into<String>) -> Self {
        Self {
            key,
            upload_id: upload_id.into(),
        }
    }

    pub const fn key(&self) -> &ObjectKey {
        &self.key
    }

    pub fn upload_id(&self) -> &str {
        &self.upload_id
    }
}

impl fmt::Debug for MultipartHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultipartHandle")
            .field("upload_id", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartPlanEntry {
    pub part_number: u32,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedPart {
    pub part_number: u32,
    pub etag: ETag,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMultipart {
    pub handle: MultipartHandle,
    pub initiated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartUploadPage {
    pub uploads: Vec<PendingMultipart>,
    pub next: Option<ListCursor>,
}

pub struct PresignedRequest {
    method: Method,
    url: Url,
    headers: HeaderMap,
    expires_at: OffsetDateTime,
}

impl PresignedRequest {
    pub const fn new(
        method: Method,
        url: Url,
        headers: HeaderMap,
        expires_at: OffsetDateTime,
    ) -> Self {
        Self {
            method,
            url,
            headers,
            expires_at,
        }
    }

    pub const fn method(&self) -> &Method {
        &self.method
    }

    pub const fn url(&self) -> &Url {
        &self.url
    }

    pub const fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }
}

impl fmt::Debug for PresignedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PresignedRequest")
            .field("method", &self.method)
            .field("url", &"<redacted>")
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

pub struct GrantContext {
    _sealed: (),
}

impl fmt::Debug for GrantContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GrantContext")
    }
}

#[async_trait]
pub trait StorageProvider: Send + Sync + 'static {
    fn caps(&self) -> &StorageCapabilities;

    fn describe(&self) -> StorageDescriptor;

    async fn put_stream(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError>;

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError>;

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError>;

    async fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError>;

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError>;

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError>;

    async fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError>;

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError>;

    async fn self_test(&self) -> Result<SelfTestReport, StorageError>;

    fn as_multipart(&self) -> Option<&dyn MultipartStorage> {
        None
    }

    fn as_presign(&self) -> Option<&dyn PresignStorage> {
        None
    }
}

#[async_trait]
pub trait MultipartStorage: Send + Sync {
    async fn create_multipart(
        &self,
        key: &ObjectKey,
        hint: PutHint,
    ) -> Result<MultipartHandle, StorageError>;

    async fn sign_part_urls(
        &self,
        handle: &MultipartHandle,
        parts: &[PartPlanEntry],
        ttl: Duration,
    ) -> Result<Vec<PresignedRequest>, StorageError>;

    async fn list_parts(&self, handle: &MultipartHandle)
        -> Result<Vec<UploadedPart>, StorageError>;

    async fn upload_part_stream(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        len: u64,
        body: ObjectBody,
    ) -> Result<UploadedPart, StorageError>;

    async fn upload_part_copy(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        src: &ObjectKey,
        range: RangeInclusive<u64>,
    ) -> Result<UploadedPart, StorageError>;

    async fn complete_multipart(
        &self,
        handle: &MultipartHandle,
        parts: &[UploadedPart],
    ) -> Result<ObjectStat, StorageError>;

    async fn abort_multipart(&self, handle: &MultipartHandle) -> Result<(), StorageError>;

    async fn list_multipart_uploads(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
    ) -> Result<MultipartUploadPage, StorageError>;
}

#[async_trait]
pub trait PresignStorage: Send + Sync {
    async fn presign_get(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        disposition: &HeaderValue,
        content_type: &str,
        authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError>;

    async fn presign_put(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError>;
}

#[cfg(test)]
impl GrantContext {
    pub(in crate::storage) const fn authorized_for_test() -> Self {
        Self { _sealed: () }
    }
}

#[cfg(test)]
mod tests;
