use std::sync::Arc;

use http::StatusCode;

use crate::config::PublicBaseUrl;
use crate::domain::clock::Clock;
use crate::domain::email::Email;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::model::LoginResponse;
use crate::features::auth::repo as accounts;
use crate::features::auth::service::{IssueSession, SessionIssue};
use crate::features::auth::sessions::{
    AuthMethod, AuthenticatedPrincipal, MintedSession, SessionClient,
};
use crate::features::auth::AuthService;
use crate::features::email::model::{
    public_link, DisplayText, LocalePreference, MailKind, MailLink, MailParams, NewMail, Recipient,
};
use crate::features::email::EmailService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{NewUser, QuotaOverride, User, UserId};
use crate::features::users::repo as users;
use crate::features::users::service::AccountPasswordPolicy;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::idempotency::{Claim, IdempotencyService, ReplayEnvelope};
use crate::infra::http::pagination::{
    CursorKey, Page, PageRequest, QueryParams, SortAllowlist, SortDirection, SortField,
    SortKeyKind, SortValue, TotalCount,
};

use super::error::InviteError;
use super::model::{
    batch_key, invite_aad, AcceptInput, CreateInput, CreateInviteResponse, InviteCreator, InviteId,
    InviteItem, InviteLookupResponse, InviteStatus, PresentedInvite, StoredInvite,
    MAX_VALIDITY_HOURS, MIN_VALIDITY_HOURS,
};
use super::repo::{self, ListedInvite, NewInvite};

pub const CREATE_TRANSACTION: &str = "auth.invites.create";
pub const RESEND_TRANSACTION: &str = "auth.invites.resend";
pub const REVOKE_TRANSACTION: &str = "auth.invites.revoke";
pub const ACCEPT_TRANSACTION: &str = "auth.invites.accept";

const STATUS_PARAM: &str = "status";

static INVITE_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "createdAt",
    "i.created_at",
    SortKeyKind::Text,
)];
static INVITE_SORT: SortAllowlist =
    SortAllowlist::new(&INVITE_SORT_FIELDS, 0, SortDirection::Desc).with_id_column("i.id");

#[derive(Clone)]
pub struct InviteService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    keys: Arc<KeyRing>,
    base_url: PublicBaseUrl,
    auth: AuthService,
    email: EmailService,
    audit: AuditService,
    idempotency: IdempotencyService,
}

pub struct InviteServiceParts {
    pub pools: DbPools,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsHandle,
    pub keys: Arc<KeyRing>,
    pub base_url: PublicBaseUrl,
    pub auth: AuthService,
    pub email: EmailService,
    pub audit: AuditService,
}

pub struct InviteQuery {
    pub page: PageRequest,
    pub status: Option<InviteStatus>,
}

pub struct AcceptContext {
    pub session: SessionClient,
    pub audit: ClientMetadata,
    pub presented_session: Option<TokenDigest>,
}

pub struct Accepted {
    pub response: LoginResponse,
    pub session: MintedSession,
}

enum Resent {
    Queued,
    Lapsed,
}

struct Minted {
    id: InviteId,
    digest: TokenDigest,
    token: Secret<String>,
    sealed: crate::infra::crypto::aead::SealedSecret,
    invite_url: String,
    validity_hours: u32,
}

impl InviteService {
    pub fn new(parts: InviteServiceParts) -> Self {
        let idempotency = IdempotencyService::new(
            parts.pools.clone(),
            Arc::clone(&parts.clock),
            Arc::clone(&parts.keys),
        );
        Self {
            pools: parts.pools,
            clock: parts.clock,
            settings: parts.settings,
            keys: parts.keys,
            base_url: parts.base_url,
            auth: parts.auth,
            email: parts.email,
            audit: parts.audit,
            idempotency,
        }
    }

    pub const fn idempotency(&self) -> &IdempotencyService {
        &self.idempotency
    }

    pub const fn auth(&self) -> &AuthService {
        &self.auth
    }

