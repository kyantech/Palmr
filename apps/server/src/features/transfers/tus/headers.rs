use http::header::{HeaderMap, HeaderName, HeaderValue, CACHE_CONTROL};
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use url::Url;

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::infra::http::pagination::MAX_WIRE_BYTES;

use super::error::TusError;

pub const TUS_RESUMABLE: HeaderName = HeaderName::from_static("tus-resumable");
pub const TUS_VERSION: HeaderName = HeaderName::from_static("tus-version");
pub const TUS_EXTENSION: HeaderName = HeaderName::from_static("tus-extension");
pub const TUS_MAX_SIZE: HeaderName = HeaderName::from_static("tus-max-size");
pub const TUS_CHECKSUM_ALGORITHM: HeaderName = HeaderName::from_static("tus-checksum-algorithm");
pub const UPLOAD_LENGTH: HeaderName = HeaderName::from_static("upload-length");
pub const UPLOAD_DEFER_LENGTH: HeaderName = HeaderName::from_static("upload-defer-length");
pub const UPLOAD_OFFSET: HeaderName = HeaderName::from_static("upload-offset");
pub const UPLOAD_METADATA: HeaderName = HeaderName::from_static("upload-metadata");
pub const UPLOAD_EXPIRES: HeaderName = HeaderName::from_static("upload-expires");
pub const UPLOAD_CONCAT: HeaderName = HeaderName::from_static("upload-concat");
pub const METHOD_OVERRIDE: HeaderName = HeaderName::from_static("x-http-method-override");

pub const PROTOCOL_VERSION: &str = "1.0.0";
pub const EXTENSIONS: &str = "creation,creation-with-upload,expiration,termination,checksum";
pub const CHECKSUM_ALGORITHM: &str = "sha256";
pub const UPLOAD_PATH: &str = "/api/v1/uploads/tus";

pub const HEADER_LENGTH: &str = "Upload-Length";
pub const HEADER_DEFER_LENGTH: &str = "Upload-Defer-Length";
pub const HEADER_METADATA: &str = "Upload-Metadata";
pub const HEADER_OVERRIDE: &str = "X-HTTP-Method-Override";

const IMF_FIXDATE: &[BorrowedFormatItem<'_>] = format_description!(
    "[weekday repr:short], [day padding:zero] [month repr:short] [year] [hour padding:zero]:[minute padding:zero]:[second padding:zero] GMT"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredLength {
    Known(ByteSize),
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodOverride {
    Head,
    Delete,
    Patch,
}

pub fn require_version(headers: &HeaderMap) -> Result<(), TusError> {
    let mut values = headers.get_all(TUS_RESUMABLE).iter();
    match (values.next(), values.next()) {
        (Some(value), None) if value.as_bytes() == PROTOCOL_VERSION.as_bytes() => Ok(()),
        _ => Err(TusError::VersionUnsupported),
    }
}

pub fn reject_concatenation(headers: &HeaderMap) -> Result<(), TusError> {
    if headers.contains_key(UPLOAD_CONCAT) {
        Err(TusError::ExtensionUnsupported)
    } else {
        Ok(())
    }
}

pub fn method_override(headers: &HeaderMap) -> Result<Option<MethodOverride>, TusError> {
    let mut values = headers.get_all(METHOD_OVERRIDE).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let invalid = || TusError::Header {
        key: HEADER_OVERRIDE,
    };
    if values.next().is_some() {
        return Err(invalid());
    }
    match value.to_str().map_err(|_| invalid())? {
        "HEAD" => Ok(Some(MethodOverride::Head)),
        "DELETE" => Ok(Some(MethodOverride::Delete)),
        "PATCH" => Ok(Some(MethodOverride::Patch)),
        _ => Err(invalid()),
    }
}

pub fn reject_method_override(headers: &HeaderMap) -> Result<(), TusError> {
    if headers.contains_key(METHOD_OVERRIDE) {
        Err(TusError::Header {
            key: HEADER_OVERRIDE,
        })
    } else {
        Ok(())
    }
}

pub fn declared_length(headers: &HeaderMap) -> Result<DeclaredLength, TusError> {
    let length = single(headers, &UPLOAD_LENGTH, HEADER_LENGTH)?;
    let defer = single(headers, &UPLOAD_DEFER_LENGTH, HEADER_DEFER_LENGTH)?;
    match (length, defer) {
        (Some(_), Some(_)) => Err(TusError::Header {
            key: HEADER_DEFER_LENGTH,
        }),
        (None, None) => Err(TusError::Header { key: HEADER_LENGTH }),
        (None, Some(value)) => {
            if value.as_bytes() == b"1" {
                Ok(DeclaredLength::Deferred)
            } else {
                Err(TusError::Header {
                    key: HEADER_DEFER_LENGTH,
                })
            }
        }
        (Some(value), None) => {
            let digits = value
                .to_str()
                .ok()
                .filter(|text| is_canonical_integer(text))
                .ok_or(TusError::Header { key: HEADER_LENGTH })?;
            let bytes: u64 = digits
                .parse()
                .map_err(|_| TusError::Header { key: HEADER_LENGTH })?;
            ByteSize::try_from(bytes)
                .ok()
                .filter(|size| size.to_i64() <= MAX_WIRE_BYTES)
                .map(DeclaredLength::Known)
                .ok_or(TusError::LengthBeyondRange)
        }
    }
}

fn single<'h>(
    headers: &'h HeaderMap,
    name: &HeaderName,
    key: &'static str,
) -> Result<Option<&'h HeaderValue>, TusError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(TusError::Header { key });
    }
    Ok(first)
}

