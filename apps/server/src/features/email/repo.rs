use sqlx::Row;

use super::model::{DisplayText, MailKind, MailParams, OutboxId, OutboxRow, OutboxState};
use crate::domain::email::Email;
use crate::domain::locale::LocaleCode;
use crate::domain::time::Timestamp;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::db::{DbError, ReadPool, WriteTx};

pub const MAX_DELIVERY_ATTEMPTS: u32 = 10;

pub struct NewOutboxRow<'a> {
    pub id: OutboxId,
    pub kind: MailKind,
    pub to_email: &'a Email,
    pub to_name: Option<&'a DisplayText>,
    pub locale: LocaleCode,
    pub params_json: &'a str,
    pub batch_key: Option<&'a str>,
    pub dedup_key: Option<&'a str>,
    pub scheduled_at: Timestamp,
    pub created_at: Timestamp,
    pub sealed_token: Option<&'a SealedSecret>,
}

const INSERT: &str = "INSERT INTO email_outbox
    (id, kind, to_email, to_name, locale, params_json, state, batch_key, dedup_key, attempts,
     max_attempts, last_error, scheduled_at, created_at, updated_at, sent_at,
     token_ciphertext, token_nonce, key_version)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, 0, ?9, NULL, ?10, ?11, ?11, NULL,
            ?12, ?13, ?14)";

const LOAD: &str = "SELECT id, kind, to_email, to_name, locale, params_json, state, attempts,
                           max_attempts, last_error, token_ciphertext, token_nonce, key_version
                      FROM email_outbox WHERE id = ?1";

const MARK_SENDING: &str = "UPDATE email_outbox
                               SET state = 'sending', attempts = ?1, updated_at = ?2
                             WHERE id = ?3 AND state IN ('pending', 'sending')";

const MARK_RETRY_ERROR: &str = "UPDATE email_outbox
                                   SET last_error = ?1, updated_at = ?2
                                 WHERE id = ?3 AND state = 'sending'";

const MARK_SENT: &str = "UPDATE email_outbox
                            SET state = 'sent', sent_at = ?1, last_error = NULL, updated_at = ?1,
                                token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
                          WHERE id = ?2 AND state IN ('pending', 'sending')";

const MARK_FAILED: &str = "UPDATE email_outbox
                              SET state = 'failed', last_error = ?1, updated_at = ?2,
                                  token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
                            WHERE id = ?3 AND state IN ('pending', 'sending')";

const CANCEL: &str = "UPDATE email_outbox
                         SET state = 'canceled', updated_at = ?1,
                             token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
                       WHERE id = ?2 AND state IN ('pending', 'sending')";

fn corrupt(column: &'static str) -> DbError {
    DbError::from(sqlx::Error::Decode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("email_outbox row has an invalid {column}"),
    ))))
}

