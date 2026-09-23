use bytes::BytesMut;
use http_body_util::{BodyExt, Limited};
use tokio::io::AsyncReadExt;

pub const STREAM_BUFFER_BYTES: usize = 256 * 1024;
pub const MIME_SNIFF_PREFIX_BYTES: u64 = 8 * 1024;
pub const JSON_BODY_LIMIT_BYTES: usize = 64 * 1024;

pub fn stream_buffer() -> BytesMut {
    BytesMut::with_capacity(STREAM_BUFFER_BYTES)
}

pub fn sha256_digest() -> Vec<u8> {
    vec![0u8; 32]
}

pub async fn sniff_prefix(file: tokio::fs::File) -> std::io::Result<Vec<u8>> {
    let mut prefix = Vec::with_capacity(MIME_SNIFF_PREFIX_BYTES as usize);
    file.take(MIME_SNIFF_PREFIX_BYTES).read_to_end(&mut prefix).await?;
    Ok(prefix)
}

pub async fn json_body(body: axum::body::Body) -> bytes::Bytes {
    Limited::new(body, JSON_BODY_LIMIT_BYTES).collect().await.unwrap().to_bytes()
}

pub fn staging_entries(root: &std::path::Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(root)
}

/// Never call `std::fs::read(path)` or `SystemTime::now()` here; see ADR 0008.
// ByteStream::collect() is forbidden; body::to_bytes too.
pub fn documented() {}
