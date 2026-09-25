use proptest::prelude::*;

use super::plan::{
    ceil_div, next_pow2_mib, plan_parts, plan_parts_with_limits, proxy_admit, proxy_ceiling_bytes,
    unknown_cumulative_bytes, unknown_exceeds_max_object, unknown_part_size,
    unknown_parts_exhausted, unknown_parts_to_reach_max_object, FileTooLargeReason, PartPlan,
    PartPlanError, MIN_PART_SIZE, PROXY_PART_SIZE,
};
use super::profile::{ProfileLimits, ProviderProfile, GIB, MIB, TIB};

const PLAN_SOURCE: &str = include_str!("plan.rs");

fn valid_limits() -> impl Strategy<Value = ProfileLimits> {
    (200u32..=2_000u32).prop_flat_map(|max_parts| {
        (
            1u32..max_parts,
            (3u32..=13).prop_map(|power| MIB << power),
            0u64..=(6 * TIB),
        )
            .prop_map(move |(safety_margin, max_part, max_object)| ProfileLimits {
                max_parts,
                safety_margin,
                max_part,
                max_object,
                ..ProfileLimits::BASELINE
            })
    })
}

fn valid_limits_and_size() -> impl Strategy<Value = (ProfileLimits, u64)> {
    valid_limits().prop_flat_map(|limits| {
        let max_object = limits.max_object;
        (Just(limits), 0u64..=max_object)
    })
}

fn valid_limits_and_sizes() -> impl Strategy<Value = (ProfileLimits, u64, u64)> {
    valid_limits().prop_flat_map(|limits| {
        let max_object = limits.max_object;
        (0u64..=max_object, 0u64..=max_object).prop_map(move |(a, b)| {
            let (small, large) = if a <= b { (a, b) } else { (b, a) };
            (limits, small, large)
        })
    })
}

#[test]
#[allow(non_snake_case)]
fn regression_R001_multipart_part_count_ceiling() {
    let profile = ProviderProfile::Generic;
    let limits = profile.limits();
    let usable = u64::from(limits.usable_parts());

    let zero = plan_parts(0, &profile).unwrap();
    assert!(zero.is_zero_byte());
    assert_eq!(zero.part_count(), 0);
    assert_eq!(zero.part_size(), None);
    assert_eq!(zero, PartPlan::ZeroByte);

    assert_eq!(
        plan_parts(5 * MIB - 1, &profile).unwrap(),
        PartPlan::Multipart {
            part_size: 8 * MIB,
            part_count: 1,
        }
    );

    let v3_ceiling = 5 * MIB * 10_000;
    let plan = plan_parts(v3_ceiling, &profile).unwrap();
    assert_eq!(
        plan,
        PartPlan::Multipart {
            part_size: 8 * MIB,
            part_count: 6_250,
        }
    );
    assert!(plan.part_count() <= usable);

    let above_v3_ceiling = 50 * GIB;
    let plan = plan_parts(above_v3_ceiling, &profile).unwrap();
    assert!(plan.part_count() <= usable);
    assert!(plan.covered_bytes() >= above_v3_ceiling);

    let five_tib = 5 * TIB;
    assert_eq!(
        plan_parts(five_tib, &profile).unwrap(),
        PartPlan::Multipart {
            part_size: GIB,
            part_count: 5_120,
        }
    );

    let error = plan_parts(five_tib + 1, &profile).unwrap_err();
    assert_eq!(error.code(), "FILE_TOO_LARGE");
    assert_eq!(error.reason(), Some(FileTooLargeReason::ObjectSize));
    assert_eq!(error.declared_bytes(), Some(five_tib + 1));
    assert_eq!(error.provider_max_bytes(), Some(five_tib));

    for (size, part_size, part_count) in [
        (1u64, 8 * MIB, 1u64),
        (MIB, 8 * MIB, 1),
        (5 * MIB, 8 * MIB, 1),
        (8 * MIB, 8 * MIB, 1),
        (8 * MIB + 1, 8 * MIB, 2),
        (100 * GIB, 16 * MIB, 6_400),
        (500 * GIB, 64 * MIB, 8_000),
        (TIB, 128 * MIB, 8_192),
    ] {
        assert_eq!(
            plan_parts(size, &profile).unwrap(),
            PartPlan::Multipart {
                part_size,
                part_count,
            },
            "size {size}"
        );
    }

    let floor_capacity = 8 * MIB * usable;
    assert_eq!(
        plan_parts(floor_capacity, &profile).unwrap(),
        PartPlan::Multipart {
            part_size: 8 * MIB,
            part_count: usable,
        }
    );
    assert_eq!(
        plan_parts(floor_capacity + 1, &profile).unwrap(),
        PartPlan::Multipart {
            part_size: 16 * MIB,
            part_count: 4_951,
        }
    );
}

