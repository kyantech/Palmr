use std::fmt::Write;

use http::header::IF_NONE_MATCH;
use http::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};

// Weak because response compression re-encodes the body without touching
// this header, so one validator names both the identity and the encoded
// representation; weak comparison is what If-None-Match uses anyway.
pub fn weak_etag_from_sha256(sha256: [u8; 32]) -> HeaderValue {
    let mut tag = String::with_capacity(68);
    tag.push_str("W/\"");
    for byte in sha256 {
        let _ = write!(tag, "{byte:02x}");
    }
    tag.push('"');
    HeaderValue::try_from(tag).unwrap_or(HeaderValue::from_static("W/\"\""))
}

pub fn weak_etag(bytes: &[u8]) -> HeaderValue {
    weak_etag_from_sha256(Sha256::digest(bytes).into())
}

pub fn matches_validator(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Ok(etag) = etag.to_str() else {
        return false;
    };
    let current = opaque_tag(etag);
    headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| candidate == "*" || opaque_tag(candidate) == current)
}

fn opaque_tag(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}
