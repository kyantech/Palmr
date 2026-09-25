use std::sync::Arc;

use crate::domain::clock::Clock;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditCode, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::{
    AuthMethod, AuthenticatedPrincipal, MintedSession, PreparedSessionCredentials, RevokedReason,
    SessionClient, SessionError, SessionRestriction, SessionService,
};
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{User, UserId};
use crate::features::users::repo as users;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::ratelimit::RetryAfter;

use super::error::LoginError;
use super::lockout::{
    self, AttemptClient, AttemptMethod, AttemptResult, FailureOutcome, LockState, LockoutPolicy,
    LoginAttempt,
};
use super::login::{password_login_enabled, CredentialProof, CredentialVerifier, LoginInput};
use super::model::{AccountView, LoginResponse, MeParts, MeResponse};
use super::repo;

pub const LOGIN_TRANSACTION: &str = "auth.login";
pub const LOGIN_FAILURE_TRANSACTION: &str = "auth.login_failed";
pub const LOGOUT_TRANSACTION: &str = "auth.logout";

#[derive(Clone)]
pub struct AuthService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    sessions: SessionService,
    audit: AuditService,
    verifier: Arc<CredentialVerifier>,
}

pub struct LoginContext {
    pub session: SessionClient,
    pub attempt: AttemptClient,
    pub audit: ClientMetadata,
    pub presented_session: Option<TokenDigest>,
}

pub struct LoggedIn {
    pub response: LoginResponse,
    pub session: MintedSession,
}

pub struct IssueSession<'a> {
    pub user_id: UserId,
    pub method: AuthMethod,
    pub client: SessionClient,
    pub credentials: &'a PreparedSessionCredentials,
    pub verified_password_hash: Option<&'a Secret<String>>,
    pub replaces: Option<&'a TokenDigest>,
}

