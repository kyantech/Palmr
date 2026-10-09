use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use chacha20::ChaCha8;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

pub const GENERATOR_ID: &str = "chacha8-ietf";
pub const GENERATOR_VERSION: u64 = 1;
pub const SEED_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const BLOCK_SIZE: u64 = 64;
pub const MAX_BLOCKS: u64 = u32::MAX as u64;
pub const MAX_STREAM_SIZE: u64 = MAX_BLOCKS * BLOCK_SIZE;
pub const STREAM_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_MATERIALIZED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenError {
    #[error("INVALID_SEED: expected exactly 64 hexadecimal characters")]
    InvalidSeed,
    #[error("INVALID_DIGEST: expected exactly 64 lowercase hexadecimal characters")]
    InvalidDigest,
    #[error("SIZE_EXCEEDS_CAPACITY: size {size} exceeds the {max}-byte capacity of one stream")]
    SizeExceedsCapacity { size: u64, max: u64 },
    #[error("SEEK_PAST_END: offset {offset} is past the end of the {size}-byte stream")]
    SeekPastEnd { offset: u64, size: u64 },
    #[error("RANGE_PAST_END: range {offset}+{length} is past the end of the {size}-byte stream")]
    RangePastEnd { offset: u64, length: u64, size: u64 },
    #[error("SLICE_TOO_LARGE: {length} bytes exceeds the {max}-byte materialization ceiling")]
    SliceTooLarge { length: u64, max: u64 },
    #[error("COUNTER_OVERFLOW: the 32-bit block counter cannot address this position")]
    CounterOverflow,
    #[error("IO: {0}")]
    Io(String),
}

fn decode_hex<const N: usize>(text: &str, canonical: bool) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        *slot = (nibble(pair[0], canonical)? << 4) | nibble(pair[1], canonical)?;
    }
    Some(out)
}

fn nibble(byte: u8, canonical: bool) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' if !canonical => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Seed([u8; SEED_LEN]);

impl Seed {
    pub fn from_bytes(bytes: [u8; SEED_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(text: &str) -> Result<Self, GenError> {
        decode_hex(text, false)
            .map(Self)
            .ok_or(GenError::InvalidSeed)
    }

    pub fn from_canonical_hex(text: &str) -> Result<Self, GenError> {
        decode_hex(text, true)
            .map(Self)
            .ok_or(GenError::InvalidSeed)
    }

    pub fn as_bytes(&self) -> &[u8; SEED_LEN] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Seed({})", self.to_hex())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_canonical_hex(text: &str) -> Result<Self, GenError> {
        decode_hex(text, true)
            .map(Self)
            .ok_or(GenError::InvalidDigest)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&encode_hex(&self.0))
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256Digest({self})")
    }
}

pub struct GeneratedStream {
    cipher: ChaCha8,
    seed: Seed,
    size: u64,
    position: u64,
    generated_bytes: u64,
    max_read_len: usize,
}

impl GeneratedStream {
    pub fn new(seed: &Seed, size: u64) -> Result<Self, GenError> {
        if size > MAX_STREAM_SIZE {
            return Err(GenError::SizeExceedsCapacity {
                size,
                max: MAX_STREAM_SIZE,
            });
        }
        let nonce = [0u8; NONCE_LEN];
        Ok(Self {
            cipher: ChaCha8::new(seed.as_bytes().into(), &nonce.into()),
            seed: *seed,
            size,
            position: 0,
            generated_bytes: 0,
            max_read_len: 0,
        })
    }

    pub fn at_offset(seed: &Seed, size: u64, offset: u64) -> Result<Self, GenError> {
        let mut stream = Self::new(seed, size)?;
        stream.seek(offset)?;
        Ok(stream)
    }

    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn remaining(&self) -> u64 {
        self.size - self.position
    }

    pub fn generated_bytes(&self) -> u64 {
        self.generated_bytes
    }

    pub fn max_read_len(&self) -> usize {
        self.max_read_len
    }

    pub fn cipher_position(&self) -> Result<u64, GenError> {
        self.cipher
            .try_current_pos::<u64>()
            .map_err(|_| GenError::CounterOverflow)
    }

