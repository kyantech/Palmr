use std::collections::HashSet;

use base64ct::{Base64, Encoding};
use http::HeaderMap;
use unicode_normalization::UnicodeNormalization;

use crate::domain::naming::NameCandidate;
use crate::domain::relative_path::RelativePath;
use crate::features::folders::FolderId;

use super::super::model::{SessionItemId, TransferSessionId};
use super::error::TusError;
use super::headers::{HEADER_METADATA, UPLOAD_METADATA};

pub const MAX_HEADER_BYTES: usize = 8 * 1024;
pub const MAX_FILENAME_BYTES: usize = 512;
pub const MAX_RELATIVE_PATH_BYTES: usize = 1024;
pub const MAX_FILETYPE_CHARS: usize = 255;

const MAX_KEY_BYTES: usize = 64;
const ID_BYTES: usize = 36;

pub const KEY_FILENAME: &str = "filename";
pub const KEY_FILETYPE: &str = "filetype";
pub const KEY_RELATIVE_PATH: &str = "relativePath";
pub const KEY_TRANSFER_SESSION_ID: &str = "transferSessionId";
pub const KEY_ITEM_ID: &str = "itemId";
pub const KEY_FOLDER_ID: &str = "folderId";
pub const KEY_REVERSE_SHARE_SESSION_ID: &str = "reverseShareSessionId";

const KNOWN_KEYS: [&str; 7] = [
    KEY_FILENAME,
    KEY_FILETYPE,
    KEY_RELATIVE_PATH,
    KEY_TRANSFER_SESSION_ID,
    KEY_ITEM_ID,
    KEY_FOLDER_ID,
    KEY_REVERSE_SHARE_SESSION_ID,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadMetadata {
    pub filename: NameCandidate,
    pub filetype: Option<String>,
    pub relative_path: Option<RelativePath>,
    pub transfer_session_id: TransferSessionId,
    pub item_id: SessionItemId,
    pub folder_id: Option<FolderId>,
    pub reverse_share_session_id: Option<String>,
}

struct Pairs<'a> {
    entries: Vec<(&'a str, Option<&'a str>)>,
}

impl<'a> Pairs<'a> {
    fn value(&self, key: &'static str) -> Option<Option<&'a str>> {
        self.entries
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
    }
}

impl UploadMetadata {
    pub fn parse(headers: &HeaderMap) -> Result<Self, TusError> {
        let raw = joined_header(headers)?;
        let pairs = split_pairs(&raw)?;

        let filename = pairs
            .value(KEY_FILENAME)
            .ok_or(TusError::Header { key: KEY_FILENAME })
            .and_then(|value| parse_filename(value))?;
        let transfer_session_id = required_id(&pairs, KEY_TRANSFER_SESSION_ID)?;
        let item_id = required_id(&pairs, KEY_ITEM_ID)?;
        let folder_id = optional_id::<FolderId>(&pairs, KEY_FOLDER_ID)?;
        let reverse_share_session_id =
            optional_id::<SessionItemId>(&pairs, KEY_REVERSE_SHARE_SESSION_ID)?
                .map(|id| id.to_string());
        let filetype = match pairs.value(KEY_FILETYPE) {
            None => None,
            Some(value) => {
                let text = decode_text(value, KEY_FILETYPE, MAX_FILETYPE_CHARS)?;
                (!text.is_empty() && text.chars().count() <= MAX_FILETYPE_CHARS).then_some(text)
            }
        };
        let relative_path = match pairs.value(KEY_RELATIVE_PATH) {
            None => None,
            Some(value) => {
                let text = decode_text(value, KEY_RELATIVE_PATH, MAX_RELATIVE_PATH_BYTES)?;
                if text.is_empty() {
                    None
                } else {
                    let path = RelativePath::parse(&text).map_err(|_| TusError::Header {
                        key: KEY_RELATIVE_PATH,
                    })?;
                    if path.leaf().display() != filename.display() {
                        return Err(TusError::Header {
                            key: KEY_RELATIVE_PATH,
                        });
                    }
                    Some(path)
                }
            }
        };
        Ok(Self {
            filename,
            filetype,
            relative_path,
            transfer_session_id,
            item_id,
            folder_id,
            reverse_share_session_id,
        })
    }

