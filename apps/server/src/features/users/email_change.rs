use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use utoipa::ToSchema;

use crate::domain::clock::Clock;
use crate::domain::email::Email;
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{
    self, UserEmailChangeConfirmedFacts, UserEmailChangeRequestedFacts,
};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target as AuditTarget, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, RevokedReason, SessionError, SessionService,
};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::email::error::EmailError;
use crate::features::email::model::{LocalePreference, MailKind, MailParams, NewMail, Recipient};
use crate::features::email::EmailService;
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::email_change_repo::{self as repo, NewVerification, Target};
use super::model::UserId;

pub const START_TRANSACTION: &str = "users.email_change_start";
pub const RESEND_TRANSACTION: &str = "users.email_change_resend";
pub const CANCEL_TRANSACTION: &str = "users.email_change_cancel";
pub const VERIFY_TRANSACTION: &str = "auth.email_verify";

pub const EMAIL_CHANGE_VALIDITY_MINUTES: u32 = 24 * 60;

pub enum EmailVerification {}

pub type EmailVerificationId = Id<EmailVerification>;

#[derive(Debug)]
pub enum EmailChangeError {
    NotFound,
    Invalid { fields: Vec<&'static str> },
    EmailTaken,
    SmtpUnavailable,
    NotPending,
    TokenInvalid,
    TokenExpired,
    RepositoryInvariant { column: &'static str },
    Session(SessionError),
    Email(EmailError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl EmailChangeError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "email_change_user_not_found",
            Self::Invalid { .. } => "email_change_invalid",
            Self::EmailTaken => "email_change_email_taken",
            Self::SmtpUnavailable => "email_change_smtp_unavailable",
            Self::NotPending => "email_change_not_pending",
            Self::TokenInvalid => "email_change_token_invalid",
            Self::TokenExpired => "email_change_token_expired",
            Self::RepositoryInvariant { .. } => "email_change_repository_invariant",
            Self::Session(error) => error.kind(),
            Self::Email(error) => error.code(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "email_change_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "email_change_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::UserNotFound),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::EmailTaken => ApiError::new(ErrorCode::UserEmailTaken),
            Self::SmtpUnavailable => ApiError::new(ErrorCode::FeatureUnavailableSmtp),
            Self::NotPending => ApiError::new(ErrorCode::EmailVerificationNotPending),
            Self::TokenInvalid => ApiError::new(ErrorCode::EmailVerificationTokenInvalid),
            Self::TokenExpired => ApiError::new(ErrorCode::EmailVerificationTokenExpired),
            Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error))
            | Self::Email(EmailError::Database(error)) => ApiError::new(error.api_code()),
            Self::RepositoryInvariant { .. }
            | Self::Session(_)
            | Self::Email(_)
            | Self::Audit(_)
            | Self::Crypto(_)
            | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for EmailChangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the user does not exist"),
            Self::Invalid { fields } => {
                write!(
                    f,
                    "the e-mail change fields {} are invalid",
                    fields.join(", ")
                )
            }
            Self::EmailTaken => f.write_str("the e-mail address is already in use"),
            Self::SmtpUnavailable => f.write_str("outbound e-mail is not configured"),
            Self::NotPending => f.write_str("the user has no pending e-mail change"),
            Self::TokenInvalid => {
                f.write_str("the verification token is malformed, unknown or no longer live")
            }
            Self::TokenExpired => f.write_str("the verification token has expired"),
            Self::RepositoryInvariant { column } => {
                write!(f, "an e-mail change row holds an invalid value in {column}")
            }
            Self::Session(error) => write!(f, "e-mail change session operation failed: {error}"),
            Self::Email(error) => write!(f, "e-mail change message could not be queued: {error}"),
            Self::Audit(error) => write!(f, "e-mail change audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "e-mail change token operation failed: {error}"),
            Self::Db(error) => write!(f, "e-mail change database operation failed: {error}"),
            Self::Time(error) => write!(f, "e-mail change timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for EmailChangeError {}

impl From<SessionError> for EmailChangeError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<EmailError> for EmailChangeError {
    fn from(error: EmailError) -> Self {
        Self::Email(error)
    }
}

impl From<AuditError> for EmailChangeError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for EmailChangeError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for EmailChangeError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<InvalidTimestamp> for EmailChangeError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeEmailRequest {
    #[schema(example = "grace@newdomain.example")]
    pub email: String,
}

impl fmt::Debug for ChangeEmailRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChangeEmailRequest").finish_non_exhaustive()
    }
}

impl JsonRequest for ChangeEmailRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("email", JsonKind::String)];
}

#[derive(Debug)]
pub struct ChangeEmailInput {
    pub email: Email,
}

impl ChangeEmailInput {
    pub fn parse(request: ChangeEmailRequest) -> Result<Self, EmailChangeError> {
        Email::parse(&request.email)
            .map(|email| Self { email })
            .map_err(|_| EmailChangeError::Invalid {
                fields: vec!["email"],
            })
    }
}

pub struct PresentedToken(TokenDigest);

