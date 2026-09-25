use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConstantTimeEq};

use super::hkdf::{KeyRing, MacPurpose};
use super::CryptoError;

pub const DIGEST_HEX_LEN: usize = 64;
pub const MAC_LEN: usize = 32;
pub const MIN_TRUNCATED_MAC_LEN: usize = 16;

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone)]
pub struct TokenDigest(String);

impl fmt::Debug for TokenDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenDigest(<redacted>)")
    }
}

impl TokenDigest {
    pub fn parse(stored: &str) -> Result<Self, CryptoError> {
        let well_formed = stored.len() == DIGEST_HEX_LEN
            && stored
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if well_formed {
            Ok(Self(stored.to_owned()))
        } else {
            Err(CryptoError::MalformedDigest)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn verify(&self, stored: &Self) -> bool {
        self.ct_eq(stored).into()
    }
}

impl ConstantTimeEq for TokenDigest {
    fn ct_eq(&self, other: &Self) -> Choice {
        self.0.as_bytes().ct_eq(other.0.as_bytes())
    }
}

pub fn sha256_hex(bytes: &[u8]) -> TokenDigest {
    TokenDigest(lower_hex(&Sha256::digest(bytes)))
}

pub fn mac_hex(ring: &KeyRing, purpose: MacPurpose, message: &[u8]) -> TokenDigest {
    TokenDigest(lower_hex(&ring.mac(purpose, message)))
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        hex.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    hex
}

pub fn verify_sha256(raw: &[u8], stored: &TokenDigest) -> bool {
    sha256_hex(raw).verify(stored)
}

impl KeyRing {
    pub fn mac(&self, purpose: MacPurpose, message: &[u8]) -> [u8; MAC_LEN] {
        let mut mac = self.hmac(purpose);
        mac.update(message);
        mac.finalize().into_bytes().into()
    }

    pub fn verify_mac(&self, purpose: MacPurpose, message: &[u8], tag: &[u8]) -> bool {
        let mut mac = self.hmac(purpose);
        mac.update(message);
        mac.verify_slice(tag).is_ok()
    }

    pub fn verify_truncated_mac(&self, purpose: MacPurpose, message: &[u8], tag: &[u8]) -> bool {
        if !(MIN_TRUNCATED_MAC_LEN..=MAC_LEN).contains(&tag.len()) {
            return false;
        }
        let mut mac = self.hmac(purpose);
        mac.update(message);
        mac.verify_truncated_left(tag).is_ok()
    }

    fn hmac(&self, purpose: MacPurpose) -> Hmac<Sha256> {
        match Hmac::<Sha256>::new_from_slice(self.mac_key(purpose).expose_secret()) {
            Ok(mac) => mac,
            Err(_) => unreachable!("HMAC accepts keys of any length"),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{
        sha256_hex, verify_sha256, TokenDigest, DIGEST_HEX_LEN, MAC_LEN, MIN_TRUNCATED_MAC_LEN,
    };
    use crate::infra::crypto::hkdf::{KeyRing, MacPurpose};
    use crate::infra::crypto::instance_key::INSTANCE_KEY_LEN;
    use crate::infra::crypto::token::Token;
    use crate::infra::crypto::CryptoError;

    const MESSAGE: &[u8] = b"sort=name&id=42";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn flip_hex(digest: &str, index: usize) -> String {
        let mut bytes = digest.as_bytes().to_vec();
        bytes[index] = if bytes[index] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn unit_hash_sha256_lowercase_hex() {
        let digest = sha256_hex(b"abc");

        assert_eq!(
            digest.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        for _ in 0..16 {
            let token = Token::mint().unwrap();
            let digest = token.digest();
            assert_eq!(digest.as_str().len(), DIGEST_HEX_LEN);
            assert!(digest
                .as_str()
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
            assert_eq!(
                TokenDigest::parse(digest.as_str()).unwrap().as_str(),
                digest.as_str()
            );
        }
    }

    #[test]
    fn unit_hash_constant_time_verify() {
        let token = Token::mint().unwrap();
        let stored = TokenDigest::parse(token.digest().as_str()).unwrap();

        assert!(token.digest().verify(&stored));
        assert!(verify_sha256(token.expose_secret(), &stored));

        let other = Token::mint().unwrap();
        assert!(!other.digest().verify(&stored));
        assert!(!verify_sha256(other.expose_secret(), &stored));
        assert!(!verify_sha256(
            token.encode().expose_secret().as_bytes(),
            &stored
        ));

        for index in [0, DIGEST_HEX_LEN / 2, DIGEST_HEX_LEN - 1] {
            let near = TokenDigest::parse(&flip_hex(stored.as_str(), index)).unwrap();
            assert!(!token.digest().verify(&near), "differs at {index}");
            assert!(!near.verify(&stored));
        }
    }

    #[rstest]
    #[case::empty("")]
    #[case::short("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a")]
    #[case::long("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad0")]
    #[case::uppercase("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD")]
    #[case::non_hex("ga7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")]
    #[case::whitespace(" a7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")]
    fn unit_hash_parse_rejects_malformed(#[case] stored: &str) {
        assert_eq!(
            TokenDigest::parse(stored).unwrap_err(),
            CryptoError::MalformedDigest
        );
    }

    #[test]
    fn unit_hmac_matches_known_answer() {
        let root: [u8; INSTANCE_KEY_LEN] = std::array::from_fn(|index| index as u8);
        let ring = KeyRing::from_root(&root);

        assert_eq!(
            hex(&ring.mac(MacPurpose::Cursor, MESSAGE)),
            "43690af07492ed945cce269b5cab0441800e459ed679d0f8870de65beff02219"
        );
    }

    #[test]
    fn unit_hmac_verify_binds_key_purpose_and_message() {
        let ring = KeyRing::from_root(&[0x11; INSTANCE_KEY_LEN]);
        let other_instance = KeyRing::from_root(&[0x22; INSTANCE_KEY_LEN]);
        let tag = ring.mac(MacPurpose::ArchiveTicket, MESSAGE);

        assert_eq!(tag.len(), MAC_LEN);
        assert_eq!(tag, ring.mac(MacPurpose::ArchiveTicket, MESSAGE));
        assert!(ring.verify_mac(MacPurpose::ArchiveTicket, MESSAGE, &tag));

        for purpose in MacPurpose::ALL {
            if purpose != MacPurpose::ArchiveTicket {
                assert_ne!(ring.mac(purpose, MESSAGE), tag);
                assert!(!ring.verify_mac(purpose, MESSAGE, &tag), "{purpose:?}");
            }
        }
        assert!(!other_instance.verify_mac(MacPurpose::ArchiveTicket, MESSAGE, &tag));
        assert!(!ring.verify_mac(MacPurpose::ArchiveTicket, b"sort=name&id=43", &tag));

        let mut altered = tag;
        altered[MAC_LEN - 1] ^= 0x01;
        assert!(!ring.verify_mac(MacPurpose::ArchiveTicket, MESSAGE, &altered));
        assert!(!ring.verify_mac(MacPurpose::ArchiveTicket, MESSAGE, &tag[..16]));
        assert!(!ring.verify_mac(MacPurpose::ArchiveTicket, MESSAGE, &[]));
        assert!(!ring.verify_mac(
            MacPurpose::ArchiveTicket,
            MESSAGE,
            &[tag.as_slice(), &[0]].concat()
        ));
    }

    #[test]
    fn unit_hmac_truncated_verify_requires_minimum_prefix() {
        let ring = KeyRing::from_root(&[0x33; INSTANCE_KEY_LEN]);
        let tag = ring.mac(MacPurpose::Cursor, MESSAGE);
        let prefix = &tag[..MIN_TRUNCATED_MAC_LEN];

        assert!(ring.verify_truncated_mac(MacPurpose::Cursor, MESSAGE, prefix));
        assert!(ring.verify_truncated_mac(MacPurpose::Cursor, MESSAGE, &tag));
        assert!(!ring.verify_truncated_mac(MacPurpose::ArchiveTicket, MESSAGE, prefix));
        assert!(!ring.verify_truncated_mac(MacPurpose::Cursor, b"sort=name&id=43", prefix));
        assert!(!ring.verify_truncated_mac(
            MacPurpose::Cursor,
            MESSAGE,
            &tag[..MIN_TRUNCATED_MAC_LEN - 1]
        ));
        assert!(!ring.verify_truncated_mac(MacPurpose::Cursor, MESSAGE, &[]));
        assert!(!ring.verify_truncated_mac(
            MacPurpose::Cursor,
            MESSAGE,
            &tag[1..=MIN_TRUNCATED_MAC_LEN]
        ));

        let mut altered = [0_u8; MIN_TRUNCATED_MAC_LEN];
        altered.copy_from_slice(prefix);
        altered[0] ^= 0x80;
        assert!(!ring.verify_truncated_mac(MacPurpose::Cursor, MESSAGE, &altered));
    }
}
