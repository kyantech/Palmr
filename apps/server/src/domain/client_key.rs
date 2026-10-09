use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientFileKey(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidClientFileKey;

impl ClientFileKey {
    pub const MAX_CHARS: usize = 128;

    pub fn parse(input: &str) -> Result<Self, InvalidClientFileKey> {
        let length = input.chars().count();
        let valid = (1..=Self::MAX_CHARS).contains(&length) && !input.chars().any(char::is_control);
        if valid {
            Ok(Self(input.to_owned()))
        } else {
            Err(InvalidClientFileKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientFileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for InvalidClientFileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "client file key must be 1-{} characters with no control character",
            ClientFileKey::MAX_CHARS
        )
    }
}

impl std::error::Error for InvalidClientFileKey {}

#[cfg(test)]
mod tests {
    use super::ClientFileKey;

    #[test]
    fn unit_client_file_key_is_opaque_but_bounded_and_control_free() {
        let wide = "\u{e9}".repeat(128);
        let emoji = "\u{1F600}".repeat(128);
        for valid in [
            "c1",
            "a".repeat(128).as_str(),
            "sha256:0f-9_A.b",
            "has space",
            "slash/key",
            "back\\slash",
            "name|1048576|1790000000000",
            "e2UmP+/9a==",
            "<script>\"quoted\"</script>",
            wide.as_str(),
            emoji.as_str(),
            " leading and trailing ",
        ] {
            let key = ClientFileKey::parse(valid).unwrap();
            assert_eq!(key.as_str(), valid);
            assert_eq!(key.to_string(), valid);
        }
        let too_many = "\u{e9}".repeat(129);
        for invalid in [
            "",
            "a".repeat(129).as_str(),
            too_many.as_str(),
            "nul\0",
            "tab\t",
            "line\nbreak",
            "bell\u{7}",
            "del\u{7f}",
            "c1\u{85}",
        ] {
            assert!(ClientFileKey::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn unit_client_file_key_serializes_safely_into_error_details() {
        use crate::domain::error_code::ErrorCode;
        use crate::infra::http::error::{ApiError, ApiErrorBody};

        let key = ClientFileKey::parse("<b>\"x\"</b>\u{2028}\\").unwrap();
        let body = ApiErrorBody::from(
            ApiError::new(ErrorCode::FileTooLarge).with_detail("itemClientId", &key),
        );
        let text = serde_json::to_string(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["error"]["details"]["itemClientId"], key.as_str());
    }
}
