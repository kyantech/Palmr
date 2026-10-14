use sqlx::sqlite::SqliteRow;
use sqlx::SqliteConnection;

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::folders::FolderId;
use crate::features::users::model::UserId;
use crate::storage::lifecycle::StorageObjectId;

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferProvider, TransferSessionId, UploadKind};
use super::super::repo::{
    bytes, column, invariant, optional_bytes, optional_parsed, parsed, FinalizeStage,
};
use super::super::state::{ItemState, TransferSessionState};
use super::super::tus::repo::{TusState, TusUploadId};

const LOAD_CONTEXT: &str = "SELECT t.id, t.transfer_session_id, t.state AS item_state,
        t.finalize_stage, t.upload_kind, t.display_name, t.relative_path,
        t.declared_size_bytes, t.reserved_bytes, t.final_object_id, t.final_object_key,
        t.resulting_file_id, t.error_code,
        ts.context, ts.user_id, ts.provider, ts.state AS session_state, ts.cancel_requested,
        ts.expires_at AS session_expires_at, ts.target_folder_id,
        ts.completed_file_count, ts.completed_bytes,
        u.username AS owner_label,
        tu.id AS tus_id, tu.state AS tus_state, tu.upload_length, tu.upload_offset,
        tu.metadata_json
      FROM transfer_session_files t
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
      LEFT JOIN users u ON u.id = ts.user_id
      LEFT JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
     WHERE t.id = ?1";

const ACQUIRE_FINALIZE_LEASE: &str = "UPDATE tus_uploads
      SET locked_by = ?2, lock_expires_at = ?3, updated_at = ?4
    WHERE id = ?1 AND state IN ('created', 'in_progress')
      AND (locked_by IS NULL OR locked_by = ?2 OR lock_expires_at <= ?4)";

const RELEASE_FINALIZE_LEASE: &str = "UPDATE tus_uploads
      SET locked_by = NULL, lock_expires_at = NULL, updated_at = ?3
    WHERE id = ?1 AND locked_by = ?2";

const RENEW_FINALIZE_LEASE: &str = "UPDATE tus_uploads
      SET lock_expires_at = ?3, updated_at = ?4
    WHERE id = ?1 AND locked_by = ?2 AND state IN ('created', 'in_progress')";

const ENTER_FINALIZING: &str = "UPDATE transfer_session_files
      SET state = 'finalizing', updated_at = ?2
    WHERE id = ?1 AND state IN ('uploading', 'finalizing') AND finalize_stage <> 'committed'";

const RECORD_INTENT: &str = "UPDATE transfer_session_files
      SET finalize_stage = 'placing', updated_at = ?2
    WHERE id = ?1 AND state = 'finalizing' AND finalize_stage IN ('none', 'placing')
      AND EXISTS (SELECT 1 FROM tus_uploads tu
                   WHERE tu.transfer_session_file_id = transfer_session_files.id
                     AND tu.state IN ('created', 'in_progress') AND tu.locked_by = ?3)
      AND EXISTS (SELECT 1 FROM transfer_sessions ts
                   WHERE ts.id = transfer_session_files.transfer_session_id
                     AND ts.cancel_requested = 0)";

const COUNT_STARTING: &str = "SELECT COUNT(*) FROM transfer_session_files
    WHERE transfer_session_id = ?1 AND id <> ?2 AND state IN ('pending', 'uploading')";

const REDRIVE_PAGE: &str = "SELECT t.id
      FROM transfer_session_files t
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
      JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
     WHERE t.upload_kind = 'tus' AND t.id > ?1
       AND ts.state IN ('created', 'uploading', 'finalizing', 'failed')
       AND ts.cancel_requested = 0
       AND ts.expires_at > ?3
       AND tu.state IN ('created', 'in_progress')
       AND (t.state = 'finalizing'
            OR (t.state = 'uploading' AND tu.upload_length IS NOT NULL
                AND tu.upload_offset = tu.upload_length))
     ORDER BY t.id
     LIMIT ?2";

const REDRIVE_COUNT: &str = "SELECT COUNT(*)
      FROM transfer_session_files t
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
      JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
     WHERE t.upload_kind = 'tus'
       AND ts.state IN ('created', 'uploading', 'finalizing', 'failed')
       AND ts.cancel_requested = 0
       AND ts.expires_at > ?1
       AND tu.state IN ('created', 'in_progress')
       AND (t.state = 'finalizing'
            OR (t.state = 'uploading' AND tu.upload_length IS NOT NULL
                AND tu.upload_offset = tu.upload_length))";

#[derive(Debug, Clone)]
pub struct TusRef {
    pub id: TusUploadId,
    pub state: TusState,
    pub length: Option<ByteSize>,
    pub offset: ByteSize,
    pub metadata_json: String,
}

#[derive(Debug, Clone)]
pub struct ItemContext {
    pub item: SessionItemId,
    pub session: TransferSessionId,
    pub owner: Option<UserId>,
    pub owner_label: Option<String>,
    pub context: String,
    pub provider: TransferProvider,
    pub kind: UploadKind,
    pub item_state: ItemState,
    pub stage: FinalizeStage,
    pub name: String,
    pub directory: String,
    pub declared: Option<ByteSize>,
    pub reserved: ByteSize,
    pub final_id: StorageObjectId,
    pub final_key: String,
    pub file_id: Option<String>,
    pub error_code: Option<String>,
    pub session_state: TransferSessionState,
    pub cancel_requested: bool,
    pub session_expires_at: Timestamp,
    pub target_folder: Option<FolderId>,
    pub completed_files: u32,
    pub completed_bytes: ByteSize,
    pub tus: Option<TusRef>,
}

