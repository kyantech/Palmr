use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::features::email::model::OutboxId;
use crate::features::users::model::UserId;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::db::{DbError, ReadPool, WriteTx};
use crate::infra::http::pagination::{Conjunction, PageRequest};

use super::error::InviteError;
use super::model::{InviteId, InviteStatus, StoredInvite};

const INSERT: &str = "INSERT INTO invites
    (id, token_hash, email, email_normalized, role, state, created_by, created_at, expires_at,
     token_ciphertext, token_nonce, key_version)
    VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8, ?9, ?10, ?11)";

const PENDING_EMAIL_EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM invites
    WHERE email_normalized = ?1 AND state = 'pending')";

const USER_EMAIL_EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM users WHERE email_normalized = ?1)";

const EXPIRE_LAPSED_FOR_EMAIL: &str = "UPDATE invites
    SET state = 'expired', token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
    WHERE email_normalized = ?1 AND state = 'pending' AND expires_at <= ?2";

const EXPIRE_LAPSED: &str = "UPDATE invites
    SET state = 'expired', token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
    WHERE id = ?1 AND state = 'pending' AND expires_at <= ?2";

const SELECT_BY_ID: &str = "SELECT id, email, role, state, expires_at, created_by
    FROM invites WHERE id = ?1";

const SELECT_BY_HASH: &str = "SELECT id, email, role, state, expires_at, created_by
    FROM invites WHERE token_hash = ?1";

const SELECT_SEALED_TOKEN: &str = "SELECT token_ciphertext, token_nonce, key_version
    FROM invites WHERE id = ?1 AND state = 'pending'";

const QUEUED_MAIL: &str = "SELECT id FROM email_outbox
    WHERE batch_key = ?1 AND state IN ('pending', 'sending')";

const REVOKE: &str = "UPDATE invites
    SET state = 'revoked', revoked_at = ?2, revoked_by = ?3,
        token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
    WHERE id = ?1 AND state = 'pending' AND expires_at > ?2";

// Deferring the foreign keys lets the claim name the account the same
// transaction inserts next; SQLite checks them at COMMIT and resets the
// pragma when the transaction ends.
const DEFER_FOREIGN_KEYS: &str = "PRAGMA defer_foreign_keys = ON";

const CLAIM: &str = "UPDATE invites
    SET state = 'accepted', accepted_at = ?3, accepted_user_id = ?2,
        token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
    WHERE token_hash = ?1 AND state = 'pending' AND expires_at > ?3
    RETURNING id, email, role, state, expires_at, created_by";

const LIST_COLUMNS: &str = "SELECT i.id, i.email, i.role, i.state, i.created_at, i.expires_at,
        i.accepted_at, i.accepted_user_id, i.created_by, u.username AS created_by_username,
        (SELECT MAX(o.created_at) FROM email_outbox o WHERE o.batch_key = 'invite:' || i.id)
            AS last_sent_at
      FROM invites i JOIN users u ON u.id = i.created_by";

pub struct NewInvite<'a> {
    pub id: InviteId,
    pub token_hash: &'a TokenDigest,
    pub email: &'a str,
    pub email_normalized: &'a str,
    pub role: Role,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub sealed_token: &'a SealedSecret,
}

pub struct ListedInvite {
    pub id: InviteId,
    pub email: Option<String>,
    pub role: Role,
    pub status: InviteStatus,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub accepted_at: Option<Timestamp>,
    pub accepted_user_id: Option<UserId>,
    pub created_by: UserId,
    pub created_by_username: String,
    pub last_sent_at: Option<Timestamp>,
}

pub async fn user_email_exists(
    tx: &mut WriteTx<'_>,
    email_normalized: &str,
) -> Result<bool, InviteError> {
    Ok(sqlx::query_scalar(USER_EMAIL_EXISTS)
        .bind(email_normalized)
        .fetch_one(tx.executor())
        .await?)
}

