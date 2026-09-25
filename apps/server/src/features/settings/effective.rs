use serde::Serialize;
use utoipa::ToSchema;

use super::model::AppSettings;
use super::SettingsHandle;
use crate::domain::alias::Alias;
use crate::domain::bytes::ByteSize;
use crate::features::email::transport::SmtpConfig;
use crate::features::users::error::UserError;
use crate::features::users::model::UserId;
use crate::features::users::repo as users;
use crate::features::users::service::{effective_quota, AccountPasswordPolicy};
use crate::infra::db::ReadPool;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSettings {
    pub password_min_length: u32,
    pub public_link_password_min_length: u32,
    #[schema(required = true)]
    pub max_file_size_bytes: Option<u64>,
    #[schema(required = true)]
    pub quota_bytes: Option<u64>,
    #[schema(required = true)]
    pub max_public_link_lifetime_days: Option<u32>,
    pub two_factor_required: bool,
    pub trusted_devices_enabled: bool,
    pub trusted_device_duration_days: u32,
    #[schema(required = true)]
    pub received_retention_max_days: Option<u32>,
    pub smtp_configured: bool,
    #[schema(value_type = String, example = "local")]
    pub storage_provider: &'static str,
    pub max_concurrent_transfers: u16,
    pub alias_pattern: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorPolicy {
    pub storage_provider: &'static str,
    pub max_concurrent_transfers: u16,
}

#[derive(Debug)]
pub enum EffectiveSettingsError {
    UserMissing,
    User(UserError),
}

#[derive(Clone)]
pub struct EffectiveSettingsService {
    reader: ReadPool,
    settings: SettingsHandle,
    operator: OperatorPolicy,
}

impl EffectiveSettingsService {
    pub const fn new(reader: ReadPool, settings: SettingsHandle, operator: OperatorPolicy) -> Self {
        Self {
            reader,
            settings,
            operator,
        }
    }

    pub async fn for_user(
        &self,
        user_id: UserId,
    ) -> Result<EffectiveSettings, EffectiveSettingsError> {
        let user = users::find_by_id(&self.reader, user_id)
            .await
            .map_err(EffectiveSettingsError::User)?
            .ok_or(EffectiveSettingsError::UserMissing)?;
        let settings = self.settings.load();
        let quota = effective_quota(user.quota, settings.quotas.default_user_quota_bytes);
        Ok(EffectiveSettings::new(&settings, quota, self.operator))
    }
}

impl EffectiveSettings {
    pub fn new(settings: &AppSettings, quota: Option<ByteSize>, operator: OperatorPolicy) -> Self {
        let security = &settings.security;
        Self {
            password_min_length: AccountPasswordPolicy::from_settings(settings).min_length(),
            public_link_password_min_length: security.public_link_password_min_length,
            max_file_size_bytes: settings.quotas.max_file_size_bytes.map(ByteSize::get),
            quota_bytes: quota.map(ByteSize::get),
            max_public_link_lifetime_days: settings.public_links.max_public_link_lifetime_days,
            two_factor_required: security.two_factor_required,
            trusted_devices_enabled: security.trusted_devices_enabled,
            trusted_device_duration_days: security.trusted_device_duration_days,
            received_retention_max_days: settings.retention.received_retention_days,
            smtp_configured: SmtpConfig::from_settings(&settings.smtp).is_ok(),
            storage_provider: operator.storage_provider,
            max_concurrent_transfers: operator.max_concurrent_transfers,
            alias_pattern: Alias::PATTERN,
        }
    }
}

#[cfg(test)]
mod tests;
