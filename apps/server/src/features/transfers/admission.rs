use std::sync::Arc;

use crate::config::StorageConfig;
use crate::domain::bytes::ByteSize;
use crate::features::folders::MAX_FOLDER_DEPTH;
use crate::infra::http::pagination::MAX_WIRE_BYTES;
use crate::storage::health::{StorageHealth, StorageStatus};
use crate::storage::planning::{
    PartLayout, Rejection, RejectionReason, UploadMethod, UploadPlanner,
};
use crate::storage::provider::StorageProvider;

use super::error::{FileTooLarge, TooLargeReason, TransferError};
use super::model::{
    wire_bytes, PlannedFile, TransferProvider, TransferS3Plan, TransferTusPlan, UploadKind,
    MAX_PRESIGN_BATCH, PRESIGN_TTL_SECONDS, TUS_CREATE_URL,
};

pub type HealthProbe = Arc<dyn Fn() -> StorageHealth + Send + Sync>;

#[derive(Clone)]
pub struct TransferStorage {
    planner: UploadPlanner,
    provider: TransferProvider,
    health: HealthProbe,
}

impl std::fmt::Debug for TransferStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferStorage")
            .field("planner", &self.planner)
            .finish_non_exhaustive()
    }
}

impl TransferStorage {
    pub fn new(planner: UploadPlanner, health: HealthProbe) -> Self {
        let provider =
            TransferProvider::parse(planner.provider_key()).unwrap_or(TransferProvider::Local);
        Self {
            planner,
            provider,
            health,
        }
    }

    #[cfg(test)]
    pub fn local_with(health: HealthProbe) -> Self {
        Self::new(UploadPlanner::local(), health)
    }

    pub fn from_operator(
        config: &StorageConfig,
        provider: Arc<dyn StorageProvider>,
        status: StorageStatus,
    ) -> Self {
        Self::new(
            UploadPlanner::from_operator(config, provider),
            Arc::new(move || status.snapshot().health),
        )
    }

    pub const fn provider(&self) -> TransferProvider {
        self.provider
    }

    pub fn is_available(&self) -> bool {
        (self.health)() != StorageHealth::Down
    }

    pub fn provider_max_object(&self) -> Option<ByteSize> {
        self.planner
            .max_object()
            .and_then(|bytes| ByteSize::try_from(bytes).ok())
    }

    pub fn effective_max_file_size(&self, policy: Option<ByteSize>) -> Option<ByteSize> {
        match (policy, self.provider_max_object()) {
            (Some(policy), Some(provider)) => Some(policy.min(provider)),
            (policy, provider) => policy.or(provider),
        }
    }

    pub fn plan(&self, size: Option<ByteSize>) -> Result<UploadKind, TooLarge> {
        match self.planner.plan(size.map(ByteSize::get)) {
            Ok(UploadMethod::Resumable) => Ok(UploadKind::Tus),
            Ok(UploadMethod::Multipart) => Ok(UploadKind::S3Multipart),
            Ok(UploadMethod::Single) => Ok(UploadKind::S3Single),
            Err(rejection) => Err(TooLarge::from(rejection)),
        }
    }

    pub fn tus_plan(&self) -> TransferTusPlan {
        TransferTusPlan {
            create_url: TUS_CREATE_URL.to_owned(),
        }
    }

