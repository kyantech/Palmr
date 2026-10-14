mod commit;
mod hooks;
mod local;
mod repo;
mod service;

pub use hooks::FinalizeHooks;
#[cfg(feature = "e2e-fixture")]
pub use hooks::PauseHook;
#[cfg(test)]
pub use hooks::{Flow, Point};
#[cfg(test)]
pub use service::FinalizeOutcome;
pub use service::{FinalizeLimits, Finalizer, FinalizerParts};

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbError, DbPools};

use super::error::TransferError;

pub async fn pending_redrive(pools: &DbPools, clock: &dyn Clock) -> Result<u64, TransferError> {
    let now = Timestamp::try_from(clock.now())?;
    let mut connection = pools
        .reader()
        .executor()
        .acquire()
        .await
        .map_err(DbError::from)?;
    repo::redrive_count(&mut connection, now).await
}

#[cfg(test)]
mod tests {
    use super::commit::RejectCause;
    use super::{FinalizeLimits, Point};
    use crate::domain::error_code::ErrorCode;
    use crate::storage::lifecycle::DeletionReason;

    const SOURCES: [(&str, &str); 6] = [
        ("commit.rs", include_str!("commit.rs")),
        ("hooks.rs", include_str!("hooks.rs")),
        ("local.rs", include_str!("local.rs")),
        ("repo.rs", include_str!("repo.rs")),
        ("service.rs", include_str!("service.rs")),
        ("mod.rs", include_str!("mod.rs")),
    ];

    fn production(source: &str) -> &str {
        source.split("#[cfg(test)]").next().unwrap_or(source)
    }

    #[test]
    fn unit_finalize_commit_transaction_performs_no_storage_io() {
        let commit = production(include_str!("commit.rs"));
        for token in [
            "std::fs",
            "tokio::fs",
            "StagingStorage",
            "staging",
            ".place(",
            ".measure(",
            ".inspect(",
            "rename(",
            "sync_all",
            "fsync",
            "unlink",
            "read_dir",
            "ObjectBody",
            "StorageProvider",
        ] {
            assert!(!commit.contains(token), "commit.rs names {token}");
        }
    }

    #[test]
    fn unit_finalize_has_no_whole_file_read_or_provider_branching() {
        for (file, source) in SOURCES {
            let source = production(source);
            for token in [
                "read_to_end",
                "read_to_string",
                "fs::read",
                "to_bytes",
                "ProviderKind",
                "caps()",
                "LocalProvider",
                "Sha256",
            ] {
                assert!(!source.contains(token), "{file} contains {token}");
            }
        }
        let inspect = production(include_str!("../../../storage/local/inspect.rs"));
        for token in ["read_to_end", "read_to_string", "fs::read", "fs::read_dir"] {
            assert!(!inspect.contains(token), "inspect.rs contains {token}");
        }
    }

    #[test]
    fn unit_finalize_rejection_causes_map_to_stable_codes_and_reasons() {
        let table = [
            (
                RejectCause::LengthMismatch,
                ErrorCode::UploadLengthMismatch,
                DeletionReason::UploadRejected,
            ),
            (
                RejectCause::FileTooLarge,
                ErrorCode::FileTooLarge,
                DeletionReason::UploadRejected,
            ),
            (
                RejectCause::QuotaExceeded,
                ErrorCode::QuotaExceeded,
                DeletionReason::QuotaRejected,
            ),
            (
                RejectCause::SessionExpired,
                ErrorCode::TransferSessionExpired,
                DeletionReason::UploadAbandoned,
            ),
            (
                RejectCause::AccountInactive,
                ErrorCode::AuthAccountInactive,
                DeletionReason::UploadRejected,
            ),
            (
                RejectCause::FolderNotFound,
                ErrorCode::FolderNotFound,
                DeletionReason::UploadRejected,
            ),
            (
                RejectCause::FolderDeleting,
                ErrorCode::FolderDeleting,
                DeletionReason::UploadRejected,
            ),
            (
                RejectCause::NameExhausted,
                ErrorCode::FileNameConflict,
                DeletionReason::UploadRejected,
            ),
        ];
        for (cause, code, reason) in table {
            assert_eq!(cause.code(), code, "{cause:?}");
            assert_eq!(cause.reason(), reason, "{cause:?}");
        }
    }

    #[test]
    fn unit_finalize_limits_and_points_are_the_documented_set() {
        let limits = FinalizeLimits::production();
        assert_eq!(limits.lease_ttl.as_secs(), 60);
        assert_eq!(limits.heartbeat * 3, limits.lease_ttl);
        assert_eq!(limits.concurrency, 4);
        assert_eq!(Point::ALL.len(), 11);
        let names: std::collections::BTreeSet<&str> =
            Point::ALL.iter().map(|point| point.as_str()).collect();
        assert_eq!(names.len(), 11);
    }
}