pub async fn insert(tx: &mut WriteTx<'_>, row: &NewOutboxRow<'_>) -> Result<(), DbError> {
    let (ciphertext, nonce, key_version) = match row.sealed_token {
        Some(sealed) => (
            Some(sealed.ciphertext().to_vec()),
            Some(sealed.nonce().to_vec()),
            Some(sealed.key_version()),
        ),
        None => (None, None, None),
    };
    sqlx::query(INSERT)
        .bind(row.id.to_string())
        .bind(row.kind.as_str())
        .bind(row.to_email.as_str())
        .bind(row.to_name.map(DisplayText::as_str))
        .bind(row.locale.as_str())
        .bind(row.params_json)
        .bind(row.batch_key)
        .bind(row.dedup_key)
        .bind(i64::from(MAX_DELIVERY_ATTEMPTS))
        .bind(row.scheduled_at.to_string())
        .bind(row.created_at.to_string())
        .bind(ciphertext)
        .bind(nonce)
        .bind(key_version)
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn load(db: &ReadPool, id: OutboxId) -> Result<Option<OutboxRow>, DbError> {
    let row = sqlx::query(LOAD)
        .bind(id.to_string())
        .fetch_optional(db.executor())
        .await?;
    row.map(row_from).transpose()
}

fn row_from(row: sqlx::sqlite::SqliteRow) -> Result<OutboxRow, DbError> {
    let id_text: String = row.try_get("id").map_err(|_| corrupt("id"))?;
    let id: OutboxId = id_text.parse().map_err(|_| corrupt("id"))?;
    let kind_text: String = row.try_get("kind").map_err(|_| corrupt("kind"))?;
    let kind: MailKind = kind_text.parse().map_err(|_| corrupt("kind"))?;
    let to_email_text: String = row.try_get("to_email").map_err(|_| corrupt("to_email"))?;
    let to_email = Email::parse(&to_email_text).map_err(|_| corrupt("to_email"))?;
    let to_name: Option<String> = row.try_get("to_name").map_err(|_| corrupt("to_name"))?;
    let locale_text: String = row.try_get("locale").map_err(|_| corrupt("locale"))?;
    let locale: LocaleCode = locale_text.parse().map_err(|_| corrupt("locale"))?;
    let params_json: String = row
        .try_get("params_json")
        .map_err(|_| corrupt("params_json"))?;
    let params = MailParams::from_json(kind, &params_json).map_err(|_| corrupt("params_json"))?;
    let state_text: String = row.try_get("state").map_err(|_| corrupt("state"))?;
    let state: OutboxState = state_text.parse().map_err(|_| corrupt("state"))?;
    let attempts = u32::try_from(
        row.try_get::<i64, _>("attempts")
            .map_err(|_| corrupt("attempts"))?,
    )
    .map_err(|_| corrupt("attempts"))?;
    let max_attempts = u32::try_from(
        row.try_get::<i64, _>("max_attempts")
            .map_err(|_| corrupt("max_attempts"))?,
    )
    .map_err(|_| corrupt("max_attempts"))?;
    let last_error: Option<String> = row
        .try_get("last_error")
        .map_err(|_| corrupt("last_error"))?;
    let sealed_token = sealed_token_from(&row)?;
    Ok(OutboxRow {
        id,
        kind,
        to_email,
        to_name: to_name.map(|name| DisplayText::new(&name)),
        locale,
        params,
        state,
        attempts,
        max_attempts,
        last_error,
        sealed_token,
    })
}

fn sealed_token_from(row: &sqlx::sqlite::SqliteRow) -> Result<Option<SealedSecret>, DbError> {
    let ciphertext: Option<Vec<u8>> = row
        .try_get("token_ciphertext")
        .map_err(|_| corrupt("token_ciphertext"))?;
    let nonce: Option<Vec<u8>> = row
        .try_get("token_nonce")
        .map_err(|_| corrupt("token_nonce"))?;
    let key_version: Option<i64> = row
        .try_get("key_version")
        .map_err(|_| corrupt("key_version"))?;
    match (ciphertext, nonce, key_version) {
        (None, None, None) => Ok(None),
        (Some(ciphertext), Some(nonce), Some(key_version)) => {
            SealedSecret::from_parts(ciphertext, &nonce, key_version)
                .map(Some)
                .map_err(|_| corrupt("token envelope"))
        }
        _ => Err(corrupt("token envelope")),
    }
}

pub async fn mark_sending(
    tx: &mut WriteTx<'_>,
    id: OutboxId,
    attempts: u32,
    updated_at: Timestamp,
) -> Result<bool, DbError> {
    let updated = sqlx::query(MARK_SENDING)
        .bind(i64::from(attempts))
        .bind(updated_at.to_string())
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn mark_retry_error(
    tx: &mut WriteTx<'_>,
    id: OutboxId,
    error_code: &str,
    updated_at: Timestamp,
) -> Result<(), DbError> {
    sqlx::query(MARK_RETRY_ERROR)
        .bind(error_code)
        .bind(updated_at.to_string())
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn mark_sent(
    tx: &mut WriteTx<'_>,
    id: OutboxId,
    sent_at: Timestamp,
) -> Result<bool, DbError> {
    let updated = sqlx::query(MARK_SENT)
        .bind(sent_at.to_string())
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn mark_failed(
    tx: &mut WriteTx<'_>,
    id: OutboxId,
    error_code: &str,
    updated_at: Timestamp,
) -> Result<bool, DbError> {
    let updated = sqlx::query(MARK_FAILED)
        .bind(error_code)
        .bind(updated_at.to_string())
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn cancel(
    tx: &mut WriteTx<'_>,
    id: OutboxId,
    updated_at: Timestamp,
) -> Result<bool, DbError> {
    let updated = sqlx::query(CANCEL)
        .bind(updated_at.to_string())
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}