pub enum SessionIssue {
    Issued {
        user: Box<User>,
        session: MintedSession,
        restriction: SessionRestriction,
    },
    Refused(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Inactive,
    CredentialChanged,
    Locked { until: Timestamp },
}

enum LoginOutcome {
    Issued {
        account: AccountView,
        session: MintedSession,
        restriction: SessionRestriction,
    },
    Refused {
        user: User,
        refusal: Refusal,
        lock: Option<LockState>,
    },
}

enum FailureAudit {
    Unknown,
    Known {
        user: Box<User>,
        result: AttemptResult,
        locked_now: Option<LockState>,
    },
}

impl AuthService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        sessions: SessionService,
        audit: AuditService,
    ) -> Result<Self, CryptoError> {
        Ok(Self {
            pools,
            clock,
            settings,
            sessions,
            audit,
            verifier: Arc::new(CredentialVerifier::new()?),
        })
    }

    pub const fn sessions(&self) -> &SessionService {
        &self.sessions
    }

    pub fn verifications_performed(&self) -> u64 {
        self.verifier.performed()
    }

    pub async fn login(
        &self,
        input: LoginInput,
        context: LoginContext,
    ) -> Result<LoggedIn, LoginError> {
        let settings = self.settings.load();
        let enabled = password_login_enabled(&settings);
        let policy = LockoutPolicy::from_settings(&settings);
        drop(settings);
        if !enabled {
            self.record_disabled(&input, &context).await?;
            return Err(LoginError::PasswordLoginDisabled);
        }

        let user = users::find_by_login_identifier(self.pools.reader(), &input.identifier).await?;
        let stored = user.as_ref().and_then(|user| user.password_hash.clone());
        let proof = self
            .verify_off_runtime(input.password.clone(), stored.clone())
            .await?;
        let (user, stored, needs_rehash) = match (user, stored, proof) {
            (Some(user), Some(stored), CredentialProof::Verified { needs_rehash }) => {
                (user, stored, needs_rehash)
            }
            (user, _, _) => {
                self.record_failure(user, &input, &context, policy).await?;
                return Err(LoginError::InvalidCredentials);
            }
        };

        let upgraded = if needs_rehash && user.is_active {
            rehash_off_runtime(input.password.clone()).await
        } else {
            None
        };
        let credentials = self.sessions.prepare_credentials()?;
        let outcome = self
            .pools
            .write_tx(self.clock.as_ref(), LOGIN_TRANSACTION, async |tx| {
                self.complete_login(
                    tx,
                    &user,
                    &stored,
                    upgraded.as_ref(),
                    &credentials,
                    &context,
                    &input,
                    policy,
                )
                .await
            })
            .await?;

        match outcome {
            LoginOutcome::Issued {
                account,
                session,
                restriction,
            } => {
                self.audit_success(&account, &context.audit);
                Ok(LoggedIn {
                    response: LoginResponse::new(&account, restriction),
                    session,
                })
            }
            LoginOutcome::Refused {
                user,
                refusal,
                lock,
            } => {
                let now = Timestamp::try_from(self.clock.now())?;
                match refusal {
                    Refusal::Locked { until } => {
                        self.audit_refused(&user, AttemptResult::LockedOut, &context.audit, now);
                        Err(LoginError::Locked {
                            retry_after: RetryAfter::covering(remaining(now, until)),
                        })
                    }
                    Refusal::Inactive => {
                        self.audit_refused(&user, AttemptResult::Inactive, &context.audit, now);
                        Err(LoginError::InvalidCredentials)
                    }
                    Refusal::CredentialChanged => {
                        self.audit_failure(
                            FailureAudit::Known {
                                user: Box::new(user),
                                result: AttemptResult::BadCredentials,
                                locked_now: lock,
                            },
                            &input.submitted,
                            &context.audit,
                            policy,
                        );
                        Err(LoginError::InvalidCredentials)
                    }
                }
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the successful-login transaction binds every fact the credential step proved"
    )]
    async fn complete_login(
        &self,
        tx: &mut WriteTx<'_>,
        user: &User,
        stored: &Secret<String>,
        upgraded: Option<&Secret<String>>,
        credentials: &PreparedSessionCredentials,
        context: &LoginContext,
        input: &LoginInput,
        policy: LockoutPolicy,
    ) -> Result<LoginOutcome, LoginError> {
        let issue = self
            .issue_session_in_tx(
                tx,
                IssueSession {
                    user_id: user.id,
                    method: AuthMethod::Password,
                    client: context.session.clone(),
                    credentials,
                    verified_password_hash: Some(stored),
                    replaces: context.presented_session.as_ref(),
                },
            )
            .await?;
        match issue {
            SessionIssue::Issued {
                user: current,
                session,
                restriction,
            } => {
                if let Some(upgraded) = upgraded {
                    users::upgrade_password_hash(tx, current.id, stored, upgraded).await?;
                }
                self.attempt(tx, input, Some(current.id), AttemptResult::Success, context)
                    .await?;
                let account = repo::account(&mut *tx.executor(), current.id)
                    .await?
                    .ok_or(LoginError::Session(SessionError::AuthRequired))?;
                Ok(LoginOutcome::Issued {
                    account,
                    session,
                    restriction,
                })
            }
            SessionIssue::Refused(refusal) => {
                let (result, lock) = match refusal {
                    Refusal::Inactive => (AttemptResult::Inactive, None),
                    Refusal::Locked { .. } => (AttemptResult::LockedOut, None),
                    Refusal::CredentialChanged => {
                        let now = Timestamp::try_from(self.clock.now())?;
                        let lock = match lockout::record_failure(tx, user.id, now, policy).await? {
                            FailureOutcome::LockedNow(state) => Some(state),
                            FailureOutcome::Counted(_) => None,
                        };
                        (AttemptResult::BadCredentials, lock)
                    }
                };
                self.attempt(tx, input, Some(user.id), result, context)
                    .await?;
                Ok(LoginOutcome::Refused {
                    user: user.clone(),
                    refusal,
                    lock,
                })
            }
        }
    }

    pub async fn issue_session_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        request: IssueSession<'_>,
    ) -> Result<SessionIssue, LoginError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let Some(user) = users::find_by_id_in_tx(tx, request.user_id).await? else {
            return Ok(SessionIssue::Refused(Refusal::Inactive));
        };
        if !user.is_active {
            return Ok(SessionIssue::Refused(Refusal::Inactive));
        }
        if let Some(verified) = request.verified_password_hash {
            let unchanged = user
                .password_hash
                .as_ref()
                .is_some_and(|current| current.expose_secret() == verified.expose_secret());
            if !unchanged {
                return Ok(SessionIssue::Refused(Refusal::CredentialChanged));
            }
        }
        if let Some(until) = lockout::state_in_tx(tx, user.id)
            .await?
            .and_then(|state| state.active_until(now))
        {
            return Ok(SessionIssue::Refused(Refusal::Locked { until }));
        }
        if user.totp_enabled && !proves_second_factor(request.method) {
            return Err(LoginError::SecondFactorUnavailable);
        }

        lockout::reset(tx, user.id, now).await?;
        users::record_login(tx, user.id, now).await?;
        if let Some(presented) = request.replaces {
            self.sessions
                .revoke_presented_in_tx(tx, presented, RevokedReason::Rotated)
                .await?;
        }
        let session = self
            .sessions
            .mint_in_tx(
                tx,
                request.client.session(user.id, request.method),
                request.credentials,
            )
            .await?;
        let restriction = self
            .sessions
            .restriction_for(user.must_change_password, user.totp_enabled);
        Ok(SessionIssue::Issued {
            user: Box::new(user),
            session,
            restriction,
        })
    }

    pub async fn logout(
        &self,
        principal: &AuthenticatedPrincipal,
        client: &ClientMetadata,
    ) -> Result<(), LoginError> {
        let revoked = self
            .pools
            .write_tx(
                self.clock.as_ref(),
                LOGOUT_TRANSACTION,
                async |tx| match self
                    .sessions
                    .revoke_one_in_tx(
                        tx,
                        principal.user_id,
                        principal.session_id,
                        RevokedReason::Logout,
                    )
                    .await
                {
                    Ok(revoked) => Ok(revoked),
                    Err(SessionError::NotFound) => Ok(false),
                    Err(error) => Err(error),
                },
            )
            .await?;
        if revoked {
            let event = AuditEvent::new(
                actions::logout(),
                Actor::user(&principal.user_id.to_string(), &principal.username),
                Outcome::Success,
                Timestamp::try_from(self.clock.now())?,
            )
            .with_target(Target::new(TargetType::Session).id(&principal.session_id.to_string()))
            .with_client(client.clone());
            self.audit.record_async(event);
        }
        Ok(())
    }

    pub async fn me(&self, principal: &AuthenticatedPrincipal) -> Result<MeResponse, LoginError> {
        let account = repo::account(self.pools.reader().executor(), principal.user_id)
            .await?
            .ok_or(LoginError::Session(SessionError::AuthRequired))?;
        let session = self.sessions.current(principal).await?;
        let recent_auth_until = self.sessions.recent_auth_until(principal.last_auth_at)?;
        let password_login_enabled = password_login_enabled(&self.settings.load());
        Ok(MeResponse::new(MeParts {
            account,
            session,
            recent_auth_until,
            restriction: principal.restriction,
            password_login_enabled,
        }))
    }

    async fn verify_off_runtime(
        &self,
        password: Secret<String>,
        stored: Option<Secret<String>>,
    ) -> Result<CredentialProof, LoginError> {
        let verifier = Arc::clone(&self.verifier);
        tokio::task::spawn_blocking(move || verifier.verify(&password, stored.as_ref()))
            .await
            .map_err(|_| LoginError::VerificationTask)
    }

    async fn record_disabled(
        &self,
        input: &LoginInput,
        context: &LoginContext,
    ) -> Result<(), LoginError> {
        self.pools
            .write_tx(self.clock.as_ref(), LOGIN_FAILURE_TRANSACTION, async |tx| {
                self.attempt(
                    tx,
                    input,
                    None,
                    AttemptResult::PasswordAuthDisabled,
                    context,
                )
                .await
            })
            .await
    }

    async fn record_failure(
        &self,
        user: Option<User>,
        input: &LoginInput,
        context: &LoginContext,
        policy: LockoutPolicy,
    ) -> Result<(), LoginError> {
        let audit = self
            .pools
            .write_tx(self.clock.as_ref(), LOGIN_FAILURE_TRANSACTION, async |tx| {
                let Some(user) = user else {
                    self.attempt(tx, input, None, AttemptResult::UnknownIdentifier, context)
                        .await?;
                    return Ok::<_, LoginError>(FailureAudit::Unknown);
                };
                let now = Timestamp::try_from(self.clock.now())?;
                let locked = lockout::state_in_tx(tx, user.id)
                    .await?
                    .is_some_and(|state| state.active_until(now).is_some());
                if locked {
                    self.attempt(tx, input, Some(user.id), AttemptResult::LockedOut, context)
                        .await?;
                    return Ok(FailureAudit::Known {
                        user: Box::new(user),
                        result: AttemptResult::LockedOut,
                        locked_now: None,
                    });
                }
                self.attempt(
                    tx,
                    input,
                    Some(user.id),
                    AttemptResult::BadCredentials,
                    context,
                )
                .await?;
                let locked_now = match lockout::record_failure(tx, user.id, now, policy).await? {
                    FailureOutcome::LockedNow(state) => Some(state),
                    FailureOutcome::Counted(_) => None,
                };
                Ok(FailureAudit::Known {
                    user: Box::new(user),
                    result: AttemptResult::BadCredentials,
                    locked_now,
                })
            })
            .await?;
        self.audit_failure(audit, &input.submitted, &context.audit, policy);
        Ok(())
    }

    async fn attempt(
        &self,
        tx: &mut WriteTx<'_>,
        input: &LoginInput,
        user_id: Option<UserId>,
        result: AttemptResult,
        context: &LoginContext,
    ) -> Result<(), LoginError> {
        lockout::record_attempt(
            tx,
            self.clock.as_ref(),
            &LoginAttempt {
                identifier: &input.identifier,
                user_id,
                method: AttemptMethod::Password,
                result,
                client: &context.attempt,
            },
        )
        .await
    }

    fn audit_success(&self, account: &AccountView, client: &ClientMetadata) {
        let Ok(now) = Timestamp::try_from(self.clock.now()) else {
            return;
        };
        let event = AuditEvent::new(
            actions::login_succeeded(AuthMethod::Password.as_str()),
            Actor::user(&account.id.to_string(), &account.username),
            Outcome::Success,
            now,
        )
        .with_target(user_target(&account.id, &account.username))
        .with_client(client.clone());
        self.audit.record_async(event);
    }

    fn audit_refused(
        &self,
        user: &User,
        result: AttemptResult,
        client: &ClientMetadata,
        now: Timestamp,
    ) {
        let code = match result {
            AttemptResult::LockedOut => AuditCode::AuthLocked,
            _ => AuditCode::AuthInvalidCredentials,
        };
        let event = AuditEvent::new(
            actions::login_failed(AttemptMethod::Password.as_str(), result.as_str()),
            Actor::user(&user.id.to_string(), &user.username),
            Outcome::Denied(code),
            now,
        )
        .with_target(user_target(&user.id, &user.username))
        .with_client(client.clone());
        self.audit.record_async(event);
    }

    fn audit_failure(
        &self,
        audit: FailureAudit,
        submitted: &str,
        client: &ClientMetadata,
        policy: LockoutPolicy,
    ) {
        let Ok(now) = Timestamp::try_from(self.clock.now()) else {
            return;
        };
        let method = AttemptMethod::Password.as_str();
        match audit {
            FailureAudit::Unknown => {
                let event = AuditEvent::new(
                    actions::login_failed(method, AttemptResult::UnknownIdentifier.as_str()),
                    Actor::anonymous(submitted),
                    Outcome::Failure(AuditCode::AuthInvalidCredentials),
                    now,
                )
                .with_client(client.clone());
                self.audit.record_async(event);
            }
            FailureAudit::Known {
                user,
                result,
                locked_now,
            } => {
                let actor = Actor::user(&user.id.to_string(), &user.username);
                let target = user_target(&user.id, &user.username);
                let event = AuditEvent::new(
                    actions::login_failed(method, result.as_str()),
                    actor.clone(),
                    Outcome::Failure(AuditCode::AuthInvalidCredentials),
                    now,
                )
                .with_target(target.clone())
                .with_client(client.clone());
                self.audit.record_async(event);
                if let Some(state) = locked_now {
                    let event = AuditEvent::new(
                        actions::login_locked_out(
                            state.failed_count,
                            state.lock_count,
                            policy.minutes(),
                        ),
                        actor,
                        Outcome::Denied(AuditCode::AuthLocked),
                        now,
                    )
                    .with_target(target)
                    .with_client(client.clone());
                    self.audit.record_async(event);
                }
            }
        }
    }
}

const fn proves_second_factor(method: AuthMethod) -> bool {
    matches!(
        method,
        AuthMethod::PasswordTotp
            | AuthMethod::PasswordBackupCode
            | AuthMethod::PasswordTrustedDevice
            | AuthMethod::External
    )
}

fn user_target(id: &UserId, username: &str) -> Target {
    Target::new(TargetType::User)
        .id(&id.to_string())
        .label(username)
}

fn remaining(now: Timestamp, until: Timestamp) -> std::time::Duration {
    std::time::Duration::try_from(until.get() - now.get()).unwrap_or(std::time::Duration::ZERO)
}

async fn rehash_off_runtime(password: Secret<String>) -> Option<Secret<String>> {
    match tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
    {
        Ok(Ok(hash)) => Some(hash),
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("the stored password hash could not be upgraded on login");
            None
        }
    }
}
