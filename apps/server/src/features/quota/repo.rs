use std::str::FromStr;

use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection, SqliteExecutor};

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::users::model::{QuotaOverride, UserId};

use super::arith::{bytes_from_column, Limbs};
use super::error::QuotaError;
use super::model::{
    ReapedReservation, ReleaseReason, ReservationContext, ReservationId, ReservationRow,
    ReservationState, TransferSessionId,
};

const SELECT_OWNER: &str =
    "SELECT used_bytes, quota_override_mode, quota_bytes, is_active FROM users WHERE id = ?1";

const SELECT_HELD_TOTAL: &str = "SELECT COALESCE(SUM(reserved_bytes & 2097151), 0) AS low,
        COALESCE(SUM((reserved_bytes >> 21) & 2097151), 0) AS mid,
        COALESCE(SUM(reserved_bytes >> 42), 0) AS high
      FROM quota_reservations
      WHERE user_id = ?1 AND state = 'held'
        AND (?2 IS NULL OR transfer_session_id <> ?2)";

const SELECT_BY_SESSION: &str = "SELECT id, user_id, transfer_session_id, context, reserved_bytes,
        committed_bytes, state, created_at, expires_at, settled_at, release_reason
      FROM quota_reservations
      WHERE transfer_session_id = ?1
      ORDER BY (state = 'held') DESC, created_at DESC
      LIMIT 1";

const SELECT_SESSION_BINDING: &str = "SELECT ts.context AS context, ts.user_id AS user_id,
        rs.owner_id AS reverse_share_owner_id
      FROM transfer_sessions ts
      LEFT JOIN reverse_share_upload_sessions rsus ON rsus.id = ts.reverse_share_upload_session_id
      LEFT JOIN reverse_shares rs ON rs.id = rsus.reverse_share_id
      WHERE ts.id = ?1";

const INSERT_HELD: &str = "INSERT INTO quota_reservations
        (id, user_id, transfer_session_id, context, reserved_bytes, state, created_at, expires_at)
      VALUES (?1, ?2, ?3, ?4, ?5, 'held', ?6, ?7)";

const UPDATE_RESERVED: &str =
    "UPDATE quota_reservations SET reserved_bytes = ?2 WHERE id = ?1 AND state = 'held'";

const COMMIT_HELD: &str = "UPDATE quota_reservations
      SET state = 'committed', committed_bytes = ?2, settled_at = ?3, release_reason = NULL
      WHERE id = ?1 AND state = 'held'";

const RELEASE_HELD: &str = "UPDATE quota_reservations
      SET state = 'released', release_reason = ?2, settled_at = ?3
      WHERE id = ?1 AND state = 'held'";

const SET_USED: &str = "UPDATE users SET used_bytes = ?3 WHERE id = ?1 AND used_bytes = ?2";

const REAP_STALE: &str = "UPDATE quota_reservations
      SET state = 'released', release_reason = 'reaped', settled_at = ?1
      WHERE id IN (SELECT id FROM quota_reservations
                    WHERE state = 'held' AND expires_at <= ?1
                    ORDER BY expires_at
                    LIMIT ?2)
        AND state = 'held' AND expires_at <= ?1
      RETURNING id, transfer_session_id, user_id, expires_at, reserved_bytes";

const AUDITED_DRIFT_SINCE: &str = "SELECT EXISTS(SELECT 1 FROM audit_events
      WHERE target_type = 'user' AND target_id = ?1 AND occurred_at >= ?2
        AND action = 'QUOTA_DRIFT_DETECTED' AND metadata_json = ?3)";

const DELETE_SETTLED: &str = "DELETE FROM quota_reservations
      WHERE id IN (SELECT id FROM quota_reservations
                    WHERE state <> 'held' AND settled_at <= ?1
                    LIMIT ?2)";

