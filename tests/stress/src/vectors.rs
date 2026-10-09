use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::gen::{GenError, Seed, GENERATOR_ID, GENERATOR_VERSION};
use crate::manifest::read_bounded;

const MAX_VECTOR_BYTES: u64 = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVectors {
    version: u64,
    generator: String,
    published: Vec<RawPublished>,
    slices: Vec<RawSlice>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPublished {
    source: String,
    seed: String,
    offset: u64,
    hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSlice {
    seed: String,
    offset: u64,
    hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceVector {
    pub source: Option<String>,
    pub seed: Seed,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceVectors {
    pub published: Vec<SliceVector>,
    pub slices: Vec<SliceVector>,
}

pub fn default_vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/generator-vectors.json")
}

pub fn decode_hex_bytes(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

fn slice_vector(
    source: Option<String>,
    seed: &str,
    offset: u64,
    hex: &str,
) -> Result<SliceVector, GenError> {
    Ok(SliceVector {
        source,
        seed: Seed::from_canonical_hex(seed)?,
        offset,
        bytes: decode_hex_bytes(hex).ok_or(GenError::InvalidDigest)?,
    })
}

impl ReferenceVectors {
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let raw: RawVectors = serde_json::from_slice(json).map_err(|error| error.to_string())?;
        if raw.version != GENERATOR_VERSION || raw.generator != GENERATOR_ID {
            return Err("vector file does not name chacha8-ietf version 1".to_owned());
        }
        let published = raw
            .published
            .iter()
            .map(|v| slice_vector(Some(v.source.clone()), &v.seed, v.offset, &v.hex))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let slices = raw
            .slices
            .iter()
            .map(|v| slice_vector(None, &v.seed, v.offset, &v.hex))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(Self { published, slices })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = read_bounded(path, MAX_VECTOR_BYTES)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&bytes)
    }

    pub fn load_default() -> Result<Self, String> {
        Self::load(&default_vectors_path())
    }
}
