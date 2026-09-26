use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use super::error::CliError;
use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit;
use crate::features::audit::actions::{self, ActionSpec, AdminRecoverFacts, PasswordResetFacts};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{Actor, AuditEvent, Outcome, Target, TargetType};
use crate::features::audit::service::AuditService;
use crate::features::auth::error::LoginError;
use crate::features::auth::sessions::{RevokedReason, SessionError, SessionService};
use crate::features::auth::{lockout, login, trusted_devices};
use crate::features::settings::SettingsService;
use crate::features::users::error::UserError;
use crate::features::users::model::{NormalizedIdentifier, User, UserId};
use crate::features::users::repo as users;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, WriteTx};

pub const ADMIN_RECOVER_TRANSACTION: &str = "operator.admin_recover";
pub const PASSWORD_RESET_TRANSACTION: &str = "operator.user_reset_password";
pub const ADMIN_RECOVER_ACTOR: &str = "palmr admin recover";
pub const PASSWORD_RESET_ACTOR: &str = "palmr user reset-password";

#[derive(Debug)]
pub enum RecoveryError {
    UserNotFound,
    User(UserError),
    Login(LoginError),
    Session(SessionError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
    HashTask,
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserNotFound => f.write_str("the account no longer exists"),
            Self::User(error) => error.fmt(f),
            Self::Login(error) => error.fmt(f),
            Self::Session(error) => error.fmt(f),
            Self::Audit(error) => error.fmt(f),
            Self::Crypto(error) => error.fmt(f),
            Self::Db(error) => error.fmt(f),
            Self::Time(error) => error.fmt(f),
            Self::HashTask => f.write_str("the password hashing task did not complete"),
        }
    }
}

impl std::error::Error for RecoveryError {}

macro_rules! recovery_from {
    ($($source:ty => $variant:ident),* $(,)?) => {
        $(impl From<$source> for RecoveryError {
            fn from(error: $source) -> Self {
                Self::$variant(error)
            }
        })*
    };
}

recovery_from! {
    UserError => User,
    LoginError => Login,
    SessionError => Session,
    AuditError => Audit,
    CryptoError => Crypto,
    DbError => Db,
    InvalidTimestamp => Time,
}

impl From<sqlx::Error> for RecoveryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRecovered {
    pub user: UserId,
    pub username: String,
    pub facts: AdminRecoverFacts,
}

#[derive(Debug)]
pub struct PasswordReset {
    pub user: UserId,
    pub username: String,
    pub facts: PasswordResetFacts,
    temporary: Secret<String>,
}

pub async fn admin_recover(
    config: &OperatorConfig,
    selector: &str,
    clock: Arc<dyn Clock>,
    out: &mut dyn Write,
) -> Result<AdminRecovered, CliError> {
    let access = DataAccess::claim(config, Access::Exclusive, clock.as_ref())?;
    let outcome = match Recovery::open(config, &access, clock).await {
        Ok(recovery) => {
            let outcome = recovery.admin(selector).await;
            recovery.close().await;
            outcome
        }
        Err(error) => Err(error),
    };
    access.release();
    let recovered = outcome?;
    let _ = writeln!(out, "{}", Summary(&recovered)).and_then(|()| out.flush());
    Ok(recovered)
}

pub async fn user_reset_password(
    config: &OperatorConfig,
    id: UserId,
    clock: Arc<dyn Clock>,
    out: &mut dyn Write,
) -> Result<PasswordReset, CliError> {
    let access = DataAccess::claim(config, Access::Exclusive, clock.as_ref())?;
    let outcome = match Recovery::open(config, &access, clock).await {
        Ok(recovery) => {
            let outcome = recovery.reset_password(id).await;
            recovery.close().await;
            outcome
        }
        Err(error) => Err(error),
    };
    access.release();
    let reset = outcome?;
    deliver(out, &reset).map_err(|_| CliError::CredentialNotDelivered { user: reset.user })?;
    Ok(reset)
}

