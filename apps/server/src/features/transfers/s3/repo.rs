use serde_json::json;
use sqlx::SqliteConnection;

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;

use super::super::error::TransferError;
use super::super::model::SessionItemId;
use super::model::MultipartId;
use super::reconcile::{Held, PartLayout};

const WRITE_BATCH: usize = 1_000;

const INSERT_MULTIPART: &str = "INSERT INTO s3_multipart_uploads
        (id, transfer_session_file_id, s3_upload_id, bucket, object_key, owner_user_id,
         total_size_bytes, part_size_bytes, part_count, state, created_at, updated_at, expires_at)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'created', ?10, ?10, ?11)";

const INSERT_PART_PLAN: &str = "WITH RECURSIVE numbers(part_number) AS (
        SELECT 1 UNION ALL SELECT part_number + 1 FROM numbers WHERE part_number < ?2)
      INSERT INTO s3_multipart_parts (s3_multipart_upload_id, part_number, size_bytes, state)
      SELECT ?1, part_number, CASE WHEN part_number < ?2 THEN ?3 ELSE ?4 END, 'planned'
        FROM numbers";

const TOUCH_OPEN: &str = "UPDATE s3_multipart_uploads
      SET state = CASE WHEN state = 'created' AND ?4 = 1 THEN 'in_progress' ELSE state END,
          updated_at = ?2, expires_at = MAX(expires_at, ?3)
    WHERE id = ?1 AND state IN ('created', 'in_progress')";

const MARK_RECONCILED: &str = "UPDATE s3_multipart_uploads
      SET state = CASE WHEN state = 'created' AND ?4 = 1 THEN 'in_progress' ELSE state END,
          last_reconciled_at = ?2, updated_at = ?2, expires_at = MAX(expires_at, ?3)
    WHERE id = ?1 AND state IN ('created', 'in_progress')";

const MARK_SIGNED: &str = "UPDATE s3_multipart_parts
      SET state = 'signed', signed_at = ?3, signed_expires_at = ?4, attempts = attempts + 1
    WHERE s3_multipart_upload_id = ?1 AND state IN ('planned', 'signed', 'failed')
      AND part_number IN (SELECT value FROM json_each(?2))";

const RECORD_REPORTS: &str = "UPDATE s3_multipart_parts
      SET state = 'uploaded', etag = reported.etag, uploaded_at = ?3
     FROM (SELECT json_extract(value, '$.n') AS part_number, json_extract(value, '$.e') AS etag
             FROM json_each(?2)) AS reported
    WHERE s3_multipart_parts.s3_multipart_upload_id = ?1
      AND s3_multipart_parts.part_number = reported.part_number
      AND s3_multipart_parts.state IN ('planned', 'signed', 'failed', 'uploaded')";

const VERIFY_PRESENT: &str = "UPDATE s3_multipart_parts
      SET state = 'verified', etag = held.etag, size_bytes = held.size,
          uploaded_at = COALESCE(uploaded_at, ?3)
     FROM (SELECT json_extract(value, '$.n') AS part_number, json_extract(value, '$.e') AS etag,
                  json_extract(value, '$.s') AS size
             FROM json_each(?2)) AS held
    WHERE s3_multipart_parts.s3_multipart_upload_id = ?1
      AND s3_multipart_parts.part_number = held.part_number";

const DEMOTE_ABSENT: &str = "UPDATE s3_multipart_parts
      SET state = 'planned', etag = NULL, uploaded_at = NULL,
          size_bytes = CASE WHEN part_number < ?3 THEN ?4 ELSE ?5 END
    WHERE s3_multipart_upload_id = ?1 AND state IN ('uploaded', 'verified')
      AND part_number NOT IN (SELECT value FROM json_each(?2))
      AND (uploaded_at IS NULL OR uploaded_at <= ?6)";

const FINISH_ABORT: &str = "UPDATE s3_multipart_uploads
      SET state = 'aborted', updated_at = ?2, completed_at = ?2
    WHERE id = ?1 AND state = 'abandoned'";