macro_rules! usage_query {
    ($page:literal) => {
        concat!(
            "WITH page AS (",
            $page,
            ")
            SELECT p.id AS user_id, p.used_bytes AS used_bytes,
                   COALESCE(f.low, 0) AS f_low, COALESCE(f.mid, 0) AS f_mid,
                   COALESCE(f.high, 0) AS f_high, COALESCE(f.n, 0) AS f_n,
                   COALESCE(f.inactive, 0) AS f_inactive,
                   COALESCE(r.low, 0) AS r_low, COALESCE(r.mid, 0) AS r_mid,
                   COALESCE(r.high, 0) AS r_high, COALESCE(r.n, 0) AS r_n,
                   COALESCE(r.inactive, 0) AS r_inactive
              FROM page p
              LEFT JOIN (
                    SELECT c.owner_id AS owner_id,
                           SUM(c.size_bytes & 2097151) AS low,
                           SUM((c.size_bytes >> 21) & 2097151) AS mid,
                           SUM(c.size_bytes >> 42) AS high,
                           COUNT(*) AS n,
                           SUM(o.state <> 'active') AS inactive
                      FROM files c
                      JOIN storage_objects o ON o.id = c.storage_object_id
                     WHERE c.owner_id IN (SELECT id FROM page)
                     GROUP BY c.owner_id) f ON f.owner_id = p.id
              LEFT JOIN (
                    SELECT c.owner_id AS owner_id,
                           SUM(c.size_bytes & 2097151) AS low,
                           SUM((c.size_bytes >> 21) & 2097151) AS mid,
                           SUM(c.size_bytes >> 42) AS high,
                           COUNT(*) AS n,
                           SUM(o.state <> 'active') AS inactive
                      FROM received_files c
                      JOIN storage_objects o ON o.id = c.storage_object_id
                     WHERE c.owner_id IN (SELECT id FROM page)
                     GROUP BY c.owner_id) r ON r.owner_id = p.id
             ORDER BY p.id"
        )
    };
}

const USAGE_PAGE: &str =
    usage_query!("SELECT id, used_bytes FROM users WHERE id > ?1 ORDER BY id LIMIT ?2");