fn deliver(out: &mut dyn Write, reset: &PasswordReset) -> io::Result<()> {
    write!(
        out,
        "Password reset completed for user {} ({}): revoked {} session(s) and {} trusted device(s); lockout cleared: {}.\nTemporary password: {}\nThe user must change this password at the next login.\n",
        reset.user,
        reset.username,
        reset.facts.sessions_revoked,
        reset.facts.trusted_devices_revoked,
        yes_no(reset.facts.lockout_cleared),
        reset.temporary.expose_secret()
    )?;
    out.flush()
}

struct Summary<'a>(&'a AdminRecovered);

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let AdminRecovered {
            user,
            username,
            facts,
        } = self.0;
        write!(
            f,
            "Admin recovery completed for user {user} ({username}): the account is an active Admin; role changed: {}, reactivated: {}, lockout cleared: {}, password login re-enabled: {}, sessions revoked: {}.",
            yes_no(facts.role_changed),
            yes_no(facts.activated),
            yes_no(facts.lockout_cleared),
            yes_no(facts.password_login_reenabled),
            facts.sessions_revoked
        )
    }
}

const fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

struct Recovery {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsService,
    sessions: SessionService,
    audit: AuditService,
}

impl Recovery {
    async fn open(
        config: &OperatorConfig,
        access: &DataAccess,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, CliError> {
        access.existing_database()?;
        let pools = DbPools::open(
            access.root(),
            config.db_read_connections,
            config.db_synchronous,
        )
        .await
        .map_err(StartupError::from)?;
        match Self::assemble(config, access, &pools, Arc::clone(&clock)).await {
            Ok((settings, sessions, audit)) => Ok(Self {
                pools,
                clock,
                settings,
                sessions,
                audit,
            }),
            Err(error) => {
                let _ = pools.shutdown().await;
                Err(error.into())
            }
        }
    }

    async fn assemble(
        config: &OperatorConfig,
        access: &DataAccess,
        pools: &DbPools,
        clock: Arc<dyn Clock>,
    ) -> Result<(SettingsService, SessionService, AuditService), StartupError> {
        let (instance_key, _) = InstanceKey::load_or_create(access.root())?;
        let settings = SettingsService::load(pools, Arc::clone(&clock), &instance_key).await?;
        let sessions = SessionService::new(
            pools.clone(),
            Arc::clone(&clock),
            settings.handle(),
            settings.keys(),
            &config.base_url,
        );
        let (audit, _) = audit::channel(1, pools.clone(), clock);
        Ok((settings, sessions, audit))
    }

    async fn close(self) {
        drop(self.sessions);
        drop(self.settings);
        let _ = self.pools.shutdown().await;
    }

    async fn select(&self, selector: &str) -> Result<Option<UserId>, RecoveryError> {
        let reader = self.pools.reader();
        if let Ok(id) = selector.parse::<UserId>() {
            if let Some(user) = users::find_by_id(reader, id).await? {
                return Ok(Some(user.id));
            }
        }
        let identifier = NormalizedIdentifier::from_input(selector);
        if identifier.as_str().is_empty() {
            return Ok(None);
        }
        Ok(users::find_by_login_identifier(reader, &identifier)
            .await?
            .map(|user| user.id))
    }

    async fn admin(&self, selector: &str) -> Result<AdminRecovered, CliError> {
        let not_found = || CliError::UserNotFound {
            user: selector.to_owned(),
        };
        let target = self
            .select(selector)
            .await
            .map_err(|source| CliError::RecoveryFailed { source })?
            .ok_or_else(not_found)?;
        self.pools
            .write_tx(self.clock.as_ref(), ADMIN_RECOVER_TRANSACTION, async |tx| {
                self.commit_admin(tx, target).await
            })
            .await
            .map_err(|error| match error {
                RecoveryError::UserNotFound => not_found(),
                source => CliError::RecoveryFailed { source },
            })
    }

    async fn commit_admin(
        &self,
        tx: &mut WriteTx<'_>,
        target: UserId,
    ) -> Result<AdminRecovered, RecoveryError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let user = users::find_by_id_in_tx(tx, target)
            .await?
            .ok_or(RecoveryError::UserNotFound)?;
        let role_changed = user.role != Role::Admin;
        let activated = !user.is_active;
        users::recover_admin(tx, user.id, now).await?;
        let sessions_revoked = if role_changed {
            self.sessions
                .revoke_all_in_tx(tx, user.id, RevokedReason::RoleChanged)
                .await?
        } else {
            0
        };
        let lockout_cleared = lockout::clear(tx, user.id, None, now).await?;
        let password_login_reenabled = login::reenable_password_login_in_tx(tx).await?;
        let facts = AdminRecoverFacts {
            role_changed,
            activated,
            lockout_cleared,
            password_login_reenabled,
            sessions_revoked,
        };
        let event = operator_event(
            actions::operator_cli_admin_recover(facts),
            ADMIN_RECOVER_ACTOR,
            &user,
            now,
        );
        self.audit.record_in_tx(tx, &event).await?;
        Ok(AdminRecovered {
            user: user.id,
            username: user.username,
            facts,
        })
    }