const RECORD_INTENT_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'finalizing', finalize_stage = 'placing', updated_at = ?2
    WHERE id = ?1 AND state = 'finalizing' AND finalize_stage IN ('none', 'placing')
      AND EXISTS (SELECT 1 FROM transfer_sessions ts
                   WHERE ts.id = transfer_session_files.transfer_session_id
                     AND ts.cancel_requested = 0
                     AND ts.state IN ('created', 'uploading', 'finalizing', 'failed'))";

const MARK_COMPLETING: &str = "UPDATE s3_multipart_uploads
      SET state = 'completing', updated_at = ?2
    WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress', 'completing')";

const REVERT_INTENT_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'uploading', finalize_stage = 'none', updated_at = ?2
    WHERE id = ?1 AND state = 'finalizing' AND finalize_stage = 'placing'";

const REVERT_COMPLETING: &str = "UPDATE s3_multipart_uploads
      SET state = 'in_progress', updated_at = ?2
    WHERE transfer_session_file_id = ?1 AND state = 'completing'";

const CLAIM_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'finalizing', updated_at = ?2
    WHERE id = ?1 AND state IN ('pending', 'uploading', 'finalizing')
      AND finalize_stage <> 'committed'
      AND EXISTS (SELECT 1 FROM transfer_sessions ts
                   WHERE ts.id = transfer_session_files.transfer_session_id
                     AND ts.cancel_requested = 0
                     AND ts.state IN ('created', 'uploading', 'finalizing', 'failed'))";

const HELD_BYTES: &str = "SELECT COALESCE(SUM(reserved_bytes), 0) FROM quota_reservations
    WHERE transfer_session_id = ?1 AND state = 'held'";

const OPEN_FILE: &str = "SELECT id, size_bytes, mime_type FROM files WHERE id = ?1";

pub struct NewMultipart<'a> {
    pub id: MultipartId,
    pub item: SessionItemId,
    pub owner: UserId,
    pub upload_id: &'a str,
    pub bucket: &'a str,
    pub object_key: &'a str,
    pub layout: PartLayout,
    pub now: Timestamp,
    pub expires_at: Timestamp,
}

fn bound(value: u64) -> Result<i64, TransferError> {
    i64::try_from(value).map_err(|_| TransferError::Invariant { what: "part_plan" })
}

