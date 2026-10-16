use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use super::EntryKind;
use crate::domain::naming::{split_extension, truncate_to_boundary};
use crate::domain::normalize::normalize;

pub const MAX_SEGMENT_BYTES: usize = 255;
pub const MAX_ENTRY_DEPTH: usize = 128;
pub const MAX_ENTRY_PATH_BYTES: usize = MAX_ENTRY_DEPTH * (MAX_SEGMENT_BYTES + 1);
pub const MAX_DISAMBIGUATION: u64 = u32::MAX as u64;
pub const PLACEHOLDER_SEGMENT: &str = "_";
const REPLACEMENT: char = '_';

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(usize);

impl NodeId {
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    UnknownParent { parent: NodeId },
    ParentNotDirectory { parent: NodeId },
    TooDeep { depth: usize },
    DisambiguationExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathViolation {
    Empty,
    TooLong,
    Absolute,
    Backslash,
    EmptySegment,
    DotSegment,
    DriveLetter,
    ControlCharacter,
    ReservedCharacter,
    TrailingSpaceOrDot,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownParent { parent } => {
                write!(f, "parent entry {} has not been allocated", parent.0)
            }
            Self::ParentNotDirectory { parent } => {
                write!(f, "parent entry {} is not a directory", parent.0)
            }
            Self::TooDeep { depth } => {
                write!(f, "entry depth {depth} exceeds {MAX_ENTRY_DEPTH} segments")
            }
            Self::DisambiguationExhausted => f.write_str("disambiguation counter exhausted"),
        }
    }
}

impl fmt::Display for PathViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for NameError {}

impl std::error::Error for PathViolation {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub id: NodeId,
    pub kind: EntryKind,
    pub path: String,
}

#[derive(Debug)]
struct Node {
    parent: Option<NodeId>,
    kind: EntryKind,
    segment: String,
    depth: usize,
}

#[derive(Debug, Default)]
pub struct ArchiveNamer {
    nodes: Vec<Node>,
    taken: BTreeSet<(Option<NodeId>, String)>,
    next_attempt: BTreeMap<(Option<NodeId>, EntryKind, String), u64>,
}

impl ArchiveNamer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn allocate(
        &mut self,
        parent: Option<NodeId>,
        display_name: &str,
        kind: EntryKind,
    ) -> Result<ResolvedName, NameError> {
        let depth = match parent {
            None => 1,
            Some(parent_id) => {
                let node = self
                    .nodes
                    .get(parent_id.0)
                    .ok_or(NameError::UnknownParent { parent: parent_id })?;
                if node.kind != EntryKind::Directory {
                    return Err(NameError::ParentNotDirectory { parent: parent_id });
                }
                node.depth + 1
            }
        };
        if depth > MAX_ENTRY_DEPTH {
            return Err(NameError::TooDeep { depth });
        }
        let cleaned = sanitize_segment(display_name);
        let segment = self.pick_unique(parent, cleaned, kind)?;
        let id = NodeId(self.nodes.len());
        self.taken.insert((parent, normalize(&segment)));
        self.nodes.push(Node {
            parent,
            kind,
            segment,
            depth,
        });
        Ok(ResolvedName {
            id,
            kind,
            path: self.path_of(id),
        })
    }

    pub fn path_of(&self, id: NodeId) -> String {
        let mut segments = Vec::new();
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            let node = &self.nodes[current.0];
            segments.push(node.segment.as_str());
            cursor = node.parent;
        }
        segments.reverse();
        segments.join("/")
    }

    fn pick_unique(
        &mut self,
        parent: Option<NodeId>,
        cleaned: String,
        kind: EntryKind,
    ) -> Result<String, NameError> {
        let (base, tail) = match kind {
            EntryKind::File => split_extension(&cleaned),
            EntryKind::Directory => (cleaned.as_str(), ""),
        };
        let mut attempt = self
            .next_attempt
            .get(&(parent, kind, cleaned.clone()))
            .copied()
            .unwrap_or(0);
        loop {
            let candidate = series_candidate(base, tail, attempt);
            if !self.taken.contains(&(parent, normalize(&candidate))) {
                self.next_attempt.insert((parent, kind, cleaned), attempt);
                return Ok(candidate);
            }
            attempt += 1;
            if attempt > MAX_DISAMBIGUATION {
                return Err(NameError::DisambiguationExhausted);
            }
        }
    }
}