    pub fn seek(&mut self, offset: u64) -> Result<(), GenError> {
        if offset > self.size {
            return Err(GenError::SeekPastEnd {
                offset,
                size: self.size,
            });
        }
        self.cipher
            .try_seek(offset)
            .map_err(|_| GenError::CounterOverflow)?;
        self.position = offset;
        Ok(())
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, GenError> {
        let take = usize::try_from(self.remaining()).map_or(buf.len(), |left| left.min(buf.len()));
        let window = &mut buf[..take];
        window.fill(0);
        self.cipher
            .try_apply_keystream(window)
            .map_err(|_| GenError::CounterOverflow)?;
        self.position += take as u64;
        self.generated_bytes += take as u64;
        self.max_read_len = self.max_read_len.max(take);
        Ok(take)
    }
}

pub struct GenReader {
    stream: GeneratedStream,
}

impl GenReader {
    pub fn new(seed: &Seed, size: u64) -> Result<Self, GenError> {
        Ok(Self {
            stream: GeneratedStream::new(seed, size)?,
        })
    }

    pub fn at_offset(seed: &Seed, size: u64, offset: u64) -> Result<Self, GenError> {
        Ok(Self {
            stream: GeneratedStream::at_offset(seed, size, offset)?,
        })
    }

    pub fn seek_to(&mut self, offset: u64) -> Result<(), GenError> {
        self.stream.seek(offset)
    }

    pub fn position(&self) -> u64 {
        self.stream.position()
    }

    pub fn stream(&self) -> &GeneratedStream {
        &self.stream
    }
}

impl AsyncRead for GenReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let window = buf.initialize_unfilled();
        let cap = window.len().min(STREAM_CHUNK_SIZE);
        match this.stream.read(&mut window[..cap]) {
            Ok(filled) => {
                buf.advance(filled);
                Poll::Ready(Ok(()))
            }
            Err(error) => Poll::Ready(Err(io::Error::other(error))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashOutcome {
    pub bytes: u64,
    pub sha256: Sha256Digest,
}

#[derive(Default)]
pub struct GenHasher {
    inner: Sha256,
    bytes: u64,
}

impl GenHasher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
        self.bytes += data.len() as u64;
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.bytes
    }

    pub fn finalize(self) -> HashOutcome {
        HashOutcome {
            bytes: self.bytes,
            sha256: Sha256Digest(self.inner.finalize().into()),
        }
    }
}

pub fn hash_generated(seed: &Seed, size: u64) -> Result<HashOutcome, GenError> {
    hash_generated_range(seed, size, 0, size)
}

pub fn hash_generated_range(
    seed: &Seed,
    size: u64,
    offset: u64,
    length: u64,
) -> Result<HashOutcome, GenError> {
    let mut stream = GeneratedStream::at_offset(seed, size, offset)?;
    hash_stream(&mut stream, length)
}

pub fn hash_stream(stream: &mut GeneratedStream, length: u64) -> Result<HashOutcome, GenError> {
    let (offset, size) = (stream.position(), stream.size());
    if length > stream.remaining() {
        return Err(GenError::RangePastEnd {
            offset,
            length,
            size,
        });
    }
    let mut hasher = GenHasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
    let mut left = length;
    while left > 0 {
        let want = usize::try_from(left).map_or(buf.len(), |l| l.min(buf.len()));
        let got = stream.read(&mut buf[..want])?;
        if got == 0 {
            return Err(GenError::RangePastEnd {
                offset,
                length,
                size,
            });
        }
        hasher.update(&buf[..got]);
        left -= got as u64;
    }
    Ok(hasher.finalize())
}

pub async fn hash_async_reader<R>(reader: &mut R) -> Result<HashOutcome, GenError>
where
    R: AsyncRead + Unpin,
{
    let mut hasher = GenHasher::new();
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
    loop {
        let got = reader
            .read(&mut buf)
            .await
            .map_err(|error| GenError::Io(error.to_string()))?;
        if got == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buf[..got]);
    }
}

pub fn generate_bytes(
    seed: &Seed,
    size: u64,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, GenError> {
    if length > MAX_MATERIALIZED_BYTES {
        return Err(GenError::SliceTooLarge {
            length,
            max: MAX_MATERIALIZED_BYTES,
        });
    }
    let mut stream = GeneratedStream::at_offset(seed, size, offset)?;
    if length > stream.remaining() {
        return Err(GenError::RangePastEnd {
            offset,
            length,
            size,
        });
    }
    let mut out = vec![0u8; length as usize];
    stream.read(&mut out)?;
    Ok(out)
}
