use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::id::Id;
use crate::domain::secret::{Secret, REDACTED};
use crate::domain::time::Timestamp;
use crate::features::auth::login::MAX_IDENTIFIER_CHARS;
use crate::features::users::model::NormalizedIdentifier;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::error::PasswordResetError;

pub enum PasswordResetToken {}

pub type PasswordResetTokenId = Id<PasswordResetToken>;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForgotPasswordRequest {
    #[schema(example = "ada@example.com")]
    pub identifier: String,
}

impl JsonRequest for ForgotPasswordRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("identifier", JsonKind::String)];
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetCheckRequest {
    /// The opaque token from the e-mailed `/reset-password/{token}` link.
    pub token: String,
}

impl fmt::Debug for ResetCheckRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResetCheckRequest")
            .field("token", &REDACTED)
            .finish()
    }
}

impl JsonRequest for ResetCheckRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("token", JsonKind::String)];
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetPasswordRequest {
    /// The opaque token from the e-mailed `/reset-password/{token}` link.
    pub token: String,
    #[schema(format = Password)]
    pub new_password: String,
}

impl fmt::Debug for ResetPasswordRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResetPasswordRequest")
            .field("token", &REDACTED)
            .field("new_password", &REDACTED)
            .finish()
    }
}

impl JsonRequest for ResetPasswordRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("token", JsonKind::String),
        JsonField::required("newPassword", JsonKind::String),
    ];
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedResponse {
    #[schema(example = true)]
    pub accepted: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetCheckResponse {
    #[schema(example = true)]
    pub valid: bool,
    #[schema(example = 8)]
    pub password_min_length: u32,
    #[schema(value_type = String, format = DateTime, example = "2026-09-22T15:03:51.204Z")]
    pub expires_at: String,
}

pub struct ForgotInput {
    pub identifier: NormalizedIdentifier,
}

impl ForgotInput {
    pub fn parse(request: ForgotPasswordRequest) -> Result<Self, PasswordResetError> {
        let identifier = NormalizedIdentifier::from_input(&request.identifier);
        let length = identifier.as_str().chars().count();
        if length == 0 || length > MAX_IDENTIFIER_CHARS {
            return Err(PasswordResetError::Invalid {
                fields: vec!["identifier"],
            });
        }
        Ok(Self { identifier })
    }
}

pub struct PresentedToken(TokenDigest);

impl PresentedToken {
    pub fn parse(encoded: &str) -> Result<Self, PasswordResetError> {
        Token::decode(encoded)
            .map(|token| Self(token.digest()))
            .map_err(|_| PasswordResetError::TokenInvalid)
    }

    pub const fn digest(&self) -> &TokenDigest {
        &self.0
    }
}

pub struct ResetInput {
    pub token: PresentedToken,
    pub new_password: Secret<String>,
}

impl ResetInput {
    pub fn parse(request: ResetPasswordRequest) -> Result<Self, PasswordResetError> {
        let ResetPasswordRequest {
            token,
            new_password,
        } = request;
        let new_password = Secret::new(new_password);
        Ok(Self {
            token: PresentedToken::parse(&token)?,
            new_password,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    Live { expires_at: Timestamp },
    Expired,
    Used,
    Invalidated,
    AccountUnavailable,
}

impl TokenState {
    pub fn classify(stored: &StoredToken, now: Timestamp) -> Self {
        if stored.used_at.is_some() {
            Self::Used
        } else if stored.invalidated_at.is_some() {
            Self::Invalidated
        } else if stored.expires_at <= now {
            Self::Expired
        } else if !stored.account_eligible {
            Self::AccountUnavailable
        } else {
            Self::Live {
                expires_at: stored.expires_at,
            }
        }
    }

    pub fn into_result(self) -> Result<Timestamp, PasswordResetError> {
        match self {
            Self::Live { expires_at } => Ok(expires_at),
            Self::Expired => Err(PasswordResetError::TokenExpired),
            Self::Used => Err(PasswordResetError::TokenUsed),
            Self::Invalidated | Self::AccountUnavailable => Err(PasswordResetError::TokenInvalid),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredToken {
    pub expires_at: Timestamp,
    pub used_at: Option<Timestamp>,
    pub invalidated_at: Option<Timestamp>,
    pub account_eligible: bool,
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn at(offset_minutes: i64) -> Timestamp {
        Timestamp::try_from(
            datetime!(2026-09-25 12:00 UTC) + time::Duration::minutes(offset_minutes),
        )
        .unwrap()
    }

    fn stored() -> StoredToken {
        StoredToken {
            expires_at: at(60),
            used_at: None,
            invalidated_at: None,
            account_eligible: true,
        }
    }

    #[test]
    fn unit_reset_token_state_maps_to_canonical_errors() {
        let now = at(0);
        assert_eq!(
            TokenState::classify(&stored(), now),
            TokenState::Live { expires_at: at(60) }
        );
        let used = StoredToken {
            used_at: Some(at(1)),
            expires_at: at(-5),
            ..stored()
        };
        assert!(matches!(
            TokenState::classify(&used, now).into_result(),
            Err(PasswordResetError::TokenUsed)
        ));
        let superseded = StoredToken {
            invalidated_at: Some(at(1)),
            ..stored()
        };
        assert!(matches!(
            TokenState::classify(&superseded, now).into_result(),
            Err(PasswordResetError::TokenInvalid)
        ));
        let expired = StoredToken {
            expires_at: now,
            ..stored()
        };
        assert!(matches!(
            TokenState::classify(&expired, now).into_result(),
            Err(PasswordResetError::TokenExpired)
        ));
        let inactive = StoredToken {
            account_eligible: false,
            ..stored()
        };
        assert!(matches!(
            TokenState::classify(&inactive, now).into_result(),
            Err(PasswordResetError::TokenInvalid)
        ));
    }

    #[test]
    fn unit_presented_token_rejects_malformed_values() {
        for malformed in ["", "short", &"A".repeat(44), &"+".repeat(43)] {
            assert!(matches!(
                PresentedToken::parse(malformed),
                Err(PasswordResetError::TokenInvalid)
            ));
        }
        let token = Token::mint().unwrap();
        let parsed = PresentedToken::parse(token.encode().expose_secret()).unwrap();
        assert_eq!(parsed.digest().as_str(), token.digest().as_str());
    }

    #[test]
    fn unit_reset_requests_never_debug_secrets() {
        let check = ResetCheckRequest {
            token: "sentinel-token-value".to_owned(),
        };
        let reset = ResetPasswordRequest {
            token: "sentinel-token-value".to_owned(),
            new_password: "sentinel-password".to_owned(),
        };
        for rendered in [format!("{check:?}"), format!("{reset:?}")] {
            assert!(!rendered.contains("sentinel"), "{rendered}");
        }
    }
}
