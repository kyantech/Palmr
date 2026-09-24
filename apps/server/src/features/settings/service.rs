use std::sync::Arc;

use super::error::SettingsError;
use super::model::{self, AppSettings, ValueType};
use super::repo;
use super::snapshot::{self, setting_aad, SettingsHandle};
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::{DbError, DbPools, WriteTx};

pub const SETTINGS_UPDATE_GROUP: &str = "settings.update_group";

#[derive(Debug, Clone, Copy)]
pub enum SettingValueInput<'a> {
    String(&'a str),
    Integer(i64),
    Boolean(bool),
    Secret(&'a str),
}

impl SettingValueInput<'_> {
    const fn value_type(&self) -> ValueType {
        match self {
            Self::String(_) => ValueType::String,
            Self::Integer(_) => ValueType::Integer,
            Self::Boolean(_) => ValueType::Boolean,
            Self::Secret(_) => ValueType::Secret,
        }
    }
}

#[derive(Clone)]
pub struct SettingsService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
    handle: SettingsHandle,
}

impl std::fmt::Debug for SettingsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsService").finish_non_exhaustive()
    }
}

impl SettingsService {
    pub async fn load(
        pools: &DbPools,
        clock: Arc<dyn Clock>,
        instance_key: &InstanceKey,
    ) -> Result<Self, SettingsError> {
        let service = Self {
            pools: pools.clone(),
            clock,
            keys: Arc::new(KeyRing::new(instance_key)),
            handle: SettingsHandle::new(AppSettings::defaults()),
        };
        service.reload().await?;
        Ok(service)
    }

    pub fn handle(&self) -> SettingsHandle {
        self.handle.clone()
    }

    pub fn keys(&self) -> Arc<KeyRing> {
        Arc::clone(&self.keys)
    }

    pub fn current(&self) -> Arc<AppSettings> {
        self.handle.load()
    }

    pub async fn reload(&self) -> Result<(), SettingsError> {
        let rows = repo::load_all(self.pools.reader()).await?;
        let settings = snapshot::build(&rows, &self.keys)?;
        self.handle.store(settings);
        Ok(())
    }

