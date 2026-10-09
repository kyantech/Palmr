use sqlx::SqliteConnection;

use super::error::TransferError;

pub const MAX_CLEANUP_PAGE: u32 = 1_000;

const ABANDONED_MULTIPART_PAGE: &str = "SELECT id, transfer_session_file_id
      FROM s3_multipart_uploads
     WHERE state = 'abandoned' AND id > ?1
     ORDER BY id
     LIMIT ?2";

const TERMINATED_TUS_PAGE: &str = "SELECT id, transfer_session_file_id
      FROM tus_uploads
     WHERE state = 'terminated' AND id > ?1
     ORDER BY id
     LIMIT ?2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCleanup {
    pub id: String,
    pub transfer_session_file_id: String,
}

pub async fn abandoned_multipart_page(
    connection: &mut SqliteConnection,
    after: Option<&str>,
    limit: u32,
) -> Result<Vec<PendingCleanup>, TransferError> {
    page(connection, ABANDONED_MULTIPART_PAGE, after, limit).await
}

pub async fn terminated_tus_page(
    connection: &mut SqliteConnection,
    after: Option<&str>,
    limit: u32,
) -> Result<Vec<PendingCleanup>, TransferError> {
    page(connection, TERMINATED_TUS_PAGE, after, limit).await
}

async fn page(
    connection: &mut SqliteConnection,
    sql: &'static str,
    after: Option<&str>,
    limit: u32,
) -> Result<Vec<PendingCleanup>, TransferError> {
    let rows: Vec<(String, String)> = sqlx::query_as(sql)
        .bind(after.unwrap_or_default())
        .bind(i64::from(limit.clamp(1, MAX_CLEANUP_PAGE)))
        .fetch_all(connection)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, transfer_session_file_id)| PendingCleanup {
            id,
            transfer_session_file_id,
        })
        .collect())
}
