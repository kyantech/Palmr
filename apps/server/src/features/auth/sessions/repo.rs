use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::{ReadPool, WriteTx};
use crate::infra::http::pagination::{Conjunction, PageRequest};

use super::error::SessionError;
use super::model::{
    AuthMethod, ResolvedSession, RevokedReason, SessionId, SessionRecord, SessionState,
    SessionSummary,
};

const INSERT_ACTIVE: &str = "INSERT INTO sessions (
        id, user_id, token_hash, csrf_token_hash, state, auth_method,
        created_at, last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at,
        ip, user_agent
    ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, ?6, ?6, ?7, ?8, ?9, ?10)";

const SELECT_RESOLVED: &str = "SELECT
        s.id, s.user_id, s.token_hash, s.csrf_token_hash, s.state, s.auth_method,
        s.created_at, s.last_seen_at, s.last_auth_at, s.idle_expires_at,
        s.absolute_expires_at, s.ip, s.user_agent,
        u.username, u.role, u.is_active, u.must_change_password, u.totp_enabled
      FROM sessions s
      JOIN users u ON u.id = s.user_id
      WHERE s.token_hash = ?1";

const ROTATE: &str = "UPDATE sessions
    SET token_hash = ?2, csrf_token_hash = ?3, last_seen_at = ?4,
        idle_expires_at = CASE WHEN absolute_expires_at < ?5 THEN absolute_expires_at ELSE ?5 END
    WHERE id = ?1 AND state = 'active' AND revoked_at IS NULL
      AND idle_expires_at > ?4 AND absolute_expires_at > ?4
    RETURNING idle_expires_at, absolute_expires_at";

const PROMOTE_PENDING: &str = "UPDATE sessions
    SET state = 'active', token_hash = ?3, csrf_token_hash = ?4, auth_method = ?5,
        mfa_token_hash = NULL, mfa_expires_at = NULL, mfa_attempts = 0,
        last_seen_at = ?6, last_auth_at = ?6, idle_expires_at = ?7,
        absolute_expires_at = ?8
    WHERE id = ?1 AND state = 'mfa_pending' AND mfa_token_hash = ?2
      AND mfa_expires_at > ?6
    RETURNING idle_expires_at, absolute_expires_at";

const TOUCH: &str = "UPDATE sessions
    SET last_seen_at = ?2, idle_expires_at = ?3
    WHERE id = ?1 AND state = 'active' AND revoked_at IS NULL
      AND last_seen_at < ?4 AND idle_expires_at > ?2 AND absolute_expires_at > ?2";

const MARK_EXPIRED: &str = "UPDATE sessions SET state = 'expired'
    WHERE id = ?1 AND state = 'active'";

const UPDATE_LAST_AUTH: &str = "UPDATE sessions SET last_auth_at = ?2
    WHERE id = ?1 AND state = 'active' AND revoked_at IS NULL
      AND idle_expires_at > ?2 AND absolute_expires_at > ?2";

const REVOKE_ONE: &str = "UPDATE sessions
    SET state = 'revoked', revoked_at = ?3, revoked_reason = ?4
    WHERE id = ?1 AND user_id = ?2 AND state IN ('active','mfa_pending')";

const REVOKE_BY_TOKEN: &str = "UPDATE sessions
    SET state = 'revoked', revoked_at = ?2, revoked_reason = ?3
    WHERE token_hash = ?1 AND state IN ('active','mfa_pending')";

const SELECT_SUMMARY: &str = "SELECT id, auth_method, created_at, last_seen_at, idle_expires_at,
        absolute_expires_at, ip, user_agent
      FROM sessions
      WHERE id = ?1 AND user_id = ?2";

const OWNED_EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND user_id = ?2)";

const REVOKE_ALL: &str = "UPDATE sessions
    SET state = 'revoked', revoked_at = ?2, revoked_reason = ?3
    WHERE user_id = ?1 AND state IN ('active','mfa_pending')";

const REVOKE_OTHERS: &str = "UPDATE sessions
    SET state = 'revoked', revoked_at = ?3, revoked_reason = ?4
    WHERE user_id = ?1 AND id <> ?2 AND state IN ('active','mfa_pending')";

