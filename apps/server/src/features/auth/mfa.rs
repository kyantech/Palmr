use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::{
    AuthMethod, MfaChallenge, MintedSession, PendingSession, PreparedSessionCredentials,
    SessionClient, SessionError, SessionRestriction,
};
use crate::features::auth::totp::model::CodeCheck;
use crate::features::auth::totp::repo as totp;
use crate::features::auth::totp::service::consume_active_code_in_tx;
use crate::features::users::model::{NormalizedIdentifier, User};
use crate::features::users::repo as users;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::crypto::totp::{backup_code_digest, TotpCode};
use crate::infra::db::WriteTx;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::infra::ratelimit::RetryAfter;

use super::error::LoginError;
use super::lockout::{self, AttemptClient, AttemptMethod, AttemptResult, LockState, LockoutPolicy};
use super::model::{AccountView, LoginResponse};
use super::repo;
use super::service::{remaining, AuthService, IssueSession, LoggedIn, Refusal, SessionIssue};
use super::trusted_devices::model::{IssuedDevice, PreparedDevice};

pub const LOGIN_TOTP_TRANSACTION: &str = "auth.login_totp";

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginTotpRequest {
    #[schema(format = Password)]
    pub mfa_token: String,
    #[schema(example = "492013")]
    pub code: String,
    /// Set `palmr_device` on success; refused with `TRUSTED_DEVICE_DISABLED` while the policy disables trusted devices.
    #[serde(default)]
    pub remember_device: Option<bool>,
}

impl JsonRequest for LoginTotpRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("mfaToken", JsonKind::String),
        JsonField::required("code", JsonKind::String),
        JsonField::optional("rememberDevice", JsonKind::Boolean),
    ];
}

enum SecondFactorCode {
    Totp(TotpCode),
    BackupCode(TokenDigest),
    Malformed,
}

impl SecondFactorCode {
    fn parse(input: &str) -> Self {
        if let Some(code) = TotpCode::parse(input) {
            Self::Totp(code)
        } else if let Some(digest) = backup_code_digest(input) {
            Self::BackupCode(digest)
        } else {
            Self::Malformed
        }
    }

    const fn attempt_method(&self) -> AttemptMethod {
        match self {
            Self::BackupCode(_) => AttemptMethod::BackupCode,
            Self::Totp(_) | Self::Malformed => AttemptMethod::Totp,
        }
    }
}

pub struct LoginTotpInput {
    mfa_token_hash: Option<TokenDigest>,
    code: SecondFactorCode,
    remember_device: bool,
}

impl LoginTotpInput {
    pub fn parse(request: LoginTotpRequest) -> Result<Self, LoginError> {
        let mut invalid = Vec::new();
        if request.mfa_token.is_empty() {
            invalid.push("mfaToken");
        }
        if request.code.is_empty() {
            invalid.push("code");
        }
        if !invalid.is_empty() {
            return Err(LoginError::Invalid { fields: invalid });
        }
        Ok(Self {
            mfa_token_hash: Token::decode(&request.mfa_token)
                .ok()
                .map(|token| token.digest()),
            code: SecondFactorCode::parse(&request.code),
            remember_device: request.remember_device.unwrap_or(false),
        })
    }
}

struct PreparedCompletion {
    credentials: PreparedSessionCredentials,
    device: Option<PreparedDevice>,
}

