use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{ensure, Result};
use palmr_stress::gen::{
    generate_bytes, hash_async_reader, hash_generated, hash_generated_range, hash_stream, GenError,
    GenHasher, GenReader, GeneratedStream, Seed, Sha256Digest, STREAM_CHUNK_SIZE,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

const MIB: u64 = 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn seed(last: u8) -> Seed {
    let mut bytes = [0u8; 32];
    bytes[31] = last;
    Seed::from_bytes(bytes)
}

fn one_shot(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

struct Probe<R> {
    inner: R,
    largest_offer: usize,
    polls: u64,
}

impl<R: AsyncRead + Unpin> AsyncRead for Probe<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.largest_offer = self.largest_offer.max(buf.remaining());
        self.polls += 1;
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[tokio::test]
async fn unit_gen_reader_small_buffers_match_the_stream() -> Result<()> {
    let size = 1000u64;
    let expected = generate_bytes(&seed(1), size, 0, size)?;
    for chunk in [1usize, 7, 63, 64, 65, 999, 1000, 4096] {
        let mut reader = GenReader::new(&seed(1), size)?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; chunk];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        ensure!(out == expected, "chunk {chunk}");
    }
    Ok(())
}

#[tokio::test]
async fn unit_gen_reader_starts_at_an_arbitrary_offset() -> Result<()> {
    let size = 50_000u64;
    let whole = generate_bytes(&seed(2), size, 0, size)?;
    for offset in [0u64, 1, 63, 64, 65, 4096, 49_999, 50_000] {
        let mut reader = GenReader::at_offset(&seed(2), size, offset)?;
        let mut out = vec![0u8; (size - offset) as usize];
        reader.read_exact(&mut out).await?;
        ensure!(out == whole[offset as usize..], "offset {offset}");
        ensure!(
            reader.read(&mut [0u8; 8]).await? == 0,
            "EOF after the declared size"
        );
        ensure!(reader.position() == size);
    }
    let mut reader = GenReader::new(&seed(2), size)?;
    reader.seek_to(40_000)?;
    let mut out = vec![0u8; 100];
    reader.read_exact(&mut out).await?;
    ensure!(out == whole[40_000..40_100]);
    ensure!(reader.seek_to(size + 1).is_err());
    Ok(())
}

#[tokio::test]
async fn unit_gen_reader_zero_length_and_oversized_streams() -> Result<()> {
    let mut empty = GenReader::new(&seed(1), 0)?;
    ensure!(empty.read(&mut [0u8; 16]).await? == 0);
    ensure!(GenReader::new(&seed(1), u64::MAX).is_err());
    ensure!(matches!(
        GenReader::at_offset(&seed(1), 10, 11),
        Err(GenError::SeekPastEnd { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn unit_gen_reader_hash_never_sees_a_buffer_larger_than_the_chunk() -> Result<()> {
    let size = 16 * MIB;
    let mut probe = Probe {
        inner: GenReader::new(&seed(1), size)?,
        largest_offer: 0,
        polls: 0,
    };
    let streamed = hash_async_reader(&mut probe).await?;
    ensure!(streamed.bytes == size);
    ensure!(
        probe.largest_offer <= STREAM_CHUNK_SIZE,
        "offered {}",
        probe.largest_offer
    );
    ensure!(probe.polls >= size / STREAM_CHUNK_SIZE as u64);
    ensure!(streamed == hash_generated(&seed(1), size)?);
    Ok(())
}

#[test]
fn unit_gen_hashing_a_stream_holds_one_bounded_buffer() -> Result<()> {
    let size = 16 * MIB;
    let mut stream = GeneratedStream::new(&seed(1), size)?;
    let outcome = hash_stream(&mut stream, size)?;
    ensure!(outcome.bytes == size);
    ensure!(stream.generated_bytes() == size);
    ensure!(
        stream.max_read_len() == STREAM_CHUNK_SIZE,
        "the largest single read was {} bytes",
        stream.max_read_len()
    );
    Ok(())
}

#[test]
fn unit_gen_hasher_empty_stream_digest() {
    let outcome = GenHasher::new().finalize();
    assert_eq!(outcome.bytes, 0);
    assert_eq!(outcome.sha256.to_string(), EMPTY_SHA256);
    assert_eq!(
        hash_generated(&seed(1), 0).map(|o| o.sha256.to_string()),
        Ok(EMPTY_SHA256.to_owned())
    );
}

#[test]
fn unit_gen_hasher_is_chunk_independent_and_counts_bytes() -> Result<()> {
    let size = 200_003u64;
    let bytes = generate_bytes(&seed(1), size, 0, size)?;
    let expected = one_shot(&bytes);
    for chunk in [1usize, 63, 64, 65, 1000, 65_536, bytes.len()] {
        let mut hasher = GenHasher::new();
        for piece in bytes.chunks(chunk) {
            hasher.update(piece);
        }
        ensure!(hasher.bytes_consumed() == size);
        let outcome = hasher.finalize();
        ensure!(
            outcome.bytes == size && outcome.sha256 == expected,
            "chunk {chunk}"
        );
    }
    ensure!(hash_generated(&seed(1), size)?.sha256 == expected);
    Ok(())
}

#[test]
fn unit_gen_hasher_partial_and_full_streams() -> Result<()> {
    let size = 10_000u64;
    let bytes = generate_bytes(&seed(2), size, 0, size)?;
    let partial = hash_generated_range(&seed(2), size, 100, 5000)?;
    ensure!(partial.bytes == 5000);
    ensure!(partial.sha256 == one_shot(&bytes[100..5100]));
    let full = hash_generated(&seed(2), size)?;
    ensure!(full.sha256 == one_shot(&bytes));
    ensure!(full.sha256 != partial.sha256);
    Ok(())
}

#[test]
fn unit_gen_hasher_distinguishes_seeds_and_sizes() -> Result<()> {
    let a = hash_generated(&seed(1), 4096)?.sha256;
    ensure!(a != hash_generated(&seed(2), 4096)?.sha256);
    ensure!(a != hash_generated(&seed(1), 4097)?.sha256);
    ensure!(a != hash_generated(&seed(1), 4095)?.sha256);
    Ok(())
}

#[test]
fn unit_gen_hasher_refuses_to_truncate_silently() {
    assert!(matches!(
        hash_generated_range(&seed(1), 100, 50, 51),
        Err(GenError::RangePastEnd { .. })
    ));
    assert!(matches!(
        hash_generated_range(&seed(1), 100, 101, 0),
        Err(GenError::SeekPastEnd { .. })
    ));
}

#[tokio::test]
async fn unit_gen_uploaded_download_and_manifest_digests_compare_without_buffering() -> Result<()> {
    let size = 5 * MIB + 17;
    let uploaded = hash_generated(&seed(1), size)?;
    let mut downloaded = GenReader::new(&seed(1), size)?;
    let observed = hash_async_reader(&mut downloaded).await?;
    ensure!(uploaded == observed);

    let mut truncated = GenReader::new(&seed(1), size - 1)?;
    ensure!(hash_async_reader(&mut truncated).await? != uploaded);
    Ok(())
}
