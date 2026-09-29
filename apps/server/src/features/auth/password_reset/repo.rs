use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite};

use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::WriteTx;

use super::error::PasswordResetError;
use super::model::{PasswordResetTokenId, StoredToken};

const INVALIDATE_OUTSTANDING: &str = "UPDATE password_reset_tokens
    SET invalidated_at = ?2
    WHERE user_id = ?1 AND used_at IS NULL AND invalidated_at IS NULL";

const INSERT: &str = "INSERT INTO password_reset_tokens
    (id, user_id, token_hash, created_at, expires_at, used_at, invalidated_at, requested_ip)
    VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)";

const SELECT_BY_HASH: &str = "SELECT t.expires_at, t.used_at, t.invalidated_at,
        EXISTS(SELECT 1 FROM users u
                WHERE u.id = t.user_id AND u.is_active = 1 AND u.password_hash IS NOT NULL)
            AS account_eligible
      FROM password_reset_tokens t
      WHERE t.token_hash = ?1";

const CONSUME: &str = "UPDATE password_reset_tokens
    SET used_at = ?2
    WHERE token_hash = ?1
      AND used_at IS NULL
      AND invalidated_at IS NULL
      AND expires_at > ?2
      AND user_id IN (SELECT id FROM users WHERE is_active = 1 AND password_hash IS NOT NULL)
    RETURNING user_id";

const SET_PASSWORD: &str = "UPDATE users
    SET password_hash = ?2, password_updated_at = ?3, must_change_password = 0, updated_at = ?3
    WHERE id = ?1 AND is_active = 1 AND password_hash IS NOT NULL";

pub struct NewResetToken<'a> {
    pub id: PasswordResetTokenId,
    pub user_id: UserId,
    pub token_hash: &'a TokenDigest,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub requested_ip: Option<&'a str>,
}

pub struct Consumed {
    pub user_id: UserId,
}

pub async fn invalidate_outstanding(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    at: Timestamp,
) -> Result<u64, PasswordResetError> {
    let updated = sqlx::query(INVALIDATE_OUTSTANDING)
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub async fn insert(
    tx: &mut WriteTx<'_>,
    token: &NewResetToken<'_>,
) -> Result<(), PasswordResetError> {
    sqlx::query(INSERT)
        .bind(token.id.to_string())
        .bind(token.user_id.to_string())
        .bind(token.token_hash.as_str())
        .bind(token.created_at.to_string())
        .bind(token.expires_at.to_string())
        .bind(token.requested_ip)
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn find<'e, E>(
    executor: E,
    token_hash: &TokenDigest,
) -> Result<Option<StoredToken>, PasswordResetError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(SELECT_BY_HASH)
        .bind(token_hash.as_str())
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(stored_from).transpose()
}

pub async fn consume(
    tx: &mut WriteTx<'_>,
    token_hash: &TokenDigest,
    at: Timestamp,
) -> Result<Option<Consumed>, PasswordResetError> {
    let row = sqlx::query(CONSUME)
        .bind(token_hash.as_str())
        .bind(at.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|row| {
        Ok(Consumed {
            user_id: parsed(&row, "user_id")?,
        })
    })
    .transpose()
}

pub async fn set_password(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    hash: &Secret<String>,
    at: Timestamp,
) -> Result<bool, PasswordResetError> {
    let updated = sqlx::query(SET_PASSWORD)
        .bind(user_id.to_string())
        .bind(hash.expose_secret().as_str())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

fn stored_from(row: &SqliteRow) -> Result<StoredToken, PasswordResetError> {
    let used_at: Option<String> = column(row, "used_at")?;
    let invalidated_at: Option<String> = column(row, "invalidated_at")?;
    Ok(StoredToken {
        expires_at: parsed(row, "expires_at")?,
        used_at: used_at
            .map(|text| text.parse().map_err(|_| invariant("used_at")))
            .transpose()?,
        invalidated_at: invalidated_at
            .map(|text| text.parse().map_err(|_| invariant("invalidated_at")))
            .transpose()?,
        account_eligible: column(row, "account_eligible")?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, PasswordResetError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<T, PasswordResetError> {
    let text: String = column(row, name)?;
    text.parse().map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> PasswordResetError {
    PasswordResetError::RepositoryInvariant { column }
}
