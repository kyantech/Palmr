use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite};

use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::{ReadPool, WriteTx};

use super::error::TotpError;
use super::model::{TotpRow, TotpState};

pub enum BackupCodeRow {}
pub type BackupCodeId = Id<BackupCodeRow>;
pub enum BackupCodeBatch {}
pub type BackupCodeBatchId = Id<BackupCodeBatch>;

const SELECT_ROW: &str = "SELECT state, secret_ciphertext, secret_nonce, key_version,
        algorithm, digits, period_seconds, last_used_step, confirmed_at, created_at
      FROM totp_secrets WHERE user_id = ?1";

const UPSERT_PENDING: &str = "INSERT INTO totp_secrets (
        user_id, secret_ciphertext, secret_nonce, key_version, state, created_at, updated_at
    ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)
    ON CONFLICT (user_id) DO UPDATE SET
        secret_ciphertext = excluded.secret_ciphertext,
        secret_nonce = excluded.secret_nonce,
        key_version = excluded.key_version,
        last_used_step = NULL,
        created_at = excluded.created_at,
        updated_at = excluded.updated_at
    WHERE totp_secrets.state = 'pending'";

const ACTIVATE: &str = "UPDATE totp_secrets
    SET state = 'active', confirmed_at = ?4, updated_at = ?4, last_used_step = ?3
    WHERE user_id = ?1 AND state = 'pending' AND secret_nonce = ?2 AND created_at > ?5";

const CONSUME_STEP: &str = "UPDATE totp_secrets SET last_used_step = ?2, updated_at = ?3
    WHERE user_id = ?1 AND state = 'active'
      AND (last_used_step IS NULL OR last_used_step < ?2)";

const DELETE_SECRET: &str = "DELETE FROM totp_secrets WHERE user_id = ?1";

const SET_ENABLED: &str = "UPDATE users SET totp_enabled = ?2, updated_at = ?3
    WHERE id = ?1 AND totp_enabled <> ?2";

const INSERT_BACKUP_CODE: &str = "INSERT INTO totp_backup_codes
        (id, user_id, batch_id, code_hash, created_at)
    VALUES (?1, ?2, ?3, ?4, ?5)";

const DELETE_BACKUP_CODES: &str = "DELETE FROM totp_backup_codes WHERE user_id = ?1";

const COUNT_UNUSED: &str = "SELECT COUNT(*) FROM totp_backup_codes
    WHERE user_id = ?1 AND used_at IS NULL";

const CONSUME_BACKUP_CODE: &str = "UPDATE totp_backup_codes SET used_at = ?3, used_ip = ?4
    WHERE user_id = ?1 AND code_hash = ?2 AND used_at IS NULL";