proptest! {
    #[test]
    fn prop_part_plan_respects_provider_limits((limits, size) in valid_limits_and_size()) {
        let usable = u64::from(limits.max_parts - limits.safety_margin);
        match plan_parts_with_limits(size, &limits) {
            Ok(PartPlan::ZeroByte) => {
                prop_assert_eq!(size, 0);
            }
            Ok(plan @ PartPlan::Multipart { part_size, part_count }) => {
                prop_assert!(size > 0);
                prop_assert!(size <= limits.max_object);
                prop_assert!(part_size >= MIN_PART_SIZE);
                prop_assert!(part_size <= limits.max_part);
                prop_assert_eq!(part_size % MIB, 0);
                prop_assert!((part_size / MIB).is_power_of_two());
                prop_assert!(part_count >= 1);
                prop_assert!(part_count <= usable);
                prop_assert!(plan.covered_bytes() >= size);

                for part_number in 1..part_count {
                    let (start, end) = plan.part_range(part_number, size).unwrap();
                    prop_assert_eq!(end - start, part_size);
                }
                let (start, end) = plan.part_range(part_count, size).unwrap();
                prop_assert_eq!(end, size);
                prop_assert!(end > start);
            }
            Err(PartPlanError::FileTooLarge {
                reason: FileTooLargeReason::ObjectSize,
                declared_bytes,
                ..
            }) => {
                prop_assert_eq!(declared_bytes, size);
                prop_assert!(size > limits.max_object);
            }
            Err(PartPlanError::FileTooLarge {
                reason: FileTooLargeReason::PartCount,
                ..
            }) => {
                prop_assert!(size > limits.max_part.saturating_mul(usable));
            }
            Err(other) => prop_assert!(false, "unexpected error: {other:?}"),
        }
    }

    #[test]
    fn prop_part_plan_monotone((limits, small, large) in valid_limits_and_sizes()) {
        prop_assert!(small <= large);
        if let (Ok(left), Ok(right)) = (
            plan_parts_with_limits(small, &limits),
            plan_parts_with_limits(large, &limits),
        ) {
            prop_assert!(left.part_size().unwrap_or(0) <= right.part_size().unwrap_or(0));
            prop_assert!(left.covered_bytes() <= right.covered_bytes());
        }
    }
}

#[test]
fn unit_unknown_length_escalation_reaches_max_object() {
    let limits = ProviderProfile::Generic.limits();
    let usable = u64::from(limits.usable_parts());

    for (part_number, expected) in [
        (1u64, 8 * MIB),
        (1_000, 8 * MIB),
        (1_001, 64 * MIB),
        (2_000, 64 * MIB),
        (2_001, 256 * MIB),
        (3_000, 256 * MIB),
        (3_001, GIB),
        (4_000, GIB),
        (4_001, 4 * GIB),
        (usable, 4 * GIB),
    ] {
        assert_eq!(
            unknown_part_size(part_number, &limits).unwrap(),
            expected,
            "part {part_number}"
        );
    }

    let needed = unknown_parts_to_reach_max_object(&limits).unwrap().unwrap();
    assert!(needed > 4_000);
    assert!(needed < usable);
    assert!(unknown_cumulative_bytes(needed, &limits).unwrap() >= limits.max_object);
    assert!(unknown_cumulative_bytes(needed - 1, &limits).unwrap() < limits.max_object);
    assert!(unknown_exceeds_max_object(needed, &limits).unwrap());
    assert!(!unknown_parts_exhausted(needed, &limits).unwrap());
    assert!(unknown_parts_exhausted(usable, &limits).unwrap());

    let clamped = ProfileLimits {
        max_part: 2 * GIB,
        ..ProviderProfile::Generic.limits()
    };
    assert_eq!(unknown_part_size(1, &clamped).unwrap(), 8 * MIB);
    assert_eq!(unknown_part_size(3_001, &clamped).unwrap(), GIB);
    assert_eq!(unknown_part_size(4_001, &clamped).unwrap(), 2 * GIB);
    assert_eq!(unknown_part_size(usable, &clamped).unwrap(), 2 * GIB);

    assert_eq!(
        unknown_part_size(0, &limits),
        Err(PartPlanError::InvalidPartNumber)
    );
}

