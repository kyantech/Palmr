use sqlx::sqlite::SqliteRow;
use sqlx::SqliteConnection;

use crate::domain::bytes::ByteSize;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::folders::FolderId;
use crate::features::users::model::UserId;

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferSessionId, UploadKind};
use super::super::repo::{bytes, column, invariant, optional_bytes, optional_parsed, parsed};
use super::super::state::{ItemState, TransferSessionState};

pub enum TusUpload {}

pub type TusUploadId = Id<TusUpload>;

const SELECT_BINDING: &str = "SELECT ts.state AS session_state, ts.target_folder_id,
        ts.deleted_target_folder_id, ts.expires_at AS session_expires_at,
        t.state AS item_state, t.upload_kind, t.display_name, t.relative_path,
        t.declared_size_bytes, t.reserved_bytes,
        tu.id AS tus_id, tu.upload_length, tu.upload_defer_length, tu.upload_offset,
        tu.state AS tus_state, tu.expires_at AS tus_expires_at
      FROM transfer_sessions ts
      JOIN transfer_session_files t ON t.transfer_session_id = ts.id
      LEFT JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
     WHERE ts.id = ?1 AND ts.user_id = ?2 AND t.id = ?3";

const SELECT_UPLOAD: &str = "SELECT tu.id, tu.transfer_session_file_id, t.transfer_session_id,
        tu.upload_length, tu.upload_offset, tu.state, tu.expires_at
      FROM tus_uploads tu
      JOIN transfer_session_files t ON t.id = tu.transfer_session_file_id
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
     WHERE tu.id = ?1 AND ts.user_id = ?2 AND tu.owner_user_id = ?2";

const INSERT_UPLOAD: &str = "INSERT INTO tus_uploads
        (id, transfer_session_file_id, owner_user_id, upload_length, upload_defer_length,
         upload_offset, staging_path, metadata_json, state, locked_by, lock_expires_at,
         created_at, updated_at, expires_at)
      VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, 'created', ?8, ?9, ?10, ?10, ?11)";

const START_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'uploading', updated_at = ?3
    WHERE id = ?1 AND transfer_session_id = ?2 AND state = 'pending'";

const PERSIST_OFFSET: &str = "UPDATE tus_uploads
      SET upload_offset = ?2,
          state = CASE WHEN ?2 > 0 THEN 'in_progress' ELSE state END,
          last_patch_at = CASE WHEN ?2 > upload_offset THEN ?3 ELSE last_patch_at END,
          updated_at = ?3,
          locked_by = CASE WHEN ?6 = 1 THEN NULL ELSE locked_by END,
          lock_expires_at = CASE WHEN ?6 = 1 THEN NULL ELSE ?5 END
    WHERE id = ?1 AND state IN ('created', 'in_progress') AND locked_by = ?4";

const OTHER_RUNNING_BYTES: &str = "SELECT COALESCE(SUM(tu.upload_offset), 0)
      FROM tus_uploads tu
      JOIN transfer_session_files t ON t.id = tu.transfer_session_file_id
     WHERE t.transfer_session_id = ?1 AND tu.id <> ?2
       AND tu.state IN ('created', 'in_progress')
       AND t.declared_size_bytes IS NULL AND t.reserved_bytes = 0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TusState {
    Created,
    InProgress,
    Completed,
    Terminated,
    Expired,
}

