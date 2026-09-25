use http::HeaderValue;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

pub const ASCII_FALLBACK_MAX_BYTES: usize = 80;
pub const EMPTY_ASCII_FALLBACK: &str = "download";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispositionType {
    Attachment,
    Inline,
}

impl DispositionType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attachment => "attachment",
            Self::Inline => "inline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentDisposition {
    kind: DispositionType,
    value: String,
}

impl ContentDisposition {
    pub fn attachment(name: &str) -> Self {
        Self::new(DispositionType::Attachment, name)
    }

    pub fn inline(name: &str) -> Self {
        Self::new(DispositionType::Inline, name)
    }

    pub fn new(kind: DispositionType, name: &str) -> Self {
        let display: String = name.nfc().collect();
        let value = format!(
            "{}; filename=\"{}\"; filename*=UTF-8''{}",
            kind.as_str(),
            ascii_fallback(&display),
            percent_encode_ext_value(&display),
        );
        Self { kind, value }
    }

    pub const fn kind(&self) -> DispositionType {
        self.kind
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn to_header_value(&self) -> HeaderValue {
        HeaderValue::from_str(&self.value)
            .unwrap_or_else(|_| HeaderValue::from_static(self.kind.as_str()))
    }
}

pub fn ascii_fallback(name: &str) -> String {
    let mut fallback = String::with_capacity(ASCII_FALLBACK_MAX_BYTES);
    for c in name.nfkd().filter(|&c| !is_combining_mark(c)) {
        match transliteration(c) {
            Some(latin) => latin
                .chars()
                .for_each(|c| push_fallback_char(&mut fallback, c)),
            None => push_fallback_char(&mut fallback, c),
        }
        if fallback.len() >= ASCII_FALLBACK_MAX_BYTES {
            break;
        }
    }
    fallback.truncate(ASCII_FALLBACK_MAX_BYTES);
    fallback.truncate(fallback.trim_end_matches(' ').len());
    if fallback.is_empty() {
        fallback.push_str(EMPTY_ASCII_FALLBACK);
    }
    fallback
}

fn push_fallback_char(fallback: &mut String, c: char) {
    let kept = match c {
        '"' | '\\' => return,
        'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | ' ' => c,
        _ => '_',
    };
    let collapses = matches!(kept, '_' | ' ') && fallback.ends_with(kept);
    let leading_space = kept == ' ' && fallback.is_empty();
    if !collapses && !leading_space {
        fallback.push(kept);
    }
}

fn transliteration(c: char) -> Option<&'static str> {
    Some(match c {
        'ß' => "ss",
        'Æ' => "AE",
        'æ' => "ae",
        'Œ' => "OE",
        'œ' => "oe",
        'Ø' => "O",
        'ø' => "o",
        'Ð' | 'Đ' => "D",
        'ð' | 'đ' => "d",
        'Ł' => "L",
        'ł' => "l",
        'Þ' => "TH",
        'þ' => "th",
        'ı' => "i",
        'Ħ' => "H",
        'ħ' => "h",
        _ => return None,
    })
}

