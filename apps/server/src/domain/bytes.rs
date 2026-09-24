use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSizeOutOfRange;

impl ByteSize {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(i64::MAX);

    pub const fn get(self) -> u64 {
        self.0.unsigned_abs()
    }

    pub const fn to_i64(self) -> i64 {
        self.0
    }

    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(sum) => Some(Self(sum)),
            None => None,
        }
    }

    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(difference) if difference >= 0 => Some(Self(difference)),
            _ => None,
        }
    }
}

impl TryFrom<u64> for ByteSize {
    type Error = ByteSizeOutOfRange;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value)
            .map(Self)
            .map_err(|_| ByteSizeOutOfRange)
    }
}

impl TryFrom<i64> for ByteSize {
    type Error = ByteSizeOutOfRange;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(ByteSizeOutOfRange)
        }
    }
}

impl From<ByteSize> for u64 {
    fn from(value: ByteSize) -> Self {
        value.get()
    }
}

impl From<ByteSize> for i64 {
    fn from(value: ByteSize) -> Self {
        value.to_i64()
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Display for ByteSizeOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("byte count must be between 0 and 9223372036854775807")
    }
}

impl std::error::Error for ByteSizeOutOfRange {}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{ByteSize, ByteSizeOutOfRange};

    fn bytes(value: i64) -> ByteSize {
        ByteSize::try_from(value).unwrap()
    }

    fn byte_count() -> impl Strategy<Value = i64> {
        prop_oneof![
            0..=i64::MAX,
            0..=4_096_i64,
            (i64::MAX - 4_096)..=i64::MAX,
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

    #[test]
    fn unit_bytesize_conversions_and_bounds() {
        assert_eq!(ByteSize::ZERO.get(), 0);
        assert_eq!(ByteSize::MAX.to_i64(), i64::MAX);
        assert_eq!(ByteSize::try_from(0_u64), Ok(ByteSize::ZERO));
        assert_eq!(
            ByteSize::try_from(i64::MAX.unsigned_abs()),
            Ok(ByteSize::MAX)
        );
        assert_eq!(
            ByteSize::try_from(i64::MAX.unsigned_abs() + 1),
            Err(ByteSizeOutOfRange)
        );
        assert_eq!(ByteSize::try_from(u64::MAX), Err(ByteSizeOutOfRange));
        for negative in [-1_i64, -4_096, i64::MIN] {
            assert_eq!(
                ByteSize::try_from(negative),
                Err(ByteSizeOutOfRange),
                "{negative}"
            );
        }
        assert_eq!(u64::from(bytes(42)), 42);
        assert_eq!(i64::from(bytes(42)), 42);
        assert_eq!(bytes(42).to_string(), "42");

        assert_eq!(ByteSize::MAX.checked_add(bytes(1)), None);
        assert_eq!(
            ByteSize::MAX.checked_add(ByteSize::ZERO),
            Some(ByteSize::MAX)
        );
        assert_eq!(ByteSize::ZERO.checked_sub(bytes(1)), None);
        assert_eq!(bytes(5).checked_sub(bytes(6)), None);
        assert_eq!(bytes(6).checked_sub(bytes(6)), Some(ByteSize::ZERO));
        assert_eq!(
            ByteSize::MAX.checked_sub(ByteSize::MAX),
            Some(ByteSize::ZERO)
        );
        assert!(ByteSize::ZERO < ByteSize::MAX);

        let unlimited: Option<ByteSize> = None;
        let zero_quota = Some(ByteSize::ZERO);
        assert_ne!(unlimited, zero_quota);
        assert!(!ByteSizeOutOfRange.to_string().is_empty());
    }

    proptest! {
        #[test]
        fn prop_bytesize_checked_arithmetic(a in byte_count(), b in byte_count()) {
            let (left, right) = (bytes(a), bytes(b));
            prop_assert_eq!(left.get(), a.unsigned_abs());
            prop_assert_eq!(ByteSize::try_from(left.get()), Ok(left));

            let sum = a.unsigned_abs() + b.unsigned_abs();
            match left.checked_add(right) {
                Some(total) => {
                    prop_assert_eq!(total.get(), sum);
                    prop_assert_eq!(total.checked_sub(right), Some(left));
                }
                None => prop_assert!(sum > i64::MAX.unsigned_abs()),
            }
            prop_assert_eq!(left.checked_add(right), right.checked_add(left));

            match left.checked_sub(right) {
                Some(rest) => {
                    prop_assert!(a >= b);
                    prop_assert_eq!(rest.to_i64(), a - b);
                    prop_assert_eq!(rest.checked_add(right), Some(left));
                }
                None => prop_assert!(a < b),
            }
        }
    }
}
