use std::str::FromStr;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::folders::FolderId;
use crate::features::users::model::UserId;
use crate::infra::http::pagination::{Conjunction, PageRequest};
use crate::storage::lifecycle::StorageObjectId;

use super::error::TransferError;
use super::model::{SessionItemId, TransferProvider, TransferSessionId, UploadKind};
use super::state::{ItemState, TransferSessionState};

const INSERT_BATCH_ROWS: usize = 200;

const SESSION_COLUMNS: &str = "ts.id, ts.provider, ts.state, ts.target_folder_id,
        ts.deleted_target_folder_id, ts.declared_file_count, ts.declared_bytes,
        ts.completed_file_count, ts.completed_bytes, ts.error_code, ts.error_request_id,
        ts.created_at, ts.updated_at, ts.expires_at,
        COALESCE(q.reserved_bytes, 0) AS held_bytes";

const HELD_JOIN: &str =
    "LEFT JOIN quota_reservations q ON q.transfer_session_id = ts.id AND q.state = 'held'";

const IN_FLIGHT_PROGRESS: &str = "'uploading', 'finalizing', 'failed'";

const SELECT_SESSION: &str = "SELECT ts.id, ts.provider, ts.state, ts.target_folder_id,
        ts.deleted_target_folder_id, ts.declared_file_count, ts.declared_bytes,
        ts.completed_file_count, ts.completed_bytes, ts.error_code, ts.error_request_id,
        ts.created_at, ts.updated_at, ts.expires_at,
        COALESCE(q.reserved_bytes, 0) AS held_bytes, 0 AS tus_bytes, 0 AS s3_bytes
      FROM transfer_sessions ts
      LEFT JOIN quota_reservations q ON q.transfer_session_id = ts.id AND q.state = 'held'
     WHERE ts.id = ?1 AND ts.user_id = ?2";

const SELECT_ITEMS: &str = "SELECT t.id, t.ordinal, t.client_file_key, t.display_name,
        t.relative_path, t.declared_size_bytes, t.reserved_bytes, t.upload_kind, t.state,
        t.finalize_stage, t.attempts, t.resulting_file_id, t.error_code, t.error_request_id,
        COALESCE(f.size_bytes, rf.size_bytes) AS result_size,
        tu.upload_offset AS tus_offset,
        tu.id IS NOT NULL AS has_tus,
        s.id IS NOT NULL AS has_s3,
        s.part_size_bytes AS s3_part_size, s.part_count AS s3_part_count,
        (SELECT COALESCE(SUM(p.size_bytes), 0) FROM s3_multipart_parts p
          WHERE p.s3_multipart_upload_id = s.id AND p.state IN ('uploaded', 'verified')) AS s3_bytes,
        (SELECT COUNT(*) FROM s3_multipart_parts p
          WHERE p.s3_multipart_upload_id = s.id AND p.state IN ('uploaded', 'verified')) AS s3_parts
      FROM transfer_session_files t
      LEFT JOIN files f ON f.id = t.resulting_file_id
      LEFT JOIN received_files rf ON rf.id = t.resulting_received_file_id
      LEFT JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
      LEFT JOIN s3_multipart_uploads s ON s.transfer_session_file_id = t.id
     WHERE t.transfer_session_id = ?1
     ORDER BY t.ordinal";

const OWNER_ACTIVE: &str = "SELECT is_active FROM users WHERE id = ?1";

const INSERT_SESSION: &str = "INSERT INTO transfer_sessions
        (id, context, user_id, provider, state, target_folder_id, declared_file_count,
         declared_bytes, created_at, updated_at, expires_at)
      VALUES (?1, 'my_files', ?2, ?3, 'created', ?4, ?5, ?6, ?7, ?7, ?8)";

const LIVE_ITEMS: &str = "SELECT id, state, finalize_stage, reserved_bytes, final_object_id,
        final_object_key
      FROM transfer_session_files
     WHERE transfer_session_id = ?1 AND state IN ('pending', 'uploading', 'finalizing', 'failed')
     ORDER BY ordinal";