    pub fn query(&self, raw_query: Option<&str>) -> Result<InviteQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let status = params
            .single(STATUS_PARAM)?
            .map(|value| {
                InviteStatus::parse(value)
                    .ok_or_else(|| crate::infra::http::pagination::invalid_param(STATUS_PARAM))
            })
            .transpose()?;
        let page = PageRequest::from_query(&params, &INVITE_SORT, self.keys.as_ref())?;
        Ok(InviteQuery { page, status })
    }

    pub async fn list(&self, query: InviteQuery) -> Result<Page<InviteItem>, InviteError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let (rows, total) = repo::list(self.pools.reader(), query.status, now, &query.page).await?;
        let page = query.page.into_page(
            rows,
            self.keys.as_ref(),
            |row, _| CursorKey::new(SortValue::Text(row.created_at.to_string()), row.id),
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page.items.into_iter().map(item_from).collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn create(
        &self,
        admin: &AuthenticatedPrincipal,
        input: CreateInput,
        claim: &Claim,
        client: &ClientMetadata,
    ) -> Result<CreateInviteResponse, InviteError> {
        let settings = self.settings.load();
        let smtp_ready = settings.smtp.is_available();
        let default_hours = settings
            .security
            .invite_validity_hours
            .clamp(MIN_VALIDITY_HOURS, MAX_VALIDITY_HOURS);
        drop(settings);
        if input.send_email && !smtp_ready {
            return Err(InviteError::SmtpUnavailable);
        }
        let minted = self.mint(input.validity_hours.unwrap_or(default_hours))?;
        self.pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                self.create_in_tx(tx, admin, &input, &minted, claim, client)
                    .await
            })
            .await
    }

    fn mint(&self, validity_hours: u32) -> Result<Minted, InviteError> {
        let token = Token::mint()?;
        let id = InviteId::generate(self.clock.as_ref());
        let encoded = token.encode();
        let sealed = self.keys.seal(
            SealPurpose::InviteToken,
            &invite_aad(id),
            encoded.expose_secret().as_bytes(),
        )?;
        let invite_url = public_link(
            self.base_url.url(),
            &MailLink::Invite,
            Some(encoded.expose_secret()),
        );
        Ok(Minted {
            id,
            digest: token.digest(),
            token: encoded,
            sealed,
            invite_url,
            validity_hours,
        })
    }

    async fn create_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        admin: &AuthenticatedPrincipal,
        input: &CreateInput,
        minted: &Minted,
        claim: &Claim,
        client: &ClientMetadata,
    ) -> Result<CreateInviteResponse, InviteError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let expires_at = Timestamp::try_from(
            now.get() + time::Duration::hours(i64::from(minted.validity_hours)),
        )?;
        let email = &input.email;
        if repo::user_email_exists(tx, email.normalized()).await? {
            return Err(InviteError::EmailTaken);
        }
        repo::expire_lapsed_for_email(tx, email.normalized(), now).await?;
        repo::insert(
            tx,
            &NewInvite {
                id: minted.id,
                token_hash: &minted.digest,
                email: email.as_str(),
                email_normalized: email.normalized(),
                role: input.role,
                created_by: admin.user_id,
                created_at: now,
                expires_at,
                sealed_token: &minted.sealed,
            },
        )
        .await?;
        if input.send_email {
            self.enqueue_mail(
                tx,
                admin.user_id,
                minted.id,
                email,
                &minted.token,
                minted.validity_hours,
            )
            .await?;
        }
        let event = AuditEvent::new(
            actions::invite_created(input.role, minted.validity_hours, input.send_email),
            Actor::user(&admin.user_id.to_string(), &admin.username),
            Outcome::Success,
            now,
        )
        .with_target(invite_target(minted.id, email.as_str()))
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;

        let response = CreateInviteResponse {
            id: minted.id.to_string(),
            invite_url: minted.invite_url.clone(),
            expires_at: expires_at.to_string(),
        };
        let body = serde_json::to_value(&response)
            .map_err(|_| InviteError::RepositoryInvariant { column: "response" })?;
        self.idempotency
            .complete(tx, claim, &ReplayEnvelope::new(StatusCode::CREATED, body))
            .await?;
        Ok(response)
    }

    async fn enqueue_mail(
        &self,
        tx: &mut WriteTx<'_>,
        inviter: UserId,
        id: InviteId,
        email: &Email,
        token: &Secret<String>,
        validity_hours: u32,
    ) -> Result<(), InviteError> {
        let inviter = users::find_by_id_in_tx(tx, inviter).await?.ok_or(
            InviteError::RepositoryInvariant {
                column: "created_by",
            },
        )?;
        let mail = NewMail::new(
            MailKind::Invite,
            Recipient::new(email.clone(), None),
            LocalePreference::InstanceDefault,
            MailParams::Invite {
                inviter_name: DisplayText::new(&display_name(&inviter)),
                expiry_hours: validity_hours,
            },
        )
        .with_token(token.clone())
        .with_batch_key(batch_key(id));
        self.email.enqueue(tx, mail).await?;
        Ok(())
    }

    pub async fn resend(
        &self,
        admin: &AuthenticatedPrincipal,
        id: InviteId,
    ) -> Result<(), InviteError> {
        let smtp_ready = self.settings.load().smtp.is_available();
        let resent = self
            .pools
            .write_tx(self.clock.as_ref(), RESEND_TRANSACTION, async |tx| {
                self.resend_in_tx(tx, admin, id, smtp_ready).await
            })
            .await?;
        match resent {
            Resent::Queued => Ok(()),
            Resent::Lapsed => Err(InviteError::Expired),
        }
    }

    async fn resend_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        admin: &AuthenticatedPrincipal,
        id: InviteId,
        smtp_ready: bool,
    ) -> Result<Resent, InviteError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let invite = repo::find_by_id(&mut *tx.executor(), id)
            .await?
            .ok_or(InviteError::NotFound)?;
        let status = invite.status(now)?;
        if status == InviteStatus::Expired && invite.state == "pending" {
            repo::expire_lapsed(tx, id, now).await?;
            self.cancel_queued_mail(tx, id).await?;
            return Ok(Resent::Lapsed);
        }
        status.into_live()?;
        if !smtp_ready {
            return Err(InviteError::SmtpUnavailable);
        }
        let sealed = repo::sealed_token(tx, id)
            .await?
            .ok_or(InviteError::RepositoryInvariant {
                column: "token_ciphertext",
            })?;
        let opened = self
            .keys
            .open(SealPurpose::InviteToken, &invite_aad(id), &sealed)?;
        let token = Secret::new(String::from_utf8(opened.expose_secret().clone()).map_err(
            |_| InviteError::RepositoryInvariant {
                column: "token_ciphertext",
            },
        )?);
        let email = invite
            .email
            .as_deref()
            .and_then(|email| Email::parse(email).ok())
            .ok_or(InviteError::RepositoryInvariant { column: "email" })?;
        let validity_hours = remaining_hours(now, invite.expires_at);
        self.enqueue_mail(tx, admin.user_id, id, &email, &token, validity_hours)
            .await?;
        Ok(Resent::Queued)
    }

    pub async fn revoke(
        &self,
        admin: &AuthenticatedPrincipal,
        id: InviteId,
        client: &ClientMetadata,
    ) -> Result<(), InviteError> {
        self.pools
            .write_tx(self.clock.as_ref(), REVOKE_TRANSACTION, async |tx| {
                self.revoke_in_tx(tx, admin, id, client).await
            })
            .await
    }

    async fn revoke_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        admin: &AuthenticatedPrincipal,
        id: InviteId,
        client: &ClientMetadata,
    ) -> Result<(), InviteError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let invite = repo::find_by_id(&mut *tx.executor(), id)
            .await?
            .ok_or(InviteError::NotFound)?;
        if repo::revoke(tx, id, admin.user_id, now).await? {
            self.cancel_queued_mail(tx, id).await?;
            let event = AuditEvent::new(
                actions::invite_revoked(invite.role),
                Actor::user(&admin.user_id.to_string(), &admin.username),
                Outcome::Success,
                now,
            )
            .with_target(invite_target(
                id,
                invite.email.as_deref().unwrap_or_default(),
            ))
            .with_client(client.clone());
            self.audit.record_in_tx(tx, &event).await?;
        } else if repo::expire_lapsed(tx, id, now).await? {
            self.cancel_queued_mail(tx, id).await?;
        }
        Ok(())
    }

    pub async fn lookup(
        &self,
        token: &PresentedInvite,
    ) -> Result<InviteLookupResponse, InviteError> {
        let invite = self.live(token).await?;
        let email = invite
            .email
            .ok_or(InviteError::RepositoryInvariant { column: "email" })?;
        let min_length = AccountPasswordPolicy::from_settings(&self.settings.load()).min_length();
        Ok(InviteLookupResponse {
            valid: true,
            email,
            password_min_length: min_length,
            expires_at: invite.expires_at.to_string(),
        })
    }

    pub async fn accept(
        &self,
        token: &PresentedInvite,
        input: AcceptInput,
        context: &AcceptContext,
    ) -> Result<Accepted, InviteError> {
        self.live(token).await?;
        let policy = AccountPasswordPolicy::from_settings(&self.settings.load());
        policy.check(input.password.expose_secret())?;
        let AcceptInput {
            first_name,
            last_name,
            username,
            password,
            locale,
        } = input;
        let password_hash = hash_off_runtime(password).await?;
        let credentials = self.auth.sessions().prepare_credentials()?;
        let user_id = UserId::generate(self.clock.as_ref());
        self.pools
            .write_tx(self.clock.as_ref(), ACCEPT_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let Some(invite) = repo::claim(tx, token.digest(), user_id, now).await? else {
                    return Err(self.unusable_in_tx(tx, token, now).await);
                };
                let email = invite
                    .email
                    .as_deref()
                    .and_then(|email| Email::parse(email).ok())
                    .ok_or(InviteError::RepositoryInvariant { column: "email" })?;
                self.cancel_queued_mail(tx, invite.id).await?;
                let clock = self.clock.as_ref();
                let user = users::insert_with_id(
                    tx,
                    clock,
                    user_id,
                    &NewUser {
                        email,
                        username: username.clone(),
                        first_name: first_name.clone(),
                        last_name: last_name.clone(),
                        password_hash: Some(password_hash.clone()),
                        must_change_password: false,
                        role: invite.role,
                        is_active: true,
                        quota: QuotaOverride::Inherit,
                        created_by: Some(invite.created_by),
                    },
                )
                .await?;
                users::insert_preferences(tx, clock, user.id, locale).await?;
                let event = AuditEvent::new(
                    actions::invite_consumed(invite.role),
                    Actor::user(&user.id.to_string(), &user.username),
                    Outcome::Success,
                    now,
                )
                .with_target(invite_target(invite.id, &user.email))
                .with_client(context.audit.clone());
                self.audit.record_in_tx(tx, &event).await?;
                self.sign_in_in_tx(tx, &user, &credentials, context).await
            })
            .await
    }

    async fn sign_in_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        user: &User,
        credentials: &crate::features::auth::sessions::PreparedSessionCredentials,
        context: &AcceptContext,
    ) -> Result<Accepted, InviteError> {
        let issue = self
            .auth
            .issue_session_in_tx(
                tx,
                IssueSession {
                    user_id: user.id,
                    method: AuthMethod::Invite,
                    client: context.session.clone(),
                    credentials,
                    verified_password_hash: None,
                    replaces: context.presented_session.as_ref(),
                    promotes: None,
                    trusted_device: None,
                },
            )
            .await?;
        let SessionIssue::Issued {
            session,
            restriction,
            ..
        } = issue
        else {
            return Err(InviteError::SessionRefused);
        };
        let account = accounts::account(&mut *tx.executor(), user.id)
            .await?
            .ok_or(InviteError::SessionRefused)?;
        Ok(Accepted {
            response: LoginResponse::new(&account, restriction),
            session,
        })
    }

    async fn unusable_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        token: &PresentedInvite,
        now: Timestamp,
    ) -> InviteError {
        match repo::find_by_hash(&mut *tx.executor(), token.digest()).await {
            Ok(Some(invite)) => match invite.status(now) {
                Ok(status) => status.into_live().err().unwrap_or(InviteError::NotFound),
                Err(error) => error,
            },
            Ok(None) => InviteError::NotFound,
            Err(error) => error,
        }
    }

    async fn live(&self, token: &PresentedInvite) -> Result<StoredInvite, InviteError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let invite = repo::find_by_hash(self.pools.reader().executor(), token.digest())
            .await?
            .ok_or(InviteError::NotFound)?;
        invite.status(now)?.into_live()?;
        Ok(invite)
    }

    async fn cancel_queued_mail(
        &self,
        tx: &mut WriteTx<'_>,
        id: InviteId,
    ) -> Result<(), InviteError> {
        for outbox in repo::queued_mail(tx, &batch_key(id)).await? {
            self.email.cancel_in_tx(tx, outbox).await?;
        }
        Ok(())
    }
}

