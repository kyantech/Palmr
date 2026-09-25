use std::sync::Arc;

use axum::extract::Request;
use http::header::USER_AGENT;
use http::HeaderMap;
use time::Duration;

use crate::app::auth_class::AuthClass;
use crate::config::PublicBaseUrl;
use crate::domain::clock::Clock;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::repo as audit_repo;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::crypto::token::Token;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::http::cookies::{self, CookiePolicy, SESSION_COOKIE};
use crate::infra::http::pagination::{
    Page, PageRequest, QueryParams, SortAllowlist, SortDirection, SortField, SortKeyKind,
    SortValue, TotalCount,
};
use crate::infra::http::proxy::ResolvedClient;

use super::error::SessionError;
use super::model::{
    AuthMethod, AuthenticatedPrincipal, MintedSession, NewSession, PreparedSessionCredentials,
    ResolvedSession, RevokedReason, SessionId, SessionItem, SessionRecord, SessionRestriction,
    SessionState,
};
use super::repo;

pub const LAST_SEEN_WRITE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

static SESSION_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "lastSeenAt",
    "last_seen_at",
    SortKeyKind::Text,
)];
static SESSION_SORT: SortAllowlist =
    SortAllowlist::new(&SESSION_SORT_FIELDS, 0, SortDirection::Desc);

#[derive(Clone)]
pub struct SessionService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    keys: Arc<KeyRing>,
    cookies: CookiePolicy,
}