pub async fn insert_active(
    tx: &mut WriteTx<'_>,
    record: &SessionRecord,
) -> Result<(), SessionError> {
    sqlx::query(INSERT_ACTIVE)
        .bind(record.id.to_string())
        .bind(record.user_id.to_string())
        .bind(record.token_hash.as_str())
        .bind(record.csrf_token_hash.as_str())
        .bind(record.auth_method.as_str())
        .bind(record.created_at.to_string())
        .bind(record.idle_expires_at.to_string())
        .bind(record.absolute_expires_at.to_string())
        .bind(record.ip_address.as_deref())
        .bind(record.user_agent.as_deref())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn find_resolved(
    reader: &ReadPool,
    digest: &TokenDigest,
) -> Result<Option<ResolvedSession>, SessionError> {
    let row = sqlx::query(SELECT_RESOLVED)
        .bind(digest.as_str())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| resolved_from(&row)).transpose()
}

pub async fn rotate(
    tx: &mut WriteTx<'_>,
    id: SessionId,
    token_hash: &TokenDigest,
    csrf_token_hash: &TokenDigest,
    now: Timestamp,
    proposed_idle_expires_at: Timestamp,
) -> Result<Option<(Timestamp, Timestamp)>, SessionError> {
    let row = sqlx::query(ROTATE)
        .bind(id.to_string())
        .bind(token_hash.as_str())
        .bind(csrf_token_hash.as_str())
        .bind(now.to_string())
        .bind(proposed_idle_expires_at.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|row| {
        Ok((
            parsed(&row, "idle_expires_at")?,
            parsed(&row, "absolute_expires_at")?,
        ))
    })
    .transpose()
}

pub struct PendingPromotion<'a> {
    pub id: SessionId,
    pub mfa_token_hash: &'a TokenDigest,
    pub token_hash: &'a TokenDigest,
    pub csrf_token_hash: &'a TokenDigest,
    pub auth_method: AuthMethod,
    pub now: Timestamp,
    pub idle_expires_at: Timestamp,
    pub absolute_expires_at: Timestamp,
}

pub async fn promote_pending(
    tx: &mut WriteTx<'_>,
    promotion: PendingPromotion<'_>,
) -> Result<Option<(Timestamp, Timestamp)>, SessionError> {
    let row = sqlx::query(PROMOTE_PENDING)
        .bind(promotion.id.to_string())
        .bind(promotion.mfa_token_hash.as_str())
        .bind(promotion.token_hash.as_str())
        .bind(promotion.csrf_token_hash.as_str())
        .bind(promotion.auth_method.as_str())
        .bind(promotion.now.to_string())
        .bind(promotion.idle_expires_at.to_string())
        .bind(promotion.absolute_expires_at.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|row| {
        Ok((
            parsed(&row, "idle_expires_at")?,
            parsed(&row, "absolute_expires_at")?,
        ))
    })
    .transpose()
}

