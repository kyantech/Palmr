use std::fmt;

use super::normalize::normalize;

pub const MAX_NAME_BYTES: usize = 255;
pub const MAX_NORMALIZED_CHARS: usize = 255;
pub const MAX_NAME_ATTEMPTS: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidName {
    Empty,
    Reserved,
    Separator,
    Control,
    TooLong,
    NormalizedOutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateError {
    DoesNotFit,
    AttemptsExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCandidate {
    display: String,
    normalized: String,
}

impl NameCandidate {
    pub fn new(display: impl Into<String>) -> Result<Self, InvalidName> {
        let display = display.into();
        validate_display(&display)?;
        let normalized = normalize(&display);
        if !(1..=MAX_NORMALIZED_CHARS).contains(&normalized.chars().count()) {
            return Err(InvalidName::NormalizedOutOfRange);
        }
        Ok(Self {
            display,
            normalized,
        })
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    pub fn into_display(self) -> String {
        self.display
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameSeries {
    requested: NameCandidate,
    base: String,
    tail: String,
}

impl NameSeries {
    pub fn parse(requested: &str) -> Result<Self, InvalidName> {
        let requested = NameCandidate::new(requested)?;
        let (base, tail) = split_extension(requested.display());
        let (base, tail) = (base.to_owned(), tail.to_owned());
        Ok(Self {
            requested,
            base,
            tail,
        })
    }

    pub fn candidate(&self, attempt: u32) -> Result<NameCandidate, CandidateError> {
        if attempt == 0 {
            return Ok(self.requested.clone());
        }
        if attempt > MAX_NAME_ATTEMPTS {
            return Err(CandidateError::AttemptsExhausted);
        }
        let suffix = format!(" ({attempt})");
        let budget = MAX_NAME_BYTES
            .checked_sub(suffix.len() + self.tail.len())
            .ok_or(CandidateError::DoesNotFit)?;
        let mut base = truncate_to_boundary(&self.base, budget);
        loop {
            match NameCandidate::new(format!("{base}{suffix}{}", self.tail)) {
                Ok(candidate) => return Ok(candidate),
                Err(InvalidName::NormalizedOutOfRange) => {
                    let (last, _) = base
                        .char_indices()
                        .next_back()
                        .ok_or(CandidateError::DoesNotFit)?;
                    base = &base[..last];
                }
                Err(_) => return Err(CandidateError::DoesNotFit),
            }
        }
    }
}

fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 && index + 1 < name.len() => name.split_at(index),
        Some(index) if index > 0 => (&name[..index], ""),
        _ => (name, ""),
    }
}

fn truncate_to_boundary(text: &str, budget: usize) -> &str {
    let mut end = budget.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn validate_display(name: &str) -> Result<(), InvalidName> {
    if name.is_empty() {
        return Err(InvalidName::Empty);
    }
    if name == "." || name == ".." {
        return Err(InvalidName::Reserved);
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(InvalidName::TooLong);
    }
    if name.contains(['/', '\\']) {
        return Err(InvalidName::Separator);
    }
    if name.chars().any(char::is_control) {
        return Err(InvalidName::Control);
    }
    Ok(())
}

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "name is empty",
            Self::Reserved => "name is a reserved path component",
            Self::Separator => "name contains a path separator",
            Self::Control => "name contains a control character",
            Self::TooLong => "name exceeds 255 bytes",
            Self::NormalizedOutOfRange => "normalized name is empty or too long",
        })
    }
}

impl fmt::Display for CandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DoesNotFit => "disambiguated name exceeds the name length limit",
            Self::AttemptsExhausted => "disambiguation attempts exhausted",
        })
    }
}

impl std::error::Error for InvalidName {}

