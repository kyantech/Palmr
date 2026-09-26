use crate::domain::bytes::ByteSize;
use crate::domain::secret::Secret;
use crate::features::settings::model::AppSettings;
use crate::infra::crypto::token::{Token, ENCODED_TOKEN_LEN};
use crate::infra::crypto::CryptoError;
use crate::infra::db::WriteTx;

use super::error::UserError;
use super::model::{QuotaOverride, UserId};
use super::repo;

pub const PASSWORD_MIN_LENGTH_FLOOR: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountPasswordPolicy {
    min_length: u32,
}

impl AccountPasswordPolicy {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self::with_configured_min_length(settings.security.password_min_length)
    }

    pub fn with_configured_min_length(configured: u32) -> Self {
        Self {
            min_length: configured.max(PASSWORD_MIN_LENGTH_FLOOR),
        }
    }

    pub const fn min_length(self) -> u32 {
        self.min_length
    }

    pub fn temporary_password(self) -> Result<Secret<String>, CryptoError> {
        let length = usize::try_from(self.min_length)
            .unwrap_or(usize::MAX)
            .max(ENCODED_TOKEN_LEN);
        let mut password = String::with_capacity(length.saturating_add(ENCODED_TOKEN_LEN));
        while password.len() < length {
            password.push_str(Token::mint()?.encode().expose_secret());
        }
        password.truncate(length);
        Ok(Secret::new(password))
    }

    pub fn check(self, password: &str) -> Result<(), UserError> {
        let min_length = usize::try_from(self.min_length).unwrap_or(usize::MAX);
        if password.chars().count() >= min_length {
            Ok(())
        } else {
            Err(UserError::PasswordPolicyViolation {
                min_length: self.min_length,
            })
        }
    }
}

pub const fn effective_quota(
    quota: QuotaOverride,
    instance_default: Option<ByteSize>,
) -> Option<ByteSize> {
    match quota {
        QuotaOverride::Bytes(bytes) => Some(bytes),
        QuotaOverride::Unlimited => None,
        QuotaOverride::Inherit => instance_default,
    }
}

pub async fn assert_active_admin_remains(
    tx: &mut WriteTx<'_>,
    target: UserId,
) -> Result<(), UserError> {
    let Some(state) = repo::admin_state(tx, target).await? else {
        return Err(UserError::NotFound);
    };
    if !state.is_active_admin() {
        return Ok(());
    }
    if repo::count_active_admins(tx).await? <= 1 {
        Err(UserError::LastAdminProtected)
    } else {
        Ok(())
    }
}