impl ItemContext {
    pub fn running_quota(&self) -> bool {
        self.declared.is_none() && self.reserved == ByteSize::ZERO
    }

    pub fn filetype_hint(&self) -> Option<String> {
        let tus = self.tus.as_ref()?;
        let parsed: serde_json::Value = serde_json::from_str(&tus.metadata_json).ok()?;
        parsed
            .get("filetype")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }
}

fn context_from(row: &SqliteRow) -> Result<ItemContext, TransferError> {
    let item_state: String = column(row, "item_state")?;
    let stage: String = column(row, "finalize_stage")?;
    let kind: String = column(row, "upload_kind")?;
    let provider: String = column(row, "provider")?;
    let session_state: String = column(row, "session_state")?;
    let cancel: i64 = column(row, "cancel_requested")?;
    let completed_files: i64 = column(row, "completed_file_count")?;
    let tus = match optional_parsed::<TusUploadId>(row, "tus_id")? {
        None => None,
        Some(id) => {
            let tus_state: String = column(row, "tus_state")?;
            Some(TusRef {
                id,
                state: TusState::parse(&tus_state).ok_or_else(|| invariant("tus_state"))?,
                length: optional_bytes(row, "upload_length")?,
                offset: bytes(row, "upload_offset")?,
                metadata_json: column(row, "metadata_json")?,
            })
        }
    };
    Ok(ItemContext {
        item: parsed(row, "id")?,
        session: parsed(row, "transfer_session_id")?,
        owner: optional_parsed(row, "user_id")?,
        owner_label: column(row, "owner_label")?,
        context: column(row, "context")?,
        provider: TransferProvider::parse(&provider).ok_or_else(|| invariant("provider"))?,
        kind: UploadKind::parse(&kind).ok_or_else(|| invariant("upload_kind"))?,
        item_state: ItemState::parse(&item_state).ok_or_else(|| invariant("state"))?,
        stage: match stage.as_str() {
            "none" => FinalizeStage::None,
            "placing" => FinalizeStage::Placing,
            "committed" => FinalizeStage::Committed,
            _ => return Err(invariant("finalize_stage")),
        },
        name: column(row, "display_name")?,
        directory: column(row, "relative_path")?,
        declared: optional_bytes(row, "declared_size_bytes")?,
        reserved: bytes(row, "reserved_bytes")?,
        final_id: parsed(row, "final_object_id")?,
        final_key: column(row, "final_object_key")?,
        file_id: column(row, "resulting_file_id")?,
        error_code: column(row, "error_code")?,
        session_state: TransferSessionState::parse(&session_state)
            .ok_or_else(|| invariant("session_state"))?,
        cancel_requested: cancel != 0,
        session_expires_at: parsed(row, "session_expires_at")?,
        target_folder: optional_parsed(row, "target_folder_id")?,
        completed_files: u32::try_from(completed_files)
            .map_err(|_| invariant("completed_file_count"))?,
        completed_bytes: bytes(row, "completed_bytes")?,
        tus,
    })
}

pub async fn load_context(
    connection: &mut SqliteConnection,
    item: SessionItemId,
) -> Result<Option<ItemContext>, TransferError> {
    let row = sqlx::query(LOAD_CONTEXT)
        .bind(item.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(context_from).transpose()
}

pub async fn acquire_lease(
    connection: &mut SqliteConnection,
    upload: TusUploadId,
    holder: &str,
    until: Timestamp,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(ACQUIRE_FINALIZE_LEASE)
        .bind(upload.to_string())
        .bind(holder)
        .bind(until.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn renew_lease(
    connection: &mut SqliteConnection,
    upload: TusUploadId,
    holder: &str,
    until: Timestamp,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(RENEW_FINALIZE_LEASE)
        .bind(upload.to_string())
        .bind(holder)
        .bind(until.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn release_lease(
    connection: &mut SqliteConnection,
    upload: TusUploadId,
    holder: &str,
    now: Timestamp,
) -> Result<(), TransferError> {
    sqlx::query(RELEASE_FINALIZE_LEASE)
        .bind(upload.to_string())
        .bind(holder)
        .bind(now.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn enter_finalizing(
    connection: &mut SqliteConnection,
    item: SessionItemId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(ENTER_FINALIZING)
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
    holder: &str,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(RECORD_INTENT)
        .bind(item.to_string())
        .bind(now.to_string())
        .bind(holder)
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn other_starting_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    item: SessionItemId,
) -> Result<u32, TransferError> {
    let total: i64 = sqlx::query_scalar(COUNT_STARTING)
        .bind(session.to_string())
        .bind(item.to_string())
        .fetch_one(connection)
        .await?;
    u32::try_from(total).map_err(|_| invariant("count"))
}

pub async fn redrive_page(
    connection: &mut SqliteConnection,
    after: Option<&str>,
    limit: u32,
    now: Timestamp,
) -> Result<Vec<SessionItemId>, TransferError> {
    let rows: Vec<String> = sqlx::query_scalar(REDRIVE_PAGE)
        .bind(after.unwrap_or_default())
        .bind(i64::from(limit.clamp(1, 1_000)))
        .bind(now.to_string())
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|id| id.parse().map_err(|_| invariant("id")))
        .collect()
}

pub async fn redrive_count(
    connection: &mut SqliteConnection,
    now: Timestamp,
) -> Result<u64, TransferError> {
    let total: i64 = sqlx::query_scalar(REDRIVE_COUNT)
        .bind(now.to_string())
        .fetch_one(connection)
        .await?;
    u64::try_from(total).map_err(|_| invariant("count"))
}