    pub fn stored_json(&self) -> String {
        let mut object = serde_json::Map::new();
        object.insert(KEY_FILENAME.into(), self.filename.display().into());
        if let Some(filetype) = &self.filetype {
            object.insert(KEY_FILETYPE.into(), filetype.as_str().into());
        }
        if let Some(path) = &self.relative_path {
            object.insert(KEY_RELATIVE_PATH.into(), path.joined().into());
        }
        object.insert(
            KEY_TRANSFER_SESSION_ID.into(),
            self.transfer_session_id.to_string().into(),
        );
        object.insert(KEY_ITEM_ID.into(), self.item_id.to_string().into());
        if let Some(folder) = self.folder_id {
            object.insert(KEY_FOLDER_ID.into(), folder.to_string().into());
        }
        serde_json::Value::Object(object).to_string()
    }
}

fn joined_header(headers: &HeaderMap) -> Result<String, TusError> {
    let invalid = || TusError::Header {
        key: HEADER_METADATA,
    };
    let mut joined = String::new();
    for value in headers.get_all(UPLOAD_METADATA) {
        if !joined.is_empty() {
            joined.push(',');
        }
        if joined.len() + value.len() > MAX_HEADER_BYTES {
            return Err(invalid());
        }
        joined.push_str(value.to_str().map_err(|_| invalid())?);
    }
    if joined.is_empty() {
        return Err(invalid());
    }
    Ok(joined)
}

fn split_pairs(raw: &str) -> Result<Pairs<'_>, TusError> {
    let invalid = || TusError::Header {
        key: HEADER_METADATA,
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut entries = Vec::new();
    for segment in raw.split(',') {
        let segment = segment.trim_matches([' ', '\t']);
        if segment.is_empty() {
            return Err(invalid());
        }
        let (key, value) = match segment.split_once(' ') {
            Some((key, value)) => (key, Some(value)),
            None => (segment, None),
        };
        if !is_key(key) || value.is_some_and(|value| value.contains([' ', '\t'])) {
            return Err(invalid());
        }
        if !seen.insert(key) {
            return Err(KNOWN_KEYS
                .iter()
                .find(|known| **known == key)
                .map_or_else(invalid, |known| TusError::Header { key: known }));
        }
        entries.push((key, value));
    }
    Ok(Pairs { entries })
}

fn is_key(key: &str) -> bool {
    (1..=MAX_KEY_BYTES).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b',')
}

fn decode_text(
    value: Option<&str>,
    key: &'static str,
    max_bytes: usize,
) -> Result<String, TusError> {
    let invalid = || TusError::Header { key };
    let encoded = value.unwrap_or_default();
    if encoded.len() > max_bytes.div_ceil(3) * 4 {
        return Err(invalid());
    }
    let decoded = Base64::decode_vec(encoded).map_err(|_| invalid())?;
    if decoded.len() > max_bytes {
        return Err(invalid());
    }
    let text = String::from_utf8(decoded).map_err(|_| invalid())?;
    if text.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(text)
}

fn parse_filename(value: Option<&str>) -> Result<NameCandidate, TusError> {
    let invalid = || TusError::Header { key: KEY_FILENAME };
    let text = decode_text(value, KEY_FILENAME, MAX_FILENAME_BYTES)?;
    if text.is_empty() {
        return Err(invalid());
    }
    let composed: String = text.nfc().collect();
    NameCandidate::new(composed).map_err(|_| invalid())
}

fn required_id<I: std::str::FromStr>(pairs: &Pairs<'_>, key: &'static str) -> Result<I, TusError> {
    optional_id(pairs, key)?.ok_or(TusError::Header { key })
}

fn optional_id<I: std::str::FromStr>(
    pairs: &Pairs<'_>,
    key: &'static str,
) -> Result<Option<I>, TusError> {
    let Some(value) = pairs.value(key) else {
        return Ok(None);
    };
    let text = decode_text(value, key, ID_BYTES)?;
    text.parse::<I>()
        .map(Some)
        .map_err(|_| TusError::Header { key })
}

#[cfg(test)]
mod tests {
    use base64ct::{Base64, Encoding};
    use http::{HeaderMap, HeaderName, HeaderValue};

    use super::{
        UploadMetadata, KEY_FILENAME, KEY_FILETYPE, KEY_FOLDER_ID, KEY_ITEM_ID, KEY_RELATIVE_PATH,
        KEY_REVERSE_SHARE_SESSION_ID, KEY_TRANSFER_SESSION_ID, MAX_FILENAME_BYTES,
        MAX_HEADER_BYTES,
    };
    use crate::features::transfers::tus::error::TusError;