    async fn reset_password(&self, id: UserId) -> Result<PasswordReset, CliError> {
        let not_found = || CliError::UserNotFound {
            user: id.to_string(),
        };
        let failed = |source| CliError::RecoveryFailed { source };
        users::find_by_id(self.pools.reader(), id)
            .await
            .map_err(|error| failed(error.into()))?
            .ok_or_else(not_found)?;
        let policy = AccountPasswordPolicy::from_settings(&self.settings.handle().load());
        let temporary = policy
            .temporary_password()
            .map_err(|error| failed(error.into()))?;
        policy
            .check(temporary.expose_secret())
            .map_err(|error| failed(error.into()))?;
        let hash = hash_off_runtime(temporary.clone()).await.map_err(failed)?;
        let (username, facts) = self
            .pools
            .write_tx(
                self.clock.as_ref(),
                PASSWORD_RESET_TRANSACTION,
                async |tx| self.commit_reset(tx, id, &hash).await,
            )
            .await
            .map_err(|error| match error {
                RecoveryError::UserNotFound => not_found(),
                source => failed(source),
            })?;
        Ok(PasswordReset {
            user: id,
            username,
            facts,
            temporary,
        })
    }

    async fn commit_reset(
        &self,
        tx: &mut WriteTx<'_>,
        id: UserId,
        hash: &Secret<String>,
    ) -> Result<(String, PasswordResetFacts), RecoveryError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let user = users::find_by_id_in_tx(tx, id)
            .await?
            .ok_or(RecoveryError::UserNotFound)?;
        users::replace_password_hash(tx, self.clock.as_ref(), user.id, hash).await?;
        users::set_must_change_password(tx, self.clock.as_ref(), user.id, true).await?;
        let sessions_revoked = self
            .sessions
            .revoke_all_in_tx(tx, user.id, RevokedReason::PasswordReset)
            .await?;
        let trusted_devices_revoked =
            trusted_devices::repo::revoke_all_in_tx(tx, user.id, now).await?;
        let lockout_cleared = lockout::clear(tx, user.id, None, now).await?;
        let facts = PasswordResetFacts {
            had_local_password: user.password_hash.is_some(),
            sessions_revoked,
            trusted_devices_revoked,
            lockout_cleared,
        };
        let event = operator_event(
            actions::operator_cli_password_reset(facts),
            PASSWORD_RESET_ACTOR,
            &user,
            now,
        );
        self.audit.record_in_tx(tx, &event).await?;
        Ok((user.username, facts))
    }
}