pub struct MfaContext {
    pub session: SessionClient,
    pub attempt: AttemptClient,
    pub audit: ClientMetadata,
    pub presented_session: Option<TokenDigest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecondFactorMethod {
    Totp,
    BackupCode,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaChallengeDetails {
    /// Opaque single-use challenge; carried back only in the body of `POST /api/v1/auth/login/totp`.
    mfa_token: String,
    expires_at: String,
    methods: [SecondFactorMethod; 2],
    trusted_device_offered: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaChallengePayload {
    code: ErrorCode,
    #[schema(value_type = String)]
    message: &'static str,
    request_id: String,
    details: MfaChallengeDetails,
}

#[derive(Serialize, ToSchema)]
pub struct MfaChallengeBody {
    error: MfaChallengePayload,
}

impl MfaChallengeBody {
    pub fn new(
        challenge: &MfaChallenge,
        trusted_device_offered: bool,
        request_id: Option<&str>,
    ) -> Self {
        let code = ErrorCode::Auth2faRequired;
        Self {
            error: MfaChallengePayload {
                code,
                message: code.default_message(),
                request_id: request_id.unwrap_or_default().to_owned(),
                details: MfaChallengeDetails {
                    mfa_token: challenge.mfa_token.expose_secret().clone(),
                    expires_at: challenge.expires_at.to_string(),
                    methods: [SecondFactorMethod::Totp, SecondFactorMethod::BackupCode],
                    trusted_device_offered,
                },
            },
        }
    }
}

enum Verdict {
    Accepted(AuthMethod),
    Failed(SecondFactorFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecondFactorFailure {
    Invalid,
    Replayed,
    BackupCodeInvalid,
}

impl SecondFactorFailure {
    const fn error(self) -> LoginError {
        match self {
            Self::Invalid => LoginError::SecondFactorInvalid,
            Self::Replayed => LoginError::SecondFactorReplayed,
            Self::BackupCodeInvalid => LoginError::BackupCodeInvalid,
        }
    }
}

enum Completion {
    Issued {
        account: Box<AccountView>,
        method: AuthMethod,
        session: Box<MintedSession>,
        restriction: SessionRestriction,
        device: Option<IssuedDevice>,
    },
    Expired,
    Replayed,
    Locked {
        user: Box<User>,
        until: Timestamp,
    },
    Failed {
        user: Box<User>,
        method: AttemptMethod,
        failure: SecondFactorFailure,
        locked_now: Option<LockState>,
    },
}

impl AuthService {
    pub async fn complete_second_factor(
        &self,
        input: LoginTotpInput,
        context: MfaContext,
    ) -> Result<LoggedIn, LoginError> {
        let policy = LockoutPolicy::from_settings(&self.settings.load());
        let Some(digest) = input.mfa_token_hash.as_ref() else {
            return Err(LoginError::SecondFactorChallengeExpired);
        };
        if self.sessions().find_pending(digest).await?.is_none() {
            return Err(LoginError::SecondFactorChallengeExpired);
        }
        let device = if input.remember_device {
            if !self.trusted_device_policy().enabled {
                return Err(LoginError::TrustedDeviceDisabled);
            }
            Some(PreparedDevice::mint()?)
        } else {
            None
        };
        let prepared = PreparedCompletion {
            credentials: self.sessions().prepare_credentials()?,
            device,
        };
        let completion = self
            .pools
            .write_tx(self.clock.as_ref(), LOGIN_TOTP_TRANSACTION, async |tx| {
                self.second_factor_in_tx(tx, digest, &input.code, &prepared, &context, policy)
                    .await
            })
            .await?;
        match completion {
            Completion::Issued {
                account,
                method,
                session,
                restriction,
                device,
            } => {
                self.audit_success(&account, method, &context.audit);
                Ok(LoggedIn {
                    response: LoginResponse::new(&account, restriction),
                    session: *session,
                    device,
                })
            }
            Completion::Expired => Err(LoginError::SecondFactorChallengeExpired),
            Completion::Replayed => Err(LoginError::SecondFactorReplayed),
            Completion::Locked { user, until } => {
                let now = Timestamp::try_from(self.clock.now())?;
                self.audit_refused(&user, AttemptResult::LockedOut, &context.audit, now);
                Err(LoginError::Locked {
                    retry_after: RetryAfter::covering(remaining(now, until)),
                })
            }
            Completion::Failed {
                user,
                method,
                failure,
                locked_now,
            } => {
                self.audit_second_factor_failure(&user, method, locked_now, &context.audit, policy);
                Err(failure.error())
            }
        }
    }

    async fn second_factor_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        digest: &TokenDigest,
        code: &SecondFactorCode,
        prepared: &PreparedCompletion,
        context: &MfaContext,
        policy: LockoutPolicy,
    ) -> Result<Completion, LoginError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let Some(pending) = self.sessions().find_pending_in_tx(tx, digest).await? else {
            return Ok(Completion::Expired);
        };
        let Some(user) = users::find_by_id_in_tx(tx, pending.user_id)
            .await?
            .filter(|user| user.is_active && user.totp_enabled)
        else {
            self.sessions().burn_pending_in_tx(tx, pending.id).await?;
            return Ok(Completion::Expired);
        };
        let identifier = NormalizedIdentifier::from_input(&user.username);
        let method = code.attempt_method();
        let challenge = PendingSession {
            id: pending.id,
            mfa_token_hash: digest,
        };

        if let Some(until) = lockout::state_in_tx(tx, user.id)
            .await?
            .and_then(|state| state.active_until(now))
        {
            self.record_second_factor_attempt(
                tx,
                &identifier,
                &user,
                method,
                AttemptResult::LockedOut,
                &context.attempt,
            )
            .await?;
            return Ok(Completion::Locked {
                user: Box::new(user),
                until,
            });
        }

        let verdict = match code {
            SecondFactorCode::Totp(code) => {
                match consume_active_code_in_tx(tx, &self.keys, user.id, code, now).await? {
                    CodeCheck::Accepted => Verdict::Accepted(AuthMethod::PasswordTotp),
                    CodeCheck::Invalid => Verdict::Failed(SecondFactorFailure::Invalid),
                    CodeCheck::Replayed => Verdict::Failed(SecondFactorFailure::Replayed),
                    CodeCheck::NotEnrolled => return Err(LoginError::SecondFactorUnavailable),
                }
            }
            SecondFactorCode::BackupCode(code) => {
                let ip = context.attempt.ip.as_deref();
                if totp::consume_backup_code(tx, user.id, code, now, ip).await? {
                    Verdict::Accepted(AuthMethod::PasswordBackupCode)
                } else {
                    Verdict::Failed(SecondFactorFailure::BackupCodeInvalid)
                }
            }
            SecondFactorCode::Malformed => Verdict::Failed(SecondFactorFailure::Invalid),
        };

        let method_used = match verdict {
            Verdict::Accepted(method_used) => method_used,
            Verdict::Failed(failure) => {
                self.sessions()
                    .record_mfa_failure_in_tx(tx, challenge)
                    .await?;
                if failure == SecondFactorFailure::Replayed {
                    return Ok(Completion::Replayed);
                }
                let locked_now = self
                    .count_second_factor_failure_in_tx(
                        tx,
                        &user,
                        &identifier,
                        method,
                        &context.attempt,
                        policy,
                    )
                    .await?;
                return Ok(Completion::Failed {
                    user: Box::new(user),
                    method,
                    failure,
                    locked_now,
                });
            }
        };

        let issue = match self
            .issue_session_in_tx(
                tx,
                IssueSession {
                    user_id: user.id,
                    method: method_used,
                    client: context.session.clone(),
                    credentials: &prepared.credentials,
                    verified_password_hash: None,
                    replaces: context.presented_session.as_ref(),
                    promotes: Some(challenge),
                    trusted_device: None,
                },
            )
            .await
        {
            Err(LoginError::Session(SessionError::AuthRequired)) => {
                return Err(LoginError::SecondFactorChallengeExpired)
            }
            issue => issue?,
        };
        let (session, restriction) = match issue {
            SessionIssue::Issued {
                session,
                restriction,
                ..
            } => (session, restriction),
            SessionIssue::Refused(Refusal::Locked { until }) => {
                return Err(LoginError::Locked {
                    retry_after: RetryAfter::covering(remaining(now, until)),
                })
            }
            SessionIssue::Refused(Refusal::Inactive | Refusal::CredentialChanged) => {
                return Err(LoginError::SecondFactorChallengeExpired)
            }
            SessionIssue::SecondFactorRequired(_) => {
                return Err(LoginError::SecondFactorUnavailable)
            }
        };
        self.record_second_factor_attempt(
            tx,
            &identifier,
            &user,
            method,
            AttemptResult::Success,
            &context.attempt,
        )
        .await?;
        let device = match &prepared.device {
            Some(device) => Some(
                self.remember_device_in_tx(tx, device, user.id, &context.session)
                    .await?,
            ),
            None => None,
        };
        let account = repo::account(&mut *tx.executor(), user.id)
            .await?
            .ok_or(LoginError::SecondFactorChallengeExpired)?;
        Ok(Completion::Issued {
            account: Box::new(account),
            method: method_used,
            session: Box::new(session),
            restriction,
            device,
        })
    }

    async fn record_second_factor_attempt(
        &self,
        tx: &mut WriteTx<'_>,
        identifier: &NormalizedIdentifier,
        user: &User,
        method: AttemptMethod,
        result: AttemptResult,
        client: &AttemptClient,
    ) -> Result<(), LoginError> {
        lockout::record_attempt(
            tx,
            self.clock.as_ref(),
            &lockout::LoginAttempt {
                identifier,
                user_id: Some(user.id),
                method,
                result,
                client,
            },
        )
        .await
    }
}