    pub async fn update_group<T, E, F>(&self, name: &'static str, work: F) -> Result<T, E>
    where
        F: AsyncFnOnce(&mut WriteTx<'_>) -> Result<T, E>,
        E: From<DbError> + From<SettingsError>,
    {
        let value = self.pools.write_tx(self.clock.as_ref(), name, work).await?;
        self.reload().await.map_err(E::from)?;
        Ok(value)
    }

    pub async fn write_setting(
        &self,
        tx: &mut WriteTx<'_>,
        key: &str,
        value: SettingValueInput<'_>,
        updated_by: Option<&str>,
    ) -> Result<(), SettingsError> {
        let spec = model::spec(key).ok_or_else(|| SettingsError::UnknownKey {
            key: key.to_owned(),
        })?;
        let provided = value.value_type();
        if spec.value_type != provided {
            return Err(SettingsError::ValueTypeMismatch {
                key: key.to_owned(),
                stored: provided.as_str().to_owned(),
                expected: spec.value_type.as_str(),
            });
        }
        let updated_at = Timestamp::try_from(self.clock.now())?.to_string();
        match value {
            SettingValueInput::String(text) => {
                let json = serde_json::Value::from(text).to_string();
                repo::upsert_value(tx, spec, &json, &updated_at, updated_by).await?;
            }
            SettingValueInput::Integer(number) => {
                let json = serde_json::Value::from(number).to_string();
                repo::upsert_value(tx, spec, &json, &updated_at, updated_by).await?;
            }
            SettingValueInput::Boolean(flag) => {
                let json = serde_json::Value::from(flag).to_string();
                repo::upsert_value(tx, spec, &json, &updated_at, updated_by).await?;
            }
            SettingValueInput::Secret(plaintext) => {
                let sealed =
                    self.keys
                        .seal(SealPurpose::Smtp, &setting_aad(key), plaintext.as_bytes())?;
                repo::upsert_secret(
                    tx,
                    key,
                    spec.group.as_str(),
                    &sealed,
                    &updated_at,
                    updated_by,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn clear_setting(
        &self,
        tx: &mut WriteTx<'_>,
        key: &str,
    ) -> Result<(), SettingsError> {
        if model::spec(key).is_none() {
            return Err(SettingsError::UnknownKey {
                key: key.to_owned(),
            });
        }
        repo::delete(tx, key).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;
    use time::macros::datetime;

    use super::*;
    use crate::config::SqliteSynchronous;
    use crate::domain::clock::TestClock;
    use crate::features::settings::error::STARTUP_SETTINGS_DECRYPT_FAILED;
    use crate::features::settings::model::{
        AssetMode, EmailLogoMode, Group, SenderFieldPolicy, SmtpSecurity, ThumbnailSourceLimit,
        SETTINGS_REGISTRY,
    };
    use crate::features::settings::snapshot;
    use crate::infra::crypto::aead::SealedSecret;
    use crate::infra::db::MIGRATOR;

    const START: time::OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
    const SMTP_SENTINEL: &str = "palmr-settings-smtp-sentinel-7c1e";

    struct Harness {
        _root: TempDir,
        root_path: PathBuf,
        pools: DbPools,
        clock: TestClock,
    }

    impl Harness {
        async fn open() -> Self {
            let root = TempDir::new().unwrap();
            let root_path = root.path().to_path_buf();
            let pools = DbPools::open(&root_path, 4, SqliteSynchronous::Full)
                .await
                .unwrap();
            pools.migrate(&MIGRATOR).await.unwrap();
            Self {
                _root: root,
                root_path,
                pools,
                clock: TestClock::new(START),
            }
        }

        fn clock(&self) -> Arc<dyn Clock> {
            Arc::new(self.clock.clone())
        }

        fn instance_key(&self) -> InstanceKey {
            InstanceKey::load_or_create(&self.root_path).unwrap().0
        }

        async fn service(&self, key: &InstanceKey) -> Result<SettingsService, SettingsError> {
            SettingsService::load(&self.pools, self.clock(), key).await
        }

        async fn insert_secret(&self, key: &str, sealed: &SealedSecret) {
            self.pools
                .write_tx(&self.clock, "test.insert_setting_secret", async |tx| {
                    repo::upsert_secret(
                        tx,
                        key,
                        "smtp",
                        sealed,
                        &Timestamp::try_from(self.clock.now()).unwrap().to_string(),
                        None,
                    )
                    .await
                })
                .await
                .unwrap();
        }

        async fn insert_raw(&self, key: &str, group: &str, value_type: &str, json: &str) {
            self.pools
                .write_tx(&self.clock, "test.insert_setting_raw", async |tx| {
                    sqlx::query(
                        "INSERT INTO app_settings
                             (key, group_name, value_type, value_json, is_secret, updated_at)
                         VALUES (?1, ?2, ?3, ?4, 0, '2026-09-24T12:00:00.000Z')",
                    )
                    .bind(key)
                    .bind(group)
                    .bind(value_type)
                    .bind(json)
                    .execute(tx.executor())
                    .await?;
                    Ok::<(), SettingsError>(())
                })
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn it_settings_defaults_match_spec_appendix_b() {
        let harness = Harness::open().await;
        let key = harness.instance_key();
        let service = harness.service(&key).await.unwrap();
        let settings = service.current();

        assert!(!settings.setup_completed());
        assert_eq!(settings.app_name(), "Palmr");
        assert_eq!(settings.app_description(), "Self-hosted file transfer");
        assert_eq!(settings.default_locale().as_str(), "en-US");
        assert!(settings.general.show_version);
        assert!(settings.general.powered_by_visible);
        assert_eq!(
            settings.general.thumbnail_source_limit,
            ThumbnailSourceLimit::MiB64
        );

        assert_eq!(settings.branding.primary_color, "#1668dc");
        assert_eq!(settings.branding.logo_mode, AssetMode::Default);
        assert_eq!(settings.branding.favicon_mode, AssetMode::Default);
        assert_eq!(settings.branding.login_background_mode, AssetMode::Default);
        assert_eq!(settings.branding.og_image_mode, AssetMode::Default);
        assert_eq!(settings.branding.email_logo_mode, EmailLogoMode::Inherit);

        assert_eq!(settings.security.password_min_length, 8);
        assert_eq!(settings.security.public_link_password_min_length, 8);
        assert_eq!(settings.security.max_login_attempts, 5);
        assert_eq!(settings.security.login_lockout_minutes, 10);
        assert_eq!(settings.security.session_idle_days, 7);
        assert_eq!(settings.security.session_absolute_days, 30);
        assert_eq!(settings.security.recent_auth_minutes, 5);
        assert_eq!(settings.security.password_reset_validity_minutes, 60);
        assert_eq!(settings.security.invite_validity_hours, 24);
        assert!(!settings.security.two_factor_required);
        assert!(settings.security.trusted_devices_enabled);
        assert_eq!(settings.security.trusted_device_duration_days, 30);

        assert_eq!(settings.quotas.max_file_size_bytes, None);
        assert_eq!(settings.quotas.default_user_quota_bytes, None);
        assert_eq!(settings.public_links.max_public_link_lifetime_days, None);
        assert_eq!(settings.retention.received_retention_days, None);
        assert_eq!(settings.audit.audit_retention_days, 90);

        let documented = settings.documented;
        assert_eq!(documented.reverse_share_max_file_size_bytes, None);
        assert_eq!(documented.reverse_share_max_file_count, None);
        assert_eq!(documented.share_expiration_days, None);
        assert_eq!(documented.reverse_share_expiration_days, None);
        assert_eq!(
            documented.reverse_share_name_field,
            SenderFieldPolicy::Optional
        );
        assert_eq!(
            documented.reverse_share_email_field,
            SenderFieldPolicy::Optional
        );
        assert_eq!(
            documented.reverse_share_description_field,
            SenderFieldPolicy::Optional
        );
        assert!(documented.reverse_share_owner_notification);
        assert!(documented.share_recipient_notification);
        assert!(!documented.provider_auto_provision);

        assert_eq!(settings.smtp.port, 587);
        assert_eq!(settings.smtp.security, SmtpSecurity::Starttls);
        assert!(settings.smtp.password.is_none());
        assert!(settings.smtp.host.is_none());
        assert!(!settings.smtp.enabled);

        for spec in SETTINGS_REGISTRY {
            assert!(Group::ALL.contains(&spec.group), "{}", spec.key);
            assert_ne!(spec.group.as_str(), "storage", "{}", spec.key);
            assert!(
                spec.key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
                "{}",
                spec.key
            );
            for variable in crate::config::Variable::ALL {
                assert_ne!(
                    spec.key,
                    variable.name().to_ascii_lowercase(),
                    "operator variable {} must not be persisted",
                    variable.name()
                );
            }
        }
        for group in Group::ALL {
            assert!(
                SETTINGS_REGISTRY.iter().any(|spec| spec.group == group),
                "no registered setting for group {}",
                group.as_str()
            );
        }
        for (index, spec) in SETTINGS_REGISTRY.iter().enumerate() {
            let duplicate = SETTINGS_REGISTRY[index + 1..]
                .iter()
                .any(|other| other.key == spec.key);
            assert!(!duplicate, "duplicate settings key {}", spec.key);
        }

        harness
            .insert_raw("storage_provider", "general", "string", "\"local\"")
            .await;
        let error = service.reload().await.unwrap_err();
        assert!(matches!(error, SettingsError::UnknownKey { .. }));
    }

    #[tokio::test]
    async fn it_settings_swap_after_commit() {
        let harness = Harness::open().await;
        let key = harness.instance_key();
        let service = harness.service(&key).await.unwrap();
        let before = service.current();
        assert_eq!(before.app_name(), "Palmr");
        assert_eq!(before.audit.audit_retention_days, 90);

        service
            .update_group(SETTINGS_UPDATE_GROUP, async |tx| {
                assert_eq!(service.current().app_name(), "Palmr");
                service
                    .write_setting(tx, "app_name", SettingValueInput::String("Nova"), None)
                    .await
            })
            .await
            .unwrap();

        assert_eq!(before.app_name(), "Palmr");
        assert_eq!(service.current().app_name(), "Nova");

        let stored: String =
            sqlx::query_scalar("SELECT value_json FROM app_settings WHERE key = 'app_name'")
                .fetch_one(harness.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(stored, "\"Nova\"");

        harness
            .pools
            .write_tx(&harness.clock, "test.commit_without_swap", async |tx| {
                repo::upsert_value(
                    tx,
                    model::spec("audit_retention_days").unwrap(),
                    "30",
                    &Timestamp::try_from(harness.clock.now())
                        .unwrap()
                        .to_string(),
                    None,
                )
                .await
            })
            .await
            .unwrap();
        assert_eq!(service.current().audit.audit_retention_days, 90);
        service.reload().await.unwrap();
        assert_eq!(service.current().audit.audit_retention_days, 30);

        let reconstructed = harness.service(&key).await.unwrap();
        assert_eq!(reconstructed.current().audit.audit_retention_days, 30);
        assert_eq!(reconstructed.current().app_name(), "Nova");
    }

    #[tokio::test]
    async fn it_settings_decrypt_failure_fatal() {
        let harness = Harness::open().await;
        let good_key = harness.instance_key();
        let good_ring = KeyRing::new(&good_key);
        let sealed = good_ring
            .seal(
                SealPurpose::Smtp,
                &setting_aad("smtp_password"),
                SMTP_SENTINEL.as_bytes(),
            )
            .unwrap();
        harness.insert_secret("smtp_password", &sealed).await;

        let loaded = harness.service(&good_key).await.unwrap();
        assert_eq!(
            loaded
                .current()
                .smtp_password()
                .unwrap()
                .expose_secret()
                .as_str(),
            SMTP_SENTINEL
        );

        let other_root = TempDir::new().unwrap();
        let other_key = InstanceKey::load_or_create(other_root.path()).unwrap().0;
        let error = harness.service(&other_key).await.unwrap_err();
        assert!(matches!(error, SettingsError::Decrypt { .. }));
        assert_eq!(error.code(), STARTUP_SETTINGS_DECRYPT_FAILED);
        assert_eq!(error.kind(), "settings_decrypt_failed");

        let startup = crate::app::lifecycle::StartupError::from(error);
        assert_eq!(startup.code(), Some(STARTUP_SETTINGS_DECRYPT_FAILED));
        assert_eq!(startup.exit_code(), 78);
        let rendered = format!("{startup} | {startup:?}");
        assert!(!rendered.contains(SMTP_SENTINEL), "{rendered}");
        assert!(!rendered.contains(&hex(sealed.ciphertext())), "{rendered}");
        assert!(
            !rendered.contains(&hex(good_key.expose_secret())),
            "{rendered}"
        );

        let mut tampered = sealed.ciphertext().to_vec();
        tampered[0] ^= 0x01;
        let tampered =
            SealedSecret::from_parts(tampered, sealed.nonce(), sealed.key_version()).unwrap();
        harness.insert_secret("smtp_password", &tampered).await;
        let error = harness.service(&good_key).await.unwrap_err();
        assert!(matches!(error, SettingsError::Decrypt { .. }));
        assert!(!error.to_string().contains(SMTP_SENTINEL));

        let wrong_aad = good_ring
            .seal(
                SealPurpose::Smtp,
                &setting_aad("smtp_username"),
                SMTP_SENTINEL.as_bytes(),
            )
            .unwrap();
        harness.insert_secret("smtp_password", &wrong_aad).await;
        let error = harness.service(&good_key).await.unwrap_err();
        assert!(matches!(error, SettingsError::Decrypt { .. }));
        assert_eq!(error.code(), STARTUP_SETTINGS_DECRYPT_FAILED);

        let rows = repo::load_all(harness.pools.reader()).await.unwrap();
        let fallback = SettingsHandle::documented_defaults();
        assert!(snapshot::build(&rows, &KeyRing::new(&other_key)).is_err());
        assert!(fallback.load().smtp_password().is_none());
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