impl std::error::Error for CandidateError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::{
        CandidateError, InvalidName, NameCandidate, NameSeries, MAX_NAME_ATTEMPTS, MAX_NAME_BYTES,
        MAX_NORMALIZED_CHARS,
    };
    use crate::domain::normalize::{normalize, tests::unicode_text};

    fn display(name: &str, attempt: u32) -> String {
        NameSeries::parse(name)
            .unwrap()
            .candidate(attempt)
            .unwrap()
            .into_display()
    }

    #[test]
    #[allow(non_snake_case)]
    fn regression_R062_duplicate_name_generator() {
        for (name, attempt_one, attempt_two) in [
            ("README", "README (1)", "README (2)"),
            (".env", ".env (1)", ".env (2)"),
            ("archive.tar.gz", "archive.tar (1).gz", "archive.tar (2).gz"),
            ("photo.jpg", "photo (1).jpg", "photo (2).jpg"),
            ("photo (1).jpg", "photo (1) (1).jpg", "photo (1) (2).jpg"),
            (
                "photo (1) (1).jpg",
                "photo (1) (1) (1).jpg",
                "photo (1) (1) (2).jpg",
            ),
            (".env.local", ".env (1).local", ".env (2).local"),
            ("a.", "a (1)", "a (2)"),
            ("a ", "a  (1)", "a  (2)"),
            ("a .txt", "a  (1).txt", "a  (2).txt"),
            ("...", ".. (1)", ".. (2)"),
            ("日本語.txt", "日本語 (1).txt", "日本語 (2).txt"),
        ] {
            assert_eq!(display(name, 0), name, "{name:?} attempt 0");
            assert_eq!(display(name, 1), attempt_one, "{name:?} attempt 1");
            assert_eq!(display(name, 2), attempt_two, "{name:?} attempt 2");
        }
    }

    #[test]
    fn unit_naming_extensionless_candidates_never_gain_a_trailing_dot() {
        for name in ["README", ".env", "a", "Makefile", ".gitignore", "x (1)"] {
            for attempt in [1, 2, 9, 10, 999, MAX_NAME_ATTEMPTS] {
                let candidate = display(name, attempt);
                assert!(!candidate.ends_with('.'), "{name:?} -> {candidate:?}");
                assert_eq!(candidate.matches('.').count(), name.matches('.').count());
            }
        }
    }

    #[test]
    fn unit_naming_empty_extension_after_the_last_dot_adds_no_dot() {
        for (name, expected) in [
            ("a.", "a (1)"),
            ("a..", "a. (1)"),
            ("trailing.dots..", "trailing.dots. (1)"),
            ("archive.tar.gz", "archive.tar (1).gz"),
            (".env", ".env (1)"),
            ("README", "README (1)"),
        ] {
            let candidate = NameSeries::parse(name).unwrap().candidate(1).unwrap();
            assert_eq!(candidate.display(), expected, "{name:?}");
            assert!(!candidate.display().ends_with('.'));
        }
    }

    #[test]
    fn unit_naming_counter_goes_before_the_last_extension_only() {
        assert_eq!(display("a.b.c.d", 3), "a.b.c (3).d");
        assert_eq!(display("trailing.dots..", 1), "trailing.dots. (1)");
        assert_eq!(display(".hidden.tar.gz", 1), ".hidden.tar (1).gz");
    }

    #[test]
    fn unit_naming_existing_counters_are_not_parsed() {
        assert_eq!(display("photo (1).jpg", 1), "photo (1) (1).jpg");
        assert_eq!(display("photo (7)", 1), "photo (7) (1)");
    }

    #[test]
    fn unit_naming_candidates_are_normalized_independently() {
        let upper = NameSeries::parse("Photo.JPG").unwrap();
        let lower = NameSeries::parse("photo.jpg").unwrap();
        for attempt in 0..=5 {
            let upper = upper.candidate(attempt).unwrap();
            let lower = lower.candidate(attempt).unwrap();
            assert_eq!(upper.normalized(), lower.normalized());
            assert_eq!(upper.normalized(), normalize(upper.display()));
        }
        assert_eq!(upper.candidate(1).unwrap().display(), "Photo (1).JPG");
        assert_eq!(upper.candidate(1).unwrap().normalized(), "photo (1).jpg");

        let composed = NameSeries::parse("caf\u{e9}.txt").unwrap();
        let decomposed = NameSeries::parse("cafe\u{301}.txt").unwrap();
        for attempt in 0..=3 {
            assert_eq!(
                composed.candidate(attempt).unwrap().normalized(),
                decomposed.candidate(attempt).unwrap().normalized()
            );
        }
        assert_eq!(
            decomposed.candidate(1).unwrap().display(),
            "cafe\u{301} (1).txt"
        );

        let fullwidth = NameSeries::parse("\u{ff30}hoto.jpg").unwrap();
        assert_eq!(fullwidth.candidate(0).unwrap().normalized(), "photo.jpg");
        assert_eq!(
            fullwidth.candidate(1).unwrap().normalized(),
            "photo (1).jpg"
        );
    }

    #[test]
    fn unit_naming_trailing_space_is_preserved_and_normalizes_like_the_trimmed_name() {
        let spaced = NameSeries::parse("a ").unwrap();
        assert_eq!(spaced.candidate(0).unwrap().display(), "a ");
        assert_eq!(spaced.candidate(0).unwrap().normalized(), "a");
        let first = spaced.candidate(1).unwrap();
        assert_eq!(first.display(), "a  (1)");
        assert_eq!(first.normalized(), "a  (1)");
        assert_ne!(
            first.normalized(),
            NameSeries::parse("a")
                .unwrap()
                .candidate(1)
                .unwrap()
                .normalized()
        );
    }

    #[test]
    fn unit_naming_rejects_invalid_requested_names() {
        for (name, expected) in [
            ("", InvalidName::Empty),
            (".", InvalidName::Reserved),
            ("..", InvalidName::Reserved),
            ("a/b", InvalidName::Separator),
            ("/", InvalidName::Separator),
            ("a\\b", InvalidName::Separator),
            ("a\0b", InvalidName::Control),
            ("a\nb", InvalidName::Control),
            ("a\u{7f}", InvalidName::Control),
            ("a\u{85}", InvalidName::Control),
            ("   ", InvalidName::NormalizedOutOfRange),
            ("\u{3000}", InvalidName::NormalizedOutOfRange),
            (
                "\u{fdfa}".repeat(15).as_str(),
                InvalidName::NormalizedOutOfRange,
            ),
        ] {
            assert_eq!(NameSeries::parse(name).unwrap_err(), expected, "{name:?}");
        }
    }

    #[test]
    fn unit_naming_length_boundary_is_measured_in_bytes() {
        let longest = "x".repeat(MAX_NAME_BYTES);
        assert!(NameSeries::parse(&longest).is_ok());
        assert_eq!(
            NameSeries::parse(&"x".repeat(MAX_NAME_BYTES + 1)).unwrap_err(),
            InvalidName::TooLong
        );

        let two_byte = format!("{}a", "\u{e9}".repeat(127));
        assert_eq!(two_byte.len(), MAX_NAME_BYTES);
        assert!(NameSeries::parse(&two_byte).is_ok());
        assert_eq!(
            NameSeries::parse(&"\u{e9}".repeat(128)).unwrap_err(),
            InvalidName::TooLong
        );

        let four_byte = format!("{}abc", "\u{1f4e6}".repeat(63));
        assert_eq!(four_byte.len(), MAX_NAME_BYTES);
        assert!(NameSeries::parse(&four_byte).is_ok());
    }

    fn assert_representable(series: &NameSeries, attempt: u32) -> NameCandidate {
        let candidate = series.candidate(attempt).unwrap();
        assert!(candidate.display().len() <= MAX_NAME_BYTES);
        assert!(candidate.normalized().chars().count() <= MAX_NORMALIZED_CHARS);
        assert_eq!(candidate, NameCandidate::new(candidate.display()).unwrap());
        assert_eq!(candidate, series.candidate(attempt).unwrap());
        candidate
    }

    #[test]
    fn unit_naming_maximum_length_ascii_name_keeps_both_by_truncating_the_base() {
        let longest = "x".repeat(MAX_NAME_BYTES);
        let series = NameSeries::parse(&longest).unwrap();
        assert_eq!(series.candidate(0).unwrap().display(), longest);
        for (attempt, suffix) in [
            (1, " (1)"),
            (9, " (9)"),
            (10, " (10)"),
            (999, " (999)"),
            (1000, " (1000)"),
        ] {
            let candidate = assert_representable(&series, attempt);
            assert_eq!(candidate.display().len(), MAX_NAME_BYTES);
            assert_eq!(
                candidate.display(),
                format!("{}{suffix}", "x".repeat(MAX_NAME_BYTES - suffix.len()))
            );
        }
    }

    #[test]
    fn unit_naming_truncation_preserves_the_whole_extension_and_suffix() {
        let name = format!("{}.tar.gz", "x".repeat(MAX_NAME_BYTES - 7));
        assert_eq!(name.len(), MAX_NAME_BYTES);
        let series = NameSeries::parse(&name).unwrap();
        let candidate = assert_representable(&series, 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1).gz", "x".repeat(MAX_NAME_BYTES - 7))
        );
        let candidate = assert_representable(&series, 12);
        assert!(candidate.display().ends_with(" (12).gz"));
        assert_eq!(candidate.display().len(), MAX_NAME_BYTES);

        let long_extension = format!("{}.{}", "b".repeat(200), "e".repeat(40));
        let candidate = assert_representable(&NameSeries::parse(&long_extension).unwrap(), 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1).{}", "b".repeat(200), "e".repeat(40))
        );

        let squeezed = format!("{}.{}", "b".repeat(100), "e".repeat(154));
        assert_eq!(squeezed.len(), MAX_NAME_BYTES);
        let candidate = assert_representable(&NameSeries::parse(&squeezed).unwrap(), 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1).{}", "b".repeat(96), "e".repeat(154))
        );
        assert_eq!(candidate.display().len(), MAX_NAME_BYTES);
    }

    #[test]
    fn unit_naming_truncation_never_splits_a_utf8_scalar() {
        for unit in ["\u{e9}", "\u{65e5}", "\u{1f4e6}"] {
            for pad in 0..unit.len() {
                let count = (MAX_NAME_BYTES - pad) / unit.len();
                let name = format!("{}{}", "p".repeat(pad), unit.repeat(count));
                assert!(name.len() <= MAX_NAME_BYTES);
                let series = NameSeries::parse(&name).unwrap();
                let first = assert_representable(&series, 1);
                let again = assert_representable(&series, 1);
                assert_eq!(first, again);
                let base = first.display().strip_suffix(" (1)").unwrap();
                assert!(name.starts_with(base));
                assert!(MAX_NAME_BYTES - first.display().len() < unit.len());
            }
        }

        let name = format!("{}.txt", "\u{1f4e6}".repeat(62));
        assert_eq!(name.len(), 252);
        let candidate = assert_representable(&NameSeries::parse(&name).unwrap(), 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1).txt", "\u{1f4e6}".repeat(61))
        );
        assert_eq!(candidate.display().len(), 252);

        let name = format!("{}abc", "\u{1f4e6}".repeat(63));
        assert_eq!(name.len(), MAX_NAME_BYTES);
        let candidate = assert_representable(&NameSeries::parse(&name).unwrap(), 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1)", "\u{1f4e6}".repeat(62))
        );
        assert_eq!(candidate.display().len(), 252);
    }

    #[test]
    fn unit_naming_normalization_expansion_shrinks_the_base_deterministically() {
        let name = "\u{fdfa}".repeat(14);
        let series = NameSeries::parse(&name).unwrap();
        assert_eq!(
            series.candidate(0).unwrap().normalized().chars().count(),
            252
        );
        let candidate = assert_representable(&series, 1);
        assert_eq!(
            candidate.display(),
            format!("{} (1)", "\u{fdfa}".repeat(13))
        );
        assert_eq!(candidate, series.candidate(1).unwrap());

        let name = format!("{}{}.txt", "\u{fdfa}".repeat(13), "x".repeat(17));
        let series = NameSeries::parse(&name).unwrap();
        assert_eq!(
            series.candidate(0).unwrap().normalized().chars().count(),
            255
        );
        let candidate = assert_representable(&series, 1);
        assert_eq!(
            candidate.display(),
            format!("{}{} (1).txt", "\u{fdfa}".repeat(13), "x".repeat(13))
        );
    }

    #[test]
    fn unit_naming_no_representable_candidate_is_reported_not_truncated_into_the_fixed_parts() {
        let name = format!("a.{}", "e".repeat(250));
        assert_eq!(name.len(), 252);
        let series = NameSeries::parse(&name).unwrap();
        let candidate = assert_representable(&series, 1);
        assert_eq!(candidate.display(), format!(" (1).{}", "e".repeat(250)));
        assert_eq!(series.candidate(10), Err(CandidateError::DoesNotFit));

        let name = format!("a.{}", "e".repeat(253));
        let series = NameSeries::parse(&name).unwrap();
        assert_eq!(series.candidate(0).unwrap().display(), name);
        assert_eq!(series.candidate(1), Err(CandidateError::DoesNotFit));
        assert_eq!(series.candidate(1000), Err(CandidateError::DoesNotFit));
    }

    #[test]
    fn unit_naming_invalid_requested_name_is_not_a_candidate_error() {
        assert_eq!(
            NameSeries::parse(&"x".repeat(MAX_NAME_BYTES + 1)).unwrap_err(),
            InvalidName::TooLong
        );
        assert_eq!(
            NameSeries::parse(&format!("{}.txt", "x".repeat(MAX_NAME_BYTES))).unwrap_err(),
            InvalidName::TooLong
        );
    }

    #[test]
    fn unit_naming_attempt_bound_is_one_original_plus_one_thousand_suffixed() {
        let series = NameSeries::parse("photo.jpg").unwrap();
        assert_eq!(MAX_NAME_ATTEMPTS, 1_000);
        assert_eq!(
            series.candidate(MAX_NAME_ATTEMPTS).unwrap().display(),
            "photo (1000).jpg"
        );
        assert_eq!(
            series.candidate(MAX_NAME_ATTEMPTS + 1),
            Err(CandidateError::AttemptsExhausted)
        );
        assert_eq!(
            series.candidate(u32::MAX),
            Err(CandidateError::AttemptsExhausted)
        );
    }

    #[test]
    fn unit_naming_attempt_zero_is_the_requested_name_verbatim() {
        for name in ["README", "Straße.TXT", "e\u{301}.md", "a ", " a", "x (1)"] {
            let candidate = NameSeries::parse(name).unwrap().candidate(0).unwrap();
            assert_eq!(candidate.display(), name);
            assert_eq!(candidate.normalized(), normalize(name));
        }
    }

    fn model_split(name: &str) -> (&str, String) {
        match name.rfind('.') {
            Some(index) if index > 0 && index + 1 < name.len() => {
                (&name[..index], name[index..].to_owned())
            }
            Some(index) if index > 0 => (&name[..index], String::new()),
            _ => (name, String::new()),
        }
    }

    fn near_limit_name() -> impl Strategy<Value = String> {
        (
            prop::sample::select(vec!["x", "\u{e9}", "\u{65e5}", "\u{1f4e6}", "\u{fdfa}"]),
            prop::sample::select(vec![
                "",
                ".txt",
                ".tar.gz",
                ".",
                ".\u{65e5}\u{672c}",
                ".jpeg",
            ]),
            prop::sample::select(vec!["", "x", "\u{e9}", " ", "."]),
            238usize..=255,
        )
            .prop_map(|(unit, extension, pad, total)| {
                let body = total.saturating_sub(extension.len());
                let mut name = String::new();
                while name.len() + unit.len() <= body {
                    name.push_str(unit);
                }
                while name.len() + pad.len().max(1) <= body {
                    name.push_str(if pad.is_empty() { "y" } else { pad });
                }
                name.push_str(extension);
                name
            })
    }

    fn colliding_name() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => prop::sample::select(vec![
                "a", "A", "a.b", "A.B", "a (1)", "a (1).b", ".env", ".ENV", "x.tar.gz",
                "X.TAR.GZ", "README", "readme", "a.", "A.", "e\u{301}", "\u{e9}",
                "E\u{301}.TXT", "\u{c9}.txt", "\u{ff21}", "photo.jpg", "Photo.JPG",
            ])
            .prop_map(String::from),
            1 => unicode_text(),
        ]
    }

    proptest! {
        #[test]
        fn prop_duplicate_name_is_a_bijection_into_the_folder(
            requests in prop::collection::vec(colliding_name(), 1..40),
        ) {
            let mut folder: HashSet<String> = HashSet::new();
            for requested in requests {
                let Ok(series) = NameSeries::parse(&requested) else { continue };
                let mut placed = None;
                for attempt in 0..=MAX_NAME_ATTEMPTS {
                    let Ok(candidate) = series.candidate(attempt) else { break };
                    prop_assert_eq!(candidate.normalized(), normalize(candidate.display()));
                    prop_assert!(NameCandidate::new(candidate.display()).is_ok());
                    if !folder.contains(candidate.normalized()) {
                        placed = Some((attempt, candidate));
                        break;
                    }
                }
                let Some((attempt, candidate)) = placed else { continue };
                let before = folder.len();
                prop_assert!(folder.insert(candidate.normalized().to_owned()));
                prop_assert_eq!(folder.len(), before + 1);
                for earlier in 0..attempt {
                    let skipped = series.candidate(earlier).unwrap();
                    prop_assert!(folder.contains(skipped.normalized()));
                }
            }
        }

        #[test]
        fn prop_naming_candidates_are_deterministic_valid_and_pairwise_distinct(
            requested in colliding_name(),
        ) {
            let Ok(series) = NameSeries::parse(&requested) else { return Ok(()) };
            let again = NameSeries::parse(&requested).unwrap();
            let (model_base, tail) = model_split(&requested);
            let mut seen = HashSet::new();
            for attempt in 0..=16 {
                let candidate = series.candidate(attempt).unwrap();
                prop_assert_eq!(&candidate, &again.candidate(attempt).unwrap());
                prop_assert_eq!(candidate.normalized(), normalize(candidate.display()));
                prop_assert!(candidate.display().len() <= MAX_NAME_BYTES);
                prop_assert!(candidate.normalized().chars().count() <= MAX_NORMALIZED_CHARS);
                prop_assert!(NameCandidate::new(candidate.display()).is_ok());
                prop_assert!(seen.insert(candidate.normalized().to_owned()));
                if attempt >= 1 {
                    let expected = format!("{model_base} ({attempt}){tail}");
                    prop_assert!(!candidate.display().ends_with('.'));
                    prop_assert_eq!(candidate.display(), expected.as_str());
                }
            }
        }

        #[test]
        fn prop_naming_long_names_keep_suffix_and_extension_and_truncate_only_the_base(
            requested in near_limit_name(),
        ) {
            let Ok(series) = NameSeries::parse(&requested) else { return Ok(()) };
            let again = NameSeries::parse(&requested).unwrap();
            let (model_base, tail) = model_split(&requested);
            let mut seen = HashSet::new();
            for attempt in [1, 2, 9, 10, 11, 99, 100, 500, 999, MAX_NAME_ATTEMPTS] {
                let candidate = series.candidate(attempt).unwrap();
                prop_assert_eq!(&candidate, &again.candidate(attempt).unwrap());
                prop_assert!(candidate.display().len() <= MAX_NAME_BYTES);
                prop_assert!(candidate.normalized().chars().count() <= MAX_NORMALIZED_CHARS);
                prop_assert_eq!(candidate.normalized(), normalize(candidate.display()));
                prop_assert!(NameCandidate::new(candidate.display()).is_ok());
                prop_assert!(seen.insert(candidate.normalized().to_owned()));

                let fixed = format!(" ({attempt}){tail}");
                prop_assert!(candidate.display().ends_with(&fixed));
                let base = &candidate.display()[..candidate.display().len() - fixed.len()];
                prop_assert!(model_base.starts_with(base));
                let full = format!("{model_base}{fixed}");
                if NameCandidate::new(full.clone()).is_ok() {
                    prop_assert_eq!(candidate.display(), full.as_str());
                }
            }
        }
    }
}