fn is_canonical_integer(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text.len() == 1 || !text.starts_with('0'))
}

pub fn imf_fixdate(at: Timestamp) -> String {
    at.get().format(IMF_FIXDATE).unwrap_or_default()
}

pub fn upload_location(base: &Url, upload: &str) -> String {
    format!(
        "{}{UPLOAD_PATH}/{upload}",
        base.as_str().trim_end_matches('/')
    )
}

pub fn no_store() -> (HeaderName, HeaderValue) {
    (CACHE_CONTROL, HeaderValue::from_static("no-store"))
}

pub fn protocol_header() -> (HeaderName, HeaderValue) {
    (TUS_RESUMABLE, HeaderValue::from_static(PROTOCOL_VERSION))
}

pub fn number_header(value: u64) -> HeaderValue {
    HeaderValue::from(value)
}

pub fn text_header(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use time::macros::datetime;

    use super::{
        declared_length, imf_fixdate, method_override, reject_concatenation, require_version,
        upload_location, DeclaredLength, MethodOverride, METHOD_OVERRIDE, TUS_RESUMABLE,
        UPLOAD_CONCAT, UPLOAD_DEFER_LENGTH, UPLOAD_LENGTH,
    };
    use crate::domain::bytes::ByteSize;
    use crate::domain::time::Timestamp;
    use crate::features::transfers::tus::error::TusError;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn size(bytes: u64) -> ByteSize {
        ByteSize::try_from(bytes).unwrap()
    }

    #[test]
    fn unit_version_must_be_exactly_one_zero_zero() {
        assert!(require_version(&headers(&[("tus-resumable", "1.0.0")])).is_ok());
        for bad in [
            headers(&[]),
            headers(&[("tus-resumable", "1.0.1")]),
            headers(&[("tus-resumable", "1.0")]),
            headers(&[("tus-resumable", "0.2.2")]),
            headers(&[("tus-resumable", "1.0.0"), ("tus-resumable", "1.0.0")]),
            headers(&[("tus-resumable", " 1.0.0")]),
        ] {
            assert!(matches!(
                require_version(&bad),
                Err(TusError::VersionUnsupported)
            ));
        }
        assert_eq!(TUS_RESUMABLE.as_str(), "tus-resumable");
    }

    #[test]
    fn unit_any_concat_header_is_unsupported() {
        for value in ["partial", "final;/a /b", ""] {
            let map = headers(&[("upload-concat", value)]);
            assert!(matches!(
                reject_concatenation(&map),
                Err(TusError::ExtensionUnsupported)
            ));
        }
        assert!(reject_concatenation(&HeaderMap::new()).is_ok());
        assert_eq!(UPLOAD_CONCAT.as_str(), "upload-concat");
    }

    #[test]
    fn unit_declared_length_accepts_only_canonical_integers_or_defer_one() {
        let known = |value: &str| declared_length(&headers(&[("upload-length", value)]));
        assert_eq!(known("0").unwrap(), DeclaredLength::Known(size(0)));
        assert_eq!(known("1").unwrap(), DeclaredLength::Known(size(1)));
        assert_eq!(
            known("9007199254740991").unwrap(),
            DeclaredLength::Known(size(9_007_199_254_740_991))
        );
        for bad in [
            "", "-1", "+1", "1.5", "1e3", "01", "00", " 1", "1 ", "0x10", "abc",
        ] {
            assert!(
                matches!(
                    known(bad),
                    Err(TusError::Header {
                        key: "Upload-Length"
                    })
                ),
                "{bad:?}"
            );
        }
        for beyond in [
            "9007199254740992",
            "9223372036854775807",
            "18446744073709551615",
        ] {
            assert!(
                matches!(known(beyond), Err(TusError::LengthBeyondRange)),
                "{beyond}"
            );
        }
        assert!(matches!(
            known("18446744073709551616"),
            Err(TusError::Header {
                key: "Upload-Length"
            })
        ));
        assert!(matches!(
            known("99999999999999999999999999999999"),
            Err(TusError::Header {
                key: "Upload-Length"
            })
        ));

        let defer = |value: &str| declared_length(&headers(&[("upload-defer-length", value)]));
        assert_eq!(defer("1").unwrap(), DeclaredLength::Deferred);
        for bad in ["0", "2", "true", "", "01", "-1"] {
            assert!(
                matches!(
                    defer(bad),
                    Err(TusError::Header {
                        key: "Upload-Defer-Length"
                    })
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            declared_length(&HeaderMap::new()),
            Err(TusError::Header {
                key: "Upload-Length"
            })
        ));
        assert!(matches!(
            declared_length(&headers(&[
                ("upload-length", "1"),
                ("upload-defer-length", "1")
            ])),
            Err(TusError::Header {
                key: "Upload-Defer-Length"
            })
        ));
        assert!(matches!(
            declared_length(&headers(&[("upload-length", "1"), ("upload-length", "1")])),
            Err(TusError::Header {
                key: "Upload-Length"
            })
        ));
        assert_eq!(UPLOAD_LENGTH.as_str(), "upload-length");
        assert_eq!(UPLOAD_DEFER_LENGTH.as_str(), "upload-defer-length");
    }

    #[test]
    fn unit_method_override_permits_only_head_delete_and_patch() {
        let one = |value: &str| method_override(&headers(&[("x-http-method-override", value)]));
        assert_eq!(one("HEAD").unwrap(), Some(MethodOverride::Head));
        assert_eq!(one("DELETE").unwrap(), Some(MethodOverride::Delete));
        assert_eq!(one("PATCH").unwrap(), Some(MethodOverride::Patch));
        assert_eq!(method_override(&HeaderMap::new()).unwrap(), None);
        for bad in [
            "GET", "POST", "PUT", "OPTIONS", "head", "delete", "", "DELETE ",
        ] {
            assert!(
                matches!(
                    one(bad),
                    Err(TusError::Header {
                        key: "X-HTTP-Method-Override"
                    })
                ),
                "{bad:?}"
            );
        }
        let twice = headers(&[
            ("x-http-method-override", "HEAD"),
            ("x-http-method-override", "DELETE"),
        ]);
        assert!(method_override(&twice).is_err());
        assert_eq!(METHOD_OVERRIDE.as_str(), "x-http-method-override");
    }

    #[test]
    fn unit_upload_expires_is_an_imf_fixdate_in_utc() {
        let at = |text: &str| text.parse::<Timestamp>().unwrap();
        assert_eq!(
            imf_fixdate(at("2026-10-09T20:00:00.000Z")),
            "Fri, 09 Oct 2026 20:00:00 GMT"
        );
        assert_eq!(
            imf_fixdate(at("2026-01-01T00:00:00.000Z")),
            "Thu, 01 Jan 2026 00:00:00 GMT"
        );
        assert_eq!(
            imf_fixdate(at("2026-02-28T23:59:59.999Z")),
            "Sat, 28 Feb 2026 23:59:59 GMT"
        );
        assert_eq!(
            imf_fixdate(at("2028-02-29T07:05:09.000Z")),
            "Tue, 29 Feb 2028 07:05:09 GMT"
        );
        let shifted = Timestamp::try_from(datetime!(2026-10-09 23:30 -03:00)).unwrap();
        assert_eq!(imf_fixdate(shifted), "Sat, 10 Oct 2026 02:30:00 GMT");
        assert!(imf_fixdate(at("2026-10-09T20:00:00.000Z")).ends_with(" GMT"));
    }

    #[test]
    fn unit_location_uses_only_the_configured_base() {
        let base = |text: &str| url::Url::parse(text).unwrap();
        assert_eq!(
            upload_location(&base("http://localhost:5487"), "abc"),
            "http://localhost:5487/api/v1/uploads/tus/abc"
        );
        assert_eq!(
            upload_location(&base("https://files.example.test/"), "abc"),
            "https://files.example.test/api/v1/uploads/tus/abc"
        );
        assert_eq!(
            upload_location(&base("https://example.test/palmr/"), "abc"),
            "https://example.test/palmr/api/v1/uploads/tus/abc"
        );
        assert_eq!(
            upload_location(&base("https://example.test/palmr"), "abc"),
            "https://example.test/palmr/api/v1/uploads/tus/abc"
        );
    }
}