fn operator_event(spec: ActionSpec, actor: &str, user: &User, now: Timestamp) -> AuditEvent {
    AuditEvent::new(spec, Actor::operator_cli(actor), Outcome::Success, now).with_target(
        Target::new(TargetType::User)
            .id(&user.id.to_string())
            .label(&user.username),
    )
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, RecoveryError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| RecoveryError::HashTask)?
        .map_err(RecoveryError::from)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{deliver, AdminRecovered, PasswordReset, RecoveryError, Summary};
    use crate::cli::error::CliError;
    use crate::domain::secret::{Secret, REDACTED};
    use crate::features::audit::actions::{self, AdminRecoverFacts, PasswordResetFacts};
    use crate::features::users::model::UserId;
    use crate::features::users::service::AccountPasswordPolicy;
    use crate::infra::crypto::token::ENCODED_TOKEN_LEN;

    const ID: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d";
    const SENTINEL: &str = "temporary-password-sentinel-Qx7Lm2Vb9";

    fn reset() -> PasswordReset {
        PasswordReset {
            user: ID.parse().unwrap(),
            username: "ada".to_owned(),
            facts: PasswordResetFacts {
                had_local_password: false,
                sessions_revoked: 2,
                trusted_devices_revoked: 1,
                lockout_cleared: true,
            },
            temporary: Secret::new(SENTINEL.to_owned()),
        }
    }

    #[test]
    fn unit_temporary_password_meets_policy_with_full_entropy() {
        for configured in [0, 8, 43, 44, 64, 129] {
            let policy = AccountPasswordPolicy::with_configured_min_length(configured);
            let mut seen = HashSet::new();
            for _ in 0..16 {
                let password = policy.temporary_password().unwrap();
                let text = password.expose_secret();
                assert_eq!(
                    text.len(),
                    usize::try_from(policy.min_length())
                        .unwrap()
                        .max(ENCODED_TOKEN_LEN)
                );
                assert!(text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
                assert!(policy.check(text).is_ok());
                assert!(seen.insert(text.clone()), "a temporary password repeated");
            }
        }
        const { assert!(ENCODED_TOKEN_LEN * 6 >= 128 + 64) };
    }

    #[test]
    fn unit_operator_recovery_never_formats_the_credential() {
        let reset = reset();
        for rendered in [format!("{reset:?}"), format!("{reset:#?}")] {
            assert!(!rendered.contains(SENTINEL), "{rendered}");
            assert!(rendered.contains(REDACTED), "{rendered}");
        }

        let mut out = Vec::new();
        deliver(&mut out, &reset).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches(SENTINEL).count(), 1);
        assert_eq!(
            text,
            format!(
                "Password reset completed for user {ID} (ada): revoked 2 session(s) and 1 trusted device(s); lockout cleared: yes.\nTemporary password: {SENTINEL}\nThe user must change this password at the next login.\n"
            )
        );

        let user: UserId = ID.parse().unwrap();
        for error in [
            CliError::CredentialNotDelivered { user },
            CliError::UserNotFound {
                user: ID.to_owned(),
            },
            CliError::RecoveryFailed {
                source: RecoveryError::HashTask,
            },
        ] {
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(!rendered.contains(SENTINEL), "{rendered}");
            }
        }

        let recovered = AdminRecovered {
            user,
            username: "ada".to_owned(),
            facts: AdminRecoverFacts {
                role_changed: true,
                activated: true,
                lockout_cleared: true,
                password_login_reenabled: false,
                sessions_revoked: 3,
            },
        };
        assert_eq!(
            Summary(&recovered).to_string(),
            format!("Admin recovery completed for user {ID} (ada): the account is an active Admin; role changed: yes, reactivated: yes, lockout cleared: yes, password login re-enabled: no, sessions revoked: 3.")
        );

        for spec in [
            actions::operator_cli_password_reset(reset.facts),
            actions::operator_cli_admin_recover(recovered.facts),
        ] {
            assert!(spec.action().as_str().starts_with("OPERATOR_CLI_"));
            assert!(!spec.metadata().as_str().contains(SENTINEL));
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                actions::operator_cli_password_reset(reset.facts)
                    .metadata()
                    .as_str()
            )
            .unwrap(),
            serde_json::json!({
                "had_local_password": false,
                "sessions_revoked": 2,
                "trusted_devices_revoked": 1,
                "lockout_cleared": true,
            })
        );
    }
}