pub fn resolve_names(
    requests: &[(Option<NodeId>, &str, EntryKind)],
) -> Result<Vec<ResolvedName>, NameError> {
    let mut namer = ArchiveNamer::new();
    requests
        .iter()
        .map(|(parent, name, kind)| namer.allocate(*parent, name, *kind))
        .collect()
}

pub fn sanitize_segment(raw: &str) -> String {
    let mapped: String = raw.chars().map(map_char).nfc().collect();
    let trimmed = mapped
        .trim_start_matches(char::is_whitespace)
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let (base, tail) = split_extension(trimmed);
    fit(base, tail, "")
}

fn map_char(c: char) -> char {
    if is_unsafe_char(c) {
        REPLACEMENT
    } else {
        c
    }
}

fn is_unsafe_char(c: char) -> bool {
    if c.is_control()
        || matches!(
            c,
            '/' | '\\'
                | ':'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
        )
    {
        return true;
    }
    std::iter::once(c)
        .nfkc()
        .any(|folded| matches!(folded, '/' | '\\' | ':') || folded.is_control())
}

fn series_candidate(base: &str, tail: &str, attempt: u64) -> String {
    if attempt == 0 {
        fit(base, tail, "")
    } else {
        fit(base, tail, &format!(" ({attempt})"))
    }
}

fn fit(base: &str, tail: &str, suffix: &str) -> String {
    let reserved = suffix.len() + tail.len();
    if base.len() + reserved <= MAX_SEGMENT_BYTES {
        let joined = format!("{base}{suffix}{tail}");
        return if joined.is_empty() {
            PLACEHOLDER_SEGMENT.to_owned()
        } else {
            joined
        };
    }
    if reserved < MAX_SEGMENT_BYTES {
        let kept = trim_cut(truncate_to_boundary(base, MAX_SEGMENT_BYTES - reserved));
        if !kept.is_empty() {
            return format!("{kept}{suffix}{tail}");
        }
    }
    let whole = format!("{base}{tail}");
    let budget = MAX_SEGMENT_BYTES.saturating_sub(suffix.len());
    let kept = trim_cut(truncate_to_boundary(&whole, budget));
    let kept = if kept.is_empty() {
        PLACEHOLDER_SEGMENT
    } else {
        kept
    };
    format!("{kept}{suffix}")
}

fn trim_cut(text: &str) -> &str {
    text.trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

pub fn validate_entry_path(path: &str) -> Result<(), PathViolation> {
    if path.is_empty() {
        return Err(PathViolation::Empty);
    }
    if path.len() > MAX_ENTRY_PATH_BYTES {
        return Err(PathViolation::TooLong);
    }
    if path.starts_with('/') {
        return Err(PathViolation::Absolute);
    }
    if path.contains('\\') {
        return Err(PathViolation::Backslash);
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(PathViolation::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return Err(PathViolation::DotSegment);
        }
        if segment.len() > MAX_SEGMENT_BYTES {
            return Err(PathViolation::TooLong);
        }
        if segment.chars().any(char::is_control) {
            return Err(PathViolation::ControlCharacter);
        }
        if segment.contains(':') {
            return Err(PathViolation::DriveLetter);
        }
        if segment.chars().any(is_unsafe_char) {
            return Err(PathViolation::ReservedCharacter);
        }
        if segment.ends_with('.') || segment.ends_with(char::is_whitespace) {
            return Err(PathViolation::TrailingSpaceOrDot);
        }
    }
    Ok(())
}