    const SESSION: &str = "0192f3a7-2a01-7c4d-8e11-aa0192f3a700";
    const ITEM: &str = "0192f3a7-2a01-7c4d-8e11-aa0192f3a701";
    const FOLDER: &str = "0192f3a7-2a01-7c4d-8e11-aa0192f3a702";

    fn b64(text: &str) -> String {
        Base64::encode_string(text.as_bytes())
    }

    fn pair(key: &str, value: &str) -> String {
        format!("{key} {}", b64(value))
    }

    fn header(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.append(
            HeaderName::from_static("upload-metadata"),
            HeaderValue::from_str(value).unwrap(),
        );
        map
    }

    fn base(filename: &str) -> Vec<String> {
        vec![
            pair(KEY_FILENAME, filename),
            pair(KEY_TRANSFER_SESSION_ID, SESSION),
            pair(KEY_ITEM_ID, ITEM),
        ]
    }

    fn parse(pairs: &[String]) -> Result<UploadMetadata, TusError> {
        UploadMetadata::parse(&header(&pairs.join(",")))
    }

    fn rejected_key(result: Result<UploadMetadata, TusError>) -> &'static str {
        match result {
            Err(TusError::Header { key }) => key,
            other => panic!("expected a header rejection, got {other:?}"),
        }
    }

    #[test]
    fn unit_minimal_metadata_binds_the_exact_item() {
        let parsed = parse(&base("photo.jpg")).unwrap();
        assert_eq!(parsed.filename.display(), "photo.jpg");
        assert_eq!(parsed.transfer_session_id.to_string(), SESSION);
        assert_eq!(parsed.item_id.to_string(), ITEM);
        assert!(parsed.filetype.is_none() && parsed.relative_path.is_none());
        assert!(parsed.folder_id.is_none() && parsed.reverse_share_session_id.is_none());
    }

    #[test]
    fn unit_every_key_is_decoded_and_unknown_keys_are_ignored() {
        let mut pairs = base("photo.jpg");
        pairs.push(pair(KEY_FILETYPE, "image/jpeg"));
        pairs.push(pair(KEY_RELATIVE_PATH, "Trip/Day 1/photo.jpg"));
        pairs.push(pair(KEY_FOLDER_ID, FOLDER));
        pairs.push("shadowKey !!!not-base64!!!".to_owned());
        pairs.push("flagOnly".to_owned());
        pairs.push(pair("ownerId", "someone-else"));
        let parsed = parse(&pairs).unwrap();
        assert_eq!(parsed.filetype.as_deref(), Some("image/jpeg"));
        assert_eq!(
            parsed.relative_path.as_ref().unwrap().joined(),
            "Trip/Day 1/photo.jpg"
        );
        assert_eq!(parsed.folder_id.unwrap().to_string(), FOLDER);
        let stored = parsed.stored_json();
        assert!(!stored.contains("shadowKey") && !stored.contains("ownerId"));
        assert!(!stored.contains("someone-else"));
        assert!(stored.len() <= 4_096);
        let json: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(json["itemId"], ITEM);
    }

    #[test]
    fn unit_required_keys_are_enforced_and_named() {
        for (omit, key) in [
            (KEY_FILENAME, "filename"),
            (KEY_TRANSFER_SESSION_ID, "transferSessionId"),
            (KEY_ITEM_ID, "itemId"),
        ] {
            let pairs: Vec<String> = base("a.txt")
                .into_iter()
                .filter(|pair| !pair.starts_with(omit))
                .collect();
            assert_eq!(rejected_key(parse(&pairs)), key);
        }
        assert_eq!(
            rejected_key(UploadMetadata::parse(&HeaderMap::new())),
            "Upload-Metadata"
        );
    }

    #[test]
    fn unit_ids_must_be_canonical_uuid_v7() {
        for bad in [
            "",
            "not-an-id",
            &SESSION.to_uppercase(),
            &SESSION.replace('-', ""),
            "0192f3a7-2a01-4c4d-8e11-aa0192f3a700",
            " 0192f3a7-2a01-7c4d-8e11-aa0192f3a700",
        ] {
            let mut pairs = base("a.txt");
            pairs[1] = pair(KEY_TRANSFER_SESSION_ID, bad);
            assert_eq!(rejected_key(parse(&pairs)), "transferSessionId", "{bad:?}");
            let mut pairs = base("a.txt");
            pairs[2] = pair(KEY_ITEM_ID, bad);
            assert_eq!(rejected_key(parse(&pairs)), "itemId", "{bad:?}");
        }
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_FOLDER_ID, "nope"));
        assert_eq!(rejected_key(parse(&pairs)), "folderId");
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_REVERSE_SHARE_SESSION_ID, "nope"));
        assert_eq!(rejected_key(parse(&pairs)), "reverseShareSessionId");
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_REVERSE_SHARE_SESSION_ID, FOLDER));
        assert_eq!(
            parse(&pairs).unwrap().reverse_share_session_id.as_deref(),
            Some(FOLDER)
        );
    }

    #[test]
    fn unit_base64_is_strict() {
        let encoded = b64("photo.jpeg");
        assert_eq!(encoded, "cGhvdG8uanBlZw==");
        for bad in [
            "cGhvdG8uanBlZw",
            "cGhvdG8uanBlZw===",
            "cGhvdG8uanBlZw=",
            "cGhvdG8u anBlZw==",
            "@@@@",
            "cGhvdG8-anBlZw==",
            "cGhvdG8_anBlZw==",
            "cGhvdG8uanBlZx==",
        ] {
            let mut pairs = base("a.txt");
            pairs[0] = format!("filename {bad}");
            let outcome = parse(&pairs);
            assert!(
                matches!(outcome, Err(TusError::Header { key: "filename" }))
                    || matches!(
                        outcome,
                        Err(TusError::Header {
                            key: "Upload-Metadata"
                        })
                    ),
                "{bad:?} -> {outcome:?}"
            );
        }
        let mut pairs = base("a.txt");
        pairs[0] = format!("filename {encoded}");
        assert_eq!(parse(&pairs).unwrap().filename.display(), "photo.jpeg");
        let mut pairs = base("a.txt");
        pairs[0] = "filename".to_owned();
        assert_eq!(rejected_key(parse(&pairs)), "filename");
        pairs[0] = "filename ".to_owned();
        assert_eq!(rejected_key(parse(&pairs)), "filename");
    }

    #[test]
    fn unit_text_must_be_utf8_without_control_characters() {
        let mut pairs = base("a.txt");
        pairs[0] = format!("filename {}", Base64::encode_string(&[0xff, 0xfe, 0x41]));
        assert_eq!(rejected_key(parse(&pairs)), "filename");
        for name in ["a\u{0}b", "a\nb", "a\u{7f}b", "a\u{85}b", "tab\there"] {
            assert_eq!(rejected_key(parse(&base(name))), "filename", "{name:?}");
        }
        for name in ["", "..", ".", "a/b", "a\\b"] {
            assert_eq!(rejected_key(parse(&base(name))), "filename", "{name:?}");
        }
    }

    #[test]
    fn unit_filename_bounds_are_utf8_bytes() {
        let at_limit = "x".repeat(255);
        assert_eq!(
            parse(&base(&at_limit)).unwrap().filename.display(),
            at_limit
        );
        let too_long = "x".repeat(256);
        assert_eq!(rejected_key(parse(&base(&too_long))), "filename");
        let beyond_metadata_cap = "x".repeat(MAX_FILENAME_BYTES + 1);
        assert_eq!(rejected_key(parse(&base(&beyond_metadata_cap))), "filename");
        let long_multibyte = "日".repeat(85);
        assert_eq!(
            parse(&base(&long_multibyte)).unwrap().filename.display(),
            long_multibyte
        );
        let over_bytes = "日".repeat(86);
        assert_eq!(rejected_key(parse(&base(&over_bytes))), "filename");
    }

    #[test]
    fn unit_unicode_filenames_are_nfc_normalized() {
        let decomposed = "re\u{301}sume\u{301}.txt";
        let parsed = parse(&base(decomposed)).unwrap();
        assert_eq!(parsed.filename.display(), "r\u{e9}sum\u{e9}.txt");
        for name in [
            "Relatório de Março (final) v2.pdf",
            "🎉🚀.png",
            "日本語のファイル.txt",
            "中文文件名.docx",
            "archive.tar.gz.bak.txt",
            "CON.txt",
            "aux",
            "name with  spaces .txt",
        ] {
            assert_eq!(parse(&base(name)).unwrap().filename.display(), name);
        }
    }

    #[test]
    fn unit_relative_path_follows_the_wire_grammar_and_matches_the_leaf() {
        let with_path = |path: &str| {
            let mut pairs = base("logo.svg");
            pairs.push(pair(KEY_RELATIVE_PATH, path));
            parse(&pairs)
        };
        assert!(with_path("Project/assets/logo.svg").is_ok());
        assert!(with_path("logo.svg").is_ok());
        assert!(with_path("").unwrap().relative_path.is_none());
        for bad in [
            "../../etc/passwd",
            "/abs/logo.svg",
            "a//logo.svg",
            "a/./logo.svg",
            "a/../logo.svg",
            "a/logo.svg/",
            "C:/logo.svg",
            "a\u{0}/logo.svg",
            "Project/assets/other.svg",
        ] {
            assert_eq!(rejected_key(with_path(bad)), "relativePath", "{bad:?}");
        }
        let deep = format!("{}logo.svg", "d/".repeat(32));
        assert_eq!(rejected_key(with_path(&deep)), "relativePath");
        let ok_deep = format!("{}logo.svg", "d/".repeat(31));
        assert!(with_path(&ok_deep).is_ok());
        let long = format!("{}/logo.svg", "d".repeat(1_100));
        assert_eq!(rejected_key(with_path(&long)), "relativePath");
    }

    #[test]
    fn unit_pair_syntax_is_strict() {
        let good = pair(KEY_FILENAME, "a.txt");
        let rest = format!(
            "{},{}",
            pair(KEY_TRANSFER_SESSION_ID, SESSION),
            pair(KEY_ITEM_ID, ITEM)
        );
        assert!(UploadMetadata::parse(&header(&format!("{good},{rest}"))).is_ok());
        assert!(UploadMetadata::parse(&header(&format!(" {good} , {rest} "))).is_ok());
        for bad in [
            format!("{good},,{rest}"),
            format!("{good},{rest},"),
            format!(",{good},{rest}"),
            format!("{},{rest}", good.replace(' ', "  ")),
            format!("{},{rest}", good.replace(' ', "\t")),
            format!("{good},{rest},{good}"),
        ] {
            assert!(
                matches!(
                    UploadMetadata::parse(&header(&bad)),
                    Err(TusError::Header { .. })
                ),
                "{bad:?}"
            );
        }
        let duplicate = format!("{good},{rest},{}", pair(KEY_ITEM_ID, FOLDER));
        assert_eq!(
            rejected_key(UploadMetadata::parse(&header(&duplicate))),
            "itemId"
        );
        let unknown_twice = format!("{good},{rest},zzz YQ==,zzz Yg==");
        assert_eq!(
            rejected_key(UploadMetadata::parse(&header(&unknown_twice))),
            "Upload-Metadata"
        );
    }

    #[test]
    fn unit_repeated_header_lines_are_combined_and_bounded() {
        let mut map = HeaderMap::new();
        for line in [
            pair(KEY_FILENAME, "a.txt"),
            pair(KEY_TRANSFER_SESSION_ID, SESSION),
            pair(KEY_ITEM_ID, ITEM),
        ] {
            map.append(
                HeaderName::from_static("upload-metadata"),
                HeaderValue::from_str(&line).unwrap(),
            );
        }
        assert!(UploadMetadata::parse(&map).is_ok());

        let mut pairs = base("a.txt");
        pairs.push(format!("pad {}", "A".repeat(MAX_HEADER_BYTES)));
        assert_eq!(rejected_key(parse(&pairs)), "Upload-Metadata");
        let mut pairs = base("a.txt");
        let filler = (0..900)
            .map(|index| format!("k{index} QQ=="))
            .collect::<Vec<_>>()
            .join(",");
        pairs.push(filler);
        assert!(parse(&pairs).is_err());
    }

    #[test]
    fn unit_filetype_is_advisory_and_bounded() {
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_FILETYPE, ""));
        assert!(parse(&pairs).unwrap().filetype.is_none());
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_FILETYPE, &"t".repeat(256)));
        assert_eq!(rejected_key(parse(&pairs)), "filetype");
        let mut pairs = base("a.txt");
        pairs.push(pair(KEY_FILETYPE, "text/plain\u{0}"));
        assert_eq!(rejected_key(parse(&pairs)), "filetype");
    }
}
