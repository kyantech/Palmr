use crate::domain::bytes::ByteSize;

use super::error::{Exceeded, QuotaError};
use super::model::{Admission, Usage, QUOTA_ADMISSION_MARGIN};

const LIMB_BITS: u32 = 21;

pub const LIMB_MASK_SQL: &str = "2097151";

pub fn bytes_from_column(value: i64, column: &'static str) -> Result<ByteSize, QuotaError> {
    ByteSize::try_from(value).map_err(|_| QuotaError::Integrity { column })
}

pub fn add(
    left: ByteSize,
    right: ByteSize,
    operation: &'static str,
) -> Result<ByteSize, QuotaError> {
    left.checked_add(right)
        .ok_or(QuotaError::Overflow { operation })
}

pub fn sub(
    left: ByteSize,
    right: ByteSize,
    operation: &'static str,
) -> Result<ByteSize, QuotaError> {
    left.checked_sub(right)
        .ok_or(QuotaError::Underflow { operation })
}

pub fn evaluate(usage: Usage, requested: ByteSize) -> Result<Admission, QuotaError> {
    let committed_and_held = add(usage.used, usage.held, "used_plus_held")?;
    let projected = add(
        committed_and_held,
        requested,
        "used_plus_held_plus_requested",
    )?;
    if let Some(quota) = usage.quota {
        if projected > quota {
            return Err(QuotaError::Exceeded(Exceeded {
                used: usage.used,
                held: usage.held,
                requested,
                quota: Some(quota),
            }));
        }
    }
    Ok(Admission {
        usage,
        requested,
        projected,
    })
}

pub fn reservation_amount(
    declared_known: ByteSize,
    unknown_count: u32,
    effective_max_file_size: Option<ByteSize>,
) -> Result<ByteSize, QuotaError> {
    let Some(max_file_size) = effective_max_file_size.filter(|_| unknown_count > 0) else {
        return Ok(declared_known);
    };
    let unknown = max_file_size
        .checked_mul(unknown_count)
        .ok_or(QuotaError::Overflow {
            operation: "unknown_count_times_max_file_size",
        })?;
    add(declared_known, unknown, "declared_plus_unknown_reservation")
}