const USAGE_ONE: &str = usage_query!("SELECT id, used_bytes FROM users WHERE id = ?1");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerRow {
    pub used: ByteSize,
    pub quota: QuotaOverride,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBinding {
    pub context: ReservationContext,
    pub owner: Option<UserId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentSums {
    pub limbs: Limbs,
    pub rows: u64,
    pub inactive_objects: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub user_id: String,
    pub used: i64,
    pub files: ContentSums,
    pub received: ContentSums,
}

fn get<'r, T>(row: &'r SqliteRow, column: &'static str) -> Result<T, QuotaError>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column).map_err(QuotaError::from)
}

fn parse<T: FromStr>(value: &str, column: &'static str) -> Result<T, QuotaError> {
    value.parse().map_err(|_| QuotaError::Integrity { column })
}

fn count(row: &SqliteRow, column: &'static str) -> Result<u64, QuotaError> {
    let value: i64 = get(row, column)?;
    u64::try_from(value).map_err(|_| QuotaError::Integrity { column })
}

fn reservation_from(row: &SqliteRow) -> Result<ReservationRow, QuotaError> {
    let id: String = get(row, "id")?;
    let user_id: String = get(row, "user_id")?;
    let session_id: String = get(row, "transfer_session_id")?;
    let context: String = get(row, "context")?;
    let reserved: i64 = get(row, "reserved_bytes")?;
    let committed: Option<i64> = get(row, "committed_bytes")?;
    let state: String = get(row, "state")?;
    let created_at: String = get(row, "created_at")?;
    let expires_at: String = get(row, "expires_at")?;
    let settled_at: Option<String> = get(row, "settled_at")?;
    let release_reason: Option<String> = get(row, "release_reason")?;
    Ok(ReservationRow {
        id: parse::<ReservationId>(&id, "quota_reservations.id")?,
        user_id: parse::<UserId>(&user_id, "quota_reservations.user_id")?,
        session_id: parse::<TransferSessionId>(
            &session_id,
            "quota_reservations.transfer_session_id",
        )?,
        context: ReservationContext::parse(&context).ok_or(QuotaError::Integrity {
            column: "quota_reservations.context",
        })?,
        reserved: bytes_from_column(reserved, "quota_reservations.reserved_bytes")?,
        committed: committed
            .map(|value| bytes_from_column(value, "quota_reservations.committed_bytes"))
            .transpose()?,
        state: ReservationState::parse(&state).ok_or(QuotaError::Integrity {
            column: "quota_reservations.state",
        })?,
        created_at: parse::<Timestamp>(&created_at, "quota_reservations.created_at")?,
        expires_at: parse::<Timestamp>(&expires_at, "quota_reservations.expires_at")?,
        settled_at: settled_at
            .map(|value| parse::<Timestamp>(&value, "quota_reservations.settled_at"))
            .transpose()?,
        release_reason: release_reason
            .map(|value| {
                ReleaseReason::parse(&value).ok_or(QuotaError::Integrity {
                    column: "quota_reservations.release_reason",
                })
            })
            .transpose()?,
    })
}

pub async fn owner(
    connection: &mut SqliteConnection,
    owner: UserId,
) -> Result<Option<OwnerRow>, QuotaError> {
    let Some(row) = sqlx::query(SELECT_OWNER)
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?
    else {
        return Ok(None);
    };
    let used: i64 = get(&row, "used_bytes")?;
    let mode: String = get(&row, "quota_override_mode")?;
    let quota_bytes: Option<i64> = get(&row, "quota_bytes")?;
    let active: i64 = get(&row, "is_active")?;
    Ok(Some(OwnerRow {
        used: bytes_from_column(used, "users.used_bytes")?,
        quota: QuotaOverride::from_columns(&mode, quota_bytes).map_err(|_| {
            QuotaError::Integrity {
                column: "users.quota_override_mode",
            }
        })?,
        active: active == 1,
    }))
}

pub async fn held_total(
    connection: &mut SqliteConnection,
    owner: UserId,
    excluding: Option<TransferSessionId>,
) -> Result<ByteSize, QuotaError> {
    let row = sqlx::query(SELECT_HELD_TOTAL)
        .bind(owner.to_string())
        .bind(excluding.map(|session| session.to_string()))
        .fetch_one(connection)
        .await?;
    Limbs {
        low: get(&row, "low")?,
        mid: get(&row, "mid")?,
        high: get(&row, "high")?,
    }
    .total("held_reservations_total")
}

pub async fn find_by_session(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<Option<ReservationRow>, QuotaError> {
    sqlx::query(SELECT_BY_SESSION)
        .bind(session.to_string())
        .fetch_optional(connection)
        .await?
        .map(|row| reservation_from(&row))
        .transpose()
}

pub async fn session_binding(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<Option<SessionBinding>, QuotaError> {
    let Some(row) = sqlx::query(SELECT_SESSION_BINDING)
        .bind(session.to_string())
        .fetch_optional(connection)
        .await?
    else {
        return Ok(None);
    };
    let context: String = get(&row, "context")?;
    let context = ReservationContext::parse(&context).ok_or(QuotaError::Integrity {
        column: "transfer_sessions.context",
    })?;
    let direct: Option<String> = get(&row, "user_id")?;
    let through_share: Option<String> = get(&row, "reverse_share_owner_id")?;
    let owner = match context {
        ReservationContext::MyFiles => direct,
        ReservationContext::ReverseShare => through_share,
    }
    .map(|value| parse::<UserId>(&value, "transfer_sessions.owner"))
    .transpose()?;
    Ok(Some(SessionBinding { context, owner }))
}

pub async fn insert_held(
    connection: &mut SqliteConnection,
    row: &ReservationRow,
) -> Result<(), QuotaError> {
    sqlx::query(INSERT_HELD)
        .bind(row.id.to_string())
        .bind(row.user_id.to_string())
        .bind(row.session_id.to_string())
        .bind(row.context.as_str())
        .bind(row.reserved.to_i64())
        .bind(row.created_at.to_string())
        .bind(row.expires_at.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn update_reserved(
    connection: &mut SqliteConnection,
    id: ReservationId,
    reserved: ByteSize,
) -> Result<bool, QuotaError> {
    let result = sqlx::query(UPDATE_RESERVED)
        .bind(id.to_string())
        .bind(reserved.to_i64())
        .execute(connection)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn commit_held(
    connection: &mut SqliteConnection,
    id: ReservationId,
    committed: ByteSize,
    settled_at: Timestamp,
) -> Result<bool, QuotaError> {
    let result = sqlx::query(COMMIT_HELD)
        .bind(id.to_string())
        .bind(committed.to_i64())
        .bind(settled_at.to_string())
        .execute(connection)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn release_held(
    connection: &mut SqliteConnection,
    id: ReservationId,
    reason: ReleaseReason,
    settled_at: Timestamp,
) -> Result<bool, QuotaError> {
    let result = sqlx::query(RELEASE_HELD)
        .bind(id.to_string())
        .bind(reason.as_str())
        .bind(settled_at.to_string())
        .execute(connection)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_used(
    connection: &mut SqliteConnection,
    owner: UserId,
    expected: ByteSize,
    next: ByteSize,
) -> Result<bool, QuotaError> {
    let result = sqlx::query(SET_USED)
        .bind(owner.to_string())
        .bind(expected.to_i64())
        .bind(next.to_i64())
        .execute(connection)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn reap_stale(
    connection: &mut SqliteConnection,
    now: Timestamp,
    limit: u32,
) -> Result<Vec<ReapedReservation>, QuotaError> {
    let rows = sqlx::query(REAP_STALE)
        .bind(now.to_string())
        .bind(i64::from(limit))
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|row| {
            let reserved: i64 = get(row, "reserved_bytes")?;
            Ok(ReapedReservation {
                id: get(row, "id")?,
                session_id: get(row, "transfer_session_id")?,
                owner_id: get(row, "user_id")?,
                expires_at: get(row, "expires_at")?,
                reserved: bytes_from_column(reserved, "quota_reservations.reserved_bytes")?,
            })
        })
        .collect()
}

pub async fn delete_settled(
    connection: &mut SqliteConnection,
    cutoff: Timestamp,
    limit: u32,
) -> Result<u64, QuotaError> {
    let result = sqlx::query(DELETE_SETTLED)
        .bind(cutoff.to_string())
        .bind(i64::from(limit))
        .execute(connection)
        .await?;
    Ok(result.rows_affected())
}

fn sums(row: &SqliteRow, prefix: &'static str) -> Result<ContentSums, QuotaError> {
    let (low, mid, high, rows, inactive) = match prefix {
        "f" => ("f_low", "f_mid", "f_high", "f_n", "f_inactive"),
        _ => ("r_low", "r_mid", "r_high", "r_n", "r_inactive"),
    };
    Ok(ContentSums {
        limbs: Limbs {
            low: get(row, low)?,
            mid: get(row, mid)?,
            high: get(row, high)?,
        },
        rows: count(row, rows)?,
        inactive_objects: count(row, inactive)?,
    })
}

fn snapshot_from(row: &SqliteRow) -> Result<UsageSnapshot, QuotaError> {
    Ok(UsageSnapshot {
        user_id: get(row, "user_id")?,
        used: get(row, "used_bytes")?,
        files: sums(row, "f")?,
        received: sums(row, "r")?,
    })
}

pub async fn usage_page<'e, E>(
    executor: E,
    after: &str,
    limit: u32,
) -> Result<Vec<UsageSnapshot>, QuotaError>
where
    E: SqliteExecutor<'e>,
{
    let rows = sqlx::query(USAGE_PAGE)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(executor)
        .await?;
    rows.iter().map(snapshot_from).collect()
}

pub async fn usage_for<'e, E>(
    executor: E,
    user_id: &str,
) -> Result<Option<UsageSnapshot>, QuotaError>
where
    E: SqliteExecutor<'e>,
{
    sqlx::query(USAGE_ONE)
        .bind(user_id)
        .fetch_optional(executor)
        .await?
        .map(|row| snapshot_from(&row))
        .transpose()
}

pub async fn drift_audited_since<'e, E>(
    executor: E,
    user_id: &str,
    since: Timestamp,
    metadata: &str,
) -> Result<bool, QuotaError>
where
    E: SqliteExecutor<'e>,
{
    Ok(sqlx::query_scalar(AUDITED_DRIFT_SINCE)
        .bind(user_id)
        .bind(since.to_string())
        .bind(metadata)
        .fetch_one(executor)
        .await?)
}
