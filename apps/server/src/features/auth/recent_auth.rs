use serde::Deserialize;
use utoipa::ToSchema;

use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::{AuthenticatedPrincipal, SessionError};
use crate::features::users::model::{NormalizedIdentifier, User};
use crate::features::users::repo as users;
use crate::infra::db::WriteTx;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::error::LoginError;
use super::lockout::{self, AttemptClient, AttemptResult, LockoutPolicy};
use super::login::CredentialProof;
use super::service::{AuthService, FailureAudit};

pub const REAUTH_TRANSACTION: &str = "auth.reauthenticate";
pub const REAUTH_FAILURE_TRANSACTION: &str = "auth.reauthenticate_failed";

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReauthenticateRequest {
    #[schema(format = Password)]
    pub password: Option<String>,
    #[schema(example = "492013")]
    #[expect(dead_code, reason = "verified by TOTP-aware re-authentication in M10")]
    pub totp_code: Option<String>,
}

impl JsonRequest for ReauthenticateRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("password", JsonKind::String),
        JsonField::optional("totpCode", JsonKind::String),
    ];
}

pub struct ReauthContext {
    pub attempt: AttemptClient,
    pub audit: ClientMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReauthMethod {
    Password,
    PasswordTotp,
    External,
}

impl ReauthMethod {
    pub fn for_user(user: &User) -> Self {
        if user.password_hash.is_none() {
            Self::External
        } else if user.totp_enabled {
            Self::PasswordTotp
        } else {
            Self::Password
        }
    }
}

enum Proven {
    Stamped,
    Refused(Refused),
}

enum Refused {
    Failure(FailureAudit),
    Locked(Box<User>),
}

impl AuthService {
    pub async fn reauthenticate(
        &self,
        principal: &AuthenticatedPrincipal,
        request: ReauthenticateRequest,
        context: ReauthContext,
    ) -> Result<(), LoginError> {
        let policy = LockoutPolicy::from_settings(&self.settings.load());
        let user = users::find_by_id(self.pools.reader(), principal.user_id)
            .await?
            .filter(|user| user.is_active)
            .ok_or(LoginError::Session(SessionError::AuthRequired))?;
        let Some(stored) = user.password_hash.clone() else {
            return Err(LoginError::ExternalReauthUnavailable);
        };
        let password = match request.password {
            Some(password) if !password.is_empty() => Secret::new(password),
            _ => {
                return Err(LoginError::Invalid {
                    fields: vec!["password"],
                })
            }
        };
        let identifier = NormalizedIdentifier::from_input(&user.username);

        let proof = self
            .verify_off_runtime(password, Some(stored.clone()))
            .await?;
        if !matches!(proof, CredentialProof::Verified { .. }) {
            let audit = self
                .pools
                .write_tx(
                    self.clock.as_ref(),
                    REAUTH_FAILURE_TRANSACTION,
                    async |tx| {
                        self.count_failure_in_tx(
                            tx,
                            user.clone(),
                            &identifier,
                            &context.attempt,
                            policy,
                        )
                        .await
                    },
                )
                .await?;
            self.audit_failure(audit, &user.username, &context.audit, policy);
            return Err(LoginError::InvalidCredentials);
        }

        let proven = self
            .pools
            .write_tx(self.clock.as_ref(), REAUTH_TRANSACTION, async |tx| {
                self.stamp_in_tx(tx, principal, &user, &stored, &identifier, &context, policy)
                    .await
            })
            .await?;
        match proven {
            Proven::Stamped => Ok(()),
            Proven::Refused(Refused::Failure(audit)) => {
                self.audit_failure(audit, &user.username, &context.audit, policy);
                Err(LoginError::InvalidCredentials)
            }
            Proven::Refused(Refused::Locked(user)) => {
                let now = Timestamp::try_from(self.clock.now())?;
                self.audit_refused(&user, AttemptResult::LockedOut, &context.audit, now);
                Err(LoginError::InvalidCredentials)
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the stamp transaction re-validates every fact the credential step proved"
    )]
    async fn stamp_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        principal: &AuthenticatedPrincipal,
        verified: &User,
        stored: &Secret<String>,
        identifier: &NormalizedIdentifier,
        context: &ReauthContext,
        policy: LockoutPolicy,
    ) -> Result<Proven, LoginError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let current = users::find_by_id_in_tx(tx, verified.id)
            .await?
            .filter(|user| user.is_active)
            .ok_or(LoginError::Session(SessionError::AuthRequired))?;
        let unchanged = current
            .password_hash
            .as_ref()
            .is_some_and(|hash| hash.expose_secret() == stored.expose_secret());
        if !unchanged {
            let audit = self
                .count_failure_in_tx(tx, current, identifier, &context.attempt, policy)
                .await?;
            return Ok(Proven::Refused(Refused::Failure(audit)));
        }
        if lockout::state_in_tx(tx, current.id)
            .await?
            .is_some_and(|state| state.active_until(now).is_some())
        {
            self.attempt(
                tx,
                identifier,
                Some(current.id),
                AttemptResult::LockedOut,
                &context.attempt,
            )
            .await?;
            return Ok(Proven::Refused(Refused::Locked(Box::new(current))));
        }
        if ReauthMethod::for_user(&current) != ReauthMethod::Password {
            return Err(LoginError::SecondFactorUnavailable);
        }

        self.sessions()
            .mark_reauthenticated_in_tx(tx, principal.session_id)
            .await?;
        lockout::reset(tx, current.id, now).await?;
        self.attempt(
            tx,
            identifier,
            Some(current.id),
            AttemptResult::Success,
            &context.attempt,
        )
        .await?;
        Ok(Proven::Stamped)
    }
}