    pub fn s3_plan(&self, kind: UploadKind, size: Option<ByteSize>) -> Option<TransferS3Plan> {
        if kind != UploadKind::S3Multipart {
            return None;
        }
        let PartLayout {
            part_size,
            part_count,
        } = self.planner.part_layout(size.map(ByteSize::get));
        Some(TransferS3Plan {
            part_size_bytes: part_size
                .and_then(|bytes| ByteSize::try_from(bytes).ok())
                .map(wire_bytes),
            part_count: part_count.and_then(|count| u32::try_from(count).ok()),
            max_presign_batch: MAX_PRESIGN_BATCH,
            presign_ttl_seconds: PRESIGN_TTL_SECONDS,
            completed_parts: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooLarge {
    Provider {
        reason: TooLargeReason,
        capacity: u64,
    },
    Profile,
}

impl From<Rejection> for TooLarge {
    fn from(rejection: Rejection) -> Self {
        let reason = match rejection.reason {
            RejectionReason::ObjectSize => TooLargeReason::ObjectSize,
            RejectionReason::PartCount => TooLargeReason::PartCount,
            RejectionReason::ProxyPartCount => TooLargeReason::ProxyPartCount,
            RejectionReason::Profile => return Self::Profile,
        };
        Self::Provider {
            reason,
            capacity: rejection.capacity,
        }
    }
}

pub fn plan_files(
    files: &[PlannedFile],
    storage: &TransferStorage,
    policy_max: Option<ByteSize>,
) -> Result<Vec<UploadKind>, TransferError> {
    let mut kinds = Vec::with_capacity(files.len());
    for file in files {
        if let (Some(size), Some(max)) = (file.size, policy_max) {
            if size > max {
                return Err(TransferError::FileTooLarge(FileTooLarge {
                    client_key: file.client_key.clone(),
                    declared: size,
                    max: Some(max),
                    provider_max: storage.provider_max_object(),
                    reason: TooLargeReason::MaxFileSize,
                }));
            }
        }
        match storage.plan(file.size) {
            Ok(kind) => kinds.push(kind),
            Err(TooLarge::Provider { reason, capacity }) => {
                let declared = file.size.ok_or(TransferError::Invariant { what: "plan" })?;
                return Err(TransferError::FileTooLarge(FileTooLarge {
                    client_key: file.client_key.clone(),
                    declared,
                    max: policy_max,
                    provider_max: ByteSize::try_from(capacity).ok(),
                    reason,
                }));
            }
            Err(TooLarge::Profile) => {
                return Err(TransferError::Invariant {
                    what: "provider_profile",
                })
            }
        }
    }
    Ok(kinds)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub declared_total: ByteSize,
    pub total: ByteSize,
    pub shares: Vec<ByteSize>,
}

pub fn reservation(
    files: &[PlannedFile],
    effective_max: Option<ByteSize>,
) -> Result<Reservation, TransferError> {
    let overflow = || TransferError::Invalid {
        fields: vec!["files"],
    };
    let mut declared_total = ByteSize::ZERO;
    let mut total = ByteSize::ZERO;
    let mut shares = Vec::with_capacity(files.len());
    for file in files {
        let share = match file.size {
            Some(size) => {
                declared_total = declared_total.checked_add(size).ok_or_else(overflow)?;
                size
            }
            None => effective_max.unwrap_or(ByteSize::ZERO),
        };
        total = total.checked_add(share).ok_or_else(overflow)?;
        shares.push(share);
    }
    if total.to_i64() > MAX_WIRE_BYTES {
        return Err(overflow());
    }
    Ok(Reservation {
        declared_total,
        total,
        shares,
    })
}

pub fn exceeds_depth(base_depth: i64, files: &[PlannedFile]) -> bool {
    files.iter().any(|file| {
        let segments = i64::try_from(file.directory.len()).unwrap_or(i64::MAX);
        segments > 0 && base_depth.saturating_add(segments - 1) > MAX_FOLDER_DEPTH
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::{plan_files, reservation, TransferStorage};
    use crate::domain::bytes::ByteSize;
    use crate::features::transfers::error::{TooLargeReason, TransferError};
    use crate::features::transfers::model::{PlannedFile, UploadKind};
    use crate::storage::health::StorageHealth;
    use crate::storage::planning::UploadPlanner;
    use crate::storage::s3::profile::{ProviderProfile, MIB, TIB};

    fn bytes(value: u64) -> ByteSize {
        ByteSize::try_from(value).unwrap()
    }

    fn file(ordinal: u32, size: Option<u64>) -> PlannedFile {
        let item = crate::features::transfers::model::ValidatedSession::parse(
            serde_json::from_value(serde_json::json!({
                "target": { "kind": "my_files" },
                "files": [{ "clientId": format!("c{ordinal}"), "name": "a.bin", "sizeBytes": size }],
            }))
            .unwrap(),
        )
        .unwrap()
        .files
        .remove(0);
        PlannedFile { ordinal, ..item }
    }

    fn healthy() -> Arc<dyn Fn() -> StorageHealth + Send + Sync> {
        Arc::new(|| StorageHealth::Ok)
    }

    fn s3(profile: ProviderProfile, proxied: bool) -> TransferStorage {
        TransferStorage::new(
            UploadPlanner::s3(profile, Arc::new(move || proxied)),
            healthy(),
        )
    }

    #[test]
    fn unit_local_storage_plans_tus_for_every_size() {
        let storage = TransferStorage::local_with(healthy());
        for size in [None, Some(bytes(0)), Some(bytes(5 * TIB + 1))] {
            assert_eq!(storage.plan(size), Ok(UploadKind::Tus));
        }
        assert_eq!(storage.provider_max_object(), None);
        assert_eq!(storage.effective_max_file_size(None), None);
        assert_eq!(
            storage.effective_max_file_size(Some(bytes(10))),
            Some(bytes(10))
        );
        assert!(storage.s3_plan(UploadKind::Tus, Some(bytes(1))).is_none());
    }

    #[test]
    fn unit_s3_part_plan_presentation_follows_the_planner() {
        let storage = s3(ProviderProfile::Minio, false);
        let plan = |size: u64| storage.s3_plan(UploadKind::S3Multipart, Some(bytes(size)));
        assert_eq!(storage.plan(Some(bytes(0))), Ok(UploadKind::S3Single));
        assert!(storage
            .s3_plan(UploadKind::S3Single, Some(bytes(0)))
            .is_none());
        let small = plan(5 * MIB - 1).unwrap();
        assert_eq!(
            small.part_size_bytes.map(|b| b.get()),
            Some(i64::try_from(8 * MIB).unwrap())
        );
        assert_eq!(small.part_count, Some(1));
        assert_eq!(plan(52_428_800_000).unwrap().part_count, Some(6250));
        assert_eq!(plan(5 * TIB).unwrap().part_count, Some(5120));
        assert_eq!(
            storage.plan(Some(bytes(5 * TIB))),
            Ok(UploadKind::S3Multipart)
        );
        assert!(storage.plan(Some(bytes(5 * TIB + 1))).is_err());
        let unknown = storage.s3_plan(UploadKind::S3Multipart, None).unwrap();
        assert_eq!((unknown.part_size_bytes, unknown.part_count), (None, None));
        assert_eq!(unknown.max_presign_batch, 16);
        assert_eq!(unknown.presign_ttl_seconds, 900);
        assert_eq!(storage.provider_max_object(), Some(bytes(5 * TIB)));
    }

    #[test]
    fn unit_size_policy_is_checked_before_the_provider_plan() {
        let storage = s3(ProviderProfile::Generic, false);
        let files = [file(0, Some(10)), file(1, Some(11))];
        let error = plan_files(&files, &storage, Some(bytes(10))).unwrap_err();
        let TransferError::FileTooLarge(large) = error else {
            panic!("expected a size rejection");
        };
        assert_eq!(large.client_key.as_str(), "c1");
        assert_eq!(large.reason, TooLargeReason::MaxFileSize);
        assert_eq!(large.max, Some(bytes(10)));

        let oversized = [file(0, Some(5 * TIB + 1))];
        let TransferError::FileTooLarge(large) =
            plan_files(&oversized, &storage, Some(bytes(6 * TIB))).unwrap_err()
        else {
            panic!("expected a provider rejection");
        };
        assert_eq!(large.reason, TooLargeReason::ObjectSize);
        assert_eq!(large.provider_max, Some(bytes(5 * TIB)));
        assert_eq!(large.max, Some(bytes(6 * TIB)));
    }

    #[test]
    fn unit_reservation_follows_the_unknown_size_rules() {
        let files = [
            file(0, Some(100)),
            file(1, None),
            file(2, None),
            file(3, Some(0)),
        ];
        let finite = reservation(&files, Some(bytes(1_000))).unwrap();
        assert_eq!(finite.declared_total, bytes(100));
        assert_eq!(finite.total, bytes(2_100));
        assert_eq!(
            finite.shares,
            [bytes(100), bytes(1_000), bytes(1_000), bytes(0)]
        );
        let unlimited = reservation(&files, None).unwrap();
        assert_eq!(unlimited.total, bytes(100));
        assert_eq!(unlimited.shares, [bytes(100), bytes(0), bytes(0), bytes(0)]);
        assert_eq!(
            unlimited
                .shares
                .iter()
                .fold(0, |sum, share| sum + share.get()),
            unlimited.total.get()
        );
    }

    #[test]
    fn unit_reservation_overflow_is_rejected_not_wrapped() {
        let huge = 9_007_199_254_740_991_u64;
        let files: Vec<PlannedFile> = (0..1_100)
            .map(|ordinal| file(ordinal, Some(huge)))
            .collect();
        assert!(matches!(
            reservation(&files, None),
            Err(TransferError::Invalid { .. })
        ));
        let unknown: Vec<PlannedFile> = (0..2_000).map(|ordinal| file(ordinal, None)).collect();
        assert!(matches!(
            reservation(&unknown, Some(bytes(5 * TIB))),
            Err(TransferError::Invalid { .. })
        ));
        assert!(reservation(&unknown, Some(bytes(TIB))).is_ok());
        let near = [file(0, Some(huge))];
        assert_eq!(reservation(&near, None).unwrap().total, bytes(huge));
    }

    #[test]
    fn unit_storage_availability_tracks_the_health_probe() {
        let down = Arc::new(AtomicBool::new(false));
        let probe = {
            let down = Arc::clone(&down);
            Arc::new(move || {
                if down.load(Ordering::SeqCst) {
                    StorageHealth::Down
                } else {
                    StorageHealth::Degraded
                }
            })
        };
        let storage = TransferStorage::local_with(probe);
        assert!(storage.is_available());
        down.store(true, Ordering::SeqCst);
        assert!(!storage.is_available());
    }

    #[test]
    fn unit_depth_guard_counts_the_target_ancestry() {
        let nested = crate::features::transfers::model::ValidatedSession::parse(
            serde_json::from_value(serde_json::json!({
                "target": { "kind": "my_files" },
                "files": [{ "clientId": "c1", "name": "f", "relativePath": "a/b/c/f" }],
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(!super::exceeds_depth(61, &nested.files));
        assert!(super::exceeds_depth(63, &nested.files));
        let flat = [file(0, Some(1))];
        assert!(!super::exceeds_depth(1_000, &flat));
    }
}
