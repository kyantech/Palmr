use time::{
    format_description::BorrowedFormatItem, macros::format_description, OffsetDateTime,
    PrimitiveDateTime,
};

const IMF_FIXDATE: &[BorrowedFormatItem<'_>] = format_description!(
    "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRangeSpec {
    Bounded { first: u64, last: u64 },
    From { first: u64 },
    Suffix { length: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    start: u64,
    end_inclusive: u64,
    size: u64,
}

impl ByteRange {
    pub const fn start(self) -> u64 {
        self.start
    }

    pub const fn end_inclusive(self) -> u64 {
        self.end_inclusive
    }

    pub const fn size(self) -> u64 {
        self.size
    }

    pub const fn length(self) -> u64 {
        self.end_inclusive - self.start + 1
    }

    pub fn content_range(self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end_inclusive, self.size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsatisfiedRange {
    size: u64,
}

impl UnsatisfiedRange {
    pub const fn size(self) -> u64 {
        self.size
    }

    pub fn content_range(self) -> String {
        format!("bytes */{}", self.size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeDecision {
    Full,
    Partial(ByteRange),
    Unsatisfiable(UnsatisfiedRange),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurrentValidators<'a> {
    pub etag: Option<&'a str>,
    pub last_modified: Option<OffsetDateTime>,
}

pub fn decide(
    range: Option<&[u8]>,
    if_range: Option<&[u8]>,
    current: CurrentValidators<'_>,
    size: u64,
) -> RangeDecision {
    let Some(Ok(spec)) = range.map(parse_first_byte_range) else {
        return RangeDecision::Full;
    };
    if if_range.is_some_and(|validator| !if_range_matches(validator, current)) {
        return RangeDecision::Full;
    }
    spec.resolve(size)
}

pub fn parse_first_byte_range(value: &[u8]) -> Result<ByteRangeSpec, MalformedRange> {
    let value = trim_ows(value);
    let equals = value
        .iter()
        .position(|&b| b == b'=')
        .ok_or(MalformedRange)?;
    if !value[..equals].eq_ignore_ascii_case(b"bytes") {
        return Err(MalformedRange);
    }
    let first = value[equals + 1..]
        .split(|&b| b == b',')
        .map(trim_ows)
        .find(|element| !element.is_empty())
        .ok_or(MalformedRange)?;
    parse_spec(first)
}

fn parse_spec(spec: &[u8]) -> Result<ByteRangeSpec, MalformedRange> {
    let dash = spec.iter().position(|&b| b == b'-').ok_or(MalformedRange)?;
    let (first, last) = (&spec[..dash], &spec[dash + 1..]);
    match (first.is_empty(), last.is_empty()) {
        (true, true) => Err(MalformedRange),
        (true, false) => Ok(ByteRangeSpec::Suffix {
            length: parse_position(last)?,
        }),
        (false, true) => Ok(ByteRangeSpec::From {
            first: parse_position(first)?,
        }),
        (false, false) => {
            let first = parse_position(first)?;
            let last = parse_position(last)?;
            if last < first {
                return Err(MalformedRange);
            }
            Ok(ByteRangeSpec::Bounded { first, last })
        }
    }
}

fn parse_position(digits: &[u8]) -> Result<u64, MalformedRange> {
    if digits.is_empty() {
        return Err(MalformedRange);
    }
    digits.iter().try_fold(0_u64, |value, &b| {
        if !b.is_ascii_digit() {
            return Err(MalformedRange);
        }
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(b - b'0')))
            .ok_or(MalformedRange)
    })
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    let is_ows = |b: &u8| *b == b' ' || *b == b'\t';
    let start = bytes.iter().position(|b| !is_ows(b)).unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_ows(b))
        .map_or(start, |at| at + 1);
    &bytes[start..end]
}

impl ByteRangeSpec {
    pub fn resolve(self, size: u64) -> RangeDecision {
        let unsatisfiable = RangeDecision::Unsatisfiable(UnsatisfiedRange { size });
        let Some(last_byte) = size.checked_sub(1) else {
            return match self {
                Self::Suffix { length } if length > 0 => RangeDecision::Full,
                _ => unsatisfiable,
            };
        };
        let (start, end_inclusive) = match self {
            Self::Bounded { first, last } if first < size => (first, last.min(last_byte)),
            Self::From { first } if first < size => (first, last_byte),
            Self::Suffix { length } if length > 0 => (size - length.min(size), last_byte),
            _ => return unsatisfiable,
        };
        RangeDecision::Partial(ByteRange {
            start,
            end_inclusive,
            size,
        })
    }
}

pub fn if_range_matches(validator: &[u8], current: CurrentValidators<'_>) -> bool {
    let Ok(validator) = std::str::from_utf8(trim_ows(validator)) else {
        return false;
    };
    if validator.starts_with('"') || validator.starts_with("W/") {
        return current
            .etag
            .is_some_and(|etag| is_strong_etag(validator) && validator == etag);
    }
    current.last_modified.is_some_and(|last_modified| {
        parse_http_date(validator).is_some_and(|date| date == last_modified.truncate_to_second())
    })
}

fn parse_http_date(text: &str) -> Option<OffsetDateTime> {
    let date = PrimitiveDateTime::parse(text, IMF_FIXDATE).ok()?;
    let canonical = date.format(IMF_FIXDATE).ok()?;
    (canonical == text).then(|| date.assume_utc())
}

fn is_strong_etag(tag: &str) -> bool {
    tag.len() >= 2
        && tag.starts_with('"')
        && tag.ends_with('"')
        && tag[1..tag.len() - 1]
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x7e).contains(&b))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::macros::datetime;

    use super::*;

    const CURRENT: CurrentValidators<'static> = CurrentValidators {
        etag: Some("\"v2\""),
        last_modified: Some(datetime!(2026-09-25 10:15:30.250 UTC)),
    };

    #[derive(Debug)]
    enum Expect {
        Full,
        Partial(&'static str, u64),
        Unsatisfiable(&'static str),
    }

    #[rstest]
    #[case::no_range(None, None, 1000, Expect::Full)]
    #[case::closed(
        Some("bytes=0-499"),
        None,
        1000,
        Expect::Partial("bytes 0-499/1000", 500)
    )]
    #[case::open_ended(
        Some("bytes=500-"),
        None,
        1000,
        Expect::Partial("bytes 500-999/1000", 500)
    )]
    #[case::suffix(
        Some("bytes=-500"),
        None,
        1000,
        Expect::Partial("bytes 500-999/1000", 500)
    )]
    #[case::whole_as_range(
        Some("bytes=0-"),
        None,
        1000,
        Expect::Partial("bytes 0-999/1000", 1000)
    )]
    #[case::single_byte(Some("bytes=7-7"), None, 1000, Expect::Partial("bytes 7-7/1000", 1))]
    #[case::end_beyond_eof(
        Some("bytes=900-5000"),
        None,
        1000,
        Expect::Partial("bytes 900-999/1000", 100)
    )]
    #[case::last_byte(
        Some("bytes=999-"),
        None,
        1000,
        Expect::Partial("bytes 999-999/1000", 1)
    )]
    #[case::start_at_eof(Some("bytes=1000-"), None, 1000, Expect::Unsatisfiable("bytes */1000"))]
    #[case::closed_start_at_eof(
        Some("bytes=1000-1010"),
        None,
        1000,
        Expect::Unsatisfiable("bytes */1000")
    )]
    #[case::start_beyond_eof(
        Some("bytes=99999999999-"),
        None,
        1000,
        Expect::Unsatisfiable("bytes */1000")
    )]
    #[case::suffix_larger_than_file(
        Some("bytes=-5000"),
        None,
        1000,
        Expect::Partial("bytes 0-999/1000", 1000)
    )]
    #[case::suffix_equal_to_file(
        Some("bytes=-1000"),
        None,
        1000,
        Expect::Partial("bytes 0-999/1000", 1000)
    )]
    #[case::suffix_zero(Some("bytes=-0"), None, 1000, Expect::Unsatisfiable("bytes */1000"))]
    #[case::empty_open(Some("bytes=0-"), None, 0, Expect::Unsatisfiable("bytes */0"))]
    #[case::empty_closed(Some("bytes=0-0"), None, 0, Expect::Unsatisfiable("bytes */0"))]
    #[case::empty_suffix(Some("bytes=-1"), None, 0, Expect::Full)]
    #[case::empty_suffix_zero(Some("bytes=-0"), None, 0, Expect::Unsatisfiable("bytes */0"))]
    #[case::one_byte_closed(Some("bytes=0-0"), None, 1, Expect::Partial("bytes 0-0/1", 1))]
    #[case::one_byte_open(Some("bytes=0-"), None, 1, Expect::Partial("bytes 0-0/1", 1))]
    #[case::one_byte_suffix(Some("bytes=-9"), None, 1, Expect::Partial("bytes 0-0/1", 1))]
    #[case::one_byte_past_end(Some("bytes=1-"), None, 1, Expect::Unsatisfiable("bytes */1"))]
    #[case::unit_case_insensitive(Some("BYTES=0-0"), None, 10, Expect::Partial("bytes 0-0/10", 1))]
    #[case::optional_whitespace(
        Some(" bytes=2-3 ,\t4-5 "),
        None,
        10,
        Expect::Partial("bytes 2-3/10", 2)
    )]
    #[case::leading_zeros(
        Some("bytes=0000000000000000000000002-03"),
        None,
        10,
        Expect::Partial("bytes 2-3/10", 2)
    )]
    #[case::malformed_empty_set(Some("bytes="), None, 10, Expect::Full)]
    #[case::malformed_only_commas(Some("bytes=, ,"), None, 10, Expect::Full)]
    #[case::malformed_letters(Some("bytes=abc-def"), None, 10, Expect::Full)]
    #[case::malformed_unit(Some("items=0-10"), None, 10, Expect::Full)]
    #[case::malformed_bare_dash(Some("bytes=-"), None, 10, Expect::Full)]
    #[case::malformed_no_dash(Some("bytes=5"), None, 10, Expect::Full)]
    #[case::malformed_no_equals(Some("bytes 0-5"), None, 10, Expect::Full)]
    #[case::malformed_sign(Some("bytes=+1-2"), None, 10, Expect::Full)]
    #[case::malformed_double_dash(Some("bytes=1-2-3"), None, 10, Expect::Full)]
    #[case::malformed_inner_space(Some("bytes=1 -2"), None, 10, Expect::Full)]
    #[case::malformed_last_before_first(Some("bytes=5-2"), None, 10, Expect::Full)]
    #[case::malformed_non_ascii(Some("bytes=\u{0661}-2"), None, 10, Expect::Full)]
    #[case::overflow_first(Some("bytes=18446744073709551616-"), None, 10, Expect::Full)]
    #[case::overflow_last(Some("bytes=0-18446744073709551616"), None, 10, Expect::Full)]
    #[case::overflow_suffix(Some("bytes=-99999999999999999999999"), None, 10, Expect::Full)]
    #[case::max_size_whole(
        Some("bytes=0-18446744073709551615"),
        None,
        u64::MAX,
        Expect::Partial("bytes 0-18446744073709551614/18446744073709551615", u64::MAX)
    )]
    #[case::max_size_suffix(
        Some("bytes=-18446744073709551615"),
        None,
        u64::MAX,
        Expect::Partial("bytes 0-18446744073709551614/18446744073709551615", u64::MAX)
    )]
    #[case::max_size_last_byte(
        Some("bytes=18446744073709551614-"),
        None,
        u64::MAX,
        Expect::Partial(
            "bytes 18446744073709551614-18446744073709551614/18446744073709551615",
            1
        )
    )]
    #[case::max_size_start_at_eof(
        Some("bytes=18446744073709551615-"),
        None,
        u64::MAX,
        Expect::Unsatisfiable("bytes */18446744073709551615")
    )]
    #[case::max_size_one_byte_suffix(
        Some("bytes=-1"),
        None,
        u64::MAX,
        Expect::Partial(
            "bytes 18446744073709551614-18446744073709551614/18446744073709551615",
            1
        )
    )]
    #[case::multi_first_only(
        Some("bytes=0-9,100-109"),
        None,
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::multi_not_coalesced(
        Some("bytes=0-9,10-19"),
        None,
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::multi_second_never_rescues(
        Some("bytes=9999-10000,0-9"),
        None,
        5000,
        Expect::Unsatisfiable("bytes */5000")
    )]
    #[case::multi_empty_elements_skipped(
        Some("bytes=,,0-9"),
        None,
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::multi_tail_ignored(
        Some("bytes=0-9,garbage"),
        None,
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::multi_malformed_first(Some("bytes=x-1,0-9"), None, 200, Expect::Full)]
    #[case::if_range_etag_match(
        Some("bytes=0-9"),
        Some("\"v2\""),
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::if_range_etag_stale(Some("bytes=0-9"), Some("\"v1\""), 200, Expect::Full)]
    #[case::if_range_weak_request(Some("bytes=0-9"), Some("W/\"v2\""), 200, Expect::Full)]
    #[case::if_range_stale_beats_unsatisfiable(
        Some("bytes=500-"),
        Some("\"v1\""),
        200,
        Expect::Full
    )]
    #[case::if_range_match_keeps_unsatisfiable(
        Some("bytes=500-"),
        Some("\"v2\""),
        200,
        Expect::Unsatisfiable("bytes */200")
    )]
    #[case::if_range_date_match(
        Some("bytes=0-9"),
        Some("Fri, 25 Sep 2026 10:15:30 GMT"),
        200,
        Expect::Partial("bytes 0-9/200", 10)
    )]
    #[case::if_range_date_stale(
        Some("bytes=0-9"),
        Some("Fri, 25 Sep 2026 10:15:29 GMT"),
        200,
        Expect::Full
    )]
    #[case::if_range_date_wrong_weekday(
        Some("bytes=0-9"),
        Some("Sat, 25 Sep 2026 10:15:30 GMT"),
        200,
        Expect::Full
    )]
    #[case::if_range_malformed(Some("bytes=0-9"), Some("yesterday"), 200, Expect::Full)]
    #[case::if_range_empty(Some("bytes=0-9"), Some(""), 200, Expect::Full)]
    #[case::if_range_without_range(None, Some("\"v2\""), 200, Expect::Full)]
    #[case::if_range_with_malformed_range(Some("bytes=x"), Some("\"v2\""), 200, Expect::Full)]
    fn unit_range_header_decision_table(
        #[case] range: Option<&str>,
        #[case] if_range: Option<&str>,
        #[case] size: u64,
        #[case] expected: Expect,
    ) {
        let decision = decide(
            range.map(str::as_bytes),
            if_range.map(str::as_bytes),
            CURRENT,
            size,
        );
        match (decision, expected) {
            (RangeDecision::Full, Expect::Full) => {}
            (RangeDecision::Partial(range), Expect::Partial(content_range, length)) => {
                assert_eq!(range.content_range(), content_range);
                assert_eq!(range.length(), length);
                assert_eq!(range.end_inclusive() - range.start() + 1, length);
                assert!(range.end_inclusive() < range.size());
                assert_eq!(range.size(), size);
            }
            (RangeDecision::Unsatisfiable(unsatisfied), Expect::Unsatisfiable(content_range)) => {
                assert_eq!(unsatisfied.content_range(), content_range);
                assert_eq!(unsatisfied.size(), size);
            }
            (decision, expected) => {
                panic!("{range:?} / {if_range:?} on {size}: got {decision:?}, want {expected:?}")
            }
        }
    }

    #[test]
    fn unit_range_if_range_requires_strong_current_etag() {
        let weak = CurrentValidators {
            etag: Some("W/\"v2\""),
            last_modified: None,
        };
        assert!(!if_range_matches(b"W/\"v2\"", weak));
        assert!(!if_range_matches(b"\"v2\"", weak));
        assert!(!if_range_matches(b"\"v2\"", CurrentValidators::default()));
        assert!(!if_range_matches(
            b"Fri, 25 Sep 2026 10:15:30 GMT",
            CurrentValidators::default()
        ));
        assert!(!if_range_matches(&[0xff, b'"'], CURRENT));
    }

    #[test]
    fn unit_range_long_multi_range_tail_is_ignored() {
        let mut header = String::from("bytes=0-0");
        for _ in 0..100_000 {
            header.push_str(",1-1");
        }
        assert_eq!(
            parse_first_byte_range(header.as_bytes()),
            Ok(ByteRangeSpec::Bounded { first: 0, last: 0 })
        );
    }
}
