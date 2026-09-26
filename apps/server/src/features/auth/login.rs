use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use utoipa::ToSchema;

use crate::domain::secret::Secret;
use crate::features::settings::model::AppSettings;
use crate::features::users::model::NormalizedIdentifier;
use crate::infra::crypto::password::{verify_password, DummyPasswordHash, PasswordVerification};
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, WriteTx};
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::error::LoginError;

pub const MAX_IDENTIFIER_CHARS: usize = 254;

pub const fn password_login_enabled(_settings: &AppSettings) -> bool {
    true
}

pub async fn reenable_password_login_in_tx(_tx: &mut WriteTx<'_>) -> Result<bool, DbError> {
    Ok(false)
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginRequest {
    #[schema(example = "ada@example.com")]
    pub identifier: String,
    #[schema(format = Password)]
    pub password: String,
}

impl JsonRequest for LoginRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("identifier", JsonKind::String),
        JsonField::required("password", JsonKind::String),
    ];
}

pub struct LoginInput {
    pub submitted: String,
    pub identifier: NormalizedIdentifier,
    pub password: Secret<String>,
}

impl LoginInput {
    pub fn parse(request: LoginRequest) -> Result<Self, LoginError> {
        let LoginRequest {
            identifier,
            password,
        } = request;
        let password = Secret::new(password);
        let normalized = NormalizedIdentifier::from_input(&identifier);
        let mut invalid = Vec::new();
        let length = normalized.as_str().chars().count();
        if length == 0 || length > MAX_IDENTIFIER_CHARS {
            invalid.push("identifier");
        }
        if password.expose_secret().is_empty() {
            invalid.push("password");
        }
        if invalid.is_empty() {
            Ok(Self {
                submitted: identifier,
                identifier: normalized,
                password,
            })
        } else {
            Err(LoginError::Invalid { fields: invalid })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialProof {
    Mismatch,
    Verified { needs_rehash: bool },
}

pub struct CredentialVerifier {
    dummy: DummyPasswordHash,
    performed: AtomicU64,
}

impl CredentialVerifier {
    pub fn new() -> Result<Self, CryptoError> {
        Ok(Self {
            dummy: DummyPasswordHash::new()?,
            performed: AtomicU64::new(0),
        })
    }

    pub fn verify(
        &self,
        password: &Secret<String>,
        stored: Option<&Secret<String>>,
    ) -> CredentialProof {
        self.performed.fetch_add(1, Ordering::Relaxed);
        let password = password.expose_secret().as_bytes();
        let Some(stored) = stored else {
            self.dummy.verify(password);
            return CredentialProof::Mismatch;
        };
        match verify_password(password, stored.expose_secret()) {
            Ok(PasswordVerification::Verified { needs_rehash }) => {
                CredentialProof::Verified { needs_rehash }
            }
            Ok(PasswordVerification::Mismatch) => CredentialProof::Mismatch,
            Err(_) => {
                tracing::error!("a stored password hash is not a valid Argon2id PHC string");
                self.dummy.verify(password);
                CredentialProof::Mismatch
            }
        }
    }

    pub fn performed(&self) -> u64 {
        self.performed.load(Ordering::Relaxed)
    }
}