impl SessionService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        keys: Arc<KeyRing>,
        base_url: &PublicBaseUrl,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            keys,
            cookies: CookiePolicy::from_base_url(base_url),
        }
    }

    pub const fn cookie_policy(&self) -> CookiePolicy {
        self.cookies
    }

    pub fn new_session(
        &self,
        user_id: UserId,
        auth_method: super::model::AuthMethod,
        request: &Request,
    ) -> NewSession {
        let ip_address = request
            .extensions()
            .get::<ResolvedClient>()
            .filter(|client| client.via_trusted_proxy())
            .map(|client| client.ip().to_string());
        let user_agent = request
            .headers()
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        NewSession {
            user_id,
            auth_method,
            ip_address,
            user_agent,
        }
    }

    pub fn emit_cookies(
        &self,
        headers: &mut HeaderMap,
        session: &MintedSession,
    ) -> Result<(), SessionError> {
        self.cookies.append_session_pair(
            headers,
            &session.session_token,
            &session.csrf_token,
            self.cookie_max_age(session.absolute_expires_at),
        )?;
        Ok(())
    }

    pub async fn mint(&self, new: NewSession) -> Result<MintedSession, SessionError> {
        let session_token = Token::mint()?;
        let csrf_token = Token::mint()?;
        let now = Timestamp::try_from(self.clock.now())?;
        let policy = self.policy();
        let absolute_expires_at = Timestamp::try_from(now.get() + policy.absolute)?;
        let idle_expires_at =
            Timestamp::try_from((now.get() + policy.idle).min(absolute_expires_at.get()))?;
        let record = SessionRecord {
            id: SessionId::generate(self.clock.as_ref()),
            user_id: new.user_id,
            token_hash: session_token.digest(),
            csrf_token_hash: csrf_token.digest(),
            state: SessionState::Active,
            auth_method: new.auth_method,
            created_at: now,
            last_seen_at: now,
            last_auth_at: now,
            idle_expires_at,
            absolute_expires_at,
            ip_address: bound(new.ip_address, 45),
            user_agent: bound(new.user_agent, 512),
        };
        self.pools
            .write_tx(self.clock.as_ref(), "sessions.mint", async |tx| {
                repo::insert_active(tx, &record).await
            })
            .await?;
        Ok(MintedSession {
            id: record.id,
            session_token: session_token.encode(),
            csrf_token: csrf_token.encode(),
            idle_expires_at,
            absolute_expires_at,
        })
    }

    pub async fn rotate(&self, id: SessionId) -> Result<MintedSession, SessionError> {
        let credentials = self.prepare_credentials()?;
        self.pools
            .write_tx(self.clock.as_ref(), "sessions.rotate", async |tx| {
                self.rotate_in_tx(tx, id, &credentials).await
            })
            .await
    }

    pub fn prepare_credentials(&self) -> Result<PreparedSessionCredentials, SessionError> {
        let session_token = Token::mint()?;
        let csrf_token = Token::mint()?;
        Ok(PreparedSessionCredentials {
            token_hash: session_token.digest(),
            csrf_token_hash: csrf_token.digest(),
            session_token: session_token.encode(),
            csrf_token: csrf_token.encode(),
        })
    }

    pub async fn rotate_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        id: SessionId,
        credentials: &PreparedSessionCredentials,
    ) -> Result<MintedSession, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let proposed_idle_expires_at = Timestamp::try_from(now.get() + self.policy().idle)?;
        let Some((idle_expires_at, absolute_expires_at)) = repo::rotate(
            tx,
            id,
            &credentials.token_hash,
            &credentials.csrf_token_hash,
            now,
            proposed_idle_expires_at,
        )
        .await?
        else {
            return Err(SessionError::AuthRequired);
        };
        Ok(minted(
            id,
            credentials,
            idle_expires_at,
            absolute_expires_at,
        ))
    }

    pub async fn promote_pending_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        id: SessionId,
        presented_mfa_token: &TokenDigest,
        auth_method: AuthMethod,
        credentials: &PreparedSessionCredentials,
    ) -> Result<MintedSession, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let policy = self.policy();
        let idle_expires_at = Timestamp::try_from(now.get() + policy.idle)?;
        let absolute_expires_at = Timestamp::try_from(now.get() + policy.absolute)?;
        let Some((idle_expires_at, absolute_expires_at)) = repo::promote_pending(
            tx,
            repo::PendingPromotion {
                id,
                mfa_token_hash: presented_mfa_token,
                token_hash: &credentials.token_hash,
                csrf_token_hash: &credentials.csrf_token_hash,
                auth_method,
                now,
                idle_expires_at,
                absolute_expires_at,
            },
        )
        .await?
        else {
            return Err(SessionError::AuthRequired);
        };
        Ok(minted(
            id,
            credentials,
            idle_expires_at,
            absolute_expires_at,
        ))
    }

    pub async fn authenticate_headers(
        &self,
        headers: &HeaderMap,
        csrf: Option<&TokenDigest>,
    ) -> Result<AuthenticatedPrincipal, SessionError> {
        let raw = cookies::read(headers, SESSION_COOKIE)
            .map_err(|_| SessionError::AuthRequired)?
            .ok_or(SessionError::AuthRequired)?;
        self.authenticate_bound(&Secret::new(raw), csrf).await
    }

    pub async fn authenticate(
        &self,
        raw: &Secret<String>,
    ) -> Result<AuthenticatedPrincipal, SessionError> {
        self.authenticate_bound(raw, None).await
    }

    pub async fn authenticate_bound(
        &self,
        raw: &Secret<String>,
        csrf: Option<&TokenDigest>,
    ) -> Result<AuthenticatedPrincipal, SessionError> {
        let token = Token::decode(raw.expose_secret()).map_err(|_| SessionError::AuthRequired)?;
        let presented = token.digest();
        let Some(mut resolved) = repo::find_resolved(self.pools.reader(), &presented).await? else {
            return Err(SessionError::AuthRequired);
        };
        if !presented.verify(&resolved.session.token_hash)
            || resolved.session.state != SessionState::Active
            || !resolved.is_active
        {
            return Err(SessionError::AuthRequired);
        }

        let now = Timestamp::try_from(self.clock.now())?;
        if now >= resolved.session.idle_expires_at || now >= resolved.session.absolute_expires_at {
            self.mark_expired(resolved.session.id).await?;
            return Err(SessionError::AuthRequired);
        }
        if csrf.is_some_and(|presented| !presented.verify(&resolved.session.csrf_token_hash)) {
            return Err(SessionError::CsrfInvalid);
        }

        let policy = self.policy();
        let threshold = Timestamp::try_from(now.get() - LAST_SEEN_WRITE_INTERVAL)?;
        if resolved.session.last_seen_at < threshold {
            let idle_expires_at = Timestamp::try_from(
                (now.get() + policy.idle).min(resolved.session.absolute_expires_at.get()),
            )?;
            let touched = self
                .pools
                .write_tx(self.clock.as_ref(), "sessions.touch", async |tx| {
                    repo::touch(tx, resolved.session.id, now, idle_expires_at, threshold).await
                })
                .await?;
            if touched {
                resolved.session.last_seen_at = now;
                resolved.session.idle_expires_at = idle_expires_at;
            }
        }

        let recent_auth = resolved.session.last_auth_at <= now
            && now.get() - resolved.session.last_auth_at.get() <= policy.recent_auth;
        let restriction = if resolved.must_change_password {
            SessionRestriction::MustChangePassword
        } else if self.settings.load().security.two_factor_required && !resolved.totp_enabled {
            SessionRestriction::MustEnrollTotp
        } else {
            SessionRestriction::None
        };
        Ok(principal(resolved, restriction, recent_auth))
    }

    pub fn enforce_class(
        &self,
        principal: &AuthenticatedPrincipal,
        class: AuthClass,
    ) -> Result<(), SessionError> {
        if matches!(class, AuthClass::Admin | AuthClass::AdminRecentAuth)
            && principal.role != Role::Admin
        {
            return Err(SessionError::Forbidden);
        }
        if matches!(
            class,
            AuthClass::AuthenticatedRecentAuth | AuthClass::AdminRecentAuth
        ) && !principal.recent_auth
        {
            return Err(SessionError::RecentAuthRequired {
                method: principal.auth_method.recent_auth_hint(),
            });
        }
        Ok(())
    }

    pub fn page_request(
        &self,
        raw_query: Option<&str>,
    ) -> Result<PageRequest, crate::infra::http::error::ApiError> {
        let params = QueryParams::parse(raw_query);
        PageRequest::from_query(&params, &SESSION_SORT, self.keys.as_ref())
    }

    pub async fn list(
        &self,
        principal: &AuthenticatedPrincipal,
        page: PageRequest,
    ) -> Result<Page<SessionItem>, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let (rows, count) =
            repo::list_active(self.pools.reader(), principal.user_id, now, &page).await?;
        Ok(page
            .into_page(
                rows,
                self.keys.as_ref(),
                |row, _| {
                    crate::infra::http::pagination::CursorKey::new(
                        SortValue::Text(row.last_seen_at.to_string()),
                        row.id,
                    )
                },
                TotalCount::Exact(count),
            )
            .map_items(|row| SessionItem::from_summary(row, principal.session_id)))
    }

    pub async fn revoke_one(
        &self,
        principal: &AuthenticatedPrincipal,
        id: SessionId,
        reason: RevokedReason,
        client: &ClientMetadata,
    ) -> Result<bool, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let current = id == principal.session_id;
        self.pools
            .write_tx(self.clock.as_ref(), "sessions.revoke_one", async |tx| {
                if repo::revoke_owned(tx, principal.user_id, id, now, reason).await? {
                    let event = self
                        .audit_event(
                            principal,
                            actions::session_revoked(reason.as_str(), current),
                            now,
                            client,
                        )
                        .with_target(Target::new(TargetType::Session).id(&id.to_string()));
                    audit_repo::insert(tx, self.clock.as_ref(), &event).await?;
                }
                Ok::<(), SessionError>(())
            })
            .await?;
        Ok(current)
    }

    pub async fn revoke_all_others(
        &self,
        principal: &AuthenticatedPrincipal,
        reason: RevokedReason,
        client: &ClientMetadata,
    ) -> Result<u64, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(
                self.clock.as_ref(),
                "sessions.revoke_all_others",
                async |tx| {
                    let revoked = repo::revoke_all_others(
                        tx,
                        principal.user_id,
                        principal.session_id,
                        now,
                        reason,
                    )
                    .await?;
                    self.record_revoked_all(tx, principal, reason, false, revoked, now, client)
                        .await?;
                    Ok(revoked)
                },
            )
            .await
    }

    pub async fn revoke_all(
        &self,
        principal: &AuthenticatedPrincipal,
        reason: RevokedReason,
        client: &ClientMetadata,
    ) -> Result<u64, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), "sessions.revoke_all", async |tx| {
                let revoked = repo::revoke_all(tx, principal.user_id, now, reason).await?;
                self.record_revoked_all(tx, principal, reason, true, revoked, now, client)
                    .await?;
                Ok(revoked)
            })
            .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the audit row snapshots every fact of the revocation it records"
    )]
    async fn record_revoked_all(
        &self,
        tx: &mut WriteTx<'_>,
        principal: &AuthenticatedPrincipal,
        reason: RevokedReason,
        include_current: bool,
        revoked: u64,
        now: Timestamp,
        client: &ClientMetadata,
    ) -> Result<(), SessionError> {
        if revoked == 0 {
            return Ok(());
        }
        let event = self
            .audit_event(
                principal,
                actions::all_sessions_revoked(reason.as_str(), include_current, revoked),
                now,
                client,
            )
            .with_target(Target::new(TargetType::User).id(&principal.user_id.to_string()));
        audit_repo::insert(tx, self.clock.as_ref(), &event).await?;
        Ok(())
    }

    fn audit_event(
        &self,
        principal: &AuthenticatedPrincipal,
        spec: actions::ActionSpec,
        now: Timestamp,
        client: &ClientMetadata,
    ) -> AuditEvent {
        AuditEvent::new(
            spec,
            Actor::user(&principal.user_id.to_string(), &principal.username),
            Outcome::Success,
            now,
        )
        .with_client(client.clone())
    }

    pub async fn revoke_one_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        user_id: UserId,
        id: SessionId,
        reason: RevokedReason,
    ) -> Result<bool, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        repo::revoke_owned(tx, user_id, id, now, reason).await
    }

    pub async fn revoke_all_others_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        user_id: UserId,
        current: SessionId,
        reason: RevokedReason,
    ) -> Result<u64, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        repo::revoke_all_others(tx, user_id, current, now, reason).await
    }

    pub async fn revoke_all_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        user_id: UserId,
        reason: RevokedReason,
    ) -> Result<u64, SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        repo::revoke_all(tx, user_id, now, reason).await
    }

    pub async fn mark_reauthenticated(&self, id: SessionId) -> Result<(), SessionError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let updated = self
            .pools
            .write_tx(
                self.clock.as_ref(),
                "sessions.mark_reauthenticated",
                async |tx| repo::update_last_auth(tx, id, now).await,
            )
            .await?;
        if updated {
            Ok(())
        } else {
            Err(SessionError::AuthRequired)
        }
    }

    pub fn cookie_max_age(&self, absolute_expires_at: Timestamp) -> u64 {
        let seconds = (absolute_expires_at.get() - self.clock.now()).whole_seconds();
        u64::try_from(seconds.max(0)).unwrap_or(0)
    }

    async fn mark_expired(&self, id: SessionId) -> Result<(), SessionError> {
        self.pools
            .write_tx(self.clock.as_ref(), "sessions.expire", async |tx| {
                repo::mark_expired(tx, id).await
            })
            .await
    }

    fn policy(&self) -> SessionPolicy {
        let settings = self.settings.load();
        SessionPolicy {
            idle: Duration::days(i64::from(settings.security.session_idle_days)),
            absolute: Duration::days(i64::from(settings.security.session_absolute_days)),
            recent_auth: Duration::minutes(i64::from(settings.security.recent_auth_minutes)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SessionPolicy {
    idle: Duration,
    absolute: Duration,
    recent_auth: Duration,
}

fn principal(
    resolved: ResolvedSession,
    restriction: SessionRestriction,
    recent_auth: bool,
) -> AuthenticatedPrincipal {
    AuthenticatedPrincipal {
        user_id: resolved.session.user_id,
        username: resolved.username,
        session_id: resolved.session.id,
        role: resolved.role,
        restriction,
        last_auth_at: resolved.session.last_auth_at,
        recent_auth,
        auth_method: resolved.session.auth_method,
    }
}

fn bound(value: Option<String>, max_chars: usize) -> Option<String> {
    value.map(|value| value.chars().take(max_chars).collect())
}

fn minted(
    id: SessionId,
    credentials: &PreparedSessionCredentials,
    idle_expires_at: Timestamp,
    absolute_expires_at: Timestamp,
) -> MintedSession {
    MintedSession {
        id,
        session_token: credentials.session_token.clone(),
        csrf_token: credentials.csrf_token.clone(),
        idle_expires_at,
        absolute_expires_at,
    }
}

trait MapPage<T> {
    fn map_items<U>(self, map: impl FnMut(T) -> U) -> Page<U>;
}

impl<T> MapPage<T> for Page<T> {
    fn map_items<U>(self, map: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(map).collect(),
            next_cursor: self.next_cursor,
            total_count: self.total_count,
        }
    }
}
