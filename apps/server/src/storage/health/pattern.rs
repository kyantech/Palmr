use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt as _, ReadBuf};

use crate::storage::provider::ObjectBody;

const COMPARE_CHUNK: usize = 8 * 1024;

pub(in crate::storage) fn byte_at(seed: u64, index: u64) -> u8 {
    let mut value = seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 31;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 29;
    value.to_le_bytes()[0]
}

#[derive(Debug, Clone, Copy)]
pub(in crate::storage) struct ProbePattern {
    seed: u64,
    position: u64,
    end: u64,
}

impl ProbePattern {
    pub(in crate::storage) const fn new(seed: u64, start: u64, len: u64) -> Self {
        Self {
            seed,
            position: start,
            end: start.saturating_add(len),
        }
    }

    pub(in crate::storage) fn body(self) -> ObjectBody {
        Box::pin(self)
    }
}

impl AsyncRead for ProbePattern {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let remaining = this.end.saturating_sub(this.position);
        let count = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.remaining());
        let filled = buf.initialize_unfilled_to(count);
        for (slot, index) in filled.iter_mut().zip(this.position..) {
            *slot = byte_at(this.seed, index);
        }
        buf.advance(count);
        this.position += count as u64;
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::storage) enum Comparison {
    Equal,
    Different,
}

pub(in crate::storage) async fn compare(
    body: ObjectBody,
    seed: u64,
    start: u64,
    len: u64,
) -> io::Result<Comparison> {
    let mut body = body.take(len.saturating_add(1));
    let mut buffer = [0_u8; COMPARE_CHUNK];
    let mut offset = 0_u64;
    loop {
        let read = body.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if offset >= len || *byte != byte_at(seed, start + offset) {
                return Ok(Comparison::Different);
            }
            offset += 1;
        }
    }
    Ok(if offset == len {
        Comparison::Equal
    } else {
        Comparison::Different
    })
}