pub fn percent_encode_ext_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_attr_char(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

const fn is_attr_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

#[cfg(test)]
mod tests {
    use mime_guess::mime::Mime;
    use percent_encoding::percent_decode_str;
    use proptest::prelude::*;
    use unicode_normalization::UnicodeNormalization;

    use super::*;
    use crate::domain::normalize::tests::unicode_text;

    struct Parsed {
        kind: String,
        filename: String,
        filename_star: String,
    }

    fn parse_independently(header: &HeaderValue) -> Parsed {
        let text = header.to_str().expect("visible ASCII header value");
        let (kind, params) = text.split_once(';').expect("parameters present");
        let media: Mime = format!("x/y;{params}")
            .parse()
            .unwrap_or_else(|error| panic!("{text:?} is not an RFC 7231 parameter list: {error}"));
        let mut filename = None;
        let mut filename_star = None;
        for (name, value) in media.params() {
            match name.as_str() {
                "filename" => filename = Some(value.as_str().to_owned()),
                "filename*" => filename_star = Some(value.as_str().to_owned()),
                other => panic!("unexpected parameter {other:?} in {text:?}"),
            }
        }
        Parsed {
            kind: kind.to_owned(),
            filename: filename.expect("filename present"),
            filename_star: filename_star.expect("filename* present"),
        }
    }

    fn assert_safe(name: &str) {
        let expected: String = name.nfc().collect();
        for (kind, disposition) in [
            ("attachment", ContentDisposition::attachment(name)),
            ("inline", ContentDisposition::inline(name)),
        ] {
            let header = disposition.to_header_value();
            assert_eq!(header.as_bytes(), disposition.as_str().as_bytes());
            assert!(
                !disposition
                    .as_str()
                    .bytes()
                    .any(|b| matches!(b, b'\r' | b'\n' | 0)),
                "{name:?}"
            );
            assert!(disposition
                .as_str()
                .bytes()
                .all(|b| b == b' ' || b.is_ascii_graphic()));

            let wire = format!("Content-Disposition: {}\r\n", disposition.as_str());
            assert_eq!(wire.matches("\r\n").count(), 1);
            assert_eq!(wire.matches(':').count(), 1, "{name:?}");

            let parsed = parse_independently(&header);
            assert_eq!(parsed.kind, kind);
            assert!(!parsed.filename.is_empty());
            assert!(parsed.filename.len() <= ASCII_FALLBACK_MAX_BYTES);
            assert!(parsed
                .filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b' ' | b'-')));
            assert!(!parsed.filename.starts_with(' ') && !parsed.filename.ends_with(' '));

            let encoded = parsed
                .filename_star
                .strip_prefix("UTF-8''")
                .expect("UTF-8 ext-value without language");
            assert!(encoded.bytes().all(|b| is_attr_char(b) || b == b'%'));
            let decoded = percent_decode_str(encoded)
                .decode_utf8()
                .expect("filename* decodes to UTF-8");
            assert_eq!(decoded, expected, "{name:?}");

            assert_eq!(
                disposition,
                ContentDisposition::new(disposition.kind(), name)
            );
        }
    }

    #[test]
    fn regression_291_content_disposition_special_names() {
        let long_unicode = "Relatório 年度報告書 تقرير 📦 ".repeat(40);
        let corpus = [
            "say \"hello\".txt",
            "a;b.txt",
            "a,b.txt",
            "back\\slash.txt",
            "line\rbreak.txt",
            "line\nbreak.txt",
            "Content-Type: text/html\r\nSet-Cookie: x=1",
            "nul\0byte.txt",
            "%0d%0a.txt",
            "",
            "\u{0301}\u{0308}\u{0327}",
            "📦 backup 2026.tar.gz",
            "年度報告書.pdf",
            "تقرير سنوي.pdf",
            "\u{202e}fdp.exe",
            "Cafe\u{0301}.txt",
            long_unicode.as_str(),
            "archive.tar.gz",
            ".env",
            "README",
            "LICENSE",
            "Makefile",
            "   ",
            "\"\\\"\\",
            "a\tb",
            "\u{7f}\u{85}\u{2028}",
            "=?UTF-8?B?Zm9v?=",
            "x'y*z%",
        ];
        for name in corpus {
            assert_safe(name);
        }
    }

    #[test]
    fn unit_content_disposition_exact_forms() {
        for (name, fallback, encoded) in [
            (
                "Relatório Anual.pdf",
                "Relatorio Anual.pdf",
                "Relat%C3%B3rio%20Anual.pdf",
            ),
            ("README", "README", "README"),
            ("Makefile", "Makefile", "Makefile"),
            (".env", ".env", ".env"),
            ("archive.tar.gz", "archive.tar.gz", "archive.tar.gz"),
            (
                "say \"hi\\there\".txt",
                "say hithere.txt",
                "say%20%22hi%5Cthere%22.txt",
            ),
            ("line\r\nbreak", "line_break", "line%0D%0Abreak"),
            ("%0d%0a", "_0d_0a", "%250d%250a"),
            ("a  (b)  c", "a _b_ c", "a%20%20%28b%29%20%20c"),
            (
                "年度報告書.pdf",
                "_.pdf",
                "%E5%B9%B4%E5%BA%A6%E5%A0%B1%E5%91%8A%E6%9B%B8.pdf",
            ),
            (
                "Straße Æon Łódź",
                "Strasse AEon Lodz",
                "Stra%C3%9Fe%20%C3%86on%20%C5%81%C3%B3d%C5%BA",
            ),
            ("", "download", ""),
            ("\u{0301}", "download", "%CC%81"),
            ("  padded  ", "padded", "%20%20padded%20%20"),
            ("Cafe\u{0301}", "Cafe", "Caf%C3%A9"),
        ] {
            assert_eq!(
                ascii_fallback(&name.nfc().collect::<String>()),
                fallback,
                "{name:?}"
            );
            assert_eq!(
                ContentDisposition::attachment(name).as_str(),
                format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"),
            );
            assert_eq!(
                ContentDisposition::inline(name).as_str(),
                format!("inline; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"),
            );
        }
    }

    #[test]
    fn unit_content_disposition_fallback_truncates_at_80_bytes() {
        let name = format!("{}.pdf", "a".repeat(100));
        assert_eq!(ascii_fallback(&name), "a".repeat(80));
        let spaced = format!("{} tail", "b".repeat(79));
        assert_eq!(ascii_fallback(&spaced), "b".repeat(79));
        assert_eq!(ascii_fallback(&"é".repeat(200)), "e".repeat(80));
    }

    fn display_name() -> impl Strategy<Value = String> {
        prop_oneof![
            prop::collection::vec(any::<char>(), 0..120).prop_map(String::from_iter),
            unicode_text(),
            prop::collection::vec(
                prop::sample::select(vec![
                    '"',
                    '\\',
                    ';',
                    ',',
                    '\r',
                    '\n',
                    '\0',
                    '%',
                    '\'',
                    '*',
                    '=',
                    ' ',
                    '\t',
                    'a',
                    '.',
                    '\u{0301}',
                    '\u{00e9}',
                    '\u{1f4e6}',
                    '\u{5e74}',
                    '\u{202e}',
                    '\u{ffff}',
                ]),
                0..120,
            )
            .prop_map(String::from_iter),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn prop_content_disposition_is_parseable(name in display_name()) {
            assert_safe(&name);
        }
    }
}