#[test]
fn unit_ceil_div_boundaries() {
    assert_eq!(ceil_div(0, 8), 0);
    assert_eq!(ceil_div(1, 8), 1);
    assert_eq!(ceil_div(7, 8), 1);
    assert_eq!(ceil_div(8, 8), 1);
    assert_eq!(ceil_div(9, 8), 2);
    assert_eq!(ceil_div(u64::MAX, 1), u64::MAX);
    assert_eq!(ceil_div(u64::MAX, u64::MAX), 1);
}

#[test]
fn unit_next_pow2_mib_boundaries() {
    assert_eq!(next_pow2_mib(0), Some(MIB));
    assert_eq!(next_pow2_mib(1), Some(MIB));
    assert_eq!(next_pow2_mib(MIB), Some(MIB));
    assert_eq!(next_pow2_mib(MIB + 1), Some(2 * MIB));
    assert_eq!(next_pow2_mib(5 * MIB), Some(8 * MIB));
    assert_eq!(next_pow2_mib(8 * MIB), Some(8 * MIB));
    assert_eq!(next_pow2_mib(8 * MIB + 1), Some(16 * MIB));
    assert_eq!(next_pow2_mib(u64::MAX), None);
}

#[test]
fn unit_plan_part_size_is_dynamic() {
    let profile = ProviderProfile::Generic;
    for (size, expected_part) in [
        (1u64, 8 * MIB),
        (8 * MIB, 8 * MIB),
        (8 * MIB + 1, 8 * MIB),
        (100 * GIB, 16 * MIB),
        (500 * GIB, 64 * MIB),
        (TIB, 128 * MIB),
        (5 * TIB, GIB),
    ] {
        let plan = plan_parts(size, &profile).unwrap();
        assert_eq!(plan.part_size(), Some(expected_part), "size {size}");
    }

    let small = plan_parts(8 * MIB, &profile).unwrap().part_size().unwrap();
    let large = plan_parts(5 * TIB, &profile).unwrap().part_size().unwrap();
    assert_ne!(small, large);
}

#[test]
fn unit_plan_uses_derived_usable_parts() {
    let single_slot = ProfileLimits {
        max_parts: 200,
        safety_margin: 199,
        ..ProfileLimits::BASELINE
    };
    let max_part = 5 * GIB;

    assert_eq!(
        plan_parts_with_limits(max_part, &single_slot).unwrap(),
        PartPlan::Multipart {
            part_size: max_part,
            part_count: 1,
        }
    );

    let error = plan_parts_with_limits(max_part + 1, &single_slot).unwrap_err();
    assert_eq!(error.reason(), Some(FileTooLargeReason::PartCount));
    assert_eq!(error.provider_max_bytes(), Some(max_part));
}

#[test]
fn unit_plan_rejects_invalid_profiles() {
    let no_slots = ProfileLimits {
        max_parts: 100,
        safety_margin: 100,
        ..ProfileLimits::BASELINE
    };
    assert_eq!(
        plan_parts_with_limits(1, &no_slots),
        Err(PartPlanError::InvalidProfile)
    );

    let below_floor = ProfileLimits {
        max_part: MIB,
        ..ProfileLimits::BASELINE
    };
    assert_eq!(
        plan_parts_with_limits(1, &below_floor),
        Err(PartPlanError::InvalidProfile)
    );
}

