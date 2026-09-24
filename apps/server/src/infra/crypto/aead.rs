use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use super::hkdf::{KeyRing, SealPurpose};
use super::{fill_random, CryptoError};
use crate::domain::secret::Secret;

pub const KEY_VERSION: i64 = 1;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

pub struct SealedSecret {
    ciphertext: Vec<u8>,
    nonce: [u8; NONCE_LEN],
    key_version: i64,
}

impl SealedSecret {
    pub fn from_parts(
        ciphertext: Vec<u8>,
        nonce: &[u8],
        key_version: i64,
    ) -> Result<Self, CryptoError> {
        if key_version != KEY_VERSION {
            return Err(CryptoError::UnsupportedKeyVersion);
        }
        let nonce =
            <[u8; NONCE_LEN]>::try_from(nonce).map_err(|_| CryptoError::MalformedEnvelope)?;
        if ciphertext.len() < TAG_LEN {
            return Err(CryptoError::MalformedEnvelope);
        }
        Ok(Self {
            ciphertext,
            nonce,
            key_version,
        })
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub const fn nonce(&self) -> &[u8; NONCE_LEN] {
        &self.nonce
    }

    pub const fn key_version(&self) -> i64 {
        self.key_version
    }
}

impl fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SealedSecret")
            .field("key_version", &self.key_version)
            .field("ciphertext_len", &self.ciphertext.len())
            .finish_non_exhaustive()
    }
}

impl KeyRing {
    pub fn seal(
        &self,
        purpose: SealPurpose,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<SealedSecret, CryptoError> {
        if aad.is_empty() {
            return Err(CryptoError::MissingAad);
        }
        let mut nonce = [0_u8; NONCE_LEN];
        fill_random(&mut nonce)?;
        let ciphertext = self
            .cipher(purpose)
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::MalformedEnvelope)?;
        Ok(SealedSecret {
            ciphertext,
            nonce,
            key_version: KEY_VERSION,
        })
    }

