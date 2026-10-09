use anyhow::{ensure, Result};
use palmr_stress::gen::{encode_hex, generate_bytes, Seed, MAX_STREAM_SIZE};
use palmr_stress::vectors::ReferenceVectors;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

#[test]
fn unit_gen_reference_vectors_match() -> Result<()> {
    let vectors = ReferenceVectors::load_default().map_err(anyhow::Error::msg)?;
    ensure!(!vectors.published.is_empty(), "published vector missing");
    ensure!(vectors.slices.len() > 10, "reference slices lost");
    for vector in vectors.published.iter().chain(&vectors.slices) {
        let actual = generate_bytes(
            &vector.seed,
            MAX_STREAM_SIZE,
            vector.offset,
            vector.bytes.len() as u64,
        )?;
        ensure!(
            actual == vector.bytes,
            "{}@{}: generated {} but vector holds {}",
            vector.seed.to_hex(),
            vector.offset,
            encode_hex(&actual),
            encode_hex(&vector.bytes)
        );
    }
    Ok(())
}

#[test]
fn unit_gen_reference_vectors_cover_boundaries_and_large_offsets() -> Result<()> {
    let vectors = ReferenceVectors::load_default().map_err(anyhow::Error::msg)?;
    let offsets: std::collections::BTreeSet<u64> =
        vectors.slices.iter().map(|v| v.offset).collect();
    for required in [0, 1, 63, 64, 65, MIB, GIB, 16 * GIB, MAX_STREAM_SIZE - 96] {
        ensure!(
            offsets.contains(&required),
            "no vector at offset {required}"
        );
    }
    let seeds: std::collections::BTreeSet<_> =
        vectors.slices.iter().map(|v| v.seed.to_hex()).collect();
    ensure!(seeds.len() >= 3, "vectors must span independent seeds");
    Ok(())
}

#[test]
fn unit_gen_published_chacha8_vector_is_the_zero_key_block() -> Result<()> {
    let seed = Seed::from_bytes([0u8; 32]);
    let block = generate_bytes(&seed, 64, 0, 64)?;
    ensure!(
        encode_hex(&block)
            == "3e00ef2f895f40d67f5bb8e81f09a5a12c840ec3ce9a7f3b181be188ef711a1e984ce172b9216f419f445367456d5619314a42a3da86b001387bfdb80e0cfe42"
    );
    Ok(())
}

#[test]
fn unit_gen_keystream_is_raw_and_unframed() -> Result<()> {
    let seed = Seed::from_hex(&"0".repeat(64))?;
    let bytes = generate_bytes(&seed, 4, 0, 4)?;
    ensure!(
        bytes == [0x3e, 0x00, 0xef, 0x2f],
        "stream must begin with the raw keystream"
    );
    Ok(())
}