pub async fn insert_multipart(
    connection: &mut SqliteConnection,
    new: &NewMultipart<'_>,
) -> Result<(), TransferError> {
    sqlx::query(INSERT_MULTIPART)
        .bind(new.id.to_string())
        .bind(new.item.to_string())
        .bind(new.upload_id)
        .bind(new.bucket)
        .bind(new.object_key)
        .bind(new.owner.to_string())
        .bind(bound(new.layout.total)?)
        .bind(bound(new.layout.part_size)?)
        .bind(i64::from(new.layout.part_count))
        .bind(new.now.to_string())
        .bind(new.expires_at.to_string())
        .execute(&mut *connection)
        .await?;
    let last = new
        .layout
        .expected(new.layout.part_count)
        .ok_or(TransferError::Invariant { what: "part_plan" })?;
    sqlx::query(INSERT_PART_PLAN)
        .bind(new.id.to_string())
        .bind(i64::from(new.layout.part_count))
        .bind(bound(new.layout.part_size)?)
        .bind(bound(last)?)
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn touch_open(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
    expires_at: Timestamp,
    promote: bool,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(TOUCH_OPEN)
        .bind(id.to_string())
        .bind(now.to_string())
        .bind(expires_at.to_string())
        .bind(i64::from(promote))
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn mark_reconciled(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
    expires_at: Timestamp,
    promote: bool,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(MARK_RECONCILED)
        .bind(id.to_string())
        .bind(now.to_string())
        .bind(expires_at.to_string())
        .bind(i64::from(promote))
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn mark_signed(
    connection: &mut SqliteConnection,
    id: MultipartId,
    numbers: &[u32],
    now: Timestamp,
    signed_expires: Timestamp,
) -> Result<(), TransferError> {
    sqlx::query(MARK_SIGNED)
        .bind(id.to_string())
        .bind(json!(numbers).to_string())
        .bind(now.to_string())
        .bind(signed_expires.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn record_reports(
    connection: &mut SqliteConnection,
    id: MultipartId,
    reports: &[(u32, &str)],
    now: Timestamp,
) -> Result<(), TransferError> {
    for chunk in reports.chunks(WRITE_BATCH) {
        let rows: Vec<_> = chunk
            .iter()
            .map(|(number, etag)| json!({"n": number, "e": etag}))
            .collect();
        sqlx::query(RECORD_REPORTS)
            .bind(id.to_string())
            .bind(json!(rows).to_string())
            .bind(now.to_string())
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

pub async fn verify_present(
    connection: &mut SqliteConnection,
    id: MultipartId,
    held: &[Held],
    now: Timestamp,
) -> Result<(), TransferError> {
    for chunk in held.chunks(WRITE_BATCH) {
        let rows: Vec<_> = chunk
            .iter()
            .filter(|part| {
                part.size > 0 && part.etag.chars().count() <= super::model::MAX_ETAG_CHARS
            })
            .map(|part| json!({"n": part.part_number, "e": part.etag, "s": part.size}))
            .collect();
        if rows.is_empty() {
            continue;
        }
        sqlx::query(VERIFY_PRESENT)
            .bind(id.to_string())
            .bind(json!(rows).to_string())
            .bind(now.to_string())
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

pub async fn demote_absent(
    connection: &mut SqliteConnection,
    id: MultipartId,
    layout: &PartLayout,
    present: &[u32],
    snapshot_started: Timestamp,
) -> Result<(), TransferError> {
    let last = layout
        .expected(layout.part_count)
        .ok_or(TransferError::Invariant { what: "part_plan" })?;
    sqlx::query(DEMOTE_ABSENT)
        .bind(id.to_string())
        .bind(json!(present).to_string())
        .bind(i64::from(layout.part_count))
        .bind(bound(layout.part_size)?)
        .bind(bound(last)?)
        .bind(snapshot_started.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn finish_abort(
    connection: &mut SqliteConnection,
    id: MultipartId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(FINISH_ABORT)
        .bind(id.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn claim_item(
    connection: &mut SqliteConnection,
    item: SessionItemId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(CLAIM_ITEM)
        .bind(item.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn record_intent(
    connection: &mut SqliteConnection,
    item: SessionItemId,
    zero_byte: bool,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let marked = sqlx::query(RECORD_INTENT_ITEM)
        .bind(item.to_string())
        .bind(now.to_string())
        .execute(&mut *connection)
        .await?
        .rows_affected();
    if marked != 1 {
        return Ok(false);
    }
    if zero_byte {
        return Ok(true);
    }
    let completing = sqlx::query(MARK_COMPLETING)
        .bind(item.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(completing == 1)
}

pub async fn revert_intent(
    connection: &mut SqliteConnection,
    item: SessionItemId,
    now: Timestamp,
) -> Result<(), TransferError> {
    sqlx::query(REVERT_INTENT_ITEM)
        .bind(item.to_string())
        .bind(now.to_string())
        .execute(&mut *connection)
        .await?;
    sqlx::query(REVERT_COMPLETING)
        .bind(item.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub id: String,
    pub size: ByteSize,
    pub mime: String,
}

pub async fn find_file(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<Option<FileRow>, TransferError> {
    let row: Option<(String, i64, String)> = sqlx::query_as(OPEN_FILE)
        .bind(id)
        .fetch_optional(connection)
        .await?;
    row.map(|(id, size, mime)| {
        Ok(FileRow {
            id,
            size: ByteSize::try_from(size)
                .map_err(|_| TransferError::Invariant { what: "file_size" })?,
            mime,
        })
    })
    .transpose()
}

pub async fn held_bytes(
    connection: &mut SqliteConnection,
    session: super::super::model::TransferSessionId,
) -> Result<ByteSize, TransferError> {
    let held: i64 = sqlx::query_scalar(HELD_BYTES)
        .bind(session.to_string())
        .fetch_one(connection)
        .await?;
    ByteSize::try_from(held).map_err(|_| TransferError::Invariant { what: "held_bytes" })
}