    pub fn open(
        &self,
        purpose: SealPurpose,
        aad: &[u8],
        sealed: &SealedSecret,
    ) -> Result<Secret<Vec<u8>>, CryptoError> {
        if aad.is_empty() {
            return Err(CryptoError::MissingAad);
        }
        self.cipher(purpose)
            .decrypt(
                XNonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad,
                },
            )
            .map(Secret::new)
            .map_err(|_| CryptoError::AuthenticationFailed)
    }

    fn cipher(&self, purpose: SealPurpose) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(self.seal_key(purpose).expose_secret()))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{SealedSecret, KEY_VERSION, NONCE_LEN, TAG_LEN};
    use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
    use crate::infra::crypto::instance_key::INSTANCE_KEY_LEN;
    use crate::infra::crypto::CryptoError;

    const ROOT: [u8; INSTANCE_KEY_LEN] = *b"aead-root-sentinel-key-bytes-042";
    const PLAINTEXT: &[u8] = b"JBSWY3DPEHPK3PXP-totp-seed-sentinel";

    fn row_aad(purpose: &str, row_id: &str) -> Vec<u8> {
        [purpose.as_bytes(), &[0], row_id.as_bytes()].concat()
    }

    fn ring() -> KeyRing {
        KeyRing::from_root(&ROOT)
    }

    fn reassemble(sealed: &SealedSecret) -> SealedSecret {
        SealedSecret::from_parts(
            sealed.ciphertext().to_vec(),
            sealed.nonce(),
            sealed.key_version(),
        )
        .unwrap()
    }

    #[test]
    fn unit_aead_round_trip() {
        let ring = ring();
        let aad = row_aad("totp", "0192f0c4-7b1e-7c3a-9d1e-000000000001");

        for purpose in SealPurpose::ALL {
            for plaintext in [PLAINTEXT, b"".as_slice()] {
                let sealed = ring.seal(purpose, &aad, plaintext).unwrap();

                assert_eq!(sealed.key_version(), KEY_VERSION);
                assert_eq!(sealed.key_version(), 1);
                assert_eq!(sealed.nonce().len(), NONCE_LEN);
                assert_eq!(sealed.nonce().len(), 24);
                assert_eq!(sealed.ciphertext().len(), plaintext.len() + TAG_LEN);
                let opened = ring.open(purpose, &aad, &reassemble(&sealed)).unwrap();
                assert_eq!(opened.expose_secret(), plaintext);
            }
        }
    }

    #[test]
    fn unit_aead_fresh_nonce_per_seal() {
        let ring = ring();
        let aad = row_aad("smtp", "settings");

        let seals: Vec<SealedSecret> = (0..32)
            .map(|_| ring.seal(SealPurpose::Smtp, &aad, PLAINTEXT).unwrap())
            .collect();

        for (index, sealed) in seals.iter().enumerate() {
            assert_ne!(sealed.nonce(), &[0_u8; NONCE_LEN]);
            for later in &seals[index + 1..] {
                assert_ne!(sealed.nonce(), later.nonce());
                assert_ne!(sealed.ciphertext(), later.ciphertext());
            }
        }
    }

    #[test]
    fn unit_aead_aad_binding() {
        let ring = ring();
        let user_a = row_aad("totp", "0192f0c4-7b1e-7c3a-9d1e-00000000000a");
        let user_b = row_aad("totp", "0192f0c4-7b1e-7c3a-9d1e-00000000000b");
        let sealed = ring.seal(SealPurpose::Totp, &user_a, PLAINTEXT).unwrap();

        assert_eq!(
            ring.open(SealPurpose::Totp, &user_a, &sealed)
                .unwrap()
                .expose_secret(),
            PLAINTEXT
        );
        let moved = reassemble(&sealed);
        assert_eq!(
            ring.open(SealPurpose::Totp, &user_b, &moved).unwrap_err(),
            CryptoError::AuthenticationFailed
        );
        for wrong in [
            row_aad("idp", "0192f0c4-7b1e-7c3a-9d1e-00000000000a"),
            user_a[..user_a.len() - 1].to_vec(),
            [user_a.as_slice(), b"x"].concat(),
        ] {
            assert_eq!(
                ring.open(SealPurpose::Totp, &wrong, &sealed).unwrap_err(),
                CryptoError::AuthenticationFailed
            );
        }
    }

    #[test]
    fn unit_aead_requires_aad() {
        let ring = ring();
        let sealed = ring.seal(SealPurpose::Idp, b"idp\0row", PLAINTEXT).unwrap();

        assert_eq!(
            ring.seal(SealPurpose::Idp, b"", PLAINTEXT).unwrap_err(),
            CryptoError::MissingAad
        );
        assert_eq!(
            ring.open(SealPurpose::Idp, b"", &sealed).unwrap_err(),
            CryptoError::MissingAad
        );
    }

    #[test]
    fn unit_aead_wrong_key_fails() {
        let ring = ring();
        let other_instance = KeyRing::from_root(&[0x5a; INSTANCE_KEY_LEN]);
        let aad = row_aad("idp", "0192f0c4-7b1e-7c3a-9d1e-000000000001");
        let sealed = ring.seal(SealPurpose::Idp, &aad, PLAINTEXT).unwrap();

        assert_eq!(
            other_instance
                .open(SealPurpose::Idp, &aad, &sealed)
                .unwrap_err(),
            CryptoError::AuthenticationFailed
        );
        for purpose in SealPurpose::ALL {
            if purpose != SealPurpose::Idp {
                assert_eq!(
                    ring.open(purpose, &aad, &sealed).unwrap_err(),
                    CryptoError::AuthenticationFailed,
                    "{purpose:?}"
                );
            }
        }
    }

    #[test]
    fn unit_aead_tampering_rejected() {
        let ring = ring();
        let aad = row_aad("oidc", "request");
        let sealed = ring.seal(SealPurpose::Oidc, &aad, PLAINTEXT).unwrap();

        for index in 0..sealed.ciphertext().len() {
            let mut ciphertext = sealed.ciphertext().to_vec();
            ciphertext[index] ^= 0x01;
            let tampered = SealedSecret::from_parts(ciphertext, sealed.nonce(), 1).unwrap();
            assert_eq!(
                ring.open(SealPurpose::Oidc, &aad, &tampered).unwrap_err(),
                CryptoError::AuthenticationFailed
            );
        }
        let mut nonce = *sealed.nonce();
        nonce[0] ^= 0x01;
        let renonced = SealedSecret::from_parts(sealed.ciphertext().to_vec(), &nonce, 1).unwrap();
        assert_eq!(
            ring.open(SealPurpose::Oidc, &aad, &renonced).unwrap_err(),
            CryptoError::AuthenticationFailed
        );
        let truncated =
            SealedSecret::from_parts(sealed.ciphertext()[1..].to_vec(), sealed.nonce(), 1).unwrap();
        assert_eq!(
            ring.open(SealPurpose::Oidc, &aad, &truncated).unwrap_err(),
            CryptoError::AuthenticationFailed
        );
    }

    #[rstest]
    #[case::empty_nonce(0, TAG_LEN, 1, CryptoError::MalformedEnvelope)]
    #[case::short_nonce(23, TAG_LEN, 1, CryptoError::MalformedEnvelope)]
    #[case::long_nonce(25, TAG_LEN, 1, CryptoError::MalformedEnvelope)]
    #[case::gcm_nonce(12, TAG_LEN, 1, CryptoError::MalformedEnvelope)]
    #[case::missing_tag(NONCE_LEN, TAG_LEN - 1, 1, CryptoError::MalformedEnvelope)]
    #[case::empty_ciphertext(NONCE_LEN, 0, 1, CryptoError::MalformedEnvelope)]
    #[case::version_zero(NONCE_LEN, TAG_LEN, 0, CryptoError::UnsupportedKeyVersion)]
    #[case::future_version(NONCE_LEN, TAG_LEN, 2, CryptoError::UnsupportedKeyVersion)]
    #[case::negative_version(NONCE_LEN, TAG_LEN, -1, CryptoError::UnsupportedKeyVersion)]
    fn unit_aead_malformed_parts_rejected(
        #[case] nonce_len: usize,
        #[case] ciphertext_len: usize,
        #[case] key_version: i64,
        #[case] expected: CryptoError,
    ) {
        let error =
            SealedSecret::from_parts(vec![0; ciphertext_len], &vec![0; nonce_len], key_version)
                .unwrap_err();

        assert_eq!(error, expected);
    }

    #[test]
    fn unit_aead_formatting_hides_contents() {
        let ring = ring();
        let sealed = ring
            .seal(SealPurpose::InviteToken, b"invite\0row", PLAINTEXT)
            .unwrap();
        let opened = ring
            .open(SealPurpose::InviteToken, b"invite\0row", &sealed)
            .unwrap();
        let plaintext = String::from_utf8(PLAINTEXT.to_vec()).unwrap();
        let ciphertext_bytes = format!("{:?}", &sealed.ciphertext()[..4]);
        let nonce_bytes = format!("{:?}", &sealed.nonce()[..4]);

        for text in [
            format!("{sealed:?}"),
            format!("{sealed:#?}"),
            format!("{opened:?}"),
            CryptoError::AuthenticationFailed.to_string(),
        ] {
            assert!(!text.contains(&plaintext), "{text}");
            assert!(!text.contains(&ciphertext_bytes[1..ciphertext_bytes.len() - 1]));
            assert!(!text.contains(&nonce_bytes[1..nonce_bytes.len() - 1]));
        }
    }
}