pub async fn expire_lapsed_for_email(
    tx: &mut WriteTx<'_>,
    email_normalized: &str,
    now: Timestamp,
) -> Result<u64, InviteError> {
    let updated = sqlx::query(EXPIRE_LAPSED_FOR_EMAIL)
        .bind(email_normalized)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub async fn expire_lapsed(
    tx: &mut WriteTx<'_>,
    id: InviteId,
    now: Timestamp,
) -> Result<bool, InviteError> {
    let updated = sqlx::query(EXPIRE_LAPSED)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn insert(tx: &mut WriteTx<'_>, invite: &NewInvite<'_>) -> Result<(), InviteError> {
    let inserted = sqlx::query(INSERT)
        .bind(invite.id.to_string())
        .bind(invite.token_hash.as_str())
        .bind(invite.email)
        .bind(invite.email_normalized)
        .bind(invite.role.as_str())
        .bind(invite.created_by.to_string())
        .bind(invite.created_at.to_string())
        .bind(invite.expires_at.to_string())
        .bind(invite.sealed_token.ciphertext())
        .bind(invite.sealed_token.nonce().as_slice())
        .bind(invite.sealed_token.key_version())
        .execute(tx.executor())
        .await
        .map_err(DbError::from);
    match inserted {
        Ok(_) => Ok(()),
        Err(error @ DbError::UniqueViolation(_)) => {
            let pending: bool = sqlx::query_scalar(PENDING_EMAIL_EXISTS)
                .bind(invite.email_normalized)
                .fetch_one(tx.executor())
                .await?;
            Err(if pending {
                InviteError::EmailTaken
            } else {
                error.into()
            })
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn find_by_id<'e, E>(
    executor: E,
    id: InviteId,
) -> Result<Option<StoredInvite>, InviteError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(SELECT_BY_ID)
        .bind(id.to_string())
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(stored_from).transpose()
}

pub async fn find_by_hash<'e, E>(
    executor: E,
    token_hash: &TokenDigest,
) -> Result<Option<StoredInvite>, InviteError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(SELECT_BY_HASH)
        .bind(token_hash.as_str())
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(stored_from).transpose()
}

pub async fn sealed_token(
    tx: &mut WriteTx<'_>,
    id: InviteId,
) -> Result<Option<SealedSecret>, InviteError> {
    let row = sqlx::query(SELECT_SEALED_TOKEN)
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let ciphertext: Option<Vec<u8>> = column(&row, "token_ciphertext")?;
    let nonce: Option<Vec<u8>> = column(&row, "token_nonce")?;
    let key_version: Option<i64> = column(&row, "key_version")?;
    match (ciphertext, nonce, key_version) {
        (Some(ciphertext), Some(nonce), Some(key_version)) => Ok(Some(SealedSecret::from_parts(
            ciphertext,
            &nonce,
            key_version,
        )?)),
        _ => Err(invariant("token_ciphertext")),
    }
}

pub async fn queued_mail(
    tx: &mut WriteTx<'_>,
    batch_key: &str,
) -> Result<Vec<OutboxId>, InviteError> {
    let ids: Vec<String> = sqlx::query_scalar(QUEUED_MAIL)
        .bind(batch_key)
        .fetch_all(tx.executor())
        .await?;
    ids.iter()
        .map(|id| id.parse().map_err(|_| invariant("email_outbox.id")))
        .collect()
}

pub async fn revoke(
    tx: &mut WriteTx<'_>,
    id: InviteId,
    by: UserId,
    now: Timestamp,
) -> Result<bool, InviteError> {
    let updated = sqlx::query(REVOKE)
        .bind(id.to_string())
        .bind(now.to_string())
        .bind(by.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn claim(
    tx: &mut WriteTx<'_>,
    token_hash: &TokenDigest,
    user_id: UserId,
    now: Timestamp,
) -> Result<Option<StoredInvite>, InviteError> {
    sqlx::query(DEFER_FOREIGN_KEYS)
        .execute(tx.executor())
        .await?;
    let row = sqlx::query(CLAIM)
        .bind(token_hash.as_str())
        .bind(user_id.to_string())
        .bind(now.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(stored_from).transpose()
}

pub async fn list(
    reader: &ReadPool,
    status: Option<InviteStatus>,
    now: Timestamp,
    page: &PageRequest,
) -> Result<(Vec<ListedInvite>, u64), InviteError> {
    let mut query = QueryBuilder::<Sqlite>::new(LIST_COLUMNS);
    let conjunction = push_status(&mut query, status, now);
    page.push_keyset(&mut query, conjunction);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let invites = rows
        .iter()
        .map(|row| listed_from(row, now))
        .collect::<Result<Vec<_>, _>>()?;

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM invites i");
    push_status(&mut count, status, now);
    let total: i64 = count
        .build_query_scalar()
        .fetch_one(reader.executor())
        .await?;
    let total = u64::try_from(total).map_err(|_| invariant("count"))?;
    Ok((invites, total))
}

fn push_status(
    query: &mut QueryBuilder<'_, Sqlite>,
    status: Option<InviteStatus>,
    now: Timestamp,
) -> Conjunction {
    let Some(status) = status else {
        return Conjunction::Where;
    };
    match status {
        InviteStatus::Pending => {
            query
                .push(" WHERE i.state = 'pending' AND i.expires_at > ")
                .push_bind(now.to_string());
        }
        InviteStatus::Expired => {
            query
                .push(" WHERE (i.state = 'expired' OR (i.state = 'pending' AND i.expires_at <= ")
                .push_bind(now.to_string())
                .push("))");
        }
        InviteStatus::Accepted => {
            query.push(" WHERE i.state = 'accepted'");
        }
        InviteStatus::Revoked => {
            query.push(" WHERE i.state = 'revoked'");
        }
    }
    Conjunction::And
}

fn stored_from(row: &SqliteRow) -> Result<StoredInvite, InviteError> {
    Ok(StoredInvite {
        id: parsed(row, "id")?,
        email: column(row, "email")?,
        role: parsed(row, "role")?,
        state: column(row, "state")?,
        expires_at: parsed(row, "expires_at")?,
        created_by: parsed(row, "created_by")?,
    })
}

fn listed_from(row: &SqliteRow, now: Timestamp) -> Result<ListedInvite, InviteError> {
    let state: String = column(row, "state")?;
    let expires_at = parsed(row, "expires_at")?;
    Ok(ListedInvite {
        id: parsed(row, "id")?,
        email: column(row, "email")?,
        role: parsed(row, "role")?,
        status: InviteStatus::of(&state, expires_at, now).ok_or_else(|| invariant("state"))?,
        created_at: parsed(row, "created_at")?,
        expires_at,
        accepted_at: optional(row, "accepted_at")?,
        accepted_user_id: optional(row, "accepted_user_id")?,
        created_by: parsed(row, "created_by")?,
        created_by_username: column(row, "created_by_username")?,
        last_sent_at: optional(row, "last_sent_at")?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, InviteError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, InviteError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

fn optional<T: std::str::FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<T>, InviteError> {
    column::<Option<String>>(row, name)?
        .map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

const fn invariant(column: &'static str) -> InviteError {
    InviteError::RepositoryInvariant { column }
}
