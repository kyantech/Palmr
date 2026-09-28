use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};
use zeroize::Zeroize;

use super::hash::{sha256_hex, TokenDigest};
use super::{fill_random, CryptoError};
use crate::domain::secret::{Secret, REDACTED};

pub const TOTP_SECRET_LEN: usize = 20;
pub const TOTP_DIGITS: usize = 6;
pub const TOTP_PERIOD_SECONDS: i64 = 30;
pub const TOTP_SKEW_STEPS: u64 = 1;

pub const BACKUP_CODE_COUNT: usize = 10;
pub const BACKUP_CODE_BYTES: usize = 10;
pub const BACKUP_CODE_CHARS: usize = 16;
const BACKUP_CODE_GROUP: usize = 4;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const TOTP_MODULUS: u32 = 1_000_000;

pub struct TotpSecret([u8; TOTP_SECRET_LEN]);

impl TotpSecret {
    pub fn mint() -> Result<Self, CryptoError> {
        let mut bytes = [0_u8; TOTP_SECRET_LEN];
        fill_random(&mut bytes)?;
        Ok(Self(bytes))
    }

    pub fn from_opened(opened: &Secret<Vec<u8>>) -> Result<Self, CryptoError> {
        <[u8; TOTP_SECRET_LEN]>::try_from(opened.expose_secret().as_slice())
            .map(Self)
            .map_err(|_| CryptoError::MalformedTotpSecret)
    }

    pub const fn expose_secret(&self) -> &[u8; TOTP_SECRET_LEN] {
        &self.0
    }

    pub fn base32(&self) -> Secret<String> {
        Secret::new(base32(&self.0))
    }

    pub fn code_at(&self, step: u64) -> [u8; TOTP_DIGITS] {
        let mut mac = match Hmac::<Sha1>::new_from_slice(&self.0) {
            Ok(mac) => mac,
            Err(_) => unreachable!("HMAC accepts keys of any length"),
        };
        mac.update(&step.to_be_bytes());
        let digest = mac.finalize().into_bytes();
        let offset = usize::from(digest[digest.len() - 1] & 0x0f);
        let truncated = u32::from_be_bytes([
            digest[offset] & 0x7f,
            digest[offset + 1],
            digest[offset + 2],
            digest[offset + 3],
        ]) % TOTP_MODULUS;
        let mut code = [0_u8; TOTP_DIGITS];
        let mut rest = truncated;
        for digit in code.iter_mut().rev() {
            *digit = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
        code
    }

    pub fn matching_step(&self, code: &TotpCode, now_step: u64) -> Option<u64> {
        let mut matched = Choice::from(0);
        let mut step = 0_u64;
        let first = now_step.saturating_sub(TOTP_SKEW_STEPS);
        for candidate in first..=now_step.saturating_add(TOTP_SKEW_STEPS) {
            let equal = self.code_at(candidate).ct_eq(&code.0);
            step = u64::conditional_select(&step, &candidate, equal);
            matched |= equal;
        }
        bool::from(matched).then_some(step)
    }
}

impl Drop for TotpSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for TotpSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TotpSecret({REDACTED})")
    }
}

pub fn time_step(unix_seconds: i64) -> Option<u64> {
    u64::try_from(unix_seconds.div_euclid(TOTP_PERIOD_SECONDS)).ok()
}

pub struct TotpCode([u8; TOTP_DIGITS]);

impl TotpCode {
    pub fn parse(input: &str) -> Option<Self> {
        let digits = <[u8; TOTP_DIGITS]>::try_from(input.as_bytes()).ok()?;
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then_some(Self(digits))
    }
}

impl Drop for TotpCode {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for TotpCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TotpCode({REDACTED})")
    }
}

pub struct BackupCode {
    display: Secret<String>,
    digest: TokenDigest,
}

impl BackupCode {
    pub fn mint_set() -> Result<Vec<Self>, CryptoError> {
        let mut codes: Vec<Self> = Vec::with_capacity(BACKUP_CODE_COUNT);
        while codes.len() < BACKUP_CODE_COUNT {
            let code = Self::mint()?;
            if codes
                .iter()
                .all(|minted| !minted.digest.verify(&code.digest))
            {
                codes.push(code);
            }
        }
        Ok(codes)
    }

