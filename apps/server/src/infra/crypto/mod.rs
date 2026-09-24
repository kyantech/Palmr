use std::fmt;

pub mod aead;
pub mod hash;
pub mod hkdf;
pub mod instance_key;
pub mod password;
pub mod token;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    Randomness,
    MissingAad,
    MalformedEnvelope,
    UnsupportedKeyVersion,
    AuthenticationFailed,
    MalformedToken,
    MalformedDigest,
    MalformedPasswordHash,
    PasswordHashing,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Randomness => "the operating system random number generator failed",
            Self::MissingAad => "associated data is required",
            Self::MalformedEnvelope => "sealed secret has a malformed nonce or ciphertext",
            Self::UnsupportedKeyVersion => "sealed secret uses an unsupported key version",
            Self::AuthenticationFailed => "sealed secret failed authentication",
            Self::MalformedToken => "token is not 32 bytes of unpadded base64url",
            Self::MalformedDigest => "digest is not 64 lowercase hexadecimal characters",
            Self::MalformedPasswordHash => {
                "stored password hash is not a supported Argon2 PHC string"
            }
            Self::PasswordHashing => "password hashing failed",
        })
    }
}

impl std::error::Error for CryptoError {}

fn fill_random(bytes: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(bytes).map_err(|_| CryptoError::Randomness)
}