const ONE_ITEM: &str = "SELECT id, state, finalize_stage, reserved_bytes, final_object_id,
        final_object_key
      FROM transfer_session_files WHERE id = ?1 AND transfer_session_id = ?2";

const CANCEL_ITEMS: &str = "UPDATE transfer_session_files
      SET state = 'canceled', updated_at = ?2
    WHERE id IN (SELECT value FROM json_each(?1))
      AND state IN ('pending', 'uploading', 'finalizing', 'failed')";

const TERMINATE_TUS: &str = "UPDATE tus_uploads SET state = 'terminated', updated_at = ?2
    WHERE transfer_session_file_id IN (SELECT value FROM json_each(?1))
      AND state IN ('created', 'in_progress')";

const ABANDON_MULTIPART: &str =
    "UPDATE s3_multipart_uploads SET state = 'abandoned', updated_at = ?2
    WHERE transfer_session_file_id IN (SELECT value FROM json_each(?1))
      AND state IN ('created', 'in_progress', 'completing')";

const MOVE_SESSION: &str = "UPDATE transfer_sessions
      SET state = ?3, updated_at = ?4, completed_at = ?5, cancel_requested = ?6,
          error_code = CASE WHEN ?3 = 'uploading' THEN NULL ELSE error_code END,
          error_request_id = CASE WHEN ?3 = 'uploading' THEN NULL ELSE error_request_id END
    WHERE id = ?1 AND user_id = ?2 AND state = ?7";

const TOUCH_SESSION: &str = "UPDATE transfer_sessions SET updated_at = ?2 WHERE id = ?1";

const RETRY_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'uploading', attempts = attempts + 1, error_code = NULL,
          error_request_id = NULL, updated_at = ?3
    WHERE id = ?1 AND transfer_session_id = ?2 AND state = 'failed'";

