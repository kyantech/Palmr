use sqlx::sqlite::SqliteRow;
use sqlx::SqliteConnection;

use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;

use super::super::super::error::TransferError;
use super::super::super::model::{SessionItemId, TransferProvider, TransferSessionId};
use super::super::super::repo::{
    bytes, column, invariant, optional_bytes, optional_parsed, parsed, FinalizeStage, LiveItem,
};
use super::super::super::state::{ItemState, TransferSessionState};
use super::super::model::{MultipartId, MultipartState, S3Ref};

pub(super) const EXPIRY_PAGE: &str = "SELECT id, expires_at
      FROM s3_multipart_uploads
     WHERE state IN ('created','in_progress') AND expires_at <= ?1
       AND expires_at >= ?2 AND (expires_at > ?2 OR (expires_at = ?2 AND id > ?3))
     ORDER BY expires_at, id
     LIMIT ?4";

pub(super) const OPEN_PAGE: &str = "SELECT id, expires_at
      FROM s3_multipart_uploads
     WHERE state IN ('created','in_progress') AND expires_at > ?1
       AND expires_at >= ?2 AND (expires_at > ?2 OR (expires_at = ?2 AND id > ?3))
     ORDER BY expires_at, id
     LIMIT ?4";

pub(super) const PRUNE_TERMINAL: &str = "DELETE FROM s3_multipart_uploads
    WHERE id IN (SELECT id FROM s3_multipart_uploads
                  WHERE state IN ('completed','aborted')
                    AND completed_at IS NOT NULL AND completed_at <= ?1
                  ORDER BY completed_at
                  LIMIT ?2)";

const LOAD_OWNED: &str = "SELECT m.id, m.s3_upload_id, m.bucket, m.object_key, m.total_size_bytes,
        m.part_size_bytes, m.part_count, m.state AS mp_state, m.expires_at AS mp_expires_at,
        m.updated_at AS mp_updated_at, m.owner_user_id,
        t.id AS item_id, t.state AS item_state, t.finalize_stage, t.reserved_bytes,
        t.final_object_id, t.final_object_key, t.transfer_session_id,
        ts.state AS session_state, ts.provider
      FROM s3_multipart_uploads m
      JOIN transfer_session_files t ON t.id = m.transfer_session_file_id
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
     WHERE m.id = ?1";

const MARK_EXPIRED: &str = "UPDATE s3_multipart_uploads
      SET state = 'abandoned', updated_at = ?2
    WHERE id = ?1 AND state IN ('created', 'in_progress') AND expires_at <= ?2";

const MARK_LOST: &str = "UPDATE s3_multipart_uploads
      SET state = 'aborted', updated_at = ?2, completed_at = ?2
    WHERE id = ?1 AND state IN ('created', 'in_progress') AND expires_at > ?2";

const FAIL_LOST_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'failed', error_code = ?2, error_request_id = NULL, reserved_bytes = 0,
          updated_at = ?3
    WHERE id = ?1 AND state = 'uploading' AND finalize_stage = 'none'";

const MARK_RECONCILED_PASSIVE: &str = "UPDATE s3_multipart_uploads
      SET state = CASE WHEN state = 'created' AND ?3 = 1 THEN 'in_progress' ELSE state END,
          last_reconciled_at = ?2, updated_at = ?2
    WHERE id = ?1 AND state IN ('created', 'in_progress')";

pub(super) const KNOWN_PAIRS: &str = "SELECT object_key, s3_upload_id
      FROM s3_multipart_uploads
     WHERE bucket = ?1 AND object_key IN (SELECT value FROM json_each(?2))";

const PROVIDER_IDENTITY_KNOWN: &str = "SELECT EXISTS(SELECT 1 FROM s3_multipart_uploads
     WHERE bucket = ?1 AND object_key = ?2 AND s3_upload_id = ?3)";

pub(super) struct OwnedMultipart {
    pub(super) s3: S3Ref,
    pub(super) expires_at: Timestamp,
    pub(super) updated_at: Timestamp,
    pub(super) owner: Option<UserId>,
    pub(super) item: LiveItem,
    pub(super) session: TransferSessionId,
    pub(super) session_state: TransferSessionState,
    pub(super) provider: TransferProvider,
}

