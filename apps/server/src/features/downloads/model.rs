use std::time::Duration;

use mime_guess::mime::{self, Mime};

use crate::domain::content_disposition::DispositionType;
use crate::domain::mime::MimeClass;
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::QueryParams;
use crate::storage::provider::ObjectStat;

pub const DOWNLOAD_CONCURRENCY: usize = 128;
pub const CHUNK_BYTES: usize = 256 * 1024;
pub const PRESIGN_TTL: Duration = Duration::from_secs(300);
pub const PERMIT_WAIT: Duration = Duration::from_secs(5);
pub const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const DISPOSITION_PARAM: &str = "disposition";

const FALLBACK_MIME: &str = "application/octet-stream";

pub fn parse_disposition(raw_query: Option<&str>) -> Result<DispositionType, ApiError> {
    let params = QueryParams::parse(raw_query);
    match params.single(DISPOSITION_PARAM)? {
        None | Some("attachment") => Ok(DispositionType::Attachment),
        Some("inline") => Ok(DispositionType::Inline),
        Some(_) => Err(ApiError::validation([DISPOSITION_PARAM])),
    }
}

pub fn effective_disposition(requested: DispositionType, mime_type: &str) -> DispositionType {
    match requested {
        DispositionType::Inline if inline_is_safe(mime_type) => DispositionType::Inline,
        _ => DispositionType::Attachment,
    }
}

pub fn inline_is_safe(mime_type: &str) -> bool {
    let Ok(parsed) = mime_type.parse::<Mime>() else {
        return false;
    };
    if parsed.suffix().is_some() || parsed.subtype().as_str().contains("xml") {
        return false;
    }
    match MimeClass::of(&parsed) {
        Some(MimeClass::Image | MimeClass::Video | MimeClass::Audio | MimeClass::Pdf) => true,
        Some(MimeClass::Text) => parsed.type_() == mime::TEXT && parsed.subtype() == mime::PLAIN,
        None => false,
    }
}

pub fn served_content_type(stored: &str) -> &str {
    if stored.parse::<Mime>().is_ok() {
        stored
    } else {
        FALLBACK_MIME
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityTag(String);

impl EntityTag {
    pub fn of(checksum: Option<&str>, stat: &ObjectStat) -> Self {
        match checksum.filter(|digest| is_sha256_hex(digest)) {
            Some(digest) => Self(format!("\"{digest}\"")),
            None => Self(format!(
                "\"{:x}-{:x}\"",
                stat.size,
                stat.modified_at.unix_timestamp_nanos() / 1_000
            )),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Clone)]
pub struct StoredFile {
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub checksum: Option<String>,
    pub object_key: String,
    pub provider: String,
}
