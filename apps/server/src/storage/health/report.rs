use std::time::Duration;

use time::OffsetDateTime;

use crate::storage::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeDepth {
    Full,
    Light,
}

impl ProbeDepth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckScope {
    Core,
    Capability,
    Housekeeping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckName {
    Layout,
    HeadBucket,
    Write,
    Stat,
    Read,
    Range,
    PresignedGet,
    PresignedPart,
    CorsPreflight,
    Multipart,
    ListMultipartUploads,
    Delete,
    Absent,
    Cleanup,
}

impl CheckName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::HeadBucket => "head_bucket",
            Self::Write => "write",
            Self::Stat => "stat",
            Self::Read => "read",
            Self::Range => "range",
            Self::PresignedGet => "presigned_get",
            Self::PresignedPart => "presigned_part",
            Self::CorsPreflight => "cors_preflight",
            Self::Multipart => "multipart",
            Self::ListMultipartUploads => "list_multipart_uploads",
            Self::Delete => "delete",
            Self::Absent => "absent",
            Self::Cleanup => "cleanup",
        }
    }

    pub const fn scope(self) -> CheckScope {
        match self {
            Self::Layout
            | Self::HeadBucket
            | Self::Write
            | Self::Stat
            | Self::Read
            | Self::Range
            | Self::Delete
            | Self::Absent => CheckScope::Core,
            Self::PresignedGet
            | Self::PresignedPart
            | Self::CorsPreflight
            | Self::Multipart
            | Self::ListMultipartUploads => CheckScope::Capability,
            Self::Cleanup => CheckScope::Housekeeping,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckStatus {
    Passed,
    Info,
    Warning,
    Failed,
    Skipped,
}

impl CheckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    Transient,
    NonTransient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Diagnosis {
    Unreachable,
    PermissionDenied,
    NotWritable,
    StorageFull,
    BucketMissing,
    ClockSkew,
    SizeMismatch,
    ContentMismatch,
    RangeMismatch,
    DeleteIneffective,
    ProviderError,
    Config,
    AddressingStyleMismatch,
    PresignRejected,
    PublicEndpointUnreachable,
    CorsPreflightRejected,
    CorsOriginMismatch,
    CorsWildcardOrigin,
    CorsMethodsMissing,
    CorsMissingEtag,
    MultipartRejected,
    ListMultipartUploadsUnsupported,
    ListMultipartUploadsUnverified,
    CleanupFailed,
    StorageReplaced,
    ProviderMismatch,
}

impl Diagnosis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::PermissionDenied => "permission_denied",
            Self::NotWritable => "not_writable",
            Self::StorageFull => "storage_full",
            Self::BucketMissing => "bucket_missing",
            Self::ClockSkew => "clock_skew",
            Self::SizeMismatch => "size_mismatch",
            Self::ContentMismatch => "content_mismatch",
            Self::RangeMismatch => "range_mismatch",
            Self::DeleteIneffective => "delete_ineffective",
            Self::ProviderError => "provider_error",
            Self::Config => "config",
            Self::AddressingStyleMismatch => "addressing_style_mismatch",
            Self::PresignRejected => "presign_rejected",
            Self::PublicEndpointUnreachable => "public_endpoint_unreachable",
            Self::CorsPreflightRejected => "cors_preflight_rejected",
            Self::CorsOriginMismatch => "cors_origin_mismatch",
            Self::CorsWildcardOrigin => "cors_wildcard_origin",
            Self::CorsMethodsMissing => "cors_methods_missing",
            Self::CorsMissingEtag => "cors_missing_etag",
            Self::MultipartRejected => "multipart_rejected",
            Self::ListMultipartUploadsUnsupported => "list_multipart_uploads_unsupported",
            Self::ListMultipartUploadsUnverified => "list_multipart_uploads_unverified",
            Self::CleanupFailed => "cleanup_failed",
            Self::StorageReplaced => "storage_replaced",
            Self::ProviderMismatch => "provider_mismatch",
        }
    }

    pub const fn class(self) -> FailureClass {
        match self {
            Self::PermissionDenied | Self::NotWritable | Self::Config | Self::ProviderMismatch => {
                FailureClass::NonTransient
            }
            _ => FailureClass::Transient,
        }
    }

    pub const fn remediation(self) -> &'static str {
        match self {
            Self::Unreachable => "The storage backend did not answer. Check that it is running and reachable from the Palmr container at the configured endpoint.",
            Self::PermissionDenied => "The storage backend refused the operation. Check the credentials and that they may read, write and delete objects in the bucket, or the ownership of the data directory.",
            Self::NotWritable => "The storage directory is read-only. Remount the volume read-write.",
            Self::StorageFull => "The storage device is out of space or quota. Free space or enlarge the volume.",
            Self::BucketMissing => "The configured bucket does not exist. Create it or correct PALMR_S3_BUCKET.",
            Self::ClockSkew => "The Palmr host clock differs from the storage provider's clock by more than its tolerance. Enable NTP on the host; containers inherit the host clock.",
            Self::SizeMismatch | Self::ContentMismatch => "The probe object came back with different bytes than were written. Check for a proxy or gateway rewriting object bodies.",
            Self::RangeMismatch => "The storage path does not honour byte ranges. Check for a proxy that strips or rewrites the Range header.",
            Self::DeleteIneffective => "A deleted probe object is still reported as present. Check bucket versioning, object lock or retention settings.",
            Self::ProviderError => "The storage backend returned an unexpected error. Check the provider logs.",
            Self::Config => "The storage configuration is invalid. Correct the PALMR_STORAGE_* or PALMR_S3_* settings and restart.",
            Self::AddressingStyleMismatch => "The public endpoint rejected a presigned request signature. Check PALMR_S3_FORCE_PATH_STYLE and that PALMR_S3_PUBLIC_ENDPOINT reaches the bucket without rewriting the host or path.",
            Self::PresignRejected => "The public endpoint rejected a presigned request. Check PALMR_S3_PUBLIC_ENDPOINT and any proxy in front of the object store.",
            Self::PublicEndpointUnreachable => "The Palmr server cannot reach the public S3 endpoint. Browsers may still reach it; verify from a browser on the deployment's network.",
            Self::CorsPreflightRejected => "The bucket refused a browser CORS preflight. Apply the recommended bucket CORS document.",
            Self::CorsOriginMismatch => "The bucket CORS rule does not allow the Palmr origin. Add the PALMR_BASE_URL origin to AllowedOrigins.",
            Self::CorsWildcardOrigin => "The bucket CORS rule allows every origin. Restrict AllowedOrigins to the PALMR_BASE_URL origin.",
            Self::CorsMethodsMissing => "The bucket CORS rule does not allow both GET and PUT. Apply the recommended bucket CORS document.",
            Self::CorsMissingEtag => "The bucket CORS rule does not expose the ETag header, so browsers cannot complete multipart uploads. Add ETag to ExposeHeaders.",
            Self::MultipartRejected => "The provider did not complete a three-part multipart upload. Check that the selected PALMR_S3_PROFILE matches the provider.",
            Self::ListMultipartUploadsUnsupported => "The provider does not list in-progress multipart uploads; abandoned uploads are aborted from Palmr's own records only.",
            Self::ListMultipartUploadsUnverified => "The provider's multipart upload listing omitted an upload in progress; abandoned uploads are aborted from Palmr's own records only.",
            Self::CleanupFailed => "Old self-test probe objects could not be removed; they are retried on the next full self-test.",
            Self::StorageReplaced => "Storage appears to have been replaced: most sampled objects are missing. Do NOT enable orphan reaping. Verify that the correct volume or bucket is mounted.",
            Self::ProviderMismatch => "Stored objects belong to a different storage provider than the configured one. Restore the previous PALMR_STORAGE_PROVIDER or migrate the objects while Palmr is stopped.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubCheck {
    pub name: CheckName,
    pub status: CheckStatus,
    pub duration: Duration,
    pub diagnosis: Option<Diagnosis>,
}