const PROTOCOL_PRESENT: &str = "SELECT EXISTS(SELECT 1 FROM tus_uploads
        WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress'))
      OR EXISTS(SELECT 1 FROM s3_multipart_uploads
        WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress'))";

const FAIL_ITEM: &str = "UPDATE transfer_session_files
      SET state = 'failed', error_code = ?3, error_request_id = ?4, updated_at = ?5
    WHERE id = ?1 AND transfer_session_id = ?2 AND state IN ('uploading', 'finalizing')";

const RECORD_SESSION_ERROR: &str =
    "UPDATE transfer_sessions SET error_code = ?2, error_request_id = ?3 WHERE id = ?1";

const COUNT_IN_FLIGHT: &str = "SELECT COUNT(*) FROM transfer_session_files
    WHERE transfer_session_id = ?1 AND state IN ('pending', 'uploading', 'finalizing')";

const COUNT_FAILED: &str = "SELECT COUNT(*) FROM transfer_session_files
    WHERE transfer_session_id = ?1 AND state = 'failed'";

const COUNT_REMAINING: &str = "SELECT COUNT(*) FROM transfer_session_files
    WHERE transfer_session_id = ?1 AND state NOT IN ('canceled', 'expired', 'skipped')";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorParts {
    pub code: String,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: TransferSessionId,
    pub provider: TransferProvider,
    pub state: TransferSessionState,
    pub target_folder_id: Option<FolderId>,
    pub deleted_target_folder_id: Option<String>,
    pub declared_file_count: u32,
    pub declared_bytes: ByteSize,
    pub completed_file_count: u32,
    pub completed_bytes: ByteSize,
    pub error: Option<ErrorParts>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub expires_at: Timestamp,
    pub held: ByteSize,
    pub uploaded: ByteSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeStage {
    None,
    Placing,
    Committed,
}

impl FinalizeStage {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "placing" => Some(Self::Placing),
            "committed" => Some(Self::Committed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub id: SessionItemId,
    pub ordinal: u32,
    pub client_key: String,
    pub name: String,
    pub directory: String,
    pub declared: Option<ByteSize>,
    pub reserved: ByteSize,
    pub kind: UploadKind,
    pub state: ItemState,
    pub finalize_stage: FinalizeStage,
    pub attempts: u32,
    pub file_id: Option<String>,
    pub result_size: Option<ByteSize>,
    pub error: Option<ErrorParts>,
    pub has_tus: bool,
    pub has_s3: bool,
    pub uploaded: ByteSize,
    pub completed_parts: Option<u32>,
    pub stored_plan: Option<(ByteSize, u32)>,
}

#[derive(Debug, Clone)]
pub struct LiveItem {
    pub id: SessionItemId,
    pub state: ItemState,
    pub finalize_stage: FinalizeStage,
    pub reserved: ByteSize,
    pub final_object_id: StorageObjectId,
    pub final_object_key: String,
}

pub struct NewSession {
    pub id: TransferSessionId,
    pub owner: UserId,
    pub provider: TransferProvider,
    pub target: Option<FolderId>,
    pub file_count: u32,
    pub declared_bytes: ByteSize,
    pub now: Timestamp,
    pub expires_at: Timestamp,
}

pub struct NewItem<'a> {
    pub id: SessionItemId,
    pub ordinal: u32,
    pub client_key: &'a str,
    pub name: &'a str,
    pub directory: String,
    pub declared: Option<ByteSize>,
    pub reserved: ByteSize,
    pub kind: UploadKind,
    pub final_object_id: StorageObjectId,
    pub final_object_key: &'a str,
}

pub(super) fn invariant(what: &'static str) -> TransferError {
    TransferError::Invariant { what }
}

pub(super) fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, TransferError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

pub(super) fn parsed<T: FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, TransferError> {
    let text: String = column(row, name)?;
    text.parse().map_err(|_| invariant(name))
}

pub(super) fn optional_parsed<T: FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<T>, TransferError> {
    let text: Option<String> = column(row, name)?;
    text.map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

pub(super) fn bytes(row: &SqliteRow, name: &'static str) -> Result<ByteSize, TransferError> {
    let value: i64 = column(row, name)?;
    ByteSize::try_from(value).map_err(|_| invariant(name))
}

pub(super) fn optional_bytes(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<ByteSize>, TransferError> {
    let value: Option<i64> = column(row, name)?;
    value
        .map(|value| ByteSize::try_from(value).map_err(|_| invariant(name)))
        .transpose()
}

fn small_count(row: &SqliteRow, name: &'static str) -> Result<u32, TransferError> {
    let value: i64 = column(row, name)?;
    u32::try_from(value).map_err(|_| invariant(name))
}

fn error_parts(row: &SqliteRow) -> Result<Option<ErrorParts>, TransferError> {
    let code: Option<String> = column(row, "error_code")?;
    let request_id: Option<String> = column(row, "error_request_id")?;
    Ok(code.map(|code| ErrorParts { code, request_id }))
}

fn session_from(row: &SqliteRow) -> Result<SessionRow, TransferError> {
    let provider: String = column(row, "provider")?;
    let state: String = column(row, "state")?;
    let tus: i64 = column(row, "tus_bytes")?;
    let s3: i64 = column(row, "s3_bytes")?;
    let in_flight = tus.checked_add(s3).ok_or_else(|| invariant("progress"))?;
    let completed = bytes(row, "completed_bytes")?;
    let uploaded = ByteSize::try_from(in_flight)
        .ok()
        .and_then(|in_flight| completed.checked_add(in_flight))
        .ok_or_else(|| invariant("progress"))?;
    Ok(SessionRow {
        id: parsed(row, "id")?,
        provider: TransferProvider::parse(&provider).ok_or_else(|| invariant("provider"))?,
        state: TransferSessionState::parse(&state).ok_or_else(|| invariant("state"))?,
        target_folder_id: optional_parsed(row, "target_folder_id")?,
        deleted_target_folder_id: column(row, "deleted_target_folder_id")?,
        declared_file_count: small_count(row, "declared_file_count")?,
        declared_bytes: bytes(row, "declared_bytes")?,
        completed_file_count: small_count(row, "completed_file_count")?,
        completed_bytes: completed,
        error: error_parts(row)?,
        created_at: parsed(row, "created_at")?,
        updated_at: parsed(row, "updated_at")?,
        expires_at: parsed(row, "expires_at")?,
        held: bytes(row, "held_bytes")?,
        uploaded,
    })
}

fn item_from(row: &SqliteRow) -> Result<ItemRow, TransferError> {
    let kind: String = column(row, "upload_kind")?;
    let state: String = column(row, "state")?;
    let stage: String = column(row, "finalize_stage")?;
    let state = ItemState::parse(&state).ok_or_else(|| invariant("state"))?;
    let has_tus: i64 = column(row, "has_tus")?;
    let has_s3: i64 = column(row, "has_s3")?;
    let result_size = optional_bytes(row, "result_size")?;
    let tus_offset: Option<i64> = column(row, "tus_offset")?;
    let s3_bytes: i64 = column(row, "s3_bytes")?;
    let s3_parts: i64 = column(row, "s3_parts")?;
    let part_size: Option<i64> = column(row, "s3_part_size")?;
    let part_count: Option<i64> = column(row, "s3_part_count")?;
    let declared = optional_bytes(row, "declared_size_bytes")?;
    let progress = match state {
        ItemState::Completed => result_size.unwrap_or(ByteSize::ZERO),
        ItemState::Uploading | ItemState::Finalizing | ItemState::Failed => {
            let raw = tus_offset.unwrap_or(0).max(s3_bytes);
            ByteSize::try_from(raw).map_err(|_| invariant("progress"))?
        }
        ItemState::Pending | ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
            ByteSize::ZERO
        }
    };
    let uploaded = match declared {
        Some(declared) => progress.min(declared),
        None => progress,
    };
    let stored_plan = match (part_size, part_count) {
        (Some(size), Some(parts)) => Some((
            ByteSize::try_from(size).map_err(|_| invariant("part_size"))?,
            u32::try_from(parts).map_err(|_| invariant("part_count"))?,
        )),
        _ => None,
    };
    Ok(ItemRow {
        id: parsed(row, "id")?,
        ordinal: small_count(row, "ordinal")?,
        client_key: column(row, "client_file_key")?,
        name: column(row, "display_name")?,
        directory: column(row, "relative_path")?,
        declared,
        reserved: bytes(row, "reserved_bytes")?,
        kind: UploadKind::parse(&kind).ok_or_else(|| invariant("upload_kind"))?,
        state,
        finalize_stage: FinalizeStage::parse(&stage).ok_or_else(|| invariant("finalize_stage"))?,
        attempts: small_count(row, "attempts")?,
        file_id: column(row, "resulting_file_id")?,
        result_size,
        error: error_parts(row)?,
        has_tus: has_tus != 0,
        has_s3: has_s3 != 0,
        uploaded,
        completed_parts: (has_s3 != 0)
            .then(|| u32::try_from(s3_parts).map_err(|_| invariant("s3_parts")))
            .transpose()?,
        stored_plan,
    })
}

fn live_from(row: &SqliteRow) -> Result<LiveItem, TransferError> {
    let state: String = column(row, "state")?;
    let stage: String = column(row, "finalize_stage")?;
    Ok(LiveItem {
        id: parsed(row, "id")?,
        state: ItemState::parse(&state).ok_or_else(|| invariant("state"))?,
        finalize_stage: FinalizeStage::parse(&stage).ok_or_else(|| invariant("finalize_stage"))?,
        reserved: bytes(row, "reserved_bytes")?,
        final_object_id: parsed(row, "final_object_id")?,
        final_object_key: column(row, "final_object_key")?,
    })
}

pub async fn owner_is_active(
    connection: &mut SqliteConnection,
    owner: UserId,
) -> Result<Option<bool>, TransferError> {
    let active: Option<i64> = sqlx::query_scalar(OWNER_ACTIVE)
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    Ok(active.map(|active| active != 0))
}

pub async fn insert_session(
    connection: &mut SqliteConnection,
    session: &NewSession,
) -> Result<(), TransferError> {
    sqlx::query(INSERT_SESSION)
        .bind(session.id.to_string())
        .bind(session.owner.to_string())
        .bind(session.provider.as_str())
        .bind(session.target.map(|target| target.to_string()))
        .bind(i64::from(session.file_count))
        .bind(session.declared_bytes.to_i64())
        .bind(session.now.to_string())
        .bind(session.expires_at.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn insert_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    items: &[NewItem<'_>],
    now: Timestamp,
) -> Result<(), TransferError> {
    let session = session.to_string();
    let now = now.to_string();
    for chunk in items.chunks(INSERT_BATCH_ROWS) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "INSERT INTO transfer_session_files (id, transfer_session_id, ordinal, \
             client_file_key, display_name, relative_path, declared_size_bytes, reserved_bytes, \
             upload_kind, final_object_id, final_object_key, created_at, updated_at) ",
        );
        query.push_values(chunk, |mut row, item| {
            row.push_bind(item.id.to_string())
                .push_bind(&session)
                .push_bind(i64::from(item.ordinal))
                .push_bind(item.client_key)
                .push_bind(item.name)
                .push_bind(&item.directory)
                .push_bind(item.declared.map(ByteSize::to_i64))
                .push_bind(item.reserved.to_i64())
                .push_bind(item.kind.as_str())
                .push_bind(item.final_object_id.to_string())
                .push_bind(item.final_object_key)
                .push_bind(&now)
                .push_bind(&now);
        });
        query.build().execute(&mut *connection).await?;
    }
    Ok(())
}

pub async fn find_session(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: TransferSessionId,
) -> Result<Option<SessionRow>, TransferError> {
    let row = sqlx::query(SELECT_SESSION)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(session_from).transpose()
}

pub async fn session_items(
    connection: &mut SqliteConnection,
    id: TransferSessionId,
) -> Result<Vec<ItemRow>, TransferError> {
    let rows = sqlx::query(SELECT_ITEMS)
        .bind(id.to_string())
        .fetch_all(connection)
        .await?;
    rows.iter().map(item_from).collect()
}

pub async fn live_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<Vec<LiveItem>, TransferError> {
    let rows = sqlx::query(LIVE_ITEMS)
        .bind(session.to_string())
        .fetch_all(connection)
        .await?;
    rows.iter().map(live_from).collect()
}

pub async fn one_item(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    item: SessionItemId,
) -> Result<Option<LiveItem>, TransferError> {
    let row = sqlx::query(ONE_ITEM)
        .bind(item.to_string())
        .bind(session.to_string())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(live_from).transpose()
}

pub async fn cancel_items(
    connection: &mut SqliteConnection,
    items: &[SessionItemId],
    now: Timestamp,
) -> Result<u64, TransferError> {
    if items.is_empty() {
        return Ok(0);
    }
    let list = json_list(items);
    let canceled = sqlx::query(CANCEL_ITEMS)
        .bind(&list)
        .bind(now.to_string())
        .execute(&mut *connection)
        .await?
        .rows_affected();
    sqlx::query(TERMINATE_TUS)
        .bind(&list)
        .bind(now.to_string())
        .execute(&mut *connection)
        .await?;
    sqlx::query(ABANDON_MULTIPART)
        .bind(&list)
        .bind(now.to_string())
        .execute(connection)
        .await?;
    Ok(canceled)
}

fn json_list(items: &[SessionItemId]) -> String {
    let ids: Vec<String> = items.iter().map(ToString::to_string).collect();
    serde_json::Value::from(ids).to_string()
}

pub struct SessionMove {
    pub from: TransferSessionState,
    pub to: TransferSessionState,
    pub now: Timestamp,
    pub finished: bool,
    pub cancel_requested: bool,
}

pub async fn move_session(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: TransferSessionId,
    change: &SessionMove,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(MOVE_SESSION)
        .bind(id.to_string())
        .bind(owner.to_string())
        .bind(change.to.as_str())
        .bind(change.now.to_string())
        .bind(change.finished.then(|| change.now.to_string()))
        .bind(i64::from(change.cancel_requested))
        .bind(change.from.as_str())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn retry_item(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    item: SessionItemId,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(RETRY_ITEM)
        .bind(item.to_string())
        .bind(session.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn fail_item(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    item: SessionItemId,
    failure: &ErrorParts,
    now: Timestamp,
) -> Result<bool, TransferError> {
    let affected = sqlx::query(FAIL_ITEM)
        .bind(item.to_string())
        .bind(session.to_string())
        .bind(&failure.code)
        .bind(failure.request_id.as_deref())
        .bind(now.to_string())
        .execute(connection)
        .await?
        .rows_affected();
    Ok(affected == 1)
}

pub async fn record_session_error(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    failure: &ErrorParts,
) -> Result<(), TransferError> {
    sqlx::query(RECORD_SESSION_ERROR)
        .bind(session.to_string())
        .bind(&failure.code)
        .bind(failure.request_id.as_deref())
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn protocol_resource_present(
    connection: &mut SqliteConnection,
    item: SessionItemId,
) -> Result<bool, TransferError> {
    let present: i64 = sqlx::query_scalar(PROTOCOL_PRESENT)
        .bind(item.to_string())
        .fetch_one(connection)
        .await?;
    Ok(present != 0)
}

pub async fn in_flight_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<u32, TransferError> {
    counted(connection, COUNT_IN_FLIGHT, session).await
}

pub async fn failed_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<u32, TransferError> {
    counted(connection, COUNT_FAILED, session).await
}

pub async fn remaining_items(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<u32, TransferError> {
    counted(connection, COUNT_REMAINING, session).await
}

async fn counted(
    connection: &mut SqliteConnection,
    sql: &'static str,
    session: TransferSessionId,
) -> Result<u32, TransferError> {
    let total: i64 = sqlx::query_scalar(sql)
        .bind(session.to_string())
        .fetch_one(connection)
        .await?;
    u32::try_from(total).map_err(|_| invariant("count"))
}

fn push_state_filter(query: &mut QueryBuilder<'_, Sqlite>, states: &[TransferSessionState]) {
    if states.is_empty() {
        return;
    }
    query.push(" AND ts.state IN (");
    let mut separated = query.separated(", ");
    for state in states {
        separated.push_bind(state.as_str());
    }
    separated.push_unseparated(")");
}

pub async fn list_sessions(
    connection: &mut SqliteConnection,
    owner: UserId,
    states: &[TransferSessionState],
    page: &PageRequest,
) -> Result<Vec<SessionRow>, TransferError> {
    let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
    query.push(SESSION_COLUMNS);
    query.push(
        ", (SELECT COALESCE(SUM(tu.upload_offset), 0)
              FROM transfer_session_files t
              JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
             WHERE t.transfer_session_id = ts.id AND t.state IN (",
    );
    query.push(IN_FLIGHT_PROGRESS);
    query.push(
        ")) AS tus_bytes,
           (SELECT COALESCE(SUM(p.size_bytes), 0)
              FROM transfer_session_files t
              JOIN s3_multipart_uploads s ON s.transfer_session_file_id = t.id
              JOIN s3_multipart_parts p ON p.s3_multipart_upload_id = s.id
             WHERE t.transfer_session_id = ts.id AND t.state IN (",
    );
    query.push(IN_FLIGHT_PROGRESS);
    query.push(") AND p.state IN ('uploaded', 'verified')) AS s3_bytes FROM transfer_sessions ts ");
    query.push(HELD_JOIN);
    query
        .push(" WHERE ts.user_id = ")
        .push_bind(owner.to_string());
    push_state_filter(&mut query, states);
    page.push_keyset(&mut query, Conjunction::And);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(connection).await?;
    rows.iter().map(session_from).collect()
}

pub async fn count_sessions(
    connection: &mut SqliteConnection,
    owner: UserId,
    states: &[TransferSessionState],
) -> Result<u64, TransferError> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) FROM transfer_sessions ts WHERE ts.user_id = ",
    );
    query.push_bind(owner.to_string());
    push_state_filter(&mut query, states);
    let total: i64 = query.build_query_scalar().fetch_one(connection).await?;
    u64::try_from(total).map_err(|_| invariant("count"))
}

pub async fn touch_session(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
    now: Timestamp,
) -> Result<(), TransferError> {
    sqlx::query(TOUCH_SESSION)
        .bind(session.to_string())
        .bind(now.to_string())
        .execute(connection)
        .await?;
    Ok(())
}