pub fn exceeds_reservation(share: ByteSize, authoritative: ByteSize) -> Result<bool, QuotaError> {
    let ceiling = add(share, QUOTA_ADMISSION_MARGIN, "share_plus_admission_margin")?;
    Ok(authoritative > ceiling)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limbs {
    pub low: i64,
    pub mid: i64,
    pub high: i64,
}

impl Limbs {
    pub const ZERO: Self = Self {
        low: 0,
        mid: 0,
        high: 0,
    };

    pub fn total(self, operation: &'static str) -> Result<ByteSize, QuotaError> {
        let mid = self
            .mid
            .checked_mul(1_i64 << LIMB_BITS)
            .ok_or(QuotaError::Overflow { operation })?;
        let high = self
            .high
            .checked_mul(1_i64 << (2 * LIMB_BITS))
            .ok_or(QuotaError::Overflow { operation })?;
        let total = self
            .low
            .checked_add(mid)
            .and_then(|partial| partial.checked_add(high))
            .ok_or(QuotaError::Overflow { operation })?;
        ByteSize::try_from(total).map_err(|_| QuotaError::Integrity { column: operation })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{
        add, bytes_from_column, evaluate, exceeds_reservation, reservation_amount, sub, Limbs,
        LIMB_BITS, LIMB_MASK_SQL,
    };
    use crate::domain::bytes::ByteSize;
    use crate::features::quota::error::QuotaError;
    use crate::features::quota::model::{Usage, QUOTA_ADMISSION_MARGIN};

    fn bytes(value: i64) -> ByteSize {
        ByteSize::try_from(value).unwrap()
    }

    fn byte_count() -> impl Strategy<Value = i64> {
        prop_oneof![
            0..=i64::MAX,
            0..=4_096_i64,
            (i64::MAX - 4_096)..=i64::MAX,
            (i64::MAX / 3 - 8)..=(i64::MAX / 3 + 8),
            prop::sample::select(vec![
                0,
                1,
                i64::MAX / 2,
                i64::MAX / 2 + 1,
                i64::MAX - 1,
                i64::MAX
            ]),
        ]
    }

    fn reference_projected(used: i64, held: i64, requested: i64) -> i128 {
        i128::from(used) + i128::from(held) + i128::from(requested)
    }

    #[test]
    fn unit_limb_mask_constant_matches_width() {
        assert_eq!(
            LIMB_MASK_SQL.parse::<i64>().unwrap(),
            (1_i64 << LIMB_BITS) - 1
        );
    }

    proptest! {
        #[test]
        fn prop_quota_arithmetic_checked(
            used in byte_count(),
            held in byte_count(),
            requested in byte_count(),
            quota in prop::option::of(byte_count()),
            negative in i64::MIN..0_i64,
            unknown_count in prop_oneof![0_u32..=4, Just(u32::MAX), 1_000_u32..=2_000],
            max_file in prop::option::of(byte_count()),
            limb_values in prop::collection::vec(byte_count(), 0..6),
        ) {
            let usage = Usage {
                used: bytes(used),
                held: bytes(held),
                quota: quota.map(bytes),
            };
            let projected = reference_projected(used, held, requested);
            let decision = evaluate(usage, bytes(requested));
            match decision {
                Err(QuotaError::Overflow { .. }) => {
                    prop_assert!(projected > i128::from(i64::MAX));
                }
                Err(QuotaError::Exceeded(exceeded)) => {
                    prop_assert!(projected <= i128::from(i64::MAX));
                    let limit = quota.unwrap();
                    prop_assert!(projected > i128::from(limit));
                    prop_assert_eq!(exceeded.quota, Some(bytes(limit)));
                    prop_assert_eq!(exceeded.used, bytes(used));
                    prop_assert_eq!(exceeded.held, bytes(held));
                    prop_assert_eq!(exceeded.requested, bytes(requested));
                }
                Ok(admission) => {
                    prop_assert!(projected <= i128::from(i64::MAX));
                    prop_assert_eq!(i128::from(admission.projected.to_i64()), projected);
                    if let Some(limit) = quota {
                        prop_assert!(projected <= i128::from(limit));
                    }
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }

            if projected <= i128::from(i64::MAX) {
                let exact = i64::try_from(projected).unwrap();
                let at_limit = Usage { quota: Some(bytes(exact)), ..usage };
                prop_assert!(evaluate(at_limit, bytes(requested)).is_ok());
                if exact > 0 {
                    let below = Usage { quota: Some(bytes(exact - 1)), ..usage };
                    let refused = matches!(
                        evaluate(below, bytes(requested)),
                        Err(QuotaError::Exceeded(_))
                    );
                    prop_assert!(refused);
                }
                let unlimited = Usage { quota: None, ..usage };
                prop_assert!(evaluate(unlimited, bytes(requested)).is_ok());
                if requested > 0 && evaluate(usage, bytes(requested)).is_ok() {
                    prop_assert!(evaluate(usage, bytes(requested - 1)).is_ok());
                }
            } else {
                let unlimited = Usage { quota: None, ..usage };
                let overflowed = matches!(
                    evaluate(unlimited, bytes(requested)),
                    Err(QuotaError::Overflow { .. })
                );
                prop_assert!(overflowed);
            }

            prop_assert!(ByteSize::try_from(negative).is_err());
            let rejected = matches!(
                bytes_from_column(negative, "column"),
                Err(QuotaError::Integrity { column: "column" })
            );
            prop_assert!(rejected);

            match add(bytes(used), bytes(held), "test") {
                Ok(sum) => prop_assert_eq!(i128::from(sum.to_i64()), i128::from(used) + i128::from(held)),
                Err(QuotaError::Overflow { .. }) => {
                    prop_assert!(i128::from(used) + i128::from(held) > i128::from(i64::MAX));
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }
            match sub(bytes(used), bytes(held), "test") {
                Ok(difference) => {
                    prop_assert!(used >= held);
                    prop_assert_eq!(difference.to_i64(), used - held);
                }
                Err(QuotaError::Underflow { .. }) => prop_assert!(used < held),
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }

            let amount = reservation_amount(bytes(used), unknown_count, max_file.map(bytes));
            let expected_amount = match max_file.filter(|_| unknown_count > 0) {
                Some(max) => i128::from(used) + i128::from(max) * i128::from(unknown_count),
                None => i128::from(used),
            };
            match amount {
                Ok(total) => prop_assert_eq!(i128::from(total.to_i64()), expected_amount),
                Err(QuotaError::Overflow { .. }) => {
                    prop_assert!(expected_amount > i128::from(i64::MAX));
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }

            let margin = i128::from(QUOTA_ADMISSION_MARGIN.to_i64());
            match exceeds_reservation(bytes(held), bytes(requested)) {
                Ok(exceeds) => {
                    prop_assert_eq!(exceeds, i128::from(requested) > i128::from(held) + margin);
                }
                Err(QuotaError::Overflow { .. }) => {
                    prop_assert!(i128::from(held) + margin > i128::from(i64::MAX));
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }

            let mask = (1_i64 << LIMB_BITS) - 1;
            let mut limbs = Limbs::ZERO;
            for value in &limb_values {
                limbs.low += value & mask;
                limbs.mid += (value >> LIMB_BITS) & mask;
                limbs.high += value >> (2 * LIMB_BITS);
            }
            let reference: i128 = limb_values.iter().map(|value| i128::from(*value)).sum();
            match limbs.total("test") {
                Ok(total) => prop_assert_eq!(i128::from(total.to_i64()), reference),
                Err(QuotaError::Overflow { .. }) => {
                    prop_assert!(reference > i128::from(i64::MAX));
                }
                Err(other) => prop_assert!(false, "unexpected error {other:?}"),
            }
        }
    }

    #[test]
    fn unit_quota_arithmetic_boundaries() {
        let usage = Usage {
            used: bytes(40),
            held: bytes(20),
            quota: Some(bytes(100)),
        };
        assert!(evaluate(usage, bytes(40)).is_ok());
        assert!(matches!(
            evaluate(usage, bytes(41)),
            Err(QuotaError::Exceeded(_))
        ));

        let zero_quota = Usage {
            used: ByteSize::ZERO,
            held: ByteSize::ZERO,
            quota: Some(ByteSize::ZERO),
        };
        assert!(evaluate(zero_quota, ByteSize::ZERO).is_ok());
        assert!(matches!(
            evaluate(zero_quota, bytes(1)),
            Err(QuotaError::Exceeded(_))
        ));

        let near_max = Usage {
            used: ByteSize::MAX,
            held: bytes(1),
            quota: None,
        };
        assert!(matches!(
            evaluate(near_max, ByteSize::ZERO),
            Err(QuotaError::Overflow { .. })
        ));
    }
}