pub async fn touch(
    tx: &mut WriteTx<'_>,
    id: SessionId,
    now: Timestamp,
    idle_expires_at: Timestamp,
    threshold: Timestamp,
) -> Result<bool, SessionError> {
    let updated = sqlx::query(TOUCH)
        .bind(id.to_string())
        .bind(now.to_string())
        .bind(idle_expires_at.to_string())
        .bind(threshold.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn mark_expired(tx: &mut WriteTx<'_>, id: SessionId) -> Result<(), SessionError> {
    sqlx::query(MARK_EXPIRED)
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn update_last_auth(
    tx: &mut WriteTx<'_>,
    id: SessionId,
    now: Timestamp,
) -> Result<bool, SessionError> {
    let updated = sqlx::query(UPDATE_LAST_AUTH)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn revoke_owned(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    id: SessionId,
    now: Timestamp,
    reason: RevokedReason,
) -> Result<bool, SessionError> {
    let updated = sqlx::query(REVOKE_ONE)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .bind(now.to_string())
        .bind(reason.as_str())
        .execute(tx.executor())
        .await?;
    if updated.rows_affected() == 1 {
        return Ok(true);
    }
    let exists: bool = sqlx::query_scalar(OWNED_EXISTS)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_one(tx.executor())
        .await?;
    if exists {
        Ok(false)
    } else {
        Err(SessionError::NotFound)
    }
}

pub async fn revoke_by_token(
    tx: &mut WriteTx<'_>,
    token_hash: &TokenDigest,
    now: Timestamp,
    reason: RevokedReason,
) -> Result<bool, SessionError> {
    let updated = sqlx::query(REVOKE_BY_TOKEN)
        .bind(token_hash.as_str())
        .bind(now.to_string())
        .bind(reason.as_str())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn find_summary(
    reader: &ReadPool,
    user_id: UserId,
    id: SessionId,
) -> Result<Option<SessionSummary>, SessionError> {
    let row = sqlx::query(SELECT_SUMMARY)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.as_ref().map(summary_from).transpose()
}

pub async fn revoke_all(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    now: Timestamp,
    reason: RevokedReason,
) -> Result<u64, SessionError> {
    let updated = sqlx::query(REVOKE_ALL)
        .bind(user_id.to_string())
        .bind(now.to_string())
        .bind(reason.as_str())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub async fn revoke_all_others(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    current: SessionId,
    now: Timestamp,
    reason: RevokedReason,
) -> Result<u64, SessionError> {
    let updated = sqlx::query(REVOKE_OTHERS)
        .bind(user_id.to_string())
        .bind(current.to_string())
        .bind(now.to_string())
        .bind(reason.as_str())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub async fn list_active(
    reader: &ReadPool,
    user_id: UserId,
    now: Timestamp,
    page: &PageRequest,
) -> Result<(Vec<SessionSummary>, u64), SessionError> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT id, auth_method, created_at, last_seen_at, idle_expires_at, \
         absolute_expires_at, ip, user_agent FROM sessions \
         WHERE user_id = ",
    );
    query
        .push_bind(user_id.to_string())
        .push(" AND state = 'active' AND revoked_at IS NULL AND idle_expires_at > ")
        .push_bind(now.to_string())
        .push(" AND absolute_expires_at > ")
        .push_bind(now.to_string());
    page.push_keyset(&mut query, Conjunction::And);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let sessions = rows
        .iter()
        .map(summary_from)
        .collect::<Result<Vec<_>, _>>()?;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions
          WHERE user_id = ?1 AND state = 'active' AND revoked_at IS NULL
            AND idle_expires_at > ?2 AND absolute_expires_at > ?2",
    )
    .bind(user_id.to_string())
    .bind(now.to_string())
    .fetch_one(reader.executor())
    .await?;
    let count = u64::try_from(count).map_err(|_| invariant("count"))?;
    Ok((sessions, count))
}

fn resolved_from(row: &SqliteRow) -> Result<ResolvedSession, SessionError> {
    let role: String = column(row, "role")?;
    Ok(ResolvedSession {
        session: SessionRecord {
            id: parsed(row, "id")?,
            user_id: parsed(row, "user_id")?,
            token_hash: digest(row, "token_hash")?,
            csrf_token_hash: digest(row, "csrf_token_hash")?,
            state: SessionState::parse(&column::<String>(row, "state")?)
                .ok_or_else(|| invariant("state"))?,
            auth_method: AuthMethod::parse(&column::<String>(row, "auth_method")?)
                .ok_or_else(|| invariant("auth_method"))?,
            created_at: parsed(row, "created_at")?,
            last_seen_at: parsed(row, "last_seen_at")?,
            last_auth_at: parsed(row, "last_auth_at")?,
            idle_expires_at: parsed(row, "idle_expires_at")?,
            absolute_expires_at: parsed(row, "absolute_expires_at")?,
            ip_address: column(row, "ip")?,
            user_agent: column(row, "user_agent")?,
        },
        username: column(row, "username")?,
        role: role.parse::<Role>().map_err(|_| invariant("role"))?,
        is_active: column(row, "is_active")?,
        must_change_password: column(row, "must_change_password")?,
        totp_enabled: column(row, "totp_enabled")?,
    })
}

fn summary_from(row: &SqliteRow) -> Result<SessionSummary, SessionError> {
    Ok(SessionSummary {
        id: parsed(row, "id")?,
        auth_method: AuthMethod::parse(&column::<String>(row, "auth_method")?)
            .ok_or_else(|| invariant("auth_method"))?,
        created_at: parsed(row, "created_at")?,
        last_seen_at: parsed(row, "last_seen_at")?,
        idle_expires_at: parsed(row, "idle_expires_at")?,
        absolute_expires_at: parsed(row, "absolute_expires_at")?,
        ip_address: column(row, "ip")?,
        user_agent: column(row, "user_agent")?,
    })
}

fn digest(row: &SqliteRow, name: &'static str) -> Result<TokenDigest, SessionError> {
    TokenDigest::parse(&column::<String>(row, name)?).map_err(|_| invariant(name))
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, SessionError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, SessionError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> SessionError {
    SessionError::RepositoryInvariant { column }
}
