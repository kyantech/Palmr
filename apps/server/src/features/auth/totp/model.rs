use std::fmt;

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use crate::domain::secret::REDACTED;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::aead::NONCE_LEN;
use crate::infra::crypto::hash::sha256_hex;
use crate::infra::crypto::totp::{TotpSecret, TOTP_DIGITS, TOTP_PERIOD_SECONDS};
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

pub const ISSUER: &str = "Palmr";
pub const ENROLLMENT_ID_LEN: usize = 32;

const AAD_LABEL: &[u8] = b"totp";
const ENROLLMENT_ID_LABEL: &[u8] = b"totp_enrollment";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpState {
    Pending,
    Active,
}

impl TotpState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            _ => None,
        }
    }
}

pub struct TotpRow {
    pub state: TotpState,
    pub secret_ciphertext: Vec<u8>,
    pub secret_nonce: Vec<u8>,
    pub key_version: i64,
    pub last_used_step: Option<u64>,
    pub confirmed_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

impl fmt::Debug for TotpRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TotpRow")
            .field("state", &self.state)
            .field("key_version", &self.key_version)
            .field("last_used_step", &self.last_used_step)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeCheck {
    Accepted,
    Invalid,
    Replayed,
    NotEnrolled,
}

pub fn secret_aad(user_id: UserId) -> Vec<u8> {
    let id = user_id.to_string();
    let mut aad = Vec::with_capacity(AAD_LABEL.len() + 1 + id.len());
    aad.extend_from_slice(AAD_LABEL);
    aad.push(0);
    aad.extend_from_slice(id.as_bytes());
    aad
}

pub fn enrollment_id(nonce: &[u8]) -> String {
    let mut input = Vec::with_capacity(ENROLLMENT_ID_LABEL.len() + 1 + NONCE_LEN);
    input.extend_from_slice(ENROLLMENT_ID_LABEL);
    input.push(0);
    input.extend_from_slice(nonce);
    sha256_hex(&input).as_str()[..ENROLLMENT_ID_LEN].to_owned()
}

pub fn enrollment_matches(nonce: &[u8], presented: &str) -> bool {
    let expected = enrollment_id(nonce);
    presented.len() == ENROLLMENT_ID_LEN
        && bool::from(expected.as_bytes().ct_eq(presented.as_bytes()))
}

pub fn provisioning_uri(account: &str, secret: &TotpSecret) -> String {
    format!(
        "otpauth://totp/{issuer}:{account}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits={TOTP_DIGITS}&period={TOTP_PERIOD_SECONDS}",
        issuer = percent_encode(ISSUER),
        account = percent_encode(account),
        secret = secret.base32().expose_secret(),
    )
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'@') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TwoFactorStatus {
    pub enabled: bool,
    #[schema(required = true, value_type = Option<String>, format = DateTime, example = "2026-03-02T10:00:00.000Z")]
    pub enrolled_at: Option<String>,
    #[schema(example = 7)]
    pub backup_codes_remaining: u32,
    pub required_by_policy: bool,
    pub can_disable: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentResponse {
    #[schema(example = "3f9c1d2ab47e5f60718293a4b5c6d7e8")]
    pub enrollment_id: String,
    #[schema(
        example = "otpauth://totp/Palmr:ada@example.com?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=Palmr&algorithm=SHA1&digits=6&period=30"
    )]
    pub otpauth_uri: String,
    #[schema(example = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP")]
    pub secret_base32: String,
    #[schema(value_type = String, format = DateTime, example = "2026-09-22T14:18:51.204Z")]
    pub expires_at: String,
}

impl fmt::Debug for EnrollmentResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollmentResponse")
            .field("enrollment_id", &self.enrollment_id)
            .field("otpauth_uri", &REDACTED)
            .field("secret_base32", &REDACTED)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupCodesResponse {
    #[schema(example = json!(["A3F2-9K1L-QW7E-M4ZP", "…"]))]
    pub backup_codes: Vec<String>,
    #[schema(value_type = String, format = DateTime, example = "2026-09-22T14:09:12.311Z")]
    pub generated_at: String,
}

impl fmt::Debug for BackupCodesResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupCodesResponse")
            .field("backup_codes", &REDACTED)
            .field("generated_at", &self.generated_at)
            .finish()
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentVerifyRequest {
    #[schema(example = "3f9c1d2ab47e5f60718293a4b5c6d7e8")]
    pub enrollment_id: String,
    #[schema(example = "492013")]
    pub code: String,
}

impl JsonRequest for EnrollmentVerifyRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("enrollmentId", JsonKind::String),
        JsonField::required("code", JsonKind::String),
    ];
}

#[cfg(test)]
mod tests {
    use super::{
        enrollment_id, enrollment_matches, provisioning_uri, secret_aad, ENROLLMENT_ID_LEN,
    };
    use crate::domain::clock::TestClock;
    use crate::domain::secret::Secret;
    use crate::features::users::model::UserId;
    use crate::infra::crypto::totp::TotpSecret;

    #[test]
    fn unit_totp_provisioning_uri_shape() {
        let secret =
            TotpSecret::from_opened(&Secret::new(b"12345678901234567890".to_vec())).unwrap();
        assert_eq!(
            provisioning_uri("ada+2fa@example.com", &secret),
            "otpauth://totp/Palmr:ada%2B2fa@example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Palmr&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn unit_totp_enrollment_id_binds_the_pending_seal() {
        let first = enrollment_id(&[1; 24]);
        assert_eq!(first.len(), ENROLLMENT_ID_LEN);
        assert_eq!(first, enrollment_id(&[1; 24]));
        assert_ne!(first, enrollment_id(&[2; 24]));
        assert!(enrollment_matches(&[1; 24], &first));
        assert!(!enrollment_matches(&[2; 24], &first));
        assert!(!enrollment_matches(&[1; 24], &first[1..]));
        assert!(!enrollment_matches(&[1; 24], ""));
    }

    #[test]
    fn unit_totp_aad_is_user_bound() {
        let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
        let ada = UserId::generate(&clock);
        let bob = UserId::generate(&clock);
        assert_eq!(
            secret_aad(ada),
            [b"totp\0".as_slice(), ada.to_string().as_bytes()].concat()
        );
        assert_ne!(secret_aad(ada), secret_aad(bob));
    }
}