fn owned_from(row: &SqliteRow) -> Result<OwnedMultipart, TransferError> {
    let state: String = column(row, "mp_state")?;
    let item_state: String = column(row, "item_state")?;
    let stage: String = column(row, "finalize_stage")?;
    let session_state: String = column(row, "session_state")?;
    let provider: String = column(row, "provider")?;
    let part_size: i64 = column(row, "part_size_bytes")?;
    let part_count: i64 = column(row, "part_count")?;
    Ok(OwnedMultipart {
        s3: S3Ref {
            id: parsed(row, "id")?,
            upload_id: column(row, "s3_upload_id")?,
            bucket: column(row, "bucket")?,
            object_key: column(row, "object_key")?,
            total: optional_bytes(row, "total_size_bytes")?,
            part_size: u64::try_from(part_size).map_err(|_| invariant("part_size_bytes"))?,
            part_count: u32::try_from(part_count).map_err(|_| invariant("part_count"))?,
            state: MultipartState::parse(&state).ok_or_else(|| invariant("mp_state"))?,
        },
        expires_at: parsed(row, "mp_expires_at")?,
        updated_at: parsed(row, "mp_updated_at")?,
        owner: optional_parsed(row, "owner_user_id")?,
        item: LiveItem {
            id: parsed(row, "item_id")?,
            state: ItemState::parse(&item_state).ok_or_else(|| invariant("item_state"))?,
            finalize_stage: FinalizeStage::parse(&stage)
                .ok_or_else(|| invariant("finalize_stage"))?,
            reserved: bytes(row, "reserved_bytes")?,
            final_object_id: parsed(row, "final_object_id")?,
            final_object_key: column(row, "final_object_key")?,
        },
        session: parsed(row, "transfer_session_id")?,
        session_state: TransferSessionState::parse(&session_state)
            .ok_or_else(|| invariant("session_state"))?,
        provider: TransferProvider::parse(&provider).ok_or_else(|| invariant("provider"))?,
    })
}

pub(super) async fn load_owned(
    connection: &mut SqliteConnection,
    id: MultipartId,
) -> Result<Option<OwnedMultipart>, TransferError> {
    let row = sqlx::query(LOAD_OWNED)
        .bind(id.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(owned_from).transpose()
}

pub(super) type PageRow = (String, String);

pub(super) async fn page(
    connection: &mut SqliteConnection,
    sql: &'static str,
    bounds: (&str, &str, &str),
    limit: u32,
) -> Result<Vec<PageRow>, TransferError> {
    let rows = sqlx::query(sql)
        .bind(bounds.0)
        .bind(bounds.1)
        .bind(bounds.2)
        .bind(i64::from(limit))
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|row| Ok((column(row, "id")?, column(row, "expires_at")?)))
        .collect()
}

pub(super) async fn mark_expired(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(MARK_EXPIRED)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub(super) async fn mark_lost(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(MARK_LOST)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub(super) async fn fail_lost_item(
    connection: &mut SqliteConnection,
    item: SessionItemId,
    code: &str,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(FAIL_LOST_ITEM)
        .bind(item.to_string())
        .bind(code)
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub(super) async fn mark_reconciled_passive(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
    promote: bool,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(MARK_RECONCILED_PASSIVE)
        .bind(id.to_string())
        .bind(now.to_string())
        .bind(i64::from(promote))
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub(super) async fn known_pairs(
    connection: &mut SqliteConnection,
    bucket: &str,
    keys: &[&str],
) -> Result<Vec<(String, String)>, TransferError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let list = serde_json::Value::from(keys.to_vec()).to_string();
    Ok(sqlx::query_as(KNOWN_PAIRS)
        .bind(bucket)
        .bind(list)
        .fetch_all(connection)
        .await?)
}

pub(super) async fn provider_identity_known(
    connection: &mut SqliteConnection,
    bucket: &str,
    key: &str,
    upload_id: &str,
) -> Result<bool, TransferError> {
    let known: i64 = sqlx::query_scalar(PROVIDER_IDENTITY_KNOWN)
        .bind(bucket)
        .bind(key)
        .bind(upload_id)
        .fetch_one(connection)
        .await?;
    Ok(known != 0)
}

pub(super) async fn prune_terminal(
    connection: &mut SqliteConnection,
    cutoff: Timestamp,
    limit: u32,
) -> Result<u64, TransferError> {
    Ok(sqlx::query(PRUNE_TERMINAL)
        .bind(cutoff.to_string())
        .bind(i64::from(limit))
        .execute(connection)
        .await?
        .rows_affected())
}
