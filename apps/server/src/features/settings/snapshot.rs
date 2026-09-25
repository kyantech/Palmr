use std::sync::Arc;

use arc_swap::ArcSwap;

use super::error::SettingsError;
use super::model::{self, AppSettings, ValueType};
use super::repo::SettingRow;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};

#[derive(Clone)]
pub struct SettingsHandle {
    inner: Arc<ArcSwap<AppSettings>>,
}

impl SettingsHandle {
    pub fn new(settings: AppSettings) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(settings)),
        }
    }

    pub fn documented_defaults() -> Self {
        Self::new(AppSettings::defaults())
    }

    pub fn load(&self) -> Arc<AppSettings> {
        self.inner.load_full()
    }

    pub(super) fn store(&self, settings: AppSettings) {
        self.inner.store(Arc::new(settings));
    }
}

pub(crate) fn setting_aad(key: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity("setting".len() + 1 + key.len());
    aad.extend_from_slice(b"setting");
    aad.push(0);
    aad.extend_from_slice(key.as_bytes());
    aad
}

pub fn suggest_setup_locale(settings: &mut AppSettings, rows: &[SettingRow], locale: LocaleCode) {
    let persisted = rows.iter().any(|row| row.key == "default_locale");
    if !settings.setup_completed() && !persisted {
        settings.general.default_locale = locale;
    }
}

pub fn build(rows: &[SettingRow], keys: &KeyRing) -> Result<AppSettings, SettingsError> {
    let mut settings = AppSettings::defaults();
    for row in rows {
        let spec = model::spec(&row.key).ok_or_else(|| SettingsError::UnknownKey {
            key: row.key.clone(),
        })?;
        if row.group != spec.group.as_str() {
            return Err(SettingsError::GroupMismatch {
                key: row.key.clone(),
                stored: row.group.clone(),
                expected: spec.group.as_str(),
            });
        }
        if row.value_type != spec.value_type.as_str() {
            return Err(SettingsError::ValueTypeMismatch {
                key: row.key.clone(),
                stored: row.value_type.clone(),
                expected: spec.value_type.as_str(),
            });
        }
        match spec.value_type {
            ValueType::Secret => {
                let sealed = SealedSecret::from_parts(
                    row.ciphertext.clone().unwrap_or_default(),
                    row.nonce.as_deref().unwrap_or_default(),
                    row.key_version,
                )
                .map_err(|_| SettingsError::Decrypt {
                    key: row.key.clone(),
                })?;
                let opened = keys
                    .open(SealPurpose::Smtp, &setting_aad(&row.key), &sealed)
                    .map_err(|_| SettingsError::Decrypt {
                        key: row.key.clone(),
                    })?;
                let plaintext =
                    String::from_utf8(opened.expose_secret().clone()).map_err(|_| {
                        SettingsError::Decrypt {
                            key: row.key.clone(),
                        }
                    })?;
                model::apply_secret(&mut settings, spec, Secret::new(plaintext))?;
            }
            ValueType::String | ValueType::Integer | ValueType::Boolean | ValueType::Json => {
                let json =
                    row.value_json
                        .as_deref()
                        .ok_or_else(|| SettingsError::MalformedValue {
                            key: row.key.clone(),
                        })?;
                let value: serde_json::Value =
                    serde_json::from_str(json).map_err(|_| SettingsError::MalformedValue {
                        key: row.key.clone(),
                    })?;
                model::apply_value(&mut settings, spec, &value)?;
            }
        }
    }
    Ok(settings)
}
