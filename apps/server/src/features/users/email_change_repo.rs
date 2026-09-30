use sqlx::sqlite::SqliteRow;
use sqlx::Row;

use crate::domain::time::Timestamp;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::{DbError, WriteTx};

use super::email_change::{EmailChangeError, EmailVerificationId};
use super::model::UserId;

const SELECT_TARGET: &str = "SELECT u.id, u.username, u.first_name, u.last_name, u.email,
        u.email_normalized, u.pending_email, u.pending_email_normalized,
        COALESCE(p.locale, 'en-US') AS locale
      FROM users u
      LEFT JOIN user_preferences p ON p.user_id = u.id
      WHERE u.id = ?1";

const HELD_BY_OTHER_USER: &str = "SELECT EXISTS(SELECT 1 FROM users
      WHERE id <> ?2 AND (email_normalized = ?1 OR pending_email_normalized = ?1))";

const CANONICAL_HELD_BY_OTHER_USER: &str =
    "SELECT EXISTS(SELECT 1 FROM users WHERE id <> ?2 AND email_normalized = ?1)";

const INVALIDATE_LIVE: &str = "UPDATE email_verifications
    SET invalidated_at = ?2
    WHERE user_id = ?1 AND purpose = 'email_change'
      AND consumed_at IS NULL AND invalidated_at IS NULL";

const INSERT: &str = "INSERT INTO email_verifications
    (id, user_id, purpose, email, email_normalized, token_hash, created_at, expires_at,
     consumed_at, invalidated_at, requested_by)
    VALUES (?1, ?2, 'email_change', ?3, ?4, ?5, ?6, ?7, NULL, NULL, ?8)";

const SELECT_LIVE: &str = "SELECT id, email, expires_at FROM email_verifications
    WHERE user_id = ?1 AND purpose = 'email_change'
      AND consumed_at IS NULL AND invalidated_at IS NULL";

const ROTATE_TOKEN: &str = "UPDATE email_verifications
    SET token_hash = ?2
    WHERE id = ?1 AND consumed_at IS NULL AND invalidated_at IS NULL";

const CONSUME: &str = "UPDATE email_verifications
    SET consumed_at = ?2
    WHERE token_hash = ?1
      AND purpose = 'email_change'
      AND consumed_at IS NULL
      AND invalidated_at IS NULL
      AND expires_at > ?2
    RETURNING user_id, email, email_normalized";

const SELECT_BY_HASH: &str = "SELECT expires_at, consumed_at, invalidated_at
    FROM email_verifications
    WHERE token_hash = ?1 AND purpose = 'email_change'";

const SET_PENDING: &str = "UPDATE users
    SET pending_email = ?2, pending_email_normalized = ?3, updated_at = ?4
    WHERE id = ?1";

const CLEAR_PENDING: &str = "UPDATE users
    SET pending_email = NULL, pending_email_normalized = NULL, updated_at = ?2
    WHERE id = ?1 AND pending_email IS NOT NULL";

const PROMOTE: &str = "UPDATE users
    SET email = ?2, email_normalized = ?3, email_verified_at = ?4,
        pending_email = NULL, pending_email_normalized = NULL, updated_at = ?4
    WHERE id = ?1 AND pending_email_normalized = ?3";

const QUEUED_MAIL: &str = "SELECT id FROM email_outbox
    WHERE batch_key = ?1 AND state IN ('pending', 'sending')";

pub struct Target {
    pub id: UserId,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub email_normalized: String,
    pub pending_email: Option<String>,
    pub pending_email_normalized: Option<String>,
    pub locale: String,
}

pub struct NewVerification<'a> {
    pub id: EmailVerificationId,
    pub user_id: UserId,
    pub email: &'a str,
    pub email_normalized: &'a str,
    pub token_hash: &'a TokenDigest,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub requested_by: UserId,
}

pub struct LiveVerification {
    pub id: EmailVerificationId,
    pub email: String,
    pub expires_at: Timestamp,
}

pub struct Consumed {
    pub user_id: UserId,
    pub email: String,
    pub email_normalized: String,
}

pub struct StoredVerification {
    pub expires_at: Timestamp,
    pub consumed_at: Option<Timestamp>,
    pub invalidated_at: Option<Timestamp>,
}

pub async fn find_target(
    tx: &mut WriteTx<'_>,
    id: UserId,
) -> Result<Option<Target>, EmailChangeError> {
    let row = sqlx::query(SELECT_TARGET)
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await
        .map_err(DbError::from)?;
    row.as_ref().map(target_from).transpose()
}

pub async fn held_by_other_user(
    tx: &mut WriteTx<'_>,
    email_normalized: &str,
    user_id: UserId,
) -> Result<bool, EmailChangeError> {
    exists(tx, HELD_BY_OTHER_USER, email_normalized, user_id).await
}

pub async fn canonical_held_by_other_user(
    tx: &mut WriteTx<'_>,
    email_normalized: &str,
    user_id: UserId,
) -> Result<bool, EmailChangeError> {
    exists(tx, CANONICAL_HELD_BY_OTHER_USER, email_normalized, user_id).await
}

async fn exists(
    tx: &mut WriteTx<'_>,
    sql: &'static str,
    email_normalized: &str,
    user_id: UserId,
) -> Result<bool, EmailChangeError> {
    Ok(sqlx::query_scalar(sql)
        .bind(email_normalized)
        .bind(user_id.to_string())
        .fetch_one(tx.executor())
        .await
        .map_err(DbError::from)?)
}

pub async fn invalidate_live(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    at: Timestamp,
) -> Result<u64, EmailChangeError> {
    let updated = sqlx::query(INVALIDATE_LIVE)
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await
        .map_err(DbError::from)?;
    Ok(updated.rows_affected())
}

