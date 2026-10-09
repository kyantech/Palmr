use std::sync::Arc;

use crate::config::StorageConfig;

use super::provider::StorageProvider;
use super::s3::plan::{
    ceil_div, plan_parts_with_limits, proxy_admit, FileTooLargeReason, PartPlan, PartPlanError,
    PROXY_PART_SIZE,
};
use super::s3::profile::{ProfileLimits, ProviderProfile};
use super::ProviderKind;

pub type ProxyProbe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct UploadPlanner {
    kind: ProviderKind,
    profile: Option<ProviderProfile>,
    proxied: ProxyProbe,
}

impl std::fmt::Debug for UploadPlanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadPlanner")
            .field("kind", &self.kind)
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadMethod {
    Resumable,
    Multipart,
    Single,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionReason {
    ObjectSize,
    PartCount,
    ProxyPartCount,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejection {
    pub reason: RejectionReason,
    pub capacity: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartLayout {
    pub part_size: Option<u64>,
    pub part_count: Option<u64>,
}

impl From<PartPlanError> for Rejection {
    fn from(error: PartPlanError) -> Self {
        match error {
            PartPlanError::FileTooLarge {
                provider_max_bytes,
                reason,
                ..
            } => Self {
                reason: match reason {
                    FileTooLargeReason::ObjectSize => RejectionReason::ObjectSize,
                    FileTooLargeReason::PartCount => RejectionReason::PartCount,
                    FileTooLargeReason::ProxyPartCount => RejectionReason::ProxyPartCount,
                },
                capacity: provider_max_bytes,
            },
            PartPlanError::InvalidProfile | PartPlanError::InvalidPartNumber => Self {
                reason: RejectionReason::Profile,
                capacity: 0,
            },
        }
    }
}

impl UploadPlanner {
    pub fn from_operator(config: &StorageConfig, provider: Arc<dyn StorageProvider>) -> Self {
        match config {
            StorageConfig::Local => Self::local(),
            StorageConfig::S3(s3) => Self::s3(
                ProviderProfile::from_config(s3.profile),
                Arc::new(move || provider.caps().requires_checksum_headers),
            ),
        }
    }

    pub fn local() -> Self {
        Self {
            kind: ProviderKind::Local,
            profile: None,
            proxied: Arc::new(|| false),
        }
    }

    pub fn s3(profile: ProviderProfile, proxied: ProxyProbe) -> Self {
        Self {
            kind: ProviderKind::S3,
            profile: Some(profile),
            proxied,
        }
    }

    pub const fn provider_key(&self) -> &'static str {
        self.kind.as_str()
    }

    fn limits(&self) -> Option<ProfileLimits> {
        self.profile.map(ProviderProfile::limits)
    }

    pub fn max_object(&self) -> Option<u64> {
        self.limits().map(|limits| limits.max_object)
    }

    pub fn plan(&self, size: Option<u64>) -> Result<UploadMethod, Rejection> {
        let Some(limits) = self.limits() else {
            return Ok(UploadMethod::Resumable);
        };
        let Some(bytes) = size else {
            return Ok(UploadMethod::Multipart);
        };
        if (self.proxied)() {
            proxy_admit(bytes, &limits)?;
            return Ok(if bytes == 0 {
                UploadMethod::Single
            } else {
                UploadMethod::Multipart
            });
        }
        match plan_parts_with_limits(bytes, &limits)? {
            PartPlan::ZeroByte => Ok(UploadMethod::Single),
            PartPlan::Multipart { .. } => Ok(UploadMethod::Multipart),
        }
    }

    pub fn part_layout(&self, size: Option<u64>) -> PartLayout {
        let unknown = PartLayout {
            part_size: None,
            part_count: None,
        };
        let (Some(limits), Some(bytes)) = (self.limits(), size) else {
            return unknown;
        };
        if (self.proxied)() {
            return PartLayout {
                part_size: Some(PROXY_PART_SIZE),
                part_count: Some(ceil_div(bytes, PROXY_PART_SIZE)),
            };
        }
        match plan_parts_with_limits(bytes, &limits) {
            Ok(PartPlan::Multipart {
                part_size,
                part_count,
            }) => PartLayout {
                part_size: Some(part_size),
                part_count: Some(part_count),
            },
            Ok(PartPlan::ZeroByte) | Err(_) => unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{PartLayout, ProxyProbe, RejectionReason, UploadMethod, UploadPlanner};
    use crate::storage::s3::profile::{ProviderProfile, GIB, MIB, TIB};

    fn direct() -> ProxyProbe {
        Arc::new(|| false)
    }

    fn proxied() -> ProxyProbe {
        Arc::new(|| true)
    }

    #[test]
    fn unit_local_planner_uses_the_resumable_protocol_for_every_size() {
        let planner = UploadPlanner::local();
        for size in [None, Some(0), Some(5 * TIB + 1), Some(u64::MAX)] {
            assert_eq!(planner.plan(size), Ok(UploadMethod::Resumable));
        }
        assert_eq!(planner.max_object(), None);
        assert_eq!(planner.provider_key(), "local");
        assert_eq!(
            planner.part_layout(Some(100)),
            PartLayout {
                part_size: None,
                part_count: None
            }
        );
    }

    #[test]
    fn unit_s3_planner_boundaries_come_from_the_part_planner() {
        let planner = UploadPlanner::s3(ProviderProfile::Minio, direct());
        assert_eq!(planner.provider_key(), "s3");
        assert_eq!(planner.max_object(), Some(5 * TIB));
        assert_eq!(planner.plan(Some(0)), Ok(UploadMethod::Single));
        for (size, part_size, part_count) in [
            (5 * MIB - 1, 8 * MIB, 1),
            (52_428_800_000, 8 * MIB, 6_250),
            (5 * TIB, GIB, 5_120),
        ] {
            assert_eq!(planner.plan(Some(size)), Ok(UploadMethod::Multipart));
            assert_eq!(
                planner.part_layout(Some(size)),
                PartLayout {
                    part_size: Some(part_size),
                    part_count: Some(part_count)
                },
                "{size}"
            );
        }
        let rejected = planner.plan(Some(5 * TIB + 1)).unwrap_err();
        assert_eq!(rejected.reason, RejectionReason::ObjectSize);
        assert_eq!(rejected.capacity, 5 * TIB);
        assert_eq!(planner.plan(None), Ok(UploadMethod::Multipart));
        assert_eq!(
            planner.part_layout(None),
            PartLayout {
                part_size: None,
                part_count: None
            }
        );
    }

    #[test]
    fn unit_proxied_planner_uses_the_degraded_ceiling() {
        let planner = UploadPlanner::s3(ProviderProfile::R2, proxied());
        let ceiling = 8 * MIB * 9_900;
        assert_eq!(planner.plan(Some(ceiling)), Ok(UploadMethod::Multipart));
        let rejected = planner.plan(Some(ceiling + 1)).unwrap_err();
        assert_eq!(rejected.reason, RejectionReason::ProxyPartCount);
        assert_eq!(rejected.capacity, ceiling);
        assert_eq!(
            planner.part_layout(Some(100 * MIB)),
            PartLayout {
                part_size: Some(8 * MIB),
                part_count: Some(13)
            }
        );
        assert_eq!(planner.plan(Some(0)), Ok(UploadMethod::Single));
    }

    #[test]
    fn unit_proxy_probe_is_read_on_every_plan() {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let probe = {
            let flag = Arc::clone(&flag);
            Arc::new(move || flag.load(std::sync::atomic::Ordering::SeqCst))
        };
        let planner = UploadPlanner::s3(ProviderProfile::R2, probe);
        let ceiling = 8 * MIB * 9_900;
        assert!(planner.plan(Some(ceiling + 1)).is_err());
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(planner.plan(Some(ceiling + 1)), Ok(UploadMethod::Multipart));
    }
}
