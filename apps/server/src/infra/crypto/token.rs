use std::fmt;

use base64ct::{Base64UrlUnpadded, Encoding};
use zeroize::Zeroize;

use super::hash::{sha256_hex, TokenDigest};
use super::{fill_random, CryptoError};
use crate::domain::secret::{Secret, REDACTED};

pub const TOKEN_LEN: usize = 32;
pub const ENCODED_TOKEN_LEN: usize = 43;

pub struct Token([u8; TOKEN_LEN]);

impl Token {
    pub fn mint() -> Result<Self, CryptoError> {
        let mut bytes = [0_u8; TOKEN_LEN];
        fill_random(&mut bytes)?;
        Ok(Self(bytes))
    }

    pub fn decode(encoded: &str) -> Result<Self, CryptoError> {
        if encoded.len() != ENCODED_TOKEN_LEN {
            return Err(CryptoError::MalformedToken);
        }
        let mut bytes = [0_u8; TOKEN_LEN];
        match Base64UrlUnpadded::decode(encoded, &mut bytes) {
            Ok(decoded) if decoded.len() == TOKEN_LEN => Ok(Self(bytes)),
            _ => {
                bytes.zeroize();
                Err(CryptoError::MalformedToken)
            }
        }
    }

    pub fn encode(&self) -> Secret<String> {
        Secret::new(Base64UrlUnpadded::encode_string(&self.0))
    }

    pub fn digest(&self) -> TokenDigest {
        sha256_hex(&self.0)
    }

    pub const fn expose_secret(&self) -> &[u8; TOKEN_LEN] {
        &self.0
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token({REDACTED})")
    }
}

#[cfg(test)]
mod tests {
    use base64ct::{Base64UrlUnpadded, Encoding};
    use rstest::rstest;

    use super::{Token, ENCODED_TOKEN_LEN, TOKEN_LEN};
    use crate::infra::crypto::hash::sha256_hex;
    use crate::infra::crypto::CryptoError;

    #[test]
    fn unit_token_entropy_and_encoding() {
        let token = Token::mint().unwrap();
        let encoded = token.encode();
        let text = encoded.expose_secret();

        assert_eq!(token.expose_secret().len(), TOKEN_LEN);
        assert_eq!(TOKEN_LEN * 8, 256);
        assert_eq!(text.len(), ENCODED_TOKEN_LEN);
        assert!(text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
        assert!(!text.contains('='));

        let decoded = Base64UrlUnpadded::decode_vec(text).unwrap();
        assert_eq!(decoded.len(), TOKEN_LEN);
        assert_eq!(decoded, token.expose_secret());
        assert_eq!(
            Token::decode(text).unwrap().expose_secret(),
            token.expose_secret()
        );
        assert_eq!(
            token.digest().as_str(),
            sha256_hex(token.expose_secret()).as_str()
        );
    }

    #[test]
    fn unit_token_mints_differ() {
        let tokens: Vec<[u8; TOKEN_LEN]> = (0..64)
            .map(|_| *Token::mint().unwrap().expose_secret())
            .collect();

        for (index, token) in tokens.iter().enumerate() {
            assert_ne!(token, &[0_u8; TOKEN_LEN]);
            assert!(!tokens[index + 1..].contains(token));
        }
    }

    #[test]
    fn unit_token_encoding_round_trips_every_byte() {
        let bytes: [u8; TOKEN_LEN] = std::array::from_fn(|index| (index * 8 + 3) as u8);
        let known = Token(bytes);
        let encoded = known.encode();

        assert_eq!(
            encoded.expose_secret(),
            "AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s"
        );
        assert_eq!(
            Token::decode(encoded.expose_secret())
                .unwrap()
                .expose_secret(),
            &bytes
        );
    }

    #[rstest]
    #[case::empty("")]
    #[case::too_short("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_")]
    #[case::too_long("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_sA")]
    #[case::padded("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s=")]
    #[case::standard_alphabet("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2+Pr8/s")]
    #[case::non_canonical_tail("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_t")]
    #[case::whitespace("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8 s")]
    #[case::non_ascii("AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8é")]
    fn unit_token_decode_rejects_malformed(#[case] encoded: &str) {
        assert_eq!(
            Token::decode(encoded).unwrap_err(),
            CryptoError::MalformedToken
        );
    }

    #[test]
    fn unit_token_never_formatted() {
        let token = Token::mint().unwrap();
        let encoded = token.encode();

        for text in [
            format!("{token:?}"),
            format!("{token:#?}"),
            format!("{encoded:?}"),
            format!("{encoded}"),
        ] {
            assert!(!text.contains(encoded.expose_secret().as_str()), "{text}");
            assert!(text.contains("<redacted>"));
        }
    }
}
