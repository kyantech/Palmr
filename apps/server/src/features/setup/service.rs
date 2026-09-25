use std::sync::Arc;

use crate::domain::clock::Clock;
use crate::domain::locale::LocaleCode;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::repo as audit_repo;
use crate::features::auth::sessions::{AuthMethod, MintedSession, SessionClient, SessionService};
use crate::features::settings::service::SettingValueInput;
use crate::features::settings::SettingsService;
use crate::features::users::model::{NewUser, QuotaOverride, User};
use crate::features::users::repo as users;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::crypto::password::hash_password;
use crate::infra::db::{DbPools, WriteTx};

use super::error::SetupError;
use super::model::{SetupInput, DEFAULT_APP_DESCRIPTION};
use super::repo;

pub const SETUP_TRANSACTION: &str = "setup.complete";

#[derive(Clone)]
pub struct SetupService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsService,
    sessions: SessionService,
}

pub struct CompletedSetup {
    pub user: User,
    pub session: MintedSession,
}

struct PreparedAdmin {
    user: NewUser,
    app_name: String,
    locale: LocaleCode,
}

impl SetupService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsService,
        sessions: SessionService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            sessions,
        }
    }

    pub const fn sessions(&self) -> &SessionService {
        &self.sessions
    }

    pub async fn complete(
        &self,
        input: SetupInput,
        client: SessionClient,
        audit_client: ClientMetadata,
    ) -> Result<CompletedSetup, SetupError> {
        let current = self.settings.current();
        if current.setup_completed() {
            return Err(SetupError::AlreadyCompleted);
        }
        AccountPasswordPolicy::from_settings(&current).check(input.password.expose_secret())?;
        drop(current);

        let password_hash = hash_off_runtime(input.password).await?;
        let credentials = self.sessions.prepare_credentials()?;
        let prepared = PreparedAdmin {
            user: NewUser {
                email: input.email,
                username: input.username,
                first_name: input.first_name,
                last_name: input.last_name,
                password_hash: Some(password_hash),
                must_change_password: false,
                role: Role::Admin,
                is_active: true,
                quota: QuotaOverride::Inherit,
                created_by: None,
            },
            app_name: input.app_name,
            locale: input.locale,
        };

        let completed = self
            .pools
            .write_tx(self.clock.as_ref(), SETUP_TRANSACTION, async |tx| {
                if repo::setup_completed(tx).await? {
                    return Err(SetupError::AlreadyCompleted);
                }
                let user = self.persist(tx, &prepared, &audit_client).await?;
                let session = self
                    .sessions
                    .mint_in_tx(
                        tx,
                        client.session(user.id, AuthMethod::Password),
                        &credentials,
                    )
                    .await?;
                Ok(CompletedSetup { user, session })
            })
            .await?;

        if let Err(error) = self.settings.reload().await {
            tracing::error!(
                kind = error.kind(),
                "settings snapshot could not be refreshed after setup committed"
            );
        }
        Ok(completed)
    }

    async fn persist(
        &self,
        tx: &mut WriteTx<'_>,
        prepared: &PreparedAdmin,
        audit_client: &ClientMetadata,
    ) -> Result<User, SetupError> {
        let clock = self.clock.as_ref();
        let user = users::insert(tx, clock, &prepared.user).await?;
        users::insert_preferences(tx, clock, user.id, prepared.locale).await?;

        let admin_id = user.id.to_string();
        let updated_by = Some(admin_id.as_str());
        for (key, value) in [
            ("app_name", SettingValueInput::String(&prepared.app_name)),
            (
                "app_description",
                SettingValueInput::String(DEFAULT_APP_DESCRIPTION),
            ),
            (
                "default_locale",
                SettingValueInput::String(prepared.locale.as_str()),
            ),
            ("setup_completed", SettingValueInput::Boolean(true)),
        ] {
            self.settings
                .write_setting(tx, key, value, updated_by)
                .await?;
        }

        let event = AuditEvent::new(
            actions::setup_completed(),
            Actor::user(&admin_id, &user.username),
            Outcome::Success,
            Timestamp::try_from(clock.now())?,
        )
        .with_target(
            Target::new(TargetType::User)
                .id(&admin_id)
                .label(&user.username),
        )
        .with_client(audit_client.clone());
        audit_repo::insert(tx, clock, &event).await?;
        Ok(user)
    }
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, SetupError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| SetupError::HashingTask)?
        .map_err(SetupError::from)
}
