use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::gen::{
    hash_generated, GenError, Seed, Sha256Digest, GENERATOR_ID, GENERATOR_VERSION, MAX_STREAM_SIZE,
};

pub const G7_MAX_SIZE: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("MANIFEST_UNREADABLE: {0}")]
    Unreadable(String),
    #[error("MANIFEST_INVALID_JSON: {0}")]
    InvalidJson(String),
    #[error("MANIFEST_UNSUPPORTED_VERSION: version must be {GENERATOR_VERSION}, found {0}")]
    UnsupportedVersion(u64),
    #[error("MANIFEST_UNSUPPORTED_GENERATOR: generator must be '{GENERATOR_ID}', found '{0}'")]
    UnsupportedGenerator(String),
    #[error("MANIFEST_NO_ENTRIES: entries must not be empty")]
    NoEntries,
    #[error(
        "MANIFEST_INVALID_SEED: entries[{index}].seed must be 64 lowercase hexadecimal characters"
    )]
    InvalidSeed { index: usize },
    #[error("MANIFEST_INVALID_SIZE: entries[{index}].size {size} exceeds the {MAX_STREAM_SIZE}-byte stream capacity")]
    InvalidSize { index: usize, size: u64 },
    #[error("MANIFEST_INVALID_DIGEST: entries[{index}].sha256 must be 64 lowercase hexadecimal characters")]
    InvalidDigest { index: usize },
    #[error("MANIFEST_DUPLICATE_ENTRY: entries[{index}] repeats seed/size {seed}/{size}")]
    DuplicateEntry {
        index: usize,
        seed: String,
        size: u64,
    },
    #[error("MANIFEST_UNSORTED: entries[{index}] is not in ascending (seed, size) order")]
    Unsorted { index: usize },
    #[error("MANIFEST_DIGEST_MISMATCH: seed {seed} size {size}: generated {bytes} bytes with sha256 {computed}, manifest expects {expected}")]
    DigestMismatch {
        seed: String,
        size: u64,
        bytes: u64,
        computed: String,
        expected: String,
    },
    #[error(transparent)]
    Generator(#[from] GenError),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    version: u64,
    generator: String,
    entries: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    seed: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub seed: Seed,
    pub size: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryReport {
    pub seed: String,
    pub size: u64,
    pub bytes: u64,
    pub computed: String,
    pub expected: String,
}

impl EntryReport {
    pub fn passed(&self) -> bool {
        self.bytes == self.size && self.computed == self.expected
    }
}

pub fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path).and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other(format!("file exceeds {limit} bytes")));
    }
    Ok(bytes)
}

pub fn default_manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/generated-manifest.json")
}

impl Manifest {
    pub fn parse(json: &[u8]) -> Result<Self, ManifestError> {
        let raw: RawManifest = serde_json::from_slice(json)
            .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
        if raw.version != GENERATOR_VERSION {
            return Err(ManifestError::UnsupportedVersion(raw.version));
        }
        if raw.generator != GENERATOR_ID {
            return Err(ManifestError::UnsupportedGenerator(raw.generator));
        }
        if raw.entries.is_empty() {
            return Err(ManifestError::NoEntries);
        }
        let mut entries: Vec<ManifestEntry> = Vec::with_capacity(raw.entries.len());
        for (index, entry) in raw.entries.into_iter().enumerate() {
            let seed = Seed::from_canonical_hex(&entry.seed)
                .map_err(|_| ManifestError::InvalidSeed { index })?;
            if entry.size > MAX_STREAM_SIZE {
                return Err(ManifestError::InvalidSize {
                    index,
                    size: entry.size,
                });
            }
            let sha256 = Sha256Digest::from_canonical_hex(&entry.sha256)
                .map_err(|_| ManifestError::InvalidDigest { index })?;
            if entries
                .iter()
                .any(|other| other.seed == seed && other.size == entry.size)
            {
                return Err(ManifestError::DuplicateEntry {
                    index,
                    seed: seed.to_hex(),
                    size: entry.size,
                });
            }
            if let Some(previous) = entries.last() {
                if (previous.seed.as_bytes(), previous.size) > (seed.as_bytes(), entry.size) {
                    return Err(ManifestError::Unsorted { index });
                }
            }
            entries.push(ManifestEntry {
                seed,
                size: entry.size,
                sha256,
            });
        }
        Ok(Self { entries })
    }

    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let bytes = read_bounded(path, MAX_MANIFEST_BYTES)
            .map_err(|error| ManifestError::Unreadable(format!("{}: {error}", path.display())))?;
        Self::parse(&bytes)
    }

    pub fn load_default() -> Result<Self, ManifestError> {
        Self::load(&default_manifest_path())
    }

    pub fn entries_up_to(&self, max_size: Option<u64>) -> Vec<&ManifestEntry> {
        self.entries
            .iter()
            .filter(|entry| max_size.is_none_or(|max| entry.size <= max))
            .collect()
    }
}

pub fn verify_entry(entry: &ManifestEntry) -> Result<EntryReport, ManifestError> {
    let digest = hash_generated(&entry.seed, entry.size)?;
    Ok(EntryReport {
        seed: entry.seed.to_hex(),
        size: entry.size,
        bytes: digest.bytes,
        computed: digest.sha256.to_string(),
        expected: entry.sha256.to_string(),
    })
}

pub fn assert_entry_matches(entry: &ManifestEntry) -> Result<EntryReport, ManifestError> {
    let report = verify_entry(entry)?;
    if report.passed() {
        Ok(report)
    } else {
        Err(ManifestError::DigestMismatch {
            seed: report.seed,
            size: report.size,
            bytes: report.bytes,
            computed: report.computed,
            expected: report.expected,
        })
    }
}