#[test]
fn unit_plan_error_fields() {
    let profile = ProviderProfile::Generic.limits();
    let too_big = 5 * TIB + 1;
    let error = plan_parts_with_limits(too_big, &profile).unwrap_err();
    assert_eq!(error.code(), "FILE_TOO_LARGE");
    assert_eq!(error.reason(), Some(FileTooLargeReason::ObjectSize));
    assert_eq!(
        error.reason().unwrap().as_str(),
        FileTooLargeReason::ObjectSize.as_str()
    );
    assert_eq!(error.declared_bytes(), Some(too_big));
    assert_eq!(error.provider_max_bytes(), Some(5 * TIB));

    let limited = ProfileLimits {
        max_part: 8 * MIB,
        max_object: 5 * TIB,
        ..ProfileLimits::BASELINE
    };
    let error = plan_parts_with_limits(100 * GIB, &limited).unwrap_err();
    assert_eq!(error.reason(), Some(FileTooLargeReason::PartCount));
    assert_eq!(
        error.reason().unwrap().as_str(),
        FileTooLargeReason::PartCount.as_str()
    );
    assert_eq!(error.declared_bytes(), Some(100 * GIB));
    assert_eq!(error.provider_max_bytes(), Some(8 * MIB * 9_900));
}

#[test]
fn unit_proxy_ceiling_exact_bytes() {
    let profile = ProviderProfile::Generic.limits();
    let ceiling = proxy_ceiling_bytes(&profile).unwrap();
    assert_eq!(ceiling, PROXY_PART_SIZE * 9_900);
    assert_eq!(ceiling, 8 * MIB * 9_900);
    assert_eq!(ceiling, 83_047_219_200);
    assert_eq!(ceiling * 32, 2_475 * GIB);

    assert!(proxy_admit(0, &profile).is_ok());
    assert!(proxy_admit(ceiling, &profile).is_ok());

    let error = proxy_admit(ceiling + 1, &profile).unwrap_err();
    assert_eq!(error.reason(), Some(FileTooLargeReason::ProxyPartCount));
    assert_eq!(
        error.reason().unwrap().as_str(),
        FileTooLargeReason::ProxyPartCount.as_str()
    );
    assert_eq!(error.declared_bytes(), Some(ceiling + 1));
    assert_eq!(error.provider_max_bytes(), Some(ceiling));

    let error = proxy_admit(5 * TIB + 1, &profile).unwrap_err();
    assert_eq!(error.reason(), Some(FileTooLargeReason::ObjectSize));
}

#[test]
fn unit_plan_is_deterministic() {
    let profile = ProviderProfile::Generic;
    for size in [
        0u64,
        1,
        5 * MIB - 1,
        8 * MIB,
        8 * MIB + 1,
        100 * GIB,
        5 * TIB,
    ] {
        let expected = plan_parts(size, &profile).unwrap();
        for _ in 0..64 {
            assert_eq!(plan_parts(size, &profile).unwrap(), expected);
        }
    }
}

#[test]
fn unit_plan_representation_is_constant_size() {
    let plan = PartPlan::Multipart {
        part_size: 5 * GIB,
        part_count: 9_900,
    };
    assert!(std::mem::size_of_val(&plan) <= 24);
}

#[test]
fn unit_plan_source_stays_pure() {
    for forbidden in [
        "f32",
        "f64",
        "Vec<",
        "vec![",
        ".collect(",
        "with_capacity",
        ".reserve(",
        "unsafe",
        "SystemTime",
        "std::env",
        "rand::",
        "tokio::",
        "tracing::",
        "unwrap()",
        "expect(",
        "panic!",
        "todo!",
        "unimplemented!",
    ] {
        assert!(
            !PLAN_SOURCE.contains(forbidden),
            "plan.rs contains {forbidden}"
        );
    }
}
