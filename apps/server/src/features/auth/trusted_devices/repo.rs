use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::{DbError, ReadPool, WriteTx};
use crate::infra::http::pagination::{Conjunction, PageRequest};

use super::error::TrustedDeviceError;
use super::model::{NewTrustedDevice, TrustedDeviceId, TrustedDeviceRecord};

pub const LAST_SEEN_COLUMN: &str = "COALESCE(last_used_at, created_at)";

const REVOKE_ALL: &str = "UPDATE trusted_devices SET revoked_at = ?2
    WHERE user_id = ?1 AND revoked_at IS NULL";

const INSERT: &str = "INSERT INTO trusted_devices (
        id, user_id, token_hash, label, created_at, last_used_at, expires_at, ip, user_agent
    ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8)";

const SELECT_USABLE: &str = "SELECT id, token_hash FROM trusted_devices
    WHERE token_hash = ?1 AND user_id = ?2 AND revoked_at IS NULL AND expires_at > ?3";

const MARK_USED: &str = "UPDATE trusted_devices SET last_used_at = ?2
    WHERE id = ?1 AND revoked_at IS NULL";

const SELECT_OWNED: &str = "SELECT token_hash, revoked_at FROM trusted_devices
    WHERE id = ?1 AND user_id = ?2";

const REVOKE_OWNED: &str = "UPDATE trusted_devices SET revoked_at = ?3
    WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL";

pub(crate) const COUNT_LISTED: &str = "SELECT COUNT(*) FROM trusted_devices
    WHERE user_id = ?1 AND revoked_at IS NULL AND expires_at > ?2";

pub async fn revoke_all_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    at: Timestamp,
) -> Result<u64, DbError> {
    let revoked = sqlx::query(REVOKE_ALL)
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(revoked.rows_affected())
}

pub async fn insert_in_tx(
    tx: &mut WriteTx<'_>,
    device: &NewTrustedDevice<'_>,
) -> Result<(), DbError> {
    sqlx::query(INSERT)
        .bind(device.id.to_string())
        .bind(device.user_id.to_string())
        .bind(device.token_hash.as_str())
        .bind(device.label.as_deref())
        .bind(device.created_at.to_string())
        .bind(device.expires_at.to_string())
        .bind(device.ip_address)
        .bind(device.user_agent)
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn find_usable_in_tx(
    tx: &mut WriteTx<'_>,
    presented: &TokenDigest,
    user_id: UserId,
    now: Timestamp,
) -> Result<Option<TrustedDeviceId>, TrustedDeviceError> {
    let row = sqlx::query(SELECT_USABLE)
        .bind(presented.as_str())
        .bind(user_id.to_string())
        .bind(now.to_string())
        .fetch_optional(tx.executor())
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored = digest(&row, "token_hash")?;
    presented
        .verify(&stored)
        .then(|| parsed(&row, "id"))
        .transpose()
}

pub async fn mark_used_in_tx(
    tx: &mut WriteTx<'_>,
    id: TrustedDeviceId,
    now: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(MARK_USED)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub struct Owned {
    pub token_hash: TokenDigest,
    pub revoked: bool,
}

pub async fn find_owned_in_tx(
    tx: &mut WriteTx<'_>,
    id: TrustedDeviceId,
    user_id: UserId,
) -> Result<Option<Owned>, TrustedDeviceError> {
    let row = sqlx::query(SELECT_OWNED)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|row| {
        Ok(Owned {
            token_hash: digest(&row, "token_hash")?,
            revoked: column::<Option<String>>(&row, "revoked_at")?.is_some(),
        })
    })
    .transpose()
}

pub async fn revoke_owned_in_tx(
    tx: &mut WriteTx<'_>,
    id: TrustedDeviceId,
    user_id: UserId,
    at: Timestamp,
) -> Result<bool, DbError> {
    let revoked = sqlx::query(REVOKE_OWNED)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(revoked.rows_affected() == 1)
}

pub async fn list(
    reader: &ReadPool,
    user_id: UserId,
    now: Timestamp,
    page: &PageRequest,
) -> Result<(Vec<TrustedDeviceRecord>, u64), TrustedDeviceError> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT id, token_hash, label, ip, created_at, expires_at, \
         COALESCE(last_used_at, created_at) AS last_seen_at \
         FROM trusted_devices WHERE user_id = ",
    );
    query
        .push_bind(user_id.to_string())
        .push(" AND revoked_at IS NULL AND expires_at > ")
        .push_bind(now.to_string());
    page.push_keyset(&mut query, Conjunction::And);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let devices = rows
        .iter()
        .map(record_from)
        .collect::<Result<Vec<_>, _>>()?;

    let count = count_usable(reader, user_id, now).await?;
    Ok((devices, count))
}

pub async fn count_usable(
    reader: &ReadPool,
    user_id: UserId,
    now: Timestamp,
) -> Result<u64, TrustedDeviceError> {
    let count: i64 = sqlx::query_scalar(COUNT_LISTED)
        .bind(user_id.to_string())
        .bind(now.to_string())
        .fetch_one(reader.executor())
        .await?;
    u64::try_from(count).map_err(|_| invariant("count"))
}

fn record_from(row: &SqliteRow) -> Result<TrustedDeviceRecord, TrustedDeviceError> {
    Ok(TrustedDeviceRecord {
        id: parsed(row, "id")?,
        token_hash: digest(row, "token_hash")?,
        label: column(row, "label")?,
        ip_address: column(row, "ip")?,
        created_at: parsed(row, "created_at")?,
        last_seen_at: parsed(row, "last_seen_at")?,
        expires_at: parsed(row, "expires_at")?,
    })
}

fn digest(row: &SqliteRow, name: &'static str) -> Result<TokenDigest, TrustedDeviceError> {
    TokenDigest::parse(&column::<String>(row, name)?).map_err(|_| invariant(name))
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, TrustedDeviceError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<T, TrustedDeviceError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> TrustedDeviceError {
    TrustedDeviceError::RepositoryInvariant { column }
}
