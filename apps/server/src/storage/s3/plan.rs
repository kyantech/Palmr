use super::profile::{ProfileLimits, ProviderProfile, GIB, MIB};

pub const MIN_PART_SIZE: u64 = 8 * MIB;
pub const PROXY_PART_SIZE: u64 = 8 * MIB;

const ESCALATION_BANDS: [(u64, u64, u64); 4] = [
    (1, 1_000, 8 * MIB),
    (1_001, 2_000, 64 * MIB),
    (2_001, 3_000, 256 * MIB),
    (3_001, 4_000, GIB),
];
const ESCALATION_TOP_FROM: u64 = 4_001;
const ESCALATION_TOP_PART: u64 = 4 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartPlan {
    ZeroByte,
    Multipart { part_size: u64, part_count: u64 },
}

impl PartPlan {
    pub const fn is_zero_byte(self) -> bool {
        matches!(self, Self::ZeroByte)
    }

    pub const fn part_size(self) -> Option<u64> {
        match self {
            Self::ZeroByte => None,
            Self::Multipart { part_size, .. } => Some(part_size),
        }
    }

    pub const fn part_count(self) -> u64 {
        match self {
            Self::ZeroByte => 0,
            Self::Multipart { part_count, .. } => part_count,
        }
    }

    pub const fn covered_bytes(self) -> u64 {
        match self {
            Self::ZeroByte => 0,
            Self::Multipart {
                part_size,
                part_count,
            } => part_size.saturating_mul(part_count),
        }
    }