pub async fn insert(
    tx: &mut WriteTx<'_>,
    verification: &NewVerification<'_>,
) -> Result<(), EmailChangeError> {
    sqlx::query(INSERT)
        .bind(verification.id.to_string())
        .bind(verification.user_id.to_string())
        .bind(verification.email)
        .bind(verification.email_normalized)
        .bind(verification.token_hash.as_str())
        .bind(verification.created_at.to_string())
        .bind(verification.expires_at.to_string())
        .bind(verification.requested_by.to_string())
        .execute(tx.executor())
        .await
        .map_err(DbError::from)?;
    Ok(())
}

pub async fn find_live(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
) -> Result<Option<LiveVerification>, EmailChangeError> {
    let row = sqlx::query(SELECT_LIVE)
        .bind(user_id.to_string())
        .fetch_optional(tx.executor())
        .await
        .map_err(DbError::from)?;
    row.map(|row| {
        Ok(LiveVerification {
            id: parsed(&row, "id")?,
            email: column(&row, "email")?,
            expires_at: parsed(&row, "expires_at")?,
        })
    })
    .transpose()
}

pub async fn rotate_token(
    tx: &mut WriteTx<'_>,
    id: EmailVerificationId,
    token_hash: &TokenDigest,
) -> Result<bool, EmailChangeError> {
    let updated = sqlx::query(ROTATE_TOKEN)
        .bind(id.to_string())
        .bind(token_hash.as_str())
        .execute(tx.executor())
        .await
        .map_err(DbError::from)?;
    Ok(updated.rows_affected() == 1)
}

pub async fn consume(
    tx: &mut WriteTx<'_>,
    token_hash: &TokenDigest,
    at: Timestamp,
) -> Result<Option<Consumed>, EmailChangeError> {
    let row = sqlx::query(CONSUME)
        .bind(token_hash.as_str())
        .bind(at.to_string())
        .fetch_optional(tx.executor())
        .await
        .map_err(DbError::from)?;
    row.map(|row| {
        Ok(Consumed {
            user_id: parsed(&row, "user_id")?,
            email: column(&row, "email")?,
            email_normalized: column(&row, "email_normalized")?,
        })
    })
    .transpose()
}

pub async fn find_by_hash(
    tx: &mut WriteTx<'_>,
    token_hash: &TokenDigest,
) -> Result<Option<StoredVerification>, EmailChangeError> {
    let row = sqlx::query(SELECT_BY_HASH)
        .bind(token_hash.as_str())
        .fetch_optional(tx.executor())
        .await
        .map_err(DbError::from)?;
    row.map(|row| {
        Ok(StoredVerification {
            expires_at: parsed(&row, "expires_at")?,
            consumed_at: optional_timestamp(&row, "consumed_at")?,
            invalidated_at: optional_timestamp(&row, "invalidated_at")?,
        })
    })
    .transpose()
}

pub async fn set_pending(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    email: &str,
    email_normalized: &str,
    at: Timestamp,
) -> Result<(), EmailChangeError> {
    sqlx::query(SET_PENDING)
        .bind(user_id.to_string())
        .bind(email)
        .bind(email_normalized)
        .bind(at.to_string())
        .execute(tx.executor())
        .await
        .map_err(|error| match DbError::from(error) {
            DbError::UniqueViolation(_) => EmailChangeError::EmailTaken,
            other => EmailChangeError::Db(other),
        })?;
    Ok(())
}

pub async fn clear_pending(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    at: Timestamp,
) -> Result<bool, EmailChangeError> {
    let updated = sqlx::query(CLEAR_PENDING)
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await
        .map_err(DbError::from)?;
    Ok(updated.rows_affected() == 1)
}

pub async fn promote(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    email: &str,
    email_normalized: &str,
    at: Timestamp,
) -> Result<bool, EmailChangeError> {
    let updated = sqlx::query(PROMOTE)
        .bind(user_id.to_string())
        .bind(email)
        .bind(email_normalized)
        .bind(at.to_string())
        .execute(tx.executor())
        .await
        .map_err(|error| match DbError::from(error) {
            DbError::UniqueViolation(_) => EmailChangeError::EmailTaken,
            other => EmailChangeError::Db(other),
        })?;
    Ok(updated.rows_affected() == 1)
}

pub async fn queued_mail(
    tx: &mut WriteTx<'_>,
    batch_key: &str,
) -> Result<Vec<crate::features::email::model::OutboxId>, EmailChangeError> {
    let ids: Vec<String> = sqlx::query_scalar(QUEUED_MAIL)
        .bind(batch_key)
        .fetch_all(tx.executor())
        .await
        .map_err(DbError::from)?;
    ids.iter()
        .map(|id| id.parse().map_err(|_| invariant("email_outbox.id")))
        .collect()
}

fn target_from(row: &SqliteRow) -> Result<Target, EmailChangeError> {
    Ok(Target {
        id: parsed(row, "id")?,
        username: column(row, "username")?,
        first_name: column(row, "first_name")?,
        last_name: column(row, "last_name")?,
        email: column(row, "email")?,
        email_normalized: column(row, "email_normalized")?,
        pending_email: column(row, "pending_email")?,
        pending_email_normalized: column(row, "pending_email_normalized")?,
        locale: column(row, "locale")?,
    })
}

fn optional_timestamp(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<Timestamp>, EmailChangeError> {
    let text: Option<String> = column(row, name)?;
    text.map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, EmailChangeError>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<T, EmailChangeError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> EmailChangeError {
    EmailChangeError::RepositoryInvariant { column }
}
