use std::fmt;

use unicode_normalization::UnicodeNormalization;

use super::naming::{InvalidName, NameCandidate};

pub const MAX_SEGMENTS: usize = 32;
pub const MAX_PATH_BYTES: usize = 1024;

const SEPARATOR: char = '/';
const FOREIGN_SEPARATOR: char = '\\';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidPath {
    Empty,
    Nul,
    LeadingSeparator,
    TrailingSeparator,
    EmptySegment,
    Unc,
    DriveLetter,
    TooManySegments,
    TooLong,
    Segment(InvalidName),
}

impl InvalidPath {
    pub const fn is_shape(&self) -> bool {
        matches!(self, Self::Empty | Self::TooManySegments | Self::TooLong)
    }
}

impl fmt::Display for InvalidPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("path has no segments"),
            Self::Nul => f.write_str("path contains a NUL character"),
            Self::LeadingSeparator => f.write_str("path starts with a separator"),
            Self::TrailingSeparator => f.write_str("path ends with a separator"),
            Self::EmptySegment => f.write_str("path contains an empty segment"),
            Self::Unc => f.write_str("path is a UNC path"),
            Self::DriveLetter => f.write_str("path starts with a drive letter"),
            Self::TooManySegments => write!(f, "path has more than {MAX_SEGMENTS} segments"),
            Self::TooLong => write!(f, "path exceeds {MAX_PATH_BYTES} bytes"),
            Self::Segment(error) => write!(f, "path segment is invalid: {error}"),
        }
    }
}

impl std::error::Error for InvalidPath {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirectoryPath {
    segments: Vec<NameCandidate>,
}

impl DirectoryPath {
    pub const fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    pub fn parse_prefix(prefix: &str) -> Result<Self, InvalidPath> {
        if prefix.is_empty() {
            return Ok(Self::root());
        }
        parse_wire(prefix).map(|segments| Self { segments })
    }

    pub fn from_segments<S: AsRef<str>>(segments: &[S]) -> Result<Self, InvalidPath> {
        if segments.len() > MAX_SEGMENTS {
            return Err(InvalidPath::TooManySegments);
        }
        let mut validated = Vec::with_capacity(segments.len());
        for segment in segments {
            let segment = segment.as_ref();
            if segment.contains('\0') {
                return Err(InvalidPath::Nul);
            }
            validated.push(validate_segment(segment)?);
        }
        if let Some(first) = segments.first() {
            if starts_with_drive_letter(first.as_ref()) {
                return Err(InvalidPath::DriveLetter);
            }
        }
        check_total_bytes(&validated)?;
        Ok(Self {
            segments: validated,
        })
    }