impl PresentedToken {
    pub fn parse(encoded: &str) -> Result<Self, EmailChangeError> {
        Token::decode(encoded)
            .map(|token| Self(token.digest()))
            .map_err(|_| EmailChangeError::TokenInvalid)
    }

    pub const fn digest(&self) -> &TokenDigest {
        &self.0
    }
}

#[derive(Clone, Copy)]
struct StartRequest<'a> {
    admin: &'a AuthenticatedPrincipal,
    id: UserId,
    email: &'a Email,
    client: &'a ClientMetadata,
    smtp_ready: bool,
}

struct Minted {
    token: Secret<String>,
    digest: TokenDigest,
}

impl Minted {
    fn new() -> Result<Self, EmailChangeError> {
        let token = Token::mint()?;
        Ok(Self {
            token: token.encode(),
            digest: token.digest(),
        })
    }
}

#[derive(Clone)]
pub struct EmailChangeService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    sessions: SessionService,
    email: EmailService,
    audit: AuditService,
}

impl EmailChangeService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        sessions: SessionService,
        email: EmailService,
        audit: AuditService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            sessions,
            email,
            audit,
        }
    }

    pub async fn start(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        input: ChangeEmailInput,
        client: &ClientMetadata,
    ) -> Result<(), EmailChangeError> {
        let request = StartRequest {
            admin,
            id,
            email: &input.email,
            client,
            smtp_ready: self.smtp_ready(),
        };
        let minted = Minted::new()?;
        self.pools
            .write_tx(self.clock.as_ref(), START_TRANSACTION, async |tx| {
                self.start_in_tx(tx, &request, &minted).await
            })
            .await
    }

    async fn start_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        request: &StartRequest<'_>,
        minted: &Minted,
    ) -> Result<(), EmailChangeError> {
        let StartRequest {
            admin,
            id,
            email,
            client,
            smtp_ready,
        } = *request;
        let now = Timestamp::try_from(self.clock.now())?;
        let target = repo::find_target(tx, id)
            .await?
            .ok_or(EmailChangeError::NotFound)?;
        if target.email_normalized == email.normalized() {
            return Err(EmailChangeError::Invalid {
                fields: vec!["email"],
            });
        }
        if !smtp_ready {
            return Err(EmailChangeError::SmtpUnavailable);
        }
        if repo::held_by_other_user(tx, email.normalized(), id).await? {
            return Err(EmailChangeError::EmailTaken);
        }
        let expires_at = Timestamp::try_from(
            now.get() + time::Duration::minutes(i64::from(EMAIL_CHANGE_VALIDITY_MINUTES)),
        )?;
        repo::invalidate_live(tx, id, now).await?;
        self.cancel_queued_mail(tx, id).await?;
        repo::set_pending(tx, id, email.as_str(), email.normalized(), now).await?;
        repo::insert(
            tx,
            &NewVerification {
                id: EmailVerificationId::generate(self.clock.as_ref()),
                user_id: id,
                email: email.as_str(),
                email_normalized: email.normalized(),
                token_hash: &minted.digest,
                created_at: now,
                expires_at,
                requested_by: admin.user_id,
            },
        )
        .await?;
        self.enqueue_mail(
            tx,
            &target,
            email,
            &minted.token,
            EMAIL_CHANGE_VALIDITY_MINUTES,
        )
        .await?;
        let spec = actions::user_email_change_requested(UserEmailChangeRequestedFacts {
            from_email: &target.email,
            to_email: email.as_str(),
            replaced_pending: target.pending_email.is_some(),
            self_change: id == admin.user_id,
        });
        let event = AuditEvent::new(
            spec,
            Actor::user(&admin.user_id.to_string(), &admin.username),
            Outcome::Success,
            now,
        )
        .with_target(user_target(&target))
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    pub async fn resend(&self, id: UserId) -> Result<(), EmailChangeError> {
        let smtp_ready = self.smtp_ready();
        let minted = Minted::new()?;
        self.pools
            .write_tx(self.clock.as_ref(), RESEND_TRANSACTION, async |tx| {
                self.resend_in_tx(tx, id, &minted, smtp_ready).await
            })
            .await
    }

    async fn resend_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        id: UserId,
        minted: &Minted,
        smtp_ready: bool,
    ) -> Result<(), EmailChangeError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let target = repo::find_target(tx, id)
            .await?
            .ok_or(EmailChangeError::NotFound)?;
        let live = repo::find_live(tx, id)
            .await?
            .filter(|live| target.pending_email.is_some() && live.expires_at > now)
            .ok_or(EmailChangeError::NotPending)?;
        if !smtp_ready {
            return Err(EmailChangeError::SmtpUnavailable);
        }
        let destination =
            Email::parse(&live.email).map_err(|_| EmailChangeError::RepositoryInvariant {
                column: "email_verifications.email",
            })?;
        if !repo::rotate_token(tx, live.id, &minted.digest).await? {
            return Err(EmailChangeError::NotPending);
        }
        self.cancel_queued_mail(tx, id).await?;
        self.enqueue_mail(
            tx,
            &target,
            &destination,
            &minted.token,
            remaining_minutes(now, live.expires_at),
        )
        .await
    }

    pub async fn cancel(&self, id: UserId) -> Result<(), EmailChangeError> {
        self.pools
            .write_tx(self.clock.as_ref(), CANCEL_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                repo::find_target(tx, id)
                    .await?
                    .ok_or(EmailChangeError::NotFound)?;
                repo::invalidate_live(tx, id, now).await?;
                repo::clear_pending(tx, id, now).await?;
                self.cancel_queued_mail(tx, id).await
            })
            .await
    }

    pub async fn verify(
        &self,
        token: &PresentedToken,
        client: &ClientMetadata,
    ) -> Result<(), EmailChangeError> {
        self.pools
            .write_tx(self.clock.as_ref(), VERIFY_TRANSACTION, async |tx| {
                self.verify_in_tx(tx, token, client).await
            })
            .await
    }

    async fn verify_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        token: &PresentedToken,
        client: &ClientMetadata,
    ) -> Result<(), EmailChangeError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let Some(consumed) = repo::consume(tx, token.digest(), now).await? else {
            return Err(self.classify_unusable(tx, token, now).await?);
        };
        let target = repo::find_target(tx, consumed.user_id)
            .await?
            .ok_or(EmailChangeError::TokenInvalid)?;
        if target.pending_email_normalized.as_deref() != Some(consumed.email_normalized.as_str()) {
            return Err(EmailChangeError::TokenInvalid);
        }
        if repo::canonical_held_by_other_user(tx, &consumed.email_normalized, target.id).await? {
            return Err(EmailChangeError::EmailTaken);
        }
        if !repo::promote(
            tx,
            target.id,
            &consumed.email,
            &consumed.email_normalized,
            now,
        )
        .await?
        {
            return Err(EmailChangeError::TokenInvalid);
        }
        let sessions_revoked = self
            .sessions
            .revoke_all_in_tx(tx, target.id, RevokedReason::AdminRequest)
            .await?;
        let trusted_devices_revoked = trusted_devices::revoke_all_in_tx(tx, target.id, now).await?;
        self.cancel_queued_mail(tx, target.id).await?;
        let spec = actions::user_email_change_confirmed(UserEmailChangeConfirmedFacts {
            from_email: &target.email,
            to_email: &consumed.email,
            sessions_revoked,
            trusted_devices_revoked,
        });
        let event = AuditEvent::new(
            spec,
            Actor::user(&target.id.to_string(), &target.username),
            Outcome::Success,
            now,
        )
        .with_target(user_target(&target))
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    async fn classify_unusable(
        &self,
        tx: &mut WriteTx<'_>,
        token: &PresentedToken,
        now: Timestamp,
    ) -> Result<EmailChangeError, EmailChangeError> {
        let stored = repo::find_by_hash(tx, token.digest()).await?;
        Ok(match stored {
            Some(stored)
                if stored.consumed_at.is_none()
                    && stored.invalidated_at.is_none()
                    && stored.expires_at <= now =>
            {
                EmailChangeError::TokenExpired
            }
            _ => EmailChangeError::TokenInvalid,
        })
    }

    async fn enqueue_mail(
        &self,
        tx: &mut WriteTx<'_>,
        target: &Target,
        destination: &Email,
        token: &Secret<String>,
        expiry_minutes: u32,
    ) -> Result<(), EmailChangeError> {
        let locale = target
            .locale
            .parse::<LocaleCode>()
            .map_or(LocalePreference::InstanceDefault, LocalePreference::Account);
        let mail = NewMail::new(
            MailKind::EmailVerification,
            Recipient::new(destination.clone(), display_name(target).as_deref()),
            locale,
            MailParams::EmailVerification { expiry_minutes },
        )
        .with_token(token.clone())
        .with_batch_key(batch_key(target.id));
        self.email.enqueue(tx, mail).await?;
        Ok(())
    }

    async fn cancel_queued_mail(
        &self,
        tx: &mut WriteTx<'_>,
        id: UserId,
    ) -> Result<(), EmailChangeError> {
        for outbox in repo::queued_mail(tx, &batch_key(id)).await? {
            self.email.cancel_in_tx(tx, outbox).await?;
        }
        Ok(())
    }

    fn smtp_ready(&self) -> bool {
        self.settings.load().smtp.is_available()
    }
}

pub fn batch_key(id: UserId) -> String {
    format!("email_change:{id}")
}

fn user_target(target: &Target) -> AuditTarget {
    AuditTarget::new(TargetType::User)
        .id(&target.id.to_string())
        .label(&target.username)
}

fn display_name(target: &Target) -> Option<String> {
    let full = format!("{} {}", target.first_name.trim(), target.last_name.trim());
    let full = full.trim();
    if full.is_empty() {
        None
    } else {
        Some(full.to_owned())
    }
}

fn remaining_minutes(now: Timestamp, expires_at: Timestamp) -> u32 {
    let seconds = (expires_at.get() - now.get()).whole_seconds().max(0);
    u32::try_from((seconds + 59) / 60)
        .unwrap_or(EMAIL_CHANGE_VALIDITY_MINUTES)
        .clamp(1, EMAIL_CHANGE_VALIDITY_MINUTES)
}