pub async fn find(reader: &ReadPool, user_id: UserId) -> Result<Option<TotpRow>, TotpError> {
    let row = sqlx::query(SELECT_ROW)
        .bind(user_id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.as_ref().map(row_from).transpose()
}

pub async fn find_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
) -> Result<Option<TotpRow>, TotpError> {
    let row = sqlx::query(SELECT_ROW)
        .bind(user_id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(row_from).transpose()
}

pub async fn upsert_pending(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    sealed: &SealedSecret,
    now: Timestamp,
) -> Result<bool, TotpError> {
    let written = sqlx::query(UPSERT_PENDING)
        .bind(user_id.to_string())
        .bind(sealed.ciphertext())
        .bind(sealed.nonce().as_slice())
        .bind(sealed.key_version())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(written.rows_affected() == 1)
}

pub async fn activate(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    nonce: &[u8],
    step: u64,
    now: Timestamp,
    created_after: Timestamp,
) -> Result<bool, TotpError> {
    let activated = sqlx::query(ACTIVATE)
        .bind(user_id.to_string())
        .bind(nonce)
        .bind(step_value(step)?)
        .bind(now.to_string())
        .bind(created_after.to_string())
        .execute(tx.executor())
        .await?;
    Ok(activated.rows_affected() == 1)
}

pub async fn consume_step(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    step: u64,
    now: Timestamp,
) -> Result<bool, TotpError> {
    let consumed = sqlx::query(CONSUME_STEP)
        .bind(user_id.to_string())
        .bind(step_value(step)?)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(consumed.rows_affected() == 1)
}

pub async fn delete_secret(tx: &mut WriteTx<'_>, user_id: UserId) -> Result<bool, TotpError> {
    let deleted = sqlx::query(DELETE_SECRET)
        .bind(user_id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(deleted.rows_affected() == 1)
}

pub async fn set_enabled(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    enabled: bool,
    now: Timestamp,
) -> Result<bool, TotpError> {
    let updated = sqlx::query(SET_ENABLED)
        .bind(user_id.to_string())
        .bind(enabled)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn replace_backup_codes(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    user_id: UserId,
    digests: &[&TokenDigest],
    now: Timestamp,
) -> Result<u64, TotpError> {
    let deleted = sqlx::query(DELETE_BACKUP_CODES)
        .bind(user_id.to_string())
        .execute(tx.executor())
        .await?
        .rows_affected();
    let batch = BackupCodeBatchId::generate(clock).to_string();
    for digest in digests {
        sqlx::query(INSERT_BACKUP_CODE)
            .bind(BackupCodeId::generate(clock).to_string())
            .bind(user_id.to_string())
            .bind(&batch)
            .bind(digest.as_str())
            .bind(now.to_string())
            .execute(tx.executor())
            .await?;
    }
    Ok(deleted)
}

pub async fn delete_backup_codes(tx: &mut WriteTx<'_>, user_id: UserId) -> Result<u64, TotpError> {
    let deleted = sqlx::query(DELETE_BACKUP_CODES)
        .bind(user_id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(deleted.rows_affected())
}

pub async fn count_unused_backup_codes(
    reader: &ReadPool,
    user_id: UserId,
) -> Result<u32, TotpError> {
    let count: i64 = sqlx::query_scalar(COUNT_UNUSED)
        .bind(user_id.to_string())
        .fetch_one(reader.executor())
        .await?;
    u32::try_from(count).map_err(|_| invariant("totp_backup_codes"))
}

pub async fn consume_backup_code(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    digest: &TokenDigest,
    now: Timestamp,
    ip: Option<&str>,
) -> Result<bool, TotpError> {
    let consumed = sqlx::query(CONSUME_BACKUP_CODE)
        .bind(user_id.to_string())
        .bind(digest.as_str())
        .bind(now.to_string())
        .bind(ip)
        .execute(tx.executor())
        .await?;
    Ok(consumed.rows_affected() == 1)
}

fn row_from(row: &SqliteRow) -> Result<TotpRow, TotpError> {
    let algorithm: String = column(row, "algorithm")?;
    let digits: i64 = column(row, "digits")?;
    let period: i64 = column(row, "period_seconds")?;
    if algorithm != "sha1" {
        return Err(invariant("algorithm"));
    }
    if digits != 6 {
        return Err(invariant("digits"));
    }
    if period != 30 {
        return Err(invariant("period_seconds"));
    }
    let last_used_step = column::<Option<i64>>(row, "last_used_step")?
        .map(|step| u64::try_from(step).map_err(|_| invariant("last_used_step")))
        .transpose()?;
    Ok(TotpRow {
        state: TotpState::parse(&column::<String>(row, "state")?)
            .ok_or_else(|| invariant("state"))?,
        secret_ciphertext: column(row, "secret_ciphertext")?,
        secret_nonce: column(row, "secret_nonce")?,
        key_version: column(row, "key_version")?,
        last_used_step,
        confirmed_at: column::<Option<String>>(row, "confirmed_at")?
            .map(|text| text.parse().map_err(|_| invariant("confirmed_at")))
            .transpose()?,
        created_at: column::<String>(row, "created_at")?
            .parse()
            .map_err(|_| invariant("created_at"))?,
    })
}

fn step_value(step: u64) -> Result<i64, TotpError> {
    i64::try_from(step).map_err(|_| invariant("last_used_step"))
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, TotpError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> TotpError {
    TotpError::RepositoryInvariant { column }
}
