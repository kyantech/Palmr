use std::{fmt, str::FromStr};

use mime_guess::mime::{self, Mime};

pub const MIME_SNIFF_PREFIX_BYTES: usize = 8 * 1024;

const MAX_MIME_LEN: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimeSource {
    Sniffed,
    Extension,
    ClientHint,
    Fallback,
}

impl MimeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sniffed => "sniffed",
            Self::Extension => "extension",
            Self::ClientHint => "client_hint",
            Self::Fallback => "fallback",
        }
    }
}

impl fmt::Display for MimeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimeClass {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
}

impl MimeClass {
    pub fn of(mime_type: &Mime) -> Option<Self> {
        match (mime_type.type_(), mime_type.subtype()) {
            (mime::IMAGE, _) => Some(Self::Image),
            (mime::VIDEO, _) => Some(Self::Video),
            (mime::AUDIO, _) => Some(Self::Audio),
            (mime::TEXT, _) => Some(Self::Text),
            (mime::APPLICATION, mime::PDF) => Some(Self::Pdf),
            (mime::APPLICATION, subtype) if is_textual_application(mime_type, &subtype) => {
                Some(Self::Text)
            }
            _ => None,
        }
    }
}

fn is_textual_application(mime_type: &Mime, subtype: &mime::Name<'_>) -> bool {
    matches!(
        subtype.as_str(),
        "json" | "javascript" | "ecmascript" | "xml" | "x-sh"
    ) || matches!(mime_type.suffix(), Some(mime::JSON | mime::XML))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMime {
    mime_type: Mime,
    source: MimeSource,
}

impl ResolvedMime {
    pub fn mime_type(&self) -> &Mime {
        &self.mime_type
    }

    pub fn essence(&self) -> &str {
        self.mime_type.essence_str()
    }

    pub fn source(&self) -> MimeSource {
        self.source
    }

    pub fn class(&self) -> Option<MimeClass> {
        MimeClass::of(&self.mime_type)
    }
}

pub fn resolve_mime(
    prefix: &[u8],
    normalized_extension: Option<&str>,
    client_hint: Option<&str>,
) -> ResolvedMime {
    let (mime_type, source) = sniff(prefix)
        .map(|found| (found, MimeSource::Sniffed))
        .or_else(|| {
            from_extension(normalized_extension).map(|found| (found, MimeSource::Extension))
        })
        .or_else(|| from_client_hint(client_hint).map(|found| (found, MimeSource::ClientHint)))
        .unwrap_or((mime::APPLICATION_OCTET_STREAM, MimeSource::Fallback));
    ResolvedMime { mime_type, source }
}

fn sniff(prefix: &[u8]) -> Option<Mime> {
    let bounded = &prefix[..prefix.len().min(MIME_SNIFF_PREFIX_BYTES)];
    infer::get(bounded).and_then(|kind| conclusive(kind.mime_type()))
}

fn from_extension(normalized_extension: Option<&str>) -> Option<Mime> {
    let extension = normalized_extension.filter(|ext| !ext.is_empty())?;
    mime_guess::from_ext(extension)
        .iter_raw()
        .find_map(conclusive)
}

fn from_client_hint(client_hint: Option<&str>) -> Option<Mime> {
    conclusive(client_hint?.trim())
}

fn conclusive(text: &str) -> Option<Mime> {
    if text.is_empty() || text.len() > MAX_MIME_LEN {
        return None;
    }
    let parsed = Mime::from_str(text).ok()?;
    let essence = Mime::from_str(parsed.essence_str()).ok()?;
    let specific = !essence.type_().as_str().is_empty()
        && !essence.subtype().as_str().is_empty()
        && essence.type_() != mime::STAR
        && essence.subtype() != mime::STAR
        && essence != mime::APPLICATION_OCTET_STREAM;
    specific.then_some(essence)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!("../../../../tests/fixtures/sniff/", $name)).as_slice()
        };
    }

    const PNG: &[u8] = fixture!("sample.png");
    const JPEG: &[u8] = fixture!("sample.jpg");
    const GIF: &[u8] = fixture!("sample.gif");
    const WEBP: &[u8] = fixture!("sample.webp");
    const PDF: &[u8] = fixture!("sample.pdf");
    const ZIP: &[u8] = fixture!("sample.zip");
    const MP4: &[u8] = fixture!("sample.mp4");
    const WEBM: &[u8] = fixture!("sample.webm");
    const OGG: &[u8] = fixture!("sample.ogg");
    const MP3: &[u8] = fixture!("sample.mp3");
    const TEXT: &[u8] = fixture!("sample.txt");
    const ZIP_NAMED_PNG: &[u8] = fixture!("zip-named.png");
    const TEXT_NAMED_PDF: &[u8] = fixture!("text-named.pdf");
    const UNKNOWN: &[u8] = b"palmr-unrecognised-payload\x7f\x01";

    const ALL_FIXTURES: &[(&str, &[u8])] = &[
        ("sample.png", PNG),
        ("sample.jpg", JPEG),
        ("sample.gif", GIF),
        ("sample.webp", WEBP),
        ("sample.pdf", PDF),
        ("sample.zip", ZIP),
        ("sample.mp4", MP4),
        ("sample.webm", WEBM),
        ("sample.ogg", OGG),
        ("sample.mp3", MP3),
        ("sample.txt", TEXT),
        ("zip-named.png", ZIP_NAMED_PNG),
        ("text-named.pdf", TEXT_NAMED_PDF),
    ];

    fn resolved(
        prefix: &[u8],
        extension: Option<&str>,
        hint: Option<&str>,
    ) -> (String, MimeSource) {
        let result = resolve_mime(prefix, extension, hint);
        (result.essence().to_owned(), result.source())
    }

    type ResolutionCase<'a> = (
        &'a str,
        &'a [u8],
        Option<&'a str>,
        Option<&'a str>,
        &'a str,
        MimeSource,
    );

    fn mime(text: &str) -> Mime {
        Mime::from_str(text).unwrap_or(mime::APPLICATION_OCTET_STREAM)
    }

    #[test]
    fn unit_mime_hybrid_resolution_table() {
        use MimeSource::{ClientHint, Extension, Fallback, Sniffed};

        let table: &[ResolutionCase<'_>] = &[
            (
                "png bytes + png",
                PNG,
                Some("png"),
                None,
                "image/png",
                Sniffed,
            ),
            (
                "png bytes + zip",
                PNG,
                Some("zip"),
                None,
                "image/png",
                Sniffed,
            ),
            (
                "zip bytes + png",
                ZIP_NAMED_PNG,
                Some("png"),
                None,
                "application/zip",
                Sniffed,
            ),
            (
                "zip bytes + png + image hint",
                ZIP_NAMED_PNG,
                Some("png"),
                Some("image/png"),
                "application/zip",
                Sniffed,
            ),
            (
                "pdf bytes + txt",
                PDF,
                Some("txt"),
                None,
                "application/pdf",
                Sniffed,
            ),
            ("jpeg bytes", JPEG, Some("jpg"), None, "image/jpeg", Sniffed),
            ("gif bytes", GIF, Some("gif"), None, "image/gif", Sniffed),
            (
                "webp bytes",
                WEBP,
                Some("webp"),
                None,
                "image/webp",
                Sniffed,
            ),
            (
                "zip bytes",
                ZIP,
                Some("zip"),
                None,
                "application/zip",
                Sniffed,
            ),
            ("mp4 bytes", MP4, Some("mp4"), None, "video/mp4", Sniffed),
            (
                "webm bytes",
                WEBM,
                Some("webm"),
                None,
                "video/webm",
                Sniffed,
            ),
            (
                "ogg opus bytes",
                OGG,
                Some("ogg"),
                None,
                "audio/opus",
                Sniffed,
            ),
            ("mp3 bytes", MP3, Some("mp3"), None, "audio/mpeg", Sniffed),
            (
                "mp4 bytes + hint",
                MP4,
                None,
                Some("text/plain"),
                "video/mp4",
                Sniffed,
            ),
            (
                "text bytes + txt",
                TEXT,
                Some("txt"),
                None,
                "text/plain",
                Extension,
            ),
            (
                "text bytes + pdf",
                TEXT_NAMED_PDF,
                Some("pdf"),
                None,
                "application/pdf",
                Extension,
            ),
            (
                "unknown bytes + csv",
                UNKNOWN,
                Some("csv"),
                None,
                "text/csv",
                Extension,
            ),
            (
                "unknown bytes + csv + hint",
                UNKNOWN,
                Some("csv"),
                Some("image/png"),
                "text/csv",
                Extension,
            ),
            (
                "unknown bytes + valid hint",
                UNKNOWN,
                None,
                Some("image/png"),
                "image/png",
                ClientHint,
            ),
            (
                "unknown bytes + unmapped ext + hint",
                UNKNOWN,
                Some("palmrunknownext"),
                Some("video/mp4"),
                "video/mp4",
                ClientHint,
            ),
            (
                "unknown bytes + invalid hint",
                UNKNOWN,
                None,
                Some("not a mime"),
                "application/octet-stream",
                Fallback,
            ),
            (
                "unknown bytes + nothing",
                UNKNOWN,
                None,
                None,
                "application/octet-stream",
                Fallback,
            ),
            (
                "zero bytes + extension",
                b"",
                Some("mp3"),
                None,
                "audio/mpeg",
                Extension,
            ),
            (
                "zero bytes + extension + hint",
                b"",
                Some("mp3"),
                Some("image/png"),
                "audio/mpeg",
                Extension,
            ),
            (
                "zero bytes + hint",
                b"",
                None,
                Some("application/pdf"),
                "application/pdf",
                ClientHint,
            ),
            (
                "zero bytes + nothing",
                b"",
                None,
                None,
                "application/octet-stream",
                Fallback,
            ),
        ];

        for &(case, prefix, extension, hint, expected_mime, expected_source) in table {
            assert_eq!(
                resolved(prefix, extension, hint),
                (expected_mime.to_owned(), expected_source),
                "{case}"
            );
        }

        for extension in [Some("png"), Some("jpg"), Some("webp"), None] {
            for hint in [Some("image/png"), Some("image/jpeg"), None] {
                let result = resolve_mime(ZIP_NAMED_PNG, extension, hint);
                assert_ne!(
                    result.class(),
                    Some(MimeClass::Image),
                    "{extension:?} {hint:?}"
                );
                assert_eq!(result.essence(), "application/zip");
            }
        }

        let classes: &[(&[u8], Option<&str>, Option<MimeClass>)] = &[
            (PNG, None, Some(MimeClass::Image)),
            (JPEG, None, Some(MimeClass::Image)),
            (GIF, None, Some(MimeClass::Image)),
            (WEBP, None, Some(MimeClass::Image)),
            (MP4, None, Some(MimeClass::Video)),
            (WEBM, None, Some(MimeClass::Video)),
            (OGG, None, Some(MimeClass::Audio)),
            (MP3, None, Some(MimeClass::Audio)),
            (PDF, None, Some(MimeClass::Pdf)),
            (TEXT, Some("txt"), Some(MimeClass::Text)),
            (ZIP, None, None),
            (UNKNOWN, None, None),
        ];
        for &(prefix, extension, expected) in classes {
            let result = resolve_mime(prefix, extension, None);
            assert_eq!(result.class(), expected);
            assert_eq!(MimeClass::of(result.mime_type()), expected);
        }
    }

    #[test]
    fn unit_mime_prefix_bounded() {
        let late_html = [
            vec![b' '; MIME_SNIFF_PREFIX_BYTES],
            b"<html><body>".to_vec(),
        ]
        .concat();
        assert!(
            infer::get(&late_html).is_some(),
            "an unbounded sniff of this input would detect the late signature"
        );
        assert_eq!(
            resolved(&late_html, None, None),
            ("application/octet-stream".to_owned(), MimeSource::Fallback)
        );
        assert_eq!(
            resolved(&late_html, Some("txt"), None),
            ("text/plain".to_owned(), MimeSource::Extension)
        );

        let late_xml = [
            vec![b'\n'; MIME_SNIFF_PREFIX_BYTES + 1],
            b"<?xml version=\"1.0\"?>".to_vec(),
        ]
        .concat();
        assert!(infer::get(&late_xml).is_some());
        assert_eq!(
            resolve_mime(&late_xml, None, None).source(),
            MimeSource::Fallback
        );

        let tag = b"<html>";
        let ending_on_last_byte = [
            vec![b' '; MIME_SNIFF_PREFIX_BYTES - tag.len()],
            tag.to_vec(),
        ]
        .concat();
        assert_eq!(ending_on_last_byte.len(), MIME_SNIFF_PREFIX_BYTES);
        assert_eq!(
            resolved(&ending_on_last_byte, None, None),
            ("text/html".to_owned(), MimeSource::Sniffed)
        );

        let longer = [
            ending_on_last_byte.clone(),
            vec![b'x'; 4 * MIME_SNIFF_PREFIX_BYTES],
        ]
        .concat();
        assert_eq!(
            resolved(&longer, None, None),
            ("text/html".to_owned(), MimeSource::Sniffed)
        );

        let terminator_past_cap = [
            vec![b' '; MIME_SNIFF_PREFIX_BYTES - tag.len() + 1],
            tag.to_vec(),
        ]
        .concat();
        assert!(infer::get(&terminator_past_cap).is_some());
        assert_eq!(
            resolve_mime(&terminator_past_cap, None, None).source(),
            MimeSource::Fallback
        );

        let png_then_padding = [PNG.to_vec(), vec![0; 3 * MIME_SNIFF_PREFIX_BYTES]].concat();
        assert_eq!(
            resolved(&png_then_padding, Some("zip"), None),
            ("image/png".to_owned(), MimeSource::Sniffed)
        );

        let zip_then_late_png = [
            ZIP.to_vec(),
            vec![0; MIME_SNIFF_PREFIX_BYTES - ZIP.len()],
            PNG.to_vec(),
        ]
        .concat();
        assert_eq!(
            resolve_mime(&zip_then_late_png, None, None).essence(),
            "application/zip"
        );

        assert_eq!(resolve_mime(&PNG[..8], None, None).essence(), "image/png");
    }

    #[test]
    fn unit_mime_sniff_fixtures_stay_below_prefix_cap() {
        for &(name, bytes) in ALL_FIXTURES {
            assert!(
                bytes.len() < MIME_SNIFF_PREFIX_BYTES,
                "{name} is {} bytes",
                bytes.len()
            );
        }
    }

    #[test]
    fn unit_mime_extension_lookup_is_case_insensitive() {
        for extension in ["png", "PNG", "Png"] {
            assert_eq!(
                resolved(UNKNOWN, Some(extension), None),
                ("image/png".to_owned(), MimeSource::Extension)
            );
        }
    }

    #[test]
    fn unit_mime_unusable_extension_falls_through() {
        for extension in ["", "bin", "palmrunknownext"] {
            assert_eq!(
                resolved(UNKNOWN, Some(extension), Some("text/plain")),
                ("text/plain".to_owned(), MimeSource::ClientHint),
                "{extension:?}"
            );
        }
    }

    #[test]
    fn unit_mime_malformed_client_hint_ignored() {
        let oversized = format!("application/{}", "x".repeat(MAX_MIME_LEN));
        for hint in [
            "",
            "   ",
            "image",
            "image/",
            "/png",
            "*/*",
            "image/*",
            "application/octet-stream",
            "image/png\r\nX-Injected: 1",
            "imagé/png",
            oversized.as_str(),
        ] {
            assert_eq!(
                resolved(UNKNOWN, None, Some(hint)),
                ("application/octet-stream".to_owned(), MimeSource::Fallback),
                "{hint:?}"
            );
        }
    }

    #[test]
    fn unit_mime_client_hint_reduced_to_lowercase_essence() {
        assert_eq!(
            resolved(UNKNOWN, None, Some(" Text/Plain; charset=UTF-8 ")),
            ("text/plain".to_owned(), MimeSource::ClientHint)
        );
    }

    #[test]
    fn unit_mime_class_helpers() {
        let table: &[(&str, Option<MimeClass>)] = &[
            ("image/png", Some(MimeClass::Image)),
            ("image/svg+xml", Some(MimeClass::Image)),
            ("video/mp4", Some(MimeClass::Video)),
            ("video/webm", Some(MimeClass::Video)),
            ("audio/mpeg", Some(MimeClass::Audio)),
            ("audio/ogg", Some(MimeClass::Audio)),
            ("audio/opus", Some(MimeClass::Audio)),
            ("application/pdf", Some(MimeClass::Pdf)),
            ("text/plain", Some(MimeClass::Text)),
            ("text/html", Some(MimeClass::Text)),
            ("text/csv", Some(MimeClass::Text)),
            ("application/json", Some(MimeClass::Text)),
            ("application/ld+json", Some(MimeClass::Text)),
            ("application/xml", Some(MimeClass::Text)),
            ("application/atom+xml", Some(MimeClass::Text)),
            ("application/javascript", Some(MimeClass::Text)),
            ("application/x-sh", Some(MimeClass::Text)),
            ("application/octet-stream", None),
            ("application/zip", None),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                None,
            ),
            ("font/woff2", None),
        ];
        for &(text, expected) in table {
            assert_eq!(MimeClass::of(&mime(text)), expected, "{text}");
        }
    }

    #[test]
    fn unit_mime_source_matches_schema_values() {
        let values = [
            MimeSource::Sniffed,
            MimeSource::Extension,
            MimeSource::ClientHint,
            MimeSource::Fallback,
        ]
        .map(MimeSource::as_str);
        assert_eq!(values, ["sniffed", "extension", "client_hint", "fallback"]);
    }

    #[test]
    fn unit_mime_resolved_type_fits_column() {
        for &(name, bytes) in ALL_FIXTURES {
            assert!(
                resolve_mime(bytes, None, None).essence().len() <= MAX_MIME_LEN,
                "{name}"
            );
        }
    }
}