    fn mint() -> Result<Self, CryptoError> {
        let mut bytes = [0_u8; BACKUP_CODE_BYTES];
        fill_random(&mut bytes)?;
        let mut normalized = base32(&bytes);
        bytes.zeroize();
        let digest = sha256_hex(normalized.as_bytes());
        let mut display = String::with_capacity(BACKUP_CODE_CHARS + 3);
        for (index, group) in normalized.as_bytes().chunks(BACKUP_CODE_GROUP).enumerate() {
            if index > 0 {
                display.push('-');
            }
            display.extend(group.iter().copied().map(char::from));
        }
        normalized.zeroize();
        Ok(Self {
            display: Secret::new(display),
            digest,
        })
    }

    pub const fn display(&self) -> &Secret<String> {
        &self.display
    }

    pub const fn digest(&self) -> &TokenDigest {
        &self.digest
    }
}

impl fmt::Debug for BackupCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BackupCode({REDACTED})")
    }
}

pub fn backup_code_digest(input: &str) -> Option<TokenDigest> {
    let mut normalized: String = input
        .chars()
        .filter(|character| *character != '-')
        .map(|character| character.to_ascii_uppercase())
        .collect();
    let well_formed = normalized.len() == BACKUP_CODE_CHARS
        && normalized.bytes().all(|byte| BASE32.contains(&byte));
    let digest = well_formed.then(|| sha256_hex(normalized.as_bytes()));
    normalized.zeroize();
    digest
}

