use std::sync::Arc;

use crate::domain::clock::Clock;
use crate::domain::email::Email;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::lockout;
use crate::features::auth::login::password_login_enabled;
use crate::features::auth::repo as accounts;
use crate::features::auth::sessions::RevokedReason;
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::auth::AuthService;
use crate::features::email::model::{LocalePreference, MailKind, MailParams, NewMail, Recipient};
use crate::features::email::EmailService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{NormalizedIdentifier, User};
use crate::features::users::repo as users;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::ratelimit::NormalizedAccount;

use super::error::PasswordResetError;
use super::model::{
    ForgotInput, PasswordResetTokenId, PresentedToken, ResetCheckResponse, ResetInput, TokenState,
};
use super::repo::{self, NewResetToken};

pub const ISSUE_TRANSACTION: &str = "auth.password_reset.issue";
pub const CONSUME_TRANSACTION: &str = "auth.password_reset.consume";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountBudget {
    Available,
    Exhausted,
}

#[derive(Clone)]
pub struct PasswordResetService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    auth: AuthService,
    email: EmailService,
    audit: AuditService,
}

struct Issuance {
    token: Secret<String>,
    digest: crate::infra::crypto::hash::TokenDigest,
    validity_minutes: u32,
    issuing: bool,
}

impl PasswordResetService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        auth: AuthService,
        email: EmailService,
        audit: AuditService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            auth,
            email,
            audit,
        }
    }

    pub async fn forgot(
        &self,
        input: ForgotInput,
        budget: AccountBudget,
        client: &ClientMetadata,
    ) -> Result<(), PasswordResetError> {
        let settings = self.settings.load();
        if !settings.smtp.is_available() {
            return Err(PasswordResetError::SmtpUnavailable);
        }
        let issuing = password_login_enabled(&settings) && budget == AccountBudget::Available;
        let validity_minutes = settings.security.password_reset_validity_minutes.max(1);
        drop(settings);

        let token = Token::mint()?;
        let issuance = Issuance {
            token: token.encode(),
            digest: token.digest(),
            validity_minutes,
            issuing,
        };
        drop(token);
        self.pools
            .write_tx(self.clock.as_ref(), ISSUE_TRANSACTION, async |tx| {
                self.issue_in_tx(tx, &input.identifier, &issuance, client)
                    .await
            })
            .await
    }

    async fn issue_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        identifier: &NormalizedIdentifier,
        issuance: &Issuance,
        client: &ClientMetadata,
    ) -> Result<(), PasswordResetError> {
        let candidate = users::find_by_login_identifier_in_tx(tx, identifier).await?;
        let Some(user) = candidate.filter(|user| issuance.issuing && eligible(user)) else {
            return Ok(());
        };
        let now = Timestamp::try_from(self.clock.now())?;
        let expires_at = Timestamp::try_from(
            now.get() + time::Duration::minutes(i64::from(issuance.validity_minutes)),
        )?;
        repo::invalidate_outstanding(tx, user.id, now).await?;
        repo::insert(
            tx,
            &NewResetToken {
                id: PasswordResetTokenId::generate(self.clock.as_ref()),
                user_id: user.id,
                token_hash: &issuance.digest,
                created_at: now,
                expires_at,
                requested_ip: client.client_ip(),
            },
        )
        .await?;
        let locale = accounts::account(&mut *tx.executor(), user.id)
            .await?
            .and_then(|account| account.locale.parse::<LocaleCode>().ok())
            .map_or(LocalePreference::InstanceDefault, LocalePreference::Account);
        let email = Email::parse(&user.email)
            .map_err(|_| PasswordResetError::RepositoryInvariant { column: "email" })?;
        let mail = NewMail::new(
            MailKind::PasswordReset,
            Recipient::new(email, display_name(&user).as_deref()),
            locale,
            MailParams::PasswordReset {
                expiry_minutes: issuance.validity_minutes,
            },
        )
        .with_token(issuance.token.clone());
        self.email.enqueue(tx, mail).await?;
        Ok(())
    }

    pub async fn rate_limit_account(
        &self,
        identifier: &NormalizedIdentifier,
    ) -> Result<NormalizedAccount, PasswordResetError> {
        let account = users::find_by_login_identifier(self.pools.reader(), identifier).await?;
        Ok(account.map_or_else(
            || NormalizedAccount::new(identifier.as_str()),
            |user| NormalizedAccount::resolved(user.id.to_string().as_bytes()),
        ))
    }

    pub async fn check(
        &self,
        token: &PresentedToken,
    ) -> Result<ResetCheckResponse, PasswordResetError> {
        let expires_at = self.live_token(token).await?;
        let min_length = AccountPasswordPolicy::from_settings(&self.settings.load()).min_length();
        Ok(ResetCheckResponse {
            valid: true,
            password_min_length: min_length,
            expires_at: expires_at.to_string(),
        })
    }

    pub async fn reset(
        &self,
        input: ResetInput,
        client: &ClientMetadata,
    ) -> Result<(), PasswordResetError> {
        let settings = self.settings.load();
        let enabled = password_login_enabled(&settings);
        let policy = AccountPasswordPolicy::from_settings(&settings);
        drop(settings);
        if !enabled {
            return Err(PasswordResetError::PasswordLoginDisabled);
        }
        self.live_token(&input.token).await?;
        policy.check(input.new_password.expose_secret())?;
        let replacement = hash_off_runtime(input.new_password).await?;
        self.pools
            .write_tx(self.clock.as_ref(), CONSUME_TRANSACTION, async |tx| {
                self.consume_in_tx(tx, &input.token, &replacement, client)
                    .await
            })
            .await
    }

    async fn consume_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        token: &PresentedToken,
        replacement: &Secret<String>,
        client: &ClientMetadata,
    ) -> Result<(), PasswordResetError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let Some(consumed) = repo::consume(tx, token.digest(), now).await? else {
            let stored = repo::find(&mut *tx.executor(), token.digest()).await?;
            return Err(stored.map_or(PasswordResetError::TokenInvalid, |stored| {
                TokenState::classify(&stored, now)
                    .into_result()
                    .err()
                    .unwrap_or(PasswordResetError::TokenInvalid)
            }));
        };
        let user = users::find_by_id_in_tx(tx, consumed.user_id)
            .await?
            .ok_or(PasswordResetError::TokenInvalid)?;
        if !repo::set_password(tx, user.id, replacement, now).await? {
            return Err(PasswordResetError::TokenInvalid);
        }
        repo::invalidate_outstanding(tx, user.id, now).await?;
        let sessions_revoked = self
            .auth
            .sessions()
            .revoke_all_in_tx(tx, user.id, RevokedReason::PasswordReset)
            .await?;
        let devices_revoked = trusted_devices::revoke_all_in_tx(tx, user.id, now).await?;
        let lockout_cleared = lockout::clear(tx, user.id, None, now).await?;
        let event = AuditEvent::new(
            actions::password_reset_completed(sessions_revoked, devices_revoked, lockout_cleared),
            Actor::user(&user.id.to_string(), &user.username),
            Outcome::Success,
            now,
        )
        .with_target(
            Target::new(TargetType::User)
                .id(&user.id.to_string())
                .label(&user.username),
        )
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    async fn live_token(&self, token: &PresentedToken) -> Result<Timestamp, PasswordResetError> {
        let now = Timestamp::try_from(self.clock.now())?;
        repo::find(self.pools.reader().executor(), token.digest())
            .await?
            .map_or(Err(PasswordResetError::TokenInvalid), |stored| {
                TokenState::classify(&stored, now).into_result()
            })
    }
}

const fn eligible(user: &User) -> bool {
    user.is_active && user.password_hash.is_some()
}

fn display_name(user: &User) -> Option<String> {
    let full = format!("{} {}", user.first_name.trim(), user.last_name.trim());
    let full = full.trim();
    if full.is_empty() {
        None
    } else {
        Some(full.to_owned())
    }
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, PasswordResetError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| PasswordResetError::HashTask)?
        .map_err(PasswordResetError::from)
}