    pub fn segments(&self) -> &[NameCandidate] {
        &self.segments
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn joined(&self) -> String {
        join(&self.segments)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativePath {
    directory: DirectoryPath,
    leaf: NameCandidate,
}

impl RelativePath {
    pub fn parse(wire: &str) -> Result<Self, InvalidPath> {
        let mut segments = parse_wire(wire)?;
        let leaf = segments.pop().ok_or(InvalidPath::Empty)?;
        Ok(Self {
            directory: DirectoryPath { segments },
            leaf,
        })
    }

    pub const fn directory(&self) -> &DirectoryPath {
        &self.directory
    }

    pub const fn leaf(&self) -> &NameCandidate {
        &self.leaf
    }

    pub fn len(&self) -> usize {
        self.directory.len() + 1
    }

    pub fn joined(&self) -> String {
        if self.directory.is_empty() {
            self.leaf.display().to_owned()
        } else {
            format!("{}/{}", self.directory.joined(), self.leaf.display())
        }
    }

    pub fn split(self) -> (DirectoryPath, NameCandidate) {
        (self.directory, self.leaf)
    }
}

fn parse_wire(wire: &str) -> Result<Vec<NameCandidate>, InvalidPath> {
    if wire.is_empty() {
        return Err(InvalidPath::Empty);
    }
    if wire.contains('\0') {
        return Err(InvalidPath::Nul);
    }
    if wire.starts_with("\\\\") {
        return Err(InvalidPath::Unc);
    }
    let folded = wire.replace(FOREIGN_SEPARATOR, "/");
    if folded.starts_with(SEPARATOR) {
        return Err(InvalidPath::LeadingSeparator);
    }
    if folded.ends_with(SEPARATOR) {
        return Err(InvalidPath::TrailingSeparator);
    }
    if starts_with_drive_letter(&folded) {
        return Err(InvalidPath::DriveLetter);
    }
    let raw: Vec<&str> = folded.split(SEPARATOR).collect();
    if raw.len() > MAX_SEGMENTS {
        return Err(InvalidPath::TooManySegments);
    }
    let mut segments = Vec::with_capacity(raw.len());
    for segment in raw {
        if segment.is_empty() {
            return Err(InvalidPath::EmptySegment);
        }
        segments.push(validate_segment(segment)?);
    }
    check_total_bytes(&segments)?;
    Ok(segments)
}

fn validate_segment(segment: &str) -> Result<NameCandidate, InvalidPath> {
    let composed: String = segment.nfc().collect();
    NameCandidate::new(composed).map_err(InvalidPath::Segment)
}

fn starts_with_drive_letter(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

fn check_total_bytes(segments: &[NameCandidate]) -> Result<(), InvalidPath> {
    let separators = segments.len().saturating_sub(1);
    let total = segments
        .iter()
        .map(|segment| segment.display().len())
        .sum::<usize>()
        + separators;
    if total > MAX_PATH_BYTES {
        Err(InvalidPath::TooLong)
    } else {
        Ok(())
    }
}

fn join(segments: &[NameCandidate]) -> String {
    segments
        .iter()
        .map(NameCandidate::display)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{DirectoryPath, InvalidPath, RelativePath, MAX_PATH_BYTES, MAX_SEGMENTS};
    use crate::domain::naming::{InvalidName, MAX_NAME_BYTES};

    fn parsed(wire: &str) -> Vec<String> {
        let path = RelativePath::parse(wire)
            .unwrap_or_else(|error| panic!("{wire:?} must parse: {error}"));
        path.directory()
            .segments()
            .iter()
            .chain(std::iter::once(path.leaf()))
            .map(|segment| segment.display().to_owned())
            .collect()
    }

    fn rejected(wire: &str) -> InvalidPath {
        RelativePath::parse(wire).expect_err(&format!("{wire:?} must be rejected"))
    }

    #[test]
    fn unit_relative_path_grammar() {
        assert_eq!(parsed("file.txt"), ["file.txt"]);
        assert_eq!(
            parsed("Photos/2026/image.jpg"),
            ["Photos", "2026", "image.jpg"]
        );
        assert_eq!(parsed("café/foto.jpg"), ["café", "foto.jpg"]);
        assert_eq!(parsed("Folder\\File.txt"), ["Folder", "File.txt"]);
        assert_eq!(
            RelativePath::parse("Folder\\File.txt").unwrap().joined(),
            "Folder/File.txt"
        );
        assert_eq!(
            parsed("cafe\u{301}/foto.jpg"),
            ["caf\u{e9}", "foto.jpg"],
            "decomposed input is composed to NFC"
        );
        assert_eq!(parsed("a b/ c .txt"), ["a b", " c .txt"]);
        assert_eq!(parsed("Photos/.hidden"), ["Photos", ".hidden"]);
        assert_eq!(parsed("a...b/c"), ["a...b", "c"]);
        assert_eq!(parsed("a/b:c"), ["a", "b:c"]);
        assert_eq!(parsed("1:/c"), ["1:", "c"]);
        assert_eq!(parsed("Case/CASE/case"), ["Case", "CASE", "case"]);

        for (wire, expected) in [
            ("/absolute/file.txt", InvalidPath::LeadingSeparator),
            ("\\absolute\\file.txt", InvalidPath::LeadingSeparator),
            ("//server/share/a.jpg", InvalidPath::LeadingSeparator),
            ("trailing/", InvalidPath::TrailingSeparator),
            ("trailing\\", InvalidPath::TrailingSeparator),
            ("a//b.txt", InvalidPath::EmptySegment),
            ("a\\\\b.txt", InvalidPath::EmptySegment),
            ("a/\\b.txt", InvalidPath::EmptySegment),
            ("", InvalidPath::Empty),
            ("/", InvalidPath::LeadingSeparator),
            ("C:/a.txt", InvalidPath::DriveLetter),
            ("C:\\a.txt", InvalidPath::DriveLetter),
            ("z:a.txt", InvalidPath::DriveLetter),
            ("a:b/c", InvalidPath::DriveLetter),
            ("\\\\server\\share\\a.jpg", InvalidPath::Unc),
            ("\\\\?\\C:\\a.txt", InvalidPath::Unc),
            ("a\u{0}b/c", InvalidPath::Nul),
            ("\u{0}", InvalidPath::Nul),
            ("a/b\u{0}", InvalidPath::Nul),
        ] {
            assert_eq!(rejected(wire), expected, "{wire:?}");
        }

        for wire in [
            "./a.txt",
            "../a.txt",
            "a/../b.txt",
            "a/./b.txt",
            "a/..",
            "a/.",
            ".",
            "..",
            "a\\..\\b",
        ] {
            assert_eq!(
                rejected(wire),
                InvalidPath::Segment(InvalidName::Reserved),
                "{wire:?}"
            );
        }

        for control in [
            '\u{1}', '\u{8}', '\u{9}', '\u{a}', '\u{d}', '\u{1b}', '\u{1f}', '\u{7f}', '\u{80}',
            '\u{85}', '\u{9f}',
        ] {
            assert_eq!(
                rejected(&format!("dir/na{control}me.txt")),
                InvalidPath::Segment(InvalidName::Control),
                "{control:?}"
            );
        }
        assert_eq!(
            parsed("dir/na\u{a0}me.txt"),
            ["dir", "na\u{a0}me.txt"],
            "U+00A0 is not a control character"
        );
        assert_eq!(
            rejected("   /a.txt"),
            InvalidPath::Segment(InvalidName::NormalizedOutOfRange),
            "a blank directory name normalizes to nothing"
        );
    }

    #[test]
    fn unit_relative_path_segment_count_boundary() {
        let at_limit = vec!["d"; MAX_SEGMENTS].join("/");
        assert_eq!(RelativePath::parse(&at_limit).unwrap().len(), 32);
        let beyond = vec!["d"; MAX_SEGMENTS + 1].join("/");
        assert_eq!(rejected(&beyond), InvalidPath::TooManySegments);
        let beyond_backslashes = vec!["d"; MAX_SEGMENTS + 1].join("\\");
        assert_eq!(rejected(&beyond_backslashes), InvalidPath::TooManySegments);
        assert!(InvalidPath::TooManySegments.is_shape());
    }

    #[test]
    fn unit_relative_path_segment_byte_boundary() {
        let ascii = "a".repeat(MAX_NAME_BYTES);
        assert_eq!(parsed(&ascii), std::slice::from_ref(&ascii));
        assert_eq!(
            rejected(&format!("{ascii}a")),
            InvalidPath::Segment(InvalidName::TooLong)
        );

        let two_byte = "é".repeat(MAX_NAME_BYTES / 2);
        assert_eq!(two_byte.len(), 254);
        assert_eq!(parsed(&two_byte), std::slice::from_ref(&two_byte));
        let padded = format!("{two_byte}a");
        assert_eq!(padded.len(), MAX_NAME_BYTES);
        assert_eq!(parsed(&padded), std::slice::from_ref(&padded));
        assert_eq!(
            rejected(&format!("{two_byte}é")),
            InvalidPath::Segment(InvalidName::TooLong),
            "256 bytes in 128 scalars is over the byte bound"
        );

        let three_byte = "日".repeat(85);
        assert_eq!(three_byte.len(), 255);
        assert_eq!(parsed(&three_byte), std::slice::from_ref(&three_byte));
        assert_eq!(
            rejected(&format!("{three_byte}日")),
            InvalidPath::Segment(InvalidName::TooLong)
        );

        let four_byte = "😀".repeat(63);
        assert_eq!(four_byte.len(), 252);
        assert_eq!(parsed(&four_byte), std::slice::from_ref(&four_byte));
        assert_eq!(
            rejected(&format!("{four_byte}😀")),
            InvalidPath::Segment(InvalidName::TooLong)
        );
    }

    #[test]
    fn unit_relative_path_total_byte_boundary() {
        let full = "a".repeat(255);
        let short = "a".repeat(254);
        let prefix = [full.as_str(), full.as_str(), full.as_str(), short.as_str()].join("/");
        assert_eq!(prefix.len(), 3 * 255 + 254 + 3);
        let at_limit = format!("{prefix}/b");
        assert_eq!(at_limit.len(), MAX_PATH_BYTES);
        assert_eq!(parsed(&at_limit).len(), 5);
        let beyond = format!("{at_limit}b");
        assert_eq!(beyond.len(), MAX_PATH_BYTES + 1);
        assert_eq!(rejected(&beyond), InvalidPath::TooLong);

        let multibyte = "日".repeat(85);
        let joined = [multibyte.as_str(); 4].join("/");
        assert_eq!(joined.len(), 4 * 255 + 3);
        assert_eq!(rejected(&format!("{joined}/x")), InvalidPath::TooLong);
        assert!(parsed(&joined).len() == 4);
    }

    #[test]
    fn unit_relative_path_total_bytes_are_measured_after_nfc() {
        let decomposed = "e\u{301}".repeat(100);
        let composed = "\u{e9}".repeat(100);
        assert_eq!(decomposed.len(), 300);
        assert_eq!(composed.len(), 200);
        let wire = [decomposed.as_str(); 5].join("/");
        assert_eq!(wire.len(), 5 * 300 + 4);
        let path = RelativePath::parse(&wire).unwrap();
        assert_eq!(path.joined().len(), 5 * 200 + 4);
        assert!(path
            .directory()
            .segments()
            .iter()
            .chain(std::iter::once(path.leaf()))
            .all(|segment| segment.display() == composed));
    }

    #[test]
    fn unit_relative_path_split_for_transfer() {
        let (directory, leaf) = RelativePath::parse("Photos/2026/image.jpg")
            .unwrap()
            .split();
        assert_eq!(directory.joined(), "Photos/2026");
        assert_eq!(directory.len(), 2);
        assert_eq!(leaf.display(), "image.jpg");
        assert_eq!(leaf.normalized(), "image.jpg");

        let (directory, leaf) = RelativePath::parse("image.jpg").unwrap().split();
        assert!(directory.is_empty());
        assert_eq!(directory.joined(), "");
        assert_eq!(directory, DirectoryPath::root());
        assert_eq!(leaf.display(), "image.jpg");

        let (directory, leaf) = RelativePath::parse("A\\B\\Report.PDF").unwrap().split();
        assert_eq!(directory.joined(), "A/B");
        assert_eq!(leaf.display(), "Report.PDF");
        assert_eq!(leaf.normalized(), "report.pdf");
        assert_eq!(
            DirectoryPath::parse_prefix(&directory.joined()).unwrap(),
            directory
        );
    }

    #[test]
    fn unit_directory_prefix_grammar() {
        assert!(DirectoryPath::parse_prefix("").unwrap().is_empty());
        assert_eq!(DirectoryPath::parse_prefix("Photos/2026").unwrap().len(), 2);
        for prefix in ["/Photos", "Photos/", "Photos//2026", "Photos/..", "C:/x"] {
            assert!(DirectoryPath::parse_prefix(prefix).is_err(), "{prefix:?}");
        }
        assert_eq!(
            DirectoryPath::parse_prefix(&vec!["d"; 33].join("/")),
            Err(InvalidPath::TooManySegments)
        );
        assert_eq!(
            DirectoryPath::parse_prefix(&vec!["d"; 32].join("/"))
                .unwrap()
                .len(),
            32
        );
    }

    #[test]
    fn unit_directory_segments_grammar() {
        let valid = DirectoryPath::from_segments(&["Photos", "2026", "Iceland"]).unwrap();
        assert_eq!(valid.joined(), "Photos/2026/Iceland");
        assert!(DirectoryPath::from_segments::<&str>(&[])
            .unwrap()
            .is_empty());
        assert_eq!(
            DirectoryPath::from_segments(&["cafe\u{301}"])
                .unwrap()
                .segments()[0]
                .display(),
            "caf\u{e9}"
        );

        for (segments, expected) in [
            (vec![""], InvalidPath::Segment(InvalidName::Empty)),
            (vec!["a", ""], InvalidPath::Segment(InvalidName::Empty)),
            (vec!["."], InvalidPath::Segment(InvalidName::Reserved)),
            (vec!["a", ".."], InvalidPath::Segment(InvalidName::Reserved)),
            (vec!["a/b"], InvalidPath::Segment(InvalidName::Separator)),
            (vec!["a\\b"], InvalidPath::Segment(InvalidName::Separator)),
            (vec!["/"], InvalidPath::Segment(InvalidName::Separator)),
            (vec!["a\u{0}b"], InvalidPath::Nul),
            (vec!["a\u{1}b"], InvalidPath::Segment(InvalidName::Control)),
            (vec!["a\u{85}b"], InvalidPath::Segment(InvalidName::Control)),
            (vec!["C:"], InvalidPath::DriveLetter),
            (vec!["c:docs", "x"], InvalidPath::DriveLetter),
        ] {
            assert_eq!(
                DirectoryPath::from_segments(&segments),
                Err(expected),
                "{segments:?}"
            );
        }
        assert!(DirectoryPath::from_segments(&["ok", "C:"]).is_ok());
        assert_eq!(
            DirectoryPath::from_segments(&vec!["d"; 33]),
            Err(InvalidPath::TooManySegments)
        );
        assert_eq!(
            DirectoryPath::from_segments(&vec!["d"; 32]).unwrap().len(),
            32
        );
        let long = "a".repeat(255);
        assert_eq!(
            DirectoryPath::from_segments(&[long.as_str(); 5]),
            Err(InvalidPath::TooLong)
        );
        assert!(DirectoryPath::from_segments(&[long.as_str(); 4]).is_ok());
        assert_eq!(
            DirectoryPath::from_segments(&[format!("{long}a")]),
            Err(InvalidPath::Segment(InvalidName::TooLong))
        );
        assert!(!InvalidPath::Segment(InvalidName::Empty).is_shape());
        assert!(InvalidPath::TooLong.is_shape());
        assert!(InvalidPath::Empty.is_shape());
    }

    fn arbitrary_path() -> impl Strategy<Value = String> {
        let piece = prop_oneof![
            4 => "[a-zA-Z0-9 ._-]{1,12}",
            2 => "[\u{e0}-\u{ff}\u{300}-\u{36f}]{1,6}",
            1 => Just(".".to_owned()),
            1 => Just("..".to_owned()),
            1 => Just(String::new()),
            1 => Just("C:".to_owned()),
            1 => "\\PC{1,4}",
        ];
        let separator = prop_oneof![Just("/"), Just("\\")];
        prop::collection::vec((piece, separator), 1..8).prop_map(|parts| {
            let mut wire = String::new();
            let count = parts.len();
            for (index, (piece, separator)) in parts.into_iter().enumerate() {
                wire.push_str(&piece);
                if index + 1 < count {
                    wire.push_str(separator);
                }
            }
            wire
        })
    }

    proptest! {
        #[test]
        fn prop_relative_path_parse_is_stable(wire in arbitrary_path()) {
            if let Ok(path) = RelativePath::parse(&wire) {
                prop_assert!(path.len() <= MAX_SEGMENTS);
                let joined = path.joined();
                prop_assert!(joined.len() <= MAX_PATH_BYTES);
                prop_assert!(!joined.contains('\\'));
                prop_assert!(!joined.starts_with('/') && !joined.ends_with('/'));
                prop_assert_eq!(RelativePath::parse(&joined).unwrap(), path.clone());
                let (directory, leaf) = path.clone().split();
                let rebuilt = if directory.is_empty() {
                    leaf.display().to_owned()
                } else {
                    format!("{}/{}", directory.joined(), leaf.display())
                };
                prop_assert_eq!(&rebuilt, &joined);
                let strings: Vec<&str> = joined.split('/').collect();
                let from_segments = DirectoryPath::from_segments(&strings).unwrap();
                prop_assert_eq!(from_segments.len(), path.len());
                prop_assert_eq!(from_segments.joined(), joined);
            }
        }

        #[test]
        fn prop_relative_path_accepts_exactly_what_segments_accept(
            wire in arbitrary_path()
        ) {
            let folded = wire.replace('\\', "/");
            let segments: Vec<&str> = folded.split('/').collect();
            let by_path = RelativePath::parse(&wire).is_ok();
            let by_segments = !folded.starts_with('/')
                && !folded.ends_with('/')
                && !wire.starts_with("\\\\")
                && DirectoryPath::from_segments(&segments).is_ok();
            prop_assert_eq!(by_path, by_segments, "{:?}", wire);
        }
    }
}