fn base32(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(char::from(BASE32[((buffer >> bits) & 0x1f) as usize]));
        }
    }
    if bits > 0 {
        encoded.push(char::from(BASE32[((buffer << (5 - bits)) & 0x1f) as usize]));
    }
    buffer.zeroize();
    encoded
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rstest::rstest;

    use super::{
        backup_code_digest, base32, time_step, BackupCode, TotpCode, TotpSecret, BACKUP_CODE_CHARS,
        BACKUP_CODE_COUNT, TOTP_SECRET_LEN,
    };
    use crate::domain::secret::Secret;
    use crate::infra::crypto::hash::sha256_hex;
    use crate::infra::crypto::CryptoError;

    const RFC_SECRET: &[u8; TOTP_SECRET_LEN] = b"12345678901234567890";

    fn rfc_secret() -> TotpSecret {
        TotpSecret::from_opened(&Secret::new(RFC_SECRET.to_vec())).unwrap()
    }

    fn code(text: &str) -> TotpCode {
        TotpCode::parse(text).unwrap()
    }

    #[rstest]
    #[case(0, "755224")]
    #[case(1, "287082")]
    #[case(2, "359152")]
    #[case(3, "969429")]
    #[case(4, "338314")]
    #[case(5, "254676")]
    #[case(6, "287922")]
    #[case(7, "162583")]
    #[case(8, "399871")]
    #[case(9, "520489")]
    fn unit_totp_hotp_matches_rfc4226_vectors(#[case] counter: u64, #[case] expected: &str) {
        assert_eq!(rfc_secret().code_at(counter), expected.as_bytes());
    }

    #[rstest]
    #[case(59, "287082")]
    #[case(1_111_111_109, "081804")]
    #[case(1_111_111_111, "050471")]
    #[case(1_234_567_890, "005924")]
    #[case(2_000_000_000, "279037")]
    #[case(20_000_000_000, "353130")]
    fn unit_totp_matches_rfc6238_sha1_vectors(#[case] unix: i64, #[case] expected: &str) {
        let step = time_step(unix).unwrap();
        assert_eq!(rfc_secret().code_at(step), expected.as_bytes());
        assert_eq!(
            rfc_secret().matching_step(&code(expected), step),
            Some(step)
        );
    }

    #[test]
    fn unit_totp_window_is_one_step_each_side() {
        let secret = rfc_secret();
        let now = time_step(1_234_567_890).unwrap();
        for offset in [-1_i64, 0, 1] {
            let step = now.checked_add_signed(offset).unwrap();
            let text = String::from_utf8(secret.code_at(step).to_vec()).unwrap();
            assert_eq!(secret.matching_step(&code(&text), now), Some(step));
        }
        for offset in [-3_i64, -2, 2, 3] {
            let step = now.checked_add_signed(offset).unwrap();
            let text = String::from_utf8(secret.code_at(step).to_vec()).unwrap();
            assert_eq!(secret.matching_step(&code(&text), now), None, "{offset}");
        }
        let at_origin = String::from_utf8(secret.code_at(0).to_vec()).unwrap();
        assert_eq!(secret.matching_step(&code(&at_origin), 0), Some(0));
    }

    #[test]
    fn unit_totp_time_step_rejects_pre_epoch() {
        assert_eq!(time_step(0), Some(0));
        assert_eq!(time_step(29), Some(0));
        assert_eq!(time_step(30), Some(1));
        assert_eq!(time_step(-1), None);
    }

    #[rstest]
    #[case::empty("")]
    #[case::short("12345")]
    #[case::long("1234567")]
    #[case::letters("12a456")]
    #[case::spaced("123 456")]
    #[case::unicode_digits("١٢٣٤٥٦")]
    fn unit_totp_code_parse_rejects_malformed(#[case] input: &str) {
        assert!(TotpCode::parse(input).is_none());
    }

    #[test]
    fn unit_totp_secret_mint_is_160_bits_base32() {
        let minted: Vec<TotpSecret> = (0..16).map(|_| TotpSecret::mint().unwrap()).collect();
        for (index, secret) in minted.iter().enumerate() {
            let text = secret.base32();
            assert_eq!(text.expose_secret().len(), 32);
            assert!(text
                .expose_secret()
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte)));
            for later in &minted[index + 1..] {
                assert_ne!(secret.expose_secret(), later.expose_secret());
            }
        }
        assert_eq!(
            base32(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        assert_eq!(
            TotpSecret::from_opened(&Secret::new(vec![0; 19])).unwrap_err(),
            CryptoError::MalformedTotpSecret
        );
    }

    #[test]
    fn unit_backup_codes_are_80_bit_and_hashed_normalized() {
        let set = BackupCode::mint_set().unwrap();
        assert_eq!(set.len(), BACKUP_CODE_COUNT);
        let digests: HashSet<String> = set
            .iter()
            .map(|code| code.digest().as_str().to_owned())
            .collect();
        assert_eq!(digests.len(), BACKUP_CODE_COUNT);
        for minted in &set {
            let display = minted.display().expose_secret();
            let groups: Vec<&str> = display.split('-').collect();
            assert_eq!(groups.len(), 4, "{display}");
            assert!(groups.iter().all(|group| group.len() == 4));
            let normalized = groups.concat();
            assert_eq!(normalized.len(), BACKUP_CODE_CHARS);
            assert_eq!(BACKUP_CODE_CHARS * 5, 80);
            assert_eq!(
                minted.digest().as_str(),
                sha256_hex(normalized.as_bytes()).as_str()
            );
            for variant in [
                display.clone(),
                display.to_ascii_lowercase(),
                normalized.clone(),
                normalized.to_ascii_lowercase(),
            ] {
                assert_eq!(
                    backup_code_digest(&variant).unwrap().as_str(),
                    minted.digest().as_str()
                );
            }
        }
        for malformed in [
            "",
            "ABCD-EFGH",
            "ABCD-EFGH-IJKL-MNO1",
            "ABCD EFGH IJKL MNOP",
        ] {
            assert!(backup_code_digest(malformed).is_none(), "{malformed}");
        }
    }

    #[test]
    fn unit_totp_material_never_formatted() {
        let secret = TotpSecret::mint().unwrap();
        let backup = BackupCode::mint_set().unwrap();
        let base = secret.base32();
        let rendered = [
            format!("{secret:?}"),
            format!("{:?}", code("123456")),
            format!("{backup:?}"),
            format!("{base:?}"),
        ];
        for text in rendered {
            assert!(text.contains("<redacted>"), "{text}");
            assert!(!text.contains(base.expose_secret().as_str()));
            assert!(!text.contains("123456"));
            for code in &backup {
                assert!(!text.contains(code.display().expose_secret().as_str()));
            }
        }
    }
}