impl TusState {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "created" => Some(Self::Created),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "terminated" => Some(Self::Terminated),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ItemBinding {
    pub state: ItemState,
    pub kind: UploadKind,
    pub name: String,
    pub directory: String,
    pub declared: Option<ByteSize>,
    pub reserved: ByteSize,
}

#[derive(Debug, Clone)]
pub struct ExistingUpload {
    pub id: TusUploadId,
    pub length: Option<ByteSize>,
    pub defer: bool,
    pub offset: ByteSize,
    pub state: TusState,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct CreateBinding {
    pub session_state: TransferSessionState,
    pub target_folder: Option<FolderId>,
    pub target_detached: bool,
    pub session_expires_at: Timestamp,
    pub item: ItemBinding,
    pub existing: Option<ExistingUpload>,
}

#[derive(Debug, Clone)]
pub struct UploadRow {
    pub id: TusUploadId,
    pub item: SessionItemId,
    pub session: TransferSessionId,
    pub length: Option<ByteSize>,
    pub offset: ByteSize,
    pub state: TusState,
    pub expires_at: Timestamp,
}

pub struct NewUpload<'a> {
    pub id: TusUploadId,
    pub item: SessionItemId,
    pub owner: UserId,
    pub length: Option<ByteSize>,
    pub staging_path: &'a str,
    pub metadata_json: &'a str,
    pub lease: Option<(&'a str, Timestamp)>,
    pub now: Timestamp,
    pub expires_at: Timestamp,
}

fn tus_state(row: &SqliteRow, name: &'static str) -> Result<TusState, TransferError> {
    let text: String = column(row, name)?;
    TusState::parse(&text).ok_or_else(|| invariant(name))
}

fn binding_from(row: &SqliteRow) -> Result<CreateBinding, TransferError> {
    let session_state: String = column(row, "session_state")?;
    let item_state: String = column(row, "item_state")?;
    let kind: String = column(row, "upload_kind")?;
    let detached: Option<String> = column(row, "deleted_target_folder_id")?;
    let existing = match optional_parsed::<TusUploadId>(row, "tus_id")? {
        None => None,
        Some(id) => {
            let defer: i64 = column(row, "upload_defer_length")?;
            Some(ExistingUpload {
                id,
                length: optional_bytes(row, "upload_length")?,
                defer: defer != 0,
                offset: bytes(row, "upload_offset")?,
                state: tus_state(row, "tus_state")?,
                expires_at: parsed(row, "tus_expires_at")?,
            })
        }
    };
    Ok(CreateBinding {
        session_state: TransferSessionState::parse(&session_state)
            .ok_or_else(|| invariant("state"))?,
        target_folder: optional_parsed(row, "target_folder_id")?,
        target_detached: detached.is_some(),
        session_expires_at: parsed(row, "session_expires_at")?,
        item: ItemBinding {
            state: ItemState::parse(&item_state).ok_or_else(|| invariant("item_state"))?,
            kind: UploadKind::parse(&kind).ok_or_else(|| invariant("upload_kind"))?,
            name: column(row, "display_name")?,
            directory: column(row, "relative_path")?,
            declared: optional_bytes(row, "declared_size_bytes")?,
            reserved: bytes(row, "reserved_bytes")?,
        },
        existing,
    })
}

fn upload_from(row: &SqliteRow) -> Result<UploadRow, TransferError> {
    Ok(UploadRow {
        id: parsed(row, "id")?,
        item: parsed(row, "transfer_session_file_id")?,
        session: parsed(row, "transfer_session_id")?,
        length: optional_bytes(row, "upload_length")?,
        offset: bytes(row, "upload_offset")?,
        state: tus_state(row, "state")?,
        expires_at: parsed(row, "expires_at")?,
    })
}

pub async fn find_binding(
    connection: &mut SqliteConnection,
    owner: UserId,
    session: TransferSessionId,
    item: SessionItemId,
) -> Result<Option<CreateBinding>, TransferError> {
    let row = sqlx::query(SELECT_BINDING)
        .bind(session.to_string())
        .bind(owner.to_string())
        .bind(item.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(binding_from).transpose()
}

pub async fn find_upload(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: TusUploadId,
) -> Result<Option<UploadRow>, TransferError> {
    let row = sqlx::query(SELECT_UPLOAD)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(upload_from).transpose()
}

pub async fn insert_upload(
    connection: &mut SqliteConnection,
    upload: &NewUpload<'_>,
) -> Result<(), TransferError> {
    sqlx::query(INSERT_UPLOAD)
        .bind(upload.id.to_string())
        .bind(upload.item.to_string())
        .bind(upload.owner.to_string())
        .bind(upload.length.map(ByteSize::to_i64))
        .bind(i64::from(upload.length.is_none()))
        .bind(upload.staging_path)
        .bind(upload.metadata_json)
        .bind(upload.lease.map(|(holder, _)| holder))
        .bind(upload.lease.map(|(_, until)| until.to_string()))
        .bind(upload.now.to_string())
        .bind(upload.expires_at.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn start_item(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    item: SessionItemId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(START_ITEM)
        .bind(item.to_string())
        .bind(session.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub struct OffsetWrite<'a> {
    pub id: TusUploadId,
    pub offset: u64,
    pub holder: &'a str,
    pub lease_until: Timestamp,
    pub release: bool,
    pub now: Timestamp,
}

pub async fn persist_offset(
    connection: &mut SqliteConnection,
    write: &OffsetWrite<'_>,
) -> Result<bool, TransferError> {
    let offset = ByteSize::try_from(write.offset).map_err(|_| invariant("upload_offset"))?;
    let affected = sqlx::query(PERSIST_OFFSET)
        .bind(write.id.to_string())
        .bind(offset.to_i64())
        .bind(write.now.to_string())
        .bind(write.holder)
        .bind(write.lease_until.to_string())
        .bind(i64::from(write.release))
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn other_running_bytes(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    upload: TusUploadId,
) -> Result<ByteSize, TransferError> {
    let total: i64 = sqlx::query_scalar(OTHER_RUNNING_BYTES)
        .bind(session.to_string())
        .bind(upload.to_string())
        .fetch_one(connection)
        .await?;
    ByteSize::try_from(total).map_err(|_| invariant("running_bytes"))
}