fn item_from(row: ListedInvite) -> InviteItem {
    InviteItem {
        id: row.id.to_string(),
        email: row.email,
        role: row.role.as_str(),
        status: row.status,
        created_by: InviteCreator {
            id: row.created_by.to_string(),
            username: row.created_by_username,
        },
        created_at: row.created_at.to_string(),
        expires_at: row.expires_at.to_string(),
        accepted_at: row.accepted_at.map(|at| at.to_string()),
        accepted_user_id: row.accepted_user_id.map(|id| id.to_string()),
        last_sent_at: row.last_sent_at.map(|at| at.to_string()),
    }
}

fn invite_target(id: InviteId, email: &str) -> Target {
    Target::new(TargetType::Invite)
        .id(&id.to_string())
        .label(email)
}

fn display_name(user: &User) -> String {
    let full = format!("{} {}", user.first_name.trim(), user.last_name.trim());
    let full = full.trim();
    if full.is_empty() {
        user.username.clone()
    } else {
        full.to_owned()
    }
}

fn remaining_hours(now: Timestamp, expires_at: Timestamp) -> u32 {
    let remaining = (expires_at.get() - now.get()).whole_minutes();
    u32::try_from((remaining + 59) / 60)
        .unwrap_or(MAX_VALIDITY_HOURS)
        .clamp(MIN_VALIDITY_HOURS, MAX_VALIDITY_HOURS)
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, InviteError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| InviteError::HashTask)?
        .map_err(InviteError::from)
}
