use http::StatusCode;

use crate::domain::error_code::ErrorCode;
use crate::storage::error::StorageError;
use crate::storage::s3::{provider_fault, ProviderFault};

use super::error::TransferError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RetryClass {
    Retryable,
    NeedsReconciliation,
    Terminal,
    Fatal,
}

impl RetryClass {
    pub const ALL: [Self; 4] = [
        Self::Retryable,
        Self::NeedsReconciliation,
        Self::Terminal,
        Self::Fatal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::NeedsReconciliation => "needs-reconciliation",
            Self::Terminal => "terminal",
            Self::Fatal => "fatal",
        }
    }

    pub const fn backs_off(self) -> bool {
        matches!(self, Self::Retryable)
    }

    pub const fn reconciles_first(self) -> bool {
        matches!(self, Self::NeedsReconciliation)
    }

    pub const fn is_final(self) -> bool {
        matches!(self, Self::Terminal | Self::Fatal)
    }

    pub const fn for_code(code: ErrorCode) -> Self {
        match code {
            ErrorCode::UploadOffsetMismatch
            | ErrorCode::UploadChecksumMismatch
            | ErrorCode::MultipartIncomplete
            | ErrorCode::MultipartAlreadyCreated
            | ErrorCode::MultipartPartRejected => Self::NeedsReconciliation,
            ErrorCode::TransferSessionNotFound
            | ErrorCode::TransferSessionStateInvalid
            | ErrorCode::TransferSessionExpired
            | ErrorCode::UploadSessionExpired
            | ErrorCode::MultipartNotFound
            | ErrorCode::MultipartAlreadyCompleted => Self::Terminal,
            ErrorCode::UploadLocked
            | ErrorCode::TransferIdleTimeout
            | ErrorCode::TransferBackpressure
            | ErrorCode::StorageWriteFailed
            | ErrorCode::StorageFull
            | ErrorCode::StorageUnavailable
            | ErrorCode::StorageUpstreamError
            | ErrorCode::DatabaseBusy
            | ErrorCode::ServiceUnavailable
            | ErrorCode::RateLimited
            | ErrorCode::IdempotencyRequestInProgress => Self::Retryable,
            ErrorCode::InternalError => Self::Fatal,
            other => {
                if other.retryable() {
                    Self::Retryable
                } else {
                    Self::Fatal
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classification {
    pub code: ErrorCode,
    class: RetryClass,
}

impl Classification {
    pub const fn of(code: ErrorCode) -> Self {
        Self {
            code,
            class: RetryClass::for_code(code),
        }
    }

    pub const fn class(self) -> RetryClass {
        self.class
    }

    pub const fn status(self) -> StatusCode {
        self.code.status()
    }

    pub const fn is_protocol_negotiation(self) -> bool {
        is_protocol_negotiation(self.code)
    }

    pub const fn is_waiting(self) -> bool {
        matches!(self.code, ErrorCode::TransferBackpressure)
    }

    pub const fn records_item_failure(self) -> bool {
        !self.is_protocol_negotiation() && !self.is_waiting()
    }
}

pub const fn is_protocol_negotiation(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::UploadOffsetMismatch | ErrorCode::UploadLocked
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageSite {
    Stream,
    Place,
    S3Control,
    S3Multipart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteFault {
    DeviceFull,
    PermissionDenied,
    Io,
}

impl From<&StorageError> for WriteFault {
    fn from(error: &StorageError) -> Self {
        match error {
            StorageError::QuotaOnDevice => Self::DeviceFull,
            StorageError::PermissionDenied => Self::PermissionDenied,
            _ => Self::Io,
        }
    }
}

pub const fn classify_write(fault: WriteFault) -> Classification {
    Classification::of(match fault {
        WriteFault::DeviceFull => ErrorCode::StorageWriteFailed,
        WriteFault::PermissionDenied => ErrorCode::StoragePermissionDenied,
        WriteFault::Io => ErrorCode::StorageUnavailable,
    })
}

pub fn classify_storage(error: &StorageError, site: StorageSite) -> Classification {
    Classification::of(match error {
        StorageError::NotFound => match site {
            StorageSite::S3Multipart => ErrorCode::MultipartNotFound,
            StorageSite::S3Control => ErrorCode::StorageUpstreamError,
            StorageSite::Stream | StorageSite::Place => ErrorCode::FileNotFound,
        },
        StorageError::RangeNotSatisfiable { .. } => ErrorCode::RangeNotSatisfiable,
        StorageError::AlreadyExists
        | StorageError::InvalidKey
        | StorageError::Config(_)
        | StorageError::Canceled => ErrorCode::InternalError,
        StorageError::QuotaOnDevice => match site {
            StorageSite::Stream => ErrorCode::StorageWriteFailed,
            StorageSite::Place | StorageSite::S3Control | StorageSite::S3Multipart => {
                ErrorCode::StorageFull
            }
        },
        StorageError::PermissionDenied => ErrorCode::StoragePermissionDenied,
        StorageError::ProviderUnavailable(_) => match provider_fault(error) {
            Some(ProviderFault::Timeout | ProviderFault::Upstream) => {
                ErrorCode::StorageUpstreamError
            }
            Some(ProviderFault::Unreachable) | None => ErrorCode::StorageUnavailable,
        },
        StorageError::Io(_) => ErrorCode::StorageUnavailable,
        StorageError::S3(_) => ErrorCode::StorageUpstreamError,
        StorageError::ProviderMismatch { .. } => ErrorCode::StorageProviderMismatch,
        StorageError::SizeMismatch { .. } => ErrorCode::StorageSizeMismatch,
    })
}

impl TransferError {
    pub fn from_multipart_storage(error: StorageError) -> Self {
        match classify_storage(&error, StorageSite::S3Multipart).code {
            ErrorCode::MultipartNotFound => Self::MultipartNotFound,
            _ => Self::Storage(error),
        }
    }

    pub fn classification(&self) -> Classification {
        match self {
            Self::Storage(error) => classify_storage(error, StorageSite::S3Control),
            Self::Recorded(code) => Classification::of(*code),
            Self::CompletionBusy => Classification::of(ErrorCode::TransferBackpressure),
            Self::StorageUnavailable => Classification::of(ErrorCode::StorageUnavailable),
            Self::Lifecycle(_) | Self::Invariant { .. } | Self::Time(_) => {
                Classification::of(ErrorCode::InternalError)
            }
            other => Classification::of(other.api_error().code()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;

    use serde_json::Value;

    use super::{
        classify_storage, classify_write, is_protocol_negotiation, Classification, RetryClass,
        StorageSite, WriteFault,
    };
    use crate::domain::bytes::ByteSize;
    use crate::domain::error_code::ErrorCode;
    use crate::domain::naming::CandidateError;
    use crate::features::folders::FolderError;
    use crate::features::quota::error::{Exceeded, QuotaError};
    use crate::features::transfers::error::{FileTooLarge, TooLargeReason, TransferError};
    use crate::features::transfers::finalize::RejectCause;
    use crate::features::transfers::tus::error::TusError;
    use crate::infra::db::DbError;
    use crate::infra::http::error::{ApiError, ApiErrorBody};
    use crate::storage::error::{Retryable, StorageError};
    use crate::storage::s3::{classify_failure, Failure, Operation};

    const SENTINEL: &str = "palmr-classification-leak-sentinel";

    struct Row {
        label: &'static str,
        classification: Classification,
        api: ApiError,
        status: u16,
        code: &'static str,
        class: RetryClass,
    }

    fn row(
        label: &'static str,
        classification: Classification,
        api: ApiError,
        status: u16,
        code: &'static str,
        class: RetryClass,
    ) -> Row {
        Row {
            label,
            classification,
            api,
            status,
            code,
            class,
        }
    }

    fn tus(
        label: &'static str,
        error: &TusError,
        status: u16,
        code: &'static str,
        class: RetryClass,
    ) -> Row {
        row(
            label,
            error.classification(),
            error.api_error(),
            status,
            code,
            class,
        )
    }

    fn transfer(
        label: &'static str,
        error: &TransferError,
        status: u16,
        code: &'static str,
        class: RetryClass,
    ) -> Row {
        row(
            label,
            error.classification(),
            error.api_error(),
            status,
            code,
            class,
        )
    }

    fn storage(
        label: &'static str,
        error: &StorageError,
        site: StorageSite,
        status: u16,
        code: &'static str,
        class: RetryClass,
    ) -> Row {
        let classification = classify_storage(error, site);
        row(
            label,
            classification,
            ApiError::new(classification.code),
            status,
            code,
            class,
        )
    }

    fn rejection(
        label: &'static str,
        cause: RejectCause,
        status: u16,
        code: &'static str,
        class: RetryClass,
    ) -> Row {
        let classification = Classification::of(cause.code());
        row(
            label,
            classification,
            ApiError::new(cause.code()),
            status,
            code,
            class,
        )
    }

    fn code_only(
        label: &'static str,
        code: ErrorCode,
        status: u16,
        wire: &'static str,
        class: RetryClass,
    ) -> Row {
        row(
            label,
            Classification::of(code),
            ApiError::new(code),
            status,
            wire,
            class,
        )
    }

    fn provider(operation: Operation, failure: Failure) -> StorageError {
        classify_failure(operation, failure)
    }

    fn too_large(reason: TooLargeReason) -> TransferError {
        TransferError::FileTooLarge(FileTooLarge {
            client_key: crate::domain::client_key::ClientFileKey::parse("k").unwrap(),
            declared: ByteSize::try_from(1_u64).unwrap(),
            max: None,
            provider_max: None,
            reason,
        })
    }

    fn quota_exceeded() -> TransferError {
        TransferError::Quota(QuotaError::Exceeded(Exceeded {
            used: ByteSize::ZERO,
            held: ByteSize::ZERO,
            requested: ByteSize::try_from(1_u64).unwrap(),
            quota: Some(ByteSize::ZERO),
        }))
    }

    fn table() -> Vec<Row> {
        use RetryClass::{Fatal, NeedsReconciliation, Retryable, Terminal};
        let service_5xx = provider(
            Operation::UploadPart,
            Failure::Service {
                status: 503,
                code: Some("SlowDown".to_owned()),
            },
        );
        let timeout = provider(Operation::CompleteMultipartUpload, Failure::Timeout);
        let unreachable = provider(Operation::HeadObject, Failure::Dispatch);
        let denied = provider(
            Operation::ListParts,
            Failure::Service {
                status: 403,
                code: Some("AccessDenied".to_owned()),
            },
        );
        let no_such_upload = provider(
            Operation::ListParts,
            Failure::Service {
                status: 404,
                code: Some("NoSuchUpload".to_owned()),
            },
        );
        let rejected = provider(
            Operation::CompleteMultipartUpload,
            Failure::Service {
                status: 400,
                code: Some("InvalidArgument".to_owned()),
            },
        );
        vec![
            tus(
                "tus version",
                &TusError::VersionUnsupported,
                412,
                "TUS_VERSION_UNSUPPORTED",
                Fatal,
            ),
            tus(
                "tus concat",
                &TusError::ExtensionUnsupported,
                501,
                "TUS_EXTENSION_UNSUPPORTED",
                Fatal,
            ),
            tus(
                "tus metadata",
                &TusError::Header {
                    key: "Upload-Metadata",
                },
                400,
                "UPLOAD_METADATA_INVALID",
                Fatal,
            ),
            tus(
                "tus body over length",
                &TusError::Overrun {
                    declared: ByteSize::ZERO,
                },
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            tus(
                "tus length over max",
                &TusError::TooLarge { max: None },
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            tus(
                "tus length out of range",
                &TusError::LengthBeyondRange,
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            tus(
                "tus media type",
                &TusError::UnsupportedMedia,
                415,
                "UNSUPPORTED_MEDIA_TYPE",
                Fatal,
            ),
            tus(
                "tus offset mismatch",
                &TusError::OffsetMismatch { expected: 7 },
                409,
                "UPLOAD_OFFSET_MISMATCH",
                NeedsReconciliation,
            ),
            tus(
                "tus locked",
                &TusError::Locked,
                423,
                "UPLOAD_LOCKED",
                Retryable,
            ),
            tus(
                "tus expired",
                &TusError::Gone,
                410,
                "UPLOAD_SESSION_EXPIRED",
                Terminal,
            ),
            tus(
                "tus checksum",
                &TusError::ChecksumMismatch { offset: 0 },
                460,
                "UPLOAD_CHECKSUM_MISMATCH",
                NeedsReconciliation,
            ),
            tus(
                "tus ENOSPC stream",
                &TusError::WriteFailed(WriteFault::DeviceFull),
                507,
                "STORAGE_WRITE_FAILED",
                Retryable,
            ),
            tus(
                "tus EACCES stream",
                &TusError::WriteFailed(WriteFault::PermissionDenied),
                500,
                "STORAGE_PERMISSION_DENIED",
                Fatal,
            ),
            tus(
                "tus EIO stream",
                &TusError::WriteFailed(WriteFault::Io),
                503,
                "STORAGE_UNAVAILABLE",
                Retryable,
            ),
            tus(
                "tus idle",
                &TusError::IdleTimeout,
                408,
                "TRANSFER_IDLE_TIMEOUT",
                Retryable,
            ),
            tus(
                "tus backpressure",
                &TusError::Backpressure { retry_after: 1 },
                503,
                "TRANSFER_BACKPRESSURE",
                Retryable,
            ),
            tus(
                "tus length mismatch",
                &TusError::LengthMismatch {
                    requested: None,
                    planned: None,
                },
                422,
                "UPLOAD_LENGTH_MISMATCH",
                Fatal,
            ),
            tus(
                "tus quota",
                &TusError::from(quota_exceeded_quota()),
                507,
                "QUOTA_EXCEEDED",
                Fatal,
            ),
            tus(
                "tus storage errno ENOSPC",
                &TusError::Storage(StorageError::QuotaOnDevice),
                507,
                "STORAGE_WRITE_FAILED",
                Retryable,
            ),
            storage(
                "place ENOSPC copy",
                &StorageError::QuotaOnDevice,
                StorageSite::Place,
                507,
                "STORAGE_FULL",
                Retryable,
            ),
            storage(
                "place EACCES",
                &StorageError::PermissionDenied,
                StorageSite::Place,
                500,
                "STORAGE_PERMISSION_DENIED",
                Fatal,
            ),
            storage(
                "place EIO",
                &StorageError::Io(io::Error::other(SENTINEL)),
                StorageSite::Place,
                503,
                "STORAGE_UNAVAILABLE",
                Retryable,
            ),
            transfer(
                "s3 plan not computable",
                &too_large(TooLargeReason::PartCount),
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            transfer(
                "s3 multipart exists",
                &TransferError::MultipartAlreadyCreated,
                409,
                "MULTIPART_ALREADY_CREATED",
                NeedsReconciliation,
            ),
            transfer(
                "s3 multipart unknown",
                &TransferError::MultipartNotFound,
                404,
                "MULTIPART_NOT_FOUND",
                Terminal,
            ),
            storage(
                "s3 NoSuchUpload",
                &no_such_upload,
                StorageSite::S3Multipart,
                404,
                "MULTIPART_NOT_FOUND",
                Terminal,
            ),
            transfer(
                "s3 missing parts",
                &TransferError::MultipartIncomplete {
                    missing: vec![2, 5],
                },
                409,
                "MULTIPART_INCOMPLETE",
                NeedsReconciliation,
            ),
            transfer(
                "s3 already complete",
                &TransferError::MultipartAlreadyCompleted,
                409,
                "MULTIPART_ALREADY_COMPLETED",
                Terminal,
            ),
            transfer(
                "s3 InvalidPart / InvalidPartOrder",
                &TransferError::MultipartPartRejected,
                422,
                "MULTIPART_PART_REJECTED",
                NeedsReconciliation,
            ),
            transfer(
                "s3 EntityTooSmall",
                &TransferError::MultipartPartInvalid {
                    reason: "provider_part_too_small",
                },
                422,
                "MULTIPART_PART_INVALID",
                Fatal,
            ),
            transfer(
                "s3 part number",
                &TransferError::MultipartPartInvalid {
                    reason: "part_number",
                },
                422,
                "MULTIPART_PART_INVALID",
                Fatal,
            ),
            transfer(
                "s3 presign batch",
                &TransferError::PresignBatchTooLarge,
                422,
                "PRESIGN_BATCH_TOO_LARGE",
                Fatal,
            ),
            storage(
                "s3 provider 5xx",
                &service_5xx,
                StorageSite::S3Control,
                502,
                "STORAGE_UPSTREAM_ERROR",
                Retryable,
            ),
            storage(
                "s3 provider timeout",
                &timeout,
                StorageSite::S3Control,
                502,
                "STORAGE_UPSTREAM_ERROR",
                Retryable,
            ),
            storage(
                "s3 provider unreachable",
                &unreachable,
                StorageSite::S3Control,
                503,
                "STORAGE_UNAVAILABLE",
                Retryable,
            ),
            storage(
                "s3 provider permission",
                &denied,
                StorageSite::S3Control,
                500,
                "STORAGE_PERMISSION_DENIED",
                Fatal,
            ),
            storage(
                "s3 provider rejection",
                &rejected,
                StorageSite::S3Control,
                502,
                "STORAGE_UPSTREAM_ERROR",
                Retryable,
            ),
            storage(
                "s3 size mismatch",
                &StorageError::SizeMismatch {
                    expected: 2,
                    actual: 1,
                },
                StorageSite::S3Control,
                500,
                "STORAGE_SIZE_MISMATCH",
                Fatal,
            ),
            rejection(
                "s3 planned size differs from HeadObject",
                RejectCause::LengthMismatch,
                422,
                "UPLOAD_LENGTH_MISMATCH",
                Fatal,
            ),
            transfer(
                "s3 storage unavailable at admission",
                &TransferError::StorageUnavailable,
                503,
                "STORAGE_UNAVAILABLE",
                Retryable,
            ),
            transfer(
                "s3 completion busy",
                &TransferError::CompletionBusy,
                503,
                "TRANSFER_BACKPRESSURE",
                Retryable,
            ),
            transfer(
                "quota admission",
                &quota_exceeded(),
                507,
                "QUOTA_EXCEEDED",
                Fatal,
            ),
            rejection(
                "quota at finalization",
                RejectCause::QuotaExceeded,
                507,
                "QUOTA_EXCEEDED",
                Fatal,
            ),
            rejection(
                "size policy",
                RejectCause::FileTooLarge,
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            transfer(
                "size policy at admission",
                &too_large(TooLargeReason::MaxFileSize),
                413,
                "FILE_TOO_LARGE",
                Fatal,
            ),
            transfer(
                "session unknown",
                &TransferError::SessionNotFound,
                404,
                "TRANSFER_SESSION_NOT_FOUND",
                Terminal,
            ),
            transfer(
                "session terminal",
                &TransferError::StateInvalid,
                409,
                "TRANSFER_SESSION_STATE_INVALID",
                Terminal,
            ),
            transfer(
                "session expired",
                &TransferError::Expired,
                410,
                "TRANSFER_SESSION_EXPIRED",
                Terminal,
            ),
            rejection(
                "session expired at finalization",
                RejectCause::SessionExpired,
                410,
                "TRANSFER_SESSION_EXPIRED",
                Terminal,
            ),
            code_only(
                "idempotency key reuse",
                ErrorCode::IdempotencyKeyConflict,
                409,
                "IDEMPOTENCY_KEY_CONFLICT",
                Fatal,
            ),
            transfer(
                "validation",
                &TransferError::Invalid {
                    fields: vec!["files"],
                },
                422,
                "VALIDATION_ERROR",
                Fatal,
            ),
            transfer(
                "batch too large",
                &TransferError::BatchTooLarge,
                422,
                "BATCH_TOO_LARGE",
                Fatal,
            ),
            transfer(
                "folder unknown",
                &TransferError::Folder(FolderError::NotFound),
                404,
                "FOLDER_NOT_FOUND",
                Fatal,
            ),
            transfer(
                "folder depth",
                &TransferError::Folder(FolderError::DepthExceeded),
                422,
                "FOLDER_DEPTH_EXCEEDED",
                Fatal,
            ),
            transfer(
                "name invalid",
                &TransferError::NameInvalid,
                422,
                "NAME_INVALID",
                Fatal,
            ),
            transfer(
                "name exhausted",
                &TransferError::Folder(FolderError::NameConflict(
                    CandidateError::AttemptsExhausted,
                )),
                409,
                "FILE_NAME_CONFLICT",
                Fatal,
            ),
            rejection(
                "name exhausted at finalization",
                RejectCause::NameExhausted,
                409,
                "FILE_NAME_CONFLICT",
                Fatal,
            ),
            transfer(
                "database busy",
                &TransferError::Db(DbError::from(sqlx::Error::PoolTimedOut)),
                503,
                "DATABASE_BUSY",
                Retryable,
            ),
            transfer(
                "unclassified fault",
                &TransferError::Invariant { what: "x" },
                500,
                "INTERNAL_ERROR",
                Fatal,
            ),
        ]
    }

    fn quota_exceeded_quota() -> QuotaError {
        QuotaError::Exceeded(Exceeded {
            used: ByteSize::ZERO,
            held: ByteSize::ZERO,
            requested: ByteSize::try_from(1_u64).unwrap(),
            quota: None,
        })
    }

    #[test]
    fn unit_transfer_error_classification_table() {
        let rows = table();
        assert!(rows.len() >= 55, "{} rows", rows.len());
        let mut intended: std::collections::BTreeMap<&str, (RetryClass, &str)> =
            std::collections::BTreeMap::new();
        for row in &rows {
            let (class, first) = intended.entry(row.code).or_insert((row.class, row.label));
            assert_eq!(
                *class, row.class,
                "{} and {first} persist the same code {} with incompatible retry classes",
                row.label, row.code
            );
        }
        for row in &rows {
            assert_eq!(row.classification.code.as_str(), row.code, "{}", row.label);
            assert_eq!(row.api.code().as_str(), row.code, "{}", row.label);
            assert_eq!(row.classification.class(), row.class, "{}", row.label);
            assert_eq!(
                row.classification.status().as_u16(),
                row.status,
                "{}",
                row.label
            );
            assert_eq!(row.api.status().as_u16(), row.status, "{}", row.label);
            assert_eq!(
                row.class.backs_off(),
                row.class == RetryClass::Retryable,
                "{}",
                row.label
            );
            if row.code != "INTERNAL_ERROR" {
                assert_ne!(
                    row.classification.code,
                    ErrorCode::InternalError,
                    "{}",
                    row.label
                );
            }
            assert_eq!(
                RetryClass::for_code(row.classification.code),
                row.class,
                "{}: the persisted code alone classifies like the live cause",
                row.label
            );
            let body =
                serde_json::to_value(ApiErrorBody::from(row.api.clone().with_request_id("r")))
                    .unwrap();
            let text = body.to_string();
            assert!(!text.contains(SENTINEL), "{}: {text}", row.label);
            assert!(!text.contains("objects/"), "{}: {text}", row.label);
            assert!(!text.contains("X-Amz"), "{}: {text}", row.label);
            assert_eq!(
                body["error"]["requestId"],
                Value::from("r"),
                "{}",
                row.label
            );
            assert!(body["error"]["details"].is_object(), "{}", row.label);
        }
        let seen: BTreeSet<&str> = rows.iter().map(|row| row.code).collect();
        for code in [
            "TUS_VERSION_UNSUPPORTED",
            "TUS_EXTENSION_UNSUPPORTED",
            "UPLOAD_METADATA_INVALID",
            "FILE_TOO_LARGE",
            "UNSUPPORTED_MEDIA_TYPE",
            "UPLOAD_OFFSET_MISMATCH",
            "UPLOAD_LOCKED",
            "UPLOAD_SESSION_EXPIRED",
            "UPLOAD_CHECKSUM_MISMATCH",
            "STORAGE_WRITE_FAILED",
            "STORAGE_FULL",
            "STORAGE_UNAVAILABLE",
            "TRANSFER_IDLE_TIMEOUT",
            "TRANSFER_BACKPRESSURE",
            "UPLOAD_LENGTH_MISMATCH",
            "QUOTA_EXCEEDED",
            "MULTIPART_ALREADY_CREATED",
            "MULTIPART_NOT_FOUND",
            "MULTIPART_INCOMPLETE",
            "MULTIPART_ALREADY_COMPLETED",
            "MULTIPART_PART_INVALID",
            "PRESIGN_BATCH_TOO_LARGE",
            "STORAGE_UPSTREAM_ERROR",
            "STORAGE_SIZE_MISMATCH",
            "TRANSFER_SESSION_NOT_FOUND",
            "TRANSFER_SESSION_STATE_INVALID",
            "TRANSFER_SESSION_EXPIRED",
            "IDEMPOTENCY_KEY_CONFLICT",
            "VALIDATION_ERROR",
            "BATCH_TOO_LARGE",
            "FOLDER_NOT_FOUND",
            "FOLDER_DEPTH_EXCEEDED",
            "NAME_INVALID",
            "FILE_NAME_CONFLICT",
            "DATABASE_BUSY",
            "INTERNAL_ERROR",
        ] {
            assert!(seen.contains(code), "{code} has no classification row");
        }
    }

    #[test]
    fn unit_transfer_error_classification_details_and_special_cases() {
        let mismatch = TusError::OffsetMismatch { expected: 9 }.api_error();
        assert_eq!(
            serde_json::to_value(ApiErrorBody::from(mismatch.with_request_id("r"))).unwrap()
                ["error"]["details"]["expectedOffset"],
            Value::from(9)
        );
        let missing = TransferError::MultipartIncomplete {
            missing: vec![2, 5],
        }
        .api_error();
        assert_eq!(
            serde_json::to_value(ApiErrorBody::from(missing.with_request_id("r"))).unwrap()
                ["error"]["details"]["missingPartNumbers"],
            serde_json::json!([2, 5])
        );
        let large = too_large(TooLargeReason::PartCount).api_error();
        assert_eq!(
            serde_json::to_value(ApiErrorBody::from(large.with_request_id("r"))).unwrap()["error"]
                ["details"]["reason"],
            Value::from("part_count")
        );

        for code in [ErrorCode::UploadOffsetMismatch, ErrorCode::UploadLocked] {
            let classification = Classification::of(code);
            assert!(classification.is_protocol_negotiation(), "{code:?}");
            assert!(!classification.records_item_failure(), "{code:?}");
            assert!(is_protocol_negotiation(code));
        }
        let waiting = Classification::of(ErrorCode::TransferBackpressure);
        assert!(waiting.is_waiting());
        assert!(!waiting.records_item_failure());
        assert!(waiting.class().backs_off());
        for code in [
            ErrorCode::QuotaExceeded,
            ErrorCode::FileTooLarge,
            ErrorCode::UploadLengthMismatch,
            ErrorCode::TransferSessionExpired,
            ErrorCode::UploadSessionExpired,
        ] {
            let classification = Classification::of(code);
            assert!(classification.records_item_failure(), "{code:?}");
            assert!(!classification.class().backs_off(), "{code:?}");
            assert!(classification.class().is_final(), "{code:?}");
        }
        for code in [
            ErrorCode::UploadOffsetMismatch,
            ErrorCode::UploadChecksumMismatch,
            ErrorCode::MultipartIncomplete,
        ] {
            let class = Classification::of(code).class();
            assert!(class.reconciles_first() && !class.backs_off(), "{code:?}");
        }
        assert_eq!(
            classify_write(WriteFault::from(&StorageError::QuotaOnDevice)).code,
            ErrorCode::StorageWriteFailed
        );
        assert_eq!(
            classify_write(WriteFault::from(&StorageError::ProviderUnavailable(
                Retryable::new(io::Error::other("x"))
            )))
            .code,
            ErrorCode::StorageUnavailable
        );
        assert_eq!(RetryClass::ALL.len(), 4);
        assert_eq!(
            RetryClass::ALL.map(RetryClass::as_str),
            ["retryable", "needs-reconciliation", "terminal", "fatal"]
        );
    }

    #[test]
    fn unit_transfer_error_non_retryable_set_never_backs_off() {
        let non_retryable = [
            "QUOTA_EXCEEDED",
            "FILE_TOO_LARGE",
            "UPLOAD_METADATA_INVALID",
            "UPLOAD_LENGTH_MISMATCH",
            "TUS_EXTENSION_UNSUPPORTED",
            "TUS_VERSION_UNSUPPORTED",
            "UNSUPPORTED_MEDIA_TYPE",
            "MULTIPART_NOT_FOUND",
            "MULTIPART_ALREADY_COMPLETED",
            "STORAGE_SIZE_MISMATCH",
            "IDEMPOTENCY_KEY_CONFLICT",
            "BATCH_TOO_LARGE",
            "FOLDER_NOT_FOUND",
            "FOLDER_DEPTH_EXCEEDED",
            "NAME_INVALID",
            "FILE_NAME_CONFLICT",
            "VALIDATION_ERROR",
        ];
        for wire in non_retryable {
            let code = ErrorCode::ALL
                .iter()
                .copied()
                .find(|code| code.as_str() == wire)
                .unwrap_or_else(|| panic!("{wire} is not in the catalogue"));
            let class = RetryClass::for_code(code);
            assert!(class.is_final(), "{wire}");
            assert!(!class.backs_off() && !class.reconciles_first(), "{wire}");
            assert!(!code.retryable(), "{wire}");
        }
        assert!(
            ErrorCode::ALL
                .iter()
                .all(|code| code.as_str() != "STORAGE_CORS_MISCONFIGURED"),
            "STORAGE_CORS_MISCONFIGURED stays a health state, never an HTTP error code"
        );
    }

    #[test]
    fn unit_transfer_error_reverse_share_contract_awaits_its_catalogue_codes() {
        let contract: [(&str, u16, RetryClass); 5] = [
            ("REVERSE_SHARE_INACTIVE", 410, RetryClass::Terminal),
            ("REVERSE_SHARE_EXPIRED", 410, RetryClass::Terminal),
            (
                "REVERSE_SHARE_FILE_TYPE_NOT_ALLOWED",
                415,
                RetryClass::Fatal,
            ),
            ("REVERSE_SHARE_LIMIT_REACHED", 409, RetryClass::Fatal),
            ("REVERSE_SHARE_NOT_FOUND", 404, RetryClass::Terminal),
        ];
        for (wire, status, class) in contract {
            match ErrorCode::ALL.iter().find(|code| code.as_str() == wire) {
                None => {}
                Some(code) => {
                    assert_eq!(code.status().as_u16(), status, "{wire}");
                    assert_eq!(RetryClass::for_code(*code), class, "{wire}");
                }
            }
        }
    }

    #[test]
    fn unit_transfer_error_final_classes_never_enter_the_web_generic_retry_path() {
        let policy = include_str!("../../../../web/src/shared/api/retryPolicy.ts");
        let web: std::collections::BTreeMap<&str, &str> = policy
            .lines()
            .filter_map(|line| {
                let (name, class) = line.trim().split_once(": \"")?;
                let class = class.strip_suffix("\",")?;
                name.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    .then_some((name, class))
            })
            .collect();
        assert!(web.len() > 100, "the web retry table was not parsed");
        for code in ErrorCode::ALL {
            let class = RetryClass::for_code(*code);
            let wire = code.as_str();
            let Some(web_class) = web.get(wire) else {
                panic!("{wire} has no web retry class");
            };
            if class.is_final() && wire != "INTERNAL_ERROR" {
                assert_eq!(*web_class, "never", "{wire} is {class:?} on the server");
            }
            if class == RetryClass::NeedsReconciliation {
                assert_eq!(
                    *web_class,
                    if wire == "UPLOAD_CHECKSUM_MISMATCH" {
                        "transient"
                    } else {
                        "never"
                    },
                    "{wire} reconciles before any retry; generic retry never applies"
                );
            }
        }
    }

    fn every_storage_error() -> Vec<StorageError> {
        vec![
            StorageError::NotFound,
            StorageError::RangeNotSatisfiable { size: 1 },
            StorageError::AlreadyExists,
            StorageError::InvalidKey,
            StorageError::QuotaOnDevice,
            StorageError::PermissionDenied,
            StorageError::ProviderUnavailable(Retryable::new(io::Error::other(SENTINEL))),
            StorageError::Config(SENTINEL.to_owned()),
            StorageError::ProviderMismatch {
                expected: crate::storage::ProviderKind::Local,
                found: crate::storage::ProviderKind::S3,
            },
            StorageError::SizeMismatch {
                expected: 2,
                actual: 1,
            },
            StorageError::Canceled,
            StorageError::Io(io::Error::other(SENTINEL)),
            StorageError::S3(Box::new(io::Error::other(SENTINEL))),
            provider(Operation::UploadPart, Failure::Timeout),
            provider(Operation::HeadObject, Failure::Dispatch),
            provider(
                Operation::HeadObject,
                Failure::Response { status: Some(502) },
            ),
            provider(
                Operation::UploadPart,
                Failure::Service {
                    status: 503,
                    code: Some("SlowDown".to_owned()),
                },
            ),
            provider(
                Operation::ListParts,
                Failure::Service {
                    status: 403,
                    code: Some("AccessDenied".to_owned()),
                },
            ),
            provider(
                Operation::ListParts,
                Failure::Service {
                    status: 404,
                    code: Some("NoSuchUpload".to_owned()),
                },
            ),
            provider(
                Operation::CompleteMultipartUpload,
                Failure::Service {
                    status: 400,
                    code: Some("InvalidPart".to_owned()),
                },
            ),
            provider(
                Operation::CompleteMultipartUpload,
                Failure::Service {
                    status: 400,
                    code: Some("EntityTooSmall".to_owned()),
                },
            ),
            provider(Operation::PutObject, Failure::Construction),
            provider(Operation::PutObject, Failure::MalformedResponse("x")),
        ]
    }

    fn every_cause() -> Vec<(String, Classification)> {
        let mut all: Vec<(String, Classification)> = table()
            .into_iter()
            .map(|row| (row.label.to_owned(), row.classification))
            .collect();
        for site in [
            StorageSite::Stream,
            StorageSite::Place,
            StorageSite::S3Control,
            StorageSite::S3Multipart,
        ] {
            for error in every_storage_error() {
                all.push((
                    format!("{site:?} {error:?}"),
                    classify_storage(&error, site),
                ));
            }
        }
        for error in every_storage_error() {
            all.push((
                format!("transfer {error:?}"),
                TransferError::Storage(error).classification(),
            ));
        }
        for fault in [
            WriteFault::DeviceFull,
            WriteFault::PermissionDenied,
            WriteFault::Io,
        ] {
            all.push((format!("{fault:?}"), classify_write(fault)));
        }
        for code in ErrorCode::ALL {
            all.push((
                format!("recorded {code:?}"),
                TransferError::Recorded(*code).classification(),
            ));
        }
        for cause in [
            RejectCause::LengthMismatch,
            RejectCause::FileTooLarge,
            RejectCause::QuotaExceeded,
            RejectCause::SessionExpired,
            RejectCause::AccountInactive,
            RejectCause::FolderNotFound,
            RejectCause::FolderDeleting,
            RejectCause::NameExhausted,
            RejectCause::MultipartLost,
        ] {
            all.push((format!("{cause:?}"), Classification::of(cause.code())));
        }
        for reason in [
            "provider_part_too_small",
            "part_number",
            "part_size",
            "body_interrupted",
            "body_too_short",
            "body_too_long",
            "duplicate_part_number",
            "etag",
        ] {
            all.push((
                format!("part invalid {reason}"),
                TransferError::MultipartPartInvalid { reason }.classification(),
            ));
        }
        all
    }

    #[test]
    fn regression_transfer_error_class_is_a_function_of_the_persisted_code() {
        let mut by_code: std::collections::BTreeMap<&str, BTreeSet<RetryClass>> =
            std::collections::BTreeMap::new();
        let mut causes = 0;
        for (label, classification) in every_cause() {
            causes += 1;
            by_code
                .entry(classification.code.as_str())
                .or_default()
                .insert(classification.class());
            assert_eq!(
                classification.class(),
                RetryClass::for_code(classification.code),
                "{label}: the live class must equal the decode of its persisted code"
            );
        }
        assert!(causes > 150, "{causes} causes were enumerated");
        for (code, classes) in &by_code {
            assert_eq!(
                classes.len(),
                1,
                "{code} is persisted by causes with incompatible retry classes: {classes:?}"
            );
        }
        for code in ErrorCode::ALL {
            let class = RetryClass::for_code(*code);
            if class.backs_off() {
                assert!(
                    code.retryable(),
                    "{} backs off but the catalogue says an unchanged retry is not safe",
                    code.as_str()
                );
            }
            if class.is_final() && code.as_str() != "INTERNAL_ERROR" {
                assert!(
                    !code.retryable(),
                    "{} is final but the catalogue marks it retryable",
                    code.as_str()
                );
            }
            let wire = code.as_str();
            assert_eq!(
                ErrorCode::ALL.iter().find(|known| known.as_str() == wire),
                Some(code),
                "{wire} round-trips through its persisted spelling"
            );
        }
    }
}
