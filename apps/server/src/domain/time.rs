use std::{fmt, str::FromStr};

use time::{
    format_description::BorrowedFormatItem, macros::format_description, OffsetDateTime,
    PrimitiveDateTime, UtcOffset,
};

const CANONICAL: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

// UTC, millisecond precision and a four-digit year keep every rendering the
// same width, so lexical order of the text equals chronological order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTimestamp;

impl Timestamp {
    pub const fn get(self) -> OffsetDateTime {
        self.0
    }
}

impl TryFrom<OffsetDateTime> for Timestamp {
    type Error = InvalidTimestamp;

    fn try_from(at: OffsetDateTime) -> Result<Self, Self::Error> {
        let utc = at
            .checked_to_offset(UtcOffset::UTC)
            .filter(|utc| (0..=9999).contains(&utc.year()))
            .ok_or(InvalidTimestamp)?;
        Ok(Self(utc.truncate_to_millisecond()))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.format(CANONICAL).map_err(|_| fmt::Error)?)
    }
}

impl FromStr for Timestamp {
    type Err = InvalidTimestamp;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parsed = PrimitiveDateTime::parse(text, CANONICAL).map_err(|_| InvalidTimestamp)?;
        let timestamp = Self::try_from(parsed.assume_utc())?;
        if timestamp.to_string() == text {
            Ok(timestamp)
        } else {
            Err(InvalidTimestamp)
        }
    }
}

impl fmt::Display for InvalidTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("timestamp is not a UTC value in the form YYYY-MM-DDTHH:MM:SS.sssZ")
    }
}

impl std::error::Error for InvalidTimestamp {}

#[cfg(test)]
mod tests {
    use time::{format_description::well_known::Rfc3339, macros::datetime, OffsetDateTime};

    use super::{InvalidTimestamp, Timestamp};

    fn render(at: OffsetDateTime) -> String {
        Timestamp::try_from(at).unwrap().to_string()
    }

    #[test]
    fn unit_rfc3339_fixed_width() {
        let cases = [
            (
                datetime!(2026-09-23 17:42:31.123 UTC),
                "2026-09-23T17:42:31.123Z",
            ),
            (
                datetime!(2026-09-23 17:42:31 UTC),
                "2026-09-23T17:42:31.000Z",
            ),
            (
                datetime!(2026-09-23 17:42:31.1 UTC),
                "2026-09-23T17:42:31.100Z",
            ),
            (
                datetime!(2026-09-23 17:42:31.123_999_999 UTC),
                "2026-09-23T17:42:31.123Z",
            ),
            (
                datetime!(2026-09-23 14:42:31.123 -3),
                "2026-09-23T17:42:31.123Z",
            ),
            (
                datetime!(2026-09-24 02:12:31.5 +8:30),
                "2026-09-23T17:42:31.500Z",
            ),
            (
                datetime!(1970-01-01 00:00:00 UTC),
                "1970-01-01T00:00:00.000Z",
            ),
            (
                datetime!(0000-01-01 00:00:00 UTC),
                "0000-01-01T00:00:00.000Z",
            ),
            (
                datetime!(0999-01-01 00:00:00 UTC),
                "0999-01-01T00:00:00.000Z",
            ),
            (
                datetime!(9999-12-31 23:59:59.999_999_999 UTC),
                "9999-12-31T23:59:59.999Z",
            ),
        ];

        for (at, expected) in cases {
            let text = render(at);
            assert_eq!(text, expected);
            assert_eq!(text.len(), 24);
            assert!(text.ends_with('Z'));
            assert_eq!(&text[19..20], ".");
            assert!(OffsetDateTime::parse(&text, &Rfc3339).is_ok());
        }
    }

    #[test]
    fn unit_timestamp_lexical_order_is_chronological() {
        let instants = [
            datetime!(0999-12-31 23:59:59.999 UTC),
            datetime!(1970-01-01 00:00:00 UTC),
            datetime!(2026-09-23 17:42:31.009 UTC),
            datetime!(2026-09-23 17:42:31.010 UTC),
            datetime!(2026-09-23 17:42:31.100 UTC),
            datetime!(2026-09-23 17:42:32 UTC),
            datetime!(2026-09-23 19:00:00 +1),
            datetime!(2026-10-01 00:00:00 UTC),
            datetime!(9999-12-31 23:59:59.999 UTC),
        ];
        let mut rendered: Vec<String> = instants.iter().map(|at| render(*at)).collect();
        let chronological = rendered.clone();
        rendered.sort();

        assert_eq!(rendered, chronological);
    }

    #[test]
    fn unit_timestamp_round_trip_preserves_milliseconds() {
        for text in [
            "2026-09-23T17:42:31.123Z",
            "2026-09-23T17:42:31.000Z",
            "2024-02-29T23:59:59.999Z",
            "0000-01-01T00:00:00.001Z",
            "9999-12-31T23:59:59.999Z",
        ] {
            let parsed: Timestamp = text.parse().unwrap();
            assert_eq!(parsed.to_string(), text);
            assert_eq!(parsed.get(), OffsetDateTime::parse(text, &Rfc3339).unwrap());
        }

        let at = Timestamp::try_from(datetime!(2026-09-23 17:42:31.123_456 UTC)).unwrap();
        assert_eq!(at.get(), datetime!(2026-09-23 17:42:31.123 UTC));
        assert_eq!(at.to_string().parse::<Timestamp>(), Ok(at));
    }

    #[test]
    fn unit_timestamp_rejects_malformed() {
        for text in [
            "",
            "2026-09-23",
            "2026-09-23T17:42:31Z",
            "2026-09-23T17:42:31.1Z",
            "2026-09-23T17:42:31.1234Z",
            "2026-09-23T17:42:31.123",
            "2026-09-23T17:42:31.123z",
            "2026-09-23t17:42:31.123Z",
            "2026-09-23 17:42:31.123Z",
            "2026-09-23T17:42:31.123+00:00",
            "2026-09-23T14:42:31.123-03:00",
            "2026-09-23T17:42:31,123Z",
            "2026-9-23T17:42:31.123Z",
            "+2026-09-23T17:42:31.123Z",
            "-0001-01-01T00:00:00.000Z",
            "2026-02-30T00:00:00.000Z",
            "2026-09-23T24:00:00.000Z",
            "2026-09-23T17:42:60.000Z",
            " 2026-09-23T17:42:31.123Z",
            "2026-09-23T17:42:31.123Z ",
        ] {
            assert_eq!(text.parse::<Timestamp>(), Err(InvalidTimestamp), "{text:?}");
        }
    }

    #[test]
    fn unit_timestamp_rejects_years_outside_four_digits() {
        assert_eq!(
            Timestamp::try_from(datetime!(-0001-12-31 23:59:59 UTC)),
            Err(InvalidTimestamp)
        );
        assert_eq!(
            Timestamp::try_from(datetime!(0000-01-01 00:59:59 +1)),
            Err(InvalidTimestamp)
        );
        assert!(!InvalidTimestamp.to_string().is_empty());
    }
}