impl SubCheck {
    pub const fn ok(&self) -> bool {
        matches!(
            self.status,
            CheckStatus::Passed | CheckStatus::Info | CheckStatus::Warning
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fact {
    RequiresChecksumHeaders,
    ListMultipartUploads,
    MultipartCompletion,
    BucketCors,
    AddressingStyle,
}

impl Fact {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequiresChecksumHeaders => "requires_checksum_headers",
            Self::ListMultipartUploads => "list_multipart_uploads",
            Self::MultipartCompletion => "multipart_completion",
            Self::BucketCors => "bucket_cors",
            Self::AddressingStyle => "addressing_style",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactReport {
    pub fact: Fact,
    pub assumed: Option<bool>,
    pub verified: Option<bool>,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTestResult {
    Passed,
    Degraded,
    Failed,
}

impl SelfTestResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTestReport {
    pub ran_at: OffsetDateTime,
    pub depth: ProbeDepth,
    pub provider: ProviderKind,
    pub duration: Duration,
    pub checks: Vec<SubCheck>,
    pub facts: Vec<FactReport>,
}

impl SelfTestReport {
    pub fn result(&self) -> SelfTestResult {
        if self.core_failure().is_some() {
            SelfTestResult::Failed
        } else if self.degradations().next().is_some() {
            SelfTestResult::Degraded
        } else {
            SelfTestResult::Passed
        }
    }

    pub fn core_failure(&self) -> Option<Diagnosis> {
        self.checks
            .iter()
            .filter(|check| check.name.scope() == CheckScope::Core)
            .find(|check| check.status == CheckStatus::Failed)
            .map(|check| check.diagnosis.unwrap_or(Diagnosis::ProviderError))
    }

    pub fn degradations(&self) -> impl Iterator<Item = Diagnosis> + '_ {
        self.checks
            .iter()
            .filter(|check| {
                check.name.scope() == CheckScope::Capability && check.status == CheckStatus::Failed
            })
            .filter_map(|check| check.diagnosis)
    }

    pub fn capabilities_verified(&self) -> bool {
        self.depth == ProbeDepth::Full && self.core_failure().is_none()
    }

    pub fn check(&self, name: CheckName) -> Option<&SubCheck> {
        self.checks.iter().find(|check| check.name == name)
    }

    pub fn fact(&self, fact: Fact) -> Option<&FactReport> {
        self.facts.iter().find(|report| report.fact == fact)
    }
}
