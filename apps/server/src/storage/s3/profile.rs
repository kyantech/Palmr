use crate::config::S3Profile;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;
pub const TIB: u64 = 1024 * GIB;

const BASELINE_MIN_PART: u64 = 5 * MIB;
const BASELINE_MAX_PART: u64 = 5 * GIB;
const BASELINE_MAX_PARTS: u32 = 10_000;
const BASELINE_MAX_OBJECT: u64 = 5 * TIB;
const BASELINE_SAFETY_MARGIN: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileLimits {
    pub min_part: u64,
    pub max_part: u64,
    pub max_parts: u32,
    pub max_object: u64,
    pub safety_margin: u32,
    pub requires_part_checksums: bool,
    pub allows_single_small_part: bool,
    pub supports_presigned_get: bool,
    pub supports_server_side_copy: bool,
}

impl ProfileLimits {
    pub const BASELINE: Self = Self {
        min_part: BASELINE_MIN_PART,
        max_part: BASELINE_MAX_PART,
        max_parts: BASELINE_MAX_PARTS,
        max_object: BASELINE_MAX_OBJECT,
        safety_margin: BASELINE_SAFETY_MARGIN,
        requires_part_checksums: false,
        allows_single_small_part: true,
        supports_presigned_get: true,
        supports_server_side_copy: true,
    };

    pub const fn usable_parts(&self) -> u32 {
        self.max_parts - self.safety_margin
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderProfile {
    Generic,
    Aws,
    Minio,
    R2,
    Rustfs,
    B2,
    Gcs,
    Wasabi,
    Garage,
}

impl ProviderProfile {
    pub const ALL: [Self; 9] = [
        Self::Generic,
        Self::Aws,
        Self::Minio,
        Self::R2,
        Self::Rustfs,
        Self::B2,
        Self::Gcs,
        Self::Wasabi,
        Self::Garage,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Aws => "aws",
            Self::Minio => "minio",
            Self::R2 => "r2",
            Self::Rustfs => "rustfs",
            Self::B2 => "b2",
            Self::Gcs => "gcs",
            Self::Wasabi => "wasabi",
            Self::Garage => "garage",
        }
    }

    pub const fn limits(self) -> ProfileLimits {
        match self {
            Self::R2 => ProfileLimits {
                requires_part_checksums: true,
                ..ProfileLimits::BASELINE
            },
            Self::Generic
            | Self::Aws
            | Self::Minio
            | Self::Rustfs
            | Self::B2
            | Self::Gcs
            | Self::Wasabi
            | Self::Garage => ProfileLimits::BASELINE,
        }
    }

    pub const fn from_config(profile: S3Profile) -> Self {
        match profile {
            S3Profile::Generic => Self::Generic,
            S3Profile::Aws => Self::Aws,
            S3Profile::Minio => Self::Minio,
            S3Profile::R2 => Self::R2,
            S3Profile::Rustfs => Self::Rustfs,
            S3Profile::B2 => Self::B2,
            S3Profile::Gcs => Self::Gcs,
            S3Profile::Wasabi => Self::Wasabi,
            S3Profile::Garage => Self::Garage,
        }
    }

    pub const fn config_profile(self) -> S3Profile {
        match self {
            Self::Generic => S3Profile::Generic,
            Self::Aws => S3Profile::Aws,
            Self::Minio => S3Profile::Minio,
            Self::R2 => S3Profile::R2,
            Self::Rustfs => S3Profile::Rustfs,
            Self::B2 => S3Profile::B2,
            Self::Gcs => S3Profile::Gcs,
            Self::Wasabi => S3Profile::Wasabi,
            Self::Garage => S3Profile::Garage,
        }
    }
}
