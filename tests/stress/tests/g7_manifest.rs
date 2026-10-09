use std::path::Path;

use anyhow::{ensure, Context, Result};
use palmr_stress::manifest::{
    assert_entry_matches, default_manifest_path, read_bounded, Manifest, ManifestError, G7_MAX_SIZE,
};

const SEED_1: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SEED_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn doc(version: &str, generator: &str, entries: &str) -> String {
    format!(r#"{{"version":{version},"generator":"{generator}","entries":[{entries}]}}"#)
}

fn entry(seed: &str, size: &str, sha256: &str) -> String {
    format!(r#"{{"seed":"{seed}","size":{size},"sha256":"{sha256}"}}"#)
}

fn parse(json: &str) -> Result<Manifest, ManifestError> {
    Manifest::parse(json.as_bytes())
}

#[test]
fn unit_chacha8_stream_matches_manifest() -> Result<()> {
    let path = default_manifest_path();
    let manifest = Manifest::load(&path).with_context(|| format!("loading {}", path.display()))?;

    let raw: serde_json::Value = serde_json::from_slice(&read_bounded(&path, 64 * 1024)?)?;
    let expected_count = raw["entries"]
        .as_array()
        .context("entries array")?
        .iter()
        .filter(|entry| {
            entry["size"]
                .as_u64()
                .is_some_and(|size| size <= G7_MAX_SIZE)
        })
        .count();

    let selected = manifest.entries_up_to(Some(G7_MAX_SIZE));
    ensure!(!selected.is_empty(), "no manifest entry qualifies for G7");
    ensure!(
        selected.len() == expected_count,
        "selected {} entries, manifest lists {expected_count} at or below 64 MiB",
        selected.len()
    );
    let seeds: std::collections::BTreeSet<_> = selected.iter().map(|e| e.seed.to_hex()).collect();
    ensure!(seeds.len() > 1, "G7 must exercise more than one key");

    for entry in selected {
        let report = assert_entry_matches(entry)?;
        ensure!(
            report.bytes == entry.size,
            "byte count differs for {report:?}"
        );
        ensure!(
            report.computed == report.expected,
            "digest differs for {report:?}"
        );
    }
    Ok(())
}

#[test]
fn unit_manifest_committed_file_is_canonical() -> Result<()> {
    let manifest = Manifest::load_default()?;
    ensure!(manifest.entries.len() >= 4, "manifest lost entries");
    let bytes = std::fs::metadata(default_manifest_path())?.len();
    ensure!(bytes < 8 * 1024, "manifest is {bytes} bytes");
    Ok(())
}

#[test]
fn unit_manifest_rejects_invalid_documents() {
    let ok = entry(SEED_1, "64", DIGEST_A);
    let cases: Vec<(&str, String, ManifestError)> = vec![
        (
            "wrong version",
            doc("2", "chacha8-ietf", &ok),
            ManifestError::UnsupportedVersion(2),
        ),
        (
            "wrong generator",
            doc("1", "chacha20-ietf", &ok),
            ManifestError::UnsupportedGenerator("chacha20-ietf".to_owned()),
        ),
        (
            "no entries",
            doc("1", "chacha8-ietf", ""),
            ManifestError::NoEntries,
        ),
        (
            "short seed",
            doc("1", "chacha8-ietf", &entry("00", "64", DIGEST_A)),
            ManifestError::InvalidSeed { index: 0 },
        ),
        (
            "uppercase seed",
            doc("1", "chacha8-ietf", &entry(&"A".repeat(64), "64", DIGEST_A)),
            ManifestError::InvalidSeed { index: 0 },
        ),
        (
            "non-hex seed",
            doc("1", "chacha8-ietf", &entry(&"g".repeat(64), "64", DIGEST_A)),
            ManifestError::InvalidSeed { index: 0 },
        ),
        (
            "truncated digest",
            doc("1", "chacha8-ietf", &entry(SEED_1, "64", &DIGEST_A[1..])),
            ManifestError::InvalidDigest { index: 0 },
        ),
        (
            "uppercase digest",
            doc("1", "chacha8-ietf", &entry(SEED_1, "64", &"A".repeat(64))),
            ManifestError::InvalidDigest { index: 0 },
        ),
        (
            "placeholder digest",
            doc("1", "chacha8-ietf", &entry(SEED_1, "64", "…")),
            ManifestError::InvalidDigest { index: 0 },
        ),
        (
            "size beyond stream capacity",
            doc(
                "1",
                "chacha8-ietf",
                &entry(SEED_1, "274877906881", DIGEST_A),
            ),
            ManifestError::InvalidSize {
                index: 0,
                size: 274_877_906_881,
            },
        ),
        (
            "duplicate seed and size",
            doc(
                "1",
                "chacha8-ietf",
                &format!(
                    "{},{}",
                    entry(SEED_1, "64", DIGEST_A),
                    entry(SEED_1, "64", DIGEST_B)
                ),
            ),
            ManifestError::DuplicateEntry {
                index: 1,
                seed: SEED_1.to_owned(),
                size: 64,
            },
        ),
        (
            "size out of order",
            doc(
                "1",
                "chacha8-ietf",
                &format!(
                    "{},{}",
                    entry(SEED_1, "128", DIGEST_A),
                    entry(SEED_1, "64", DIGEST_B)
                ),
            ),
            ManifestError::Unsorted { index: 1 },
        ),
        (
            "seed out of order",
            doc(
                "1",
                "chacha8-ietf",
                &format!(
                    "{},{}",
                    entry(SEED_2, "64", DIGEST_A),
                    entry(SEED_1, "64", DIGEST_B)
                ),
            ),
            ManifestError::Unsorted { index: 1 },
        ),
    ];
    for (name, json, expected) in cases {
        assert_eq!(parse(&json).err(), Some(expected), "{name}");
    }
}

#[test]
fn unit_manifest_rejects_malformed_json_shapes() {
    let ok = entry(SEED_1, "64", DIGEST_A);
    let shapes = [
        String::new(),
        "{".to_owned(),
        "[]".to_owned(),
        "null".to_owned(),
        r#"{"version":1,"generator":"chacha8-ietf"}"#.to_owned(),
        format!(r#"{{"version":1,"generator":"chacha8-ietf","entries":[{ok}],"extra":1}}"#),
        r#"{"version":1,"generator":"chacha8-ietf","entries":{}}"#.to_owned(),
        r#"{"version":1,"generator":"chacha8-ietf","entries":[null]}"#.to_owned(),
        doc("1", "chacha8-ietf", &entry(SEED_1, "-1", DIGEST_A)),
        doc("1", "chacha8-ietf", &entry(SEED_1, "1.5", DIGEST_A)),
        doc("1", "chacha8-ietf", &entry(SEED_1, "\"64\"", DIGEST_A)),
        doc("1", "chacha8-ietf", &entry(SEED_1, "1e3", DIGEST_A)),
        doc(
            "1",
            "chacha8-ietf",
            &format!(r#"{{"seed":"{SEED_1}","size":64}}"#),
        ),
        doc(
            "1",
            "chacha8-ietf",
            &format!(r#"{{"seed":"{SEED_1}","size":64,"sha256":"{DIGEST_A}","note":"x"}}"#),
        ),
        doc("1.5", "chacha8-ietf", &ok),
    ];
    for json in shapes {
        assert!(
            matches!(parse(&json), Err(ManifestError::InvalidJson(_))),
            "{json}"
        );
    }
}

#[test]
fn unit_manifest_missing_file_is_an_error_not_a_skip() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-manifest.json");
    assert!(matches!(
        Manifest::load(&missing),
        Err(ManifestError::Unreadable(_))
    ));
}

#[test]
fn unit_manifest_digest_mismatch_is_a_hard_failure() -> Result<()> {
    let manifest = Manifest::load_default()?;
    let entry = manifest
        .entries_up_to(Some(G7_MAX_SIZE))
        .into_iter()
        .min_by_key(|entry| entry.size)
        .context("no G7 entry")?
        .clone();

    let mut altered = entry.clone();
    let mut bytes = *altered.sha256.as_bytes();
    bytes[0] ^= 0x01;
    altered.sha256 = palmr_stress::gen::Sha256Digest::from_bytes(bytes);
    ensure!(matches!(
        assert_entry_matches(&altered),
        Err(ManifestError::DigestMismatch { .. })
    ));

    let mut wrong_size = entry;
    wrong_size.size -= 1;
    ensure!(matches!(
        assert_entry_matches(&wrong_size),
        Err(ManifestError::DigestMismatch { .. })
    ));
    Ok(())
}