    pub const fn part_range(self, part_number: u64, size: u64) -> Option<(u64, u64)> {
        match self {
            Self::ZeroByte => None,
            Self::Multipart {
                part_size,
                part_count,
            } => {
                if part_number == 0 || part_number > part_count {
                    return None;
                }
                let start = match (part_number - 1).checked_mul(part_size) {
                    Some(start) => start,
                    None => return None,
                };
                let end = match part_number.checked_mul(part_size) {
                    Some(end) => {
                        if end < size {
                            end
                        } else {
                            size
                        }
                    }
                    None => size,
                };
                Some((start, end))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTooLargeReason {
    ObjectSize,
    PartCount,
    ProxyPartCount,
}

impl FileTooLargeReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObjectSize => "object_size",
            Self::PartCount => "part_count",
            Self::ProxyPartCount => "proxy_part_count",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartPlanError {
    #[error("the declared file size exceeds the storage provider limit")]
    FileTooLarge {
        declared_bytes: u64,
        provider_max_bytes: u64,
        reason: FileTooLargeReason,
    },
    #[error("the storage provider profile cannot produce a part plan")]
    InvalidProfile,
    #[error("multipart part numbers are one-based")]
    InvalidPartNumber,
}

impl PartPlanError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::FileTooLarge { .. } => "FILE_TOO_LARGE",
            Self::InvalidProfile | Self::InvalidPartNumber => "INTERNAL_ERROR",
        }
    }

    pub const fn reason(self) -> Option<FileTooLargeReason> {
        match self {
            Self::FileTooLarge { reason, .. } => Some(reason),
            Self::InvalidProfile | Self::InvalidPartNumber => None,
        }
    }

    pub const fn declared_bytes(self) -> Option<u64> {
        match self {
            Self::FileTooLarge { declared_bytes, .. } => Some(declared_bytes),
            Self::InvalidProfile | Self::InvalidPartNumber => None,
        }
    }

    pub const fn provider_max_bytes(self) -> Option<u64> {
        match self {
            Self::FileTooLarge {
                provider_max_bytes, ..
            } => Some(provider_max_bytes),
            Self::InvalidProfile | Self::InvalidPartNumber => None,
        }
    }
}

pub fn ceil_div(value: u64, divisor: u64) -> u64 {
    value.div_ceil(divisor)
}

pub fn next_pow2_mib(bytes: u64) -> Option<u64> {
    let mib = ceil_div(bytes, MIB).max(1);
    match mib.checked_next_power_of_two() {
        Some(power) => power.checked_mul(MIB),
        None => None,
    }
}

fn usable_parts(profile: &ProfileLimits) -> Result<u64, PartPlanError> {
    let max_parts = u64::from(profile.max_parts);
    let safety_margin = u64::from(profile.safety_margin);
    if safety_margin >= max_parts {
        return Err(PartPlanError::InvalidProfile);
    }
    Ok(max_parts - safety_margin)
}

fn known_length_max_part(profile: &ProfileLimits) -> Result<u64, PartPlanError> {
    if profile.max_part < MIN_PART_SIZE {
        return Err(PartPlanError::InvalidProfile);
    }
    Ok(profile.max_part)
}

fn escalation_max_part(profile: &ProfileLimits) -> Result<u64, PartPlanError> {
    if profile.max_part == 0 {
        return Err(PartPlanError::InvalidProfile);
    }
    Ok(profile.max_part)
}

pub fn plan_parts(size: u64, profile: &ProviderProfile) -> Result<PartPlan, PartPlanError> {
    plan_parts_with_limits(size, &profile.limits())
}

pub fn plan_parts_with_limits(
    size: u64,
    profile: &ProfileLimits,
) -> Result<PartPlan, PartPlanError> {
    if size > profile.max_object {
        return Err(PartPlanError::FileTooLarge {
            declared_bytes: size,
            provider_max_bytes: profile.max_object,
            reason: FileTooLargeReason::ObjectSize,
        });
    }

    if size == 0 {
        return Ok(PartPlan::ZeroByte);
    }

    let usable = usable_parts(profile)?;
    let max_part = known_length_max_part(profile)?;
    let required = ceil_div(size, usable);
    let part_size = match next_pow2_mib(required) {
        Some(candidate) => candidate.clamp(MIN_PART_SIZE, max_part),
        None => max_part,
    };
    let part_count = ceil_div(size, part_size);

    if part_count > usable {
        let capacity = max_part.saturating_mul(usable).min(profile.max_object);
        return Err(PartPlanError::FileTooLarge {
            declared_bytes: size,
            provider_max_bytes: capacity,
            reason: FileTooLargeReason::PartCount,
        });
    }

    Ok(PartPlan::Multipart {
        part_size,
        part_count,
    })
}

fn escalation_part_size(part_number: u64, max_part: u64) -> u64 {
    for (low, high, band_size) in ESCALATION_BANDS {
        if (low..=high).contains(&part_number) {
            return band_size.min(max_part);
        }
    }
    ESCALATION_TOP_PART.min(max_part)
}

pub fn unknown_part_size(part_number: u64, profile: &ProfileLimits) -> Result<u64, PartPlanError> {
    if part_number == 0 {
        return Err(PartPlanError::InvalidPartNumber);
    }
    Ok(escalation_part_size(
        part_number,
        escalation_max_part(profile)?,
    ))
}

pub fn unknown_cumulative_bytes(
    part_number: u64,
    profile: &ProfileLimits,
) -> Result<u64, PartPlanError> {
    if part_number == 0 {
        return Err(PartPlanError::InvalidPartNumber);
    }
    let max_part = escalation_max_part(profile)?;
    let mut total = 0u64;
    for (low, high, band_size) in ESCALATION_BANDS {
        if part_number < low {
            break;
        }
        let count = part_number.min(high) - low + 1;
        total = total.saturating_add(band_size.min(max_part).saturating_mul(count));
    }
    if part_number >= ESCALATION_TOP_FROM {
        let count = part_number - (ESCALATION_TOP_FROM - 1);
        let size = ESCALATION_TOP_PART.min(max_part);
        total = total.saturating_add(size.saturating_mul(count));
    }
    Ok(total)
}

pub fn unknown_parts_exhausted(
    part_number: u64,
    profile: &ProfileLimits,
) -> Result<bool, PartPlanError> {
    Ok(part_number >= usable_parts(profile)?)
}

pub fn unknown_exceeds_max_object(
    part_number: u64,
    profile: &ProfileLimits,
) -> Result<bool, PartPlanError> {
    Ok(unknown_cumulative_bytes(part_number, profile)? >= profile.max_object)
}

pub fn unknown_parts_to_reach_max_object(
    profile: &ProfileLimits,
) -> Result<Option<u64>, PartPlanError> {
    escalation_max_part(profile)?;
    let usable = usable_parts(profile)?;
    if unknown_cumulative_bytes(usable, profile)? < profile.max_object {
        return Ok(None);
    }
    let mut low = 1u64;
    let mut high = usable;
    while low < high {
        let mid = low + (high - low) / 2;
        if unknown_cumulative_bytes(mid, profile)? >= profile.max_object {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    Ok(Some(low))
}

pub fn proxy_ceiling_bytes(profile: &ProfileLimits) -> Result<u64, PartPlanError> {
    Ok(PROXY_PART_SIZE.saturating_mul(usable_parts(profile)?))
}

pub fn proxy_admit(size: u64, profile: &ProfileLimits) -> Result<(), PartPlanError> {
    if size > profile.max_object {
        return Err(PartPlanError::FileTooLarge {
            declared_bytes: size,
            provider_max_bytes: profile.max_object,
            reason: FileTooLargeReason::ObjectSize,
        });
    }
    if size == 0 {
        return Ok(());
    }
    let ceiling = proxy_ceiling_bytes(profile)?;
    if size > ceiling {
        return Err(PartPlanError::FileTooLarge {
            declared_bytes: size,
            provider_max_bytes: ceiling,
            reason: FileTooLargeReason::ProxyPartCount,
        });
    }
    Ok(())
}
