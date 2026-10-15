use sqlx::SqliteConnection;

use crate::domain::time::Timestamp;

use super::error::TransferError;

pub const MAX_CLEANUP_PAGE: u32 = 1_000;

pub(crate) const ABANDONED_MULTIPART_PAGE: &str = "SELECT id, transfer_session_file_id
      FROM s3_multipart_uploads
     WHERE state = 'abandoned' AND id > ?1
     ORDER BY id
     LIMIT ?2";

const TERMINATED_TUS_PAGE: &str = "SELECT id, transfer_session_file_id
      FROM tus_uploads
     WHERE state = 'terminated' AND staging_removed_at IS NULL AND id > ?1
     ORDER BY id
     LIMIT ?2";

pub(crate) const TUS_STAGING_PAGE: &str = "SELECT id, transfer_session_file_id
      FROM tus_uploads
     WHERE state IN ('completed','terminated','expired') AND staging_removed_at IS NULL
       AND (locked_by IS NULL OR lock_expires_at <= ?3)
       AND id > ?1
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
    let rows: Vec<(String, String)> = sqlx::query_as(ABANDONED_MULTIPART_PAGE)
        .bind(after.unwrap_or_default())
        .bind(i64::from(limit.clamp(1, MAX_CLEANUP_PAGE)))
        .fetch_all(connection)
        .await?;
    Ok(pending(rows))
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the terminated-only discovery view; the reaper pages every terminal state through tus_staging_page"
    )
)]
pub async fn terminated_tus_page(
    connection: &mut SqliteConnection,
    after: Option<&str>,
    limit: u32,
) -> Result<Vec<PendingCleanup>, TransferError> {
    let rows: Vec<(String, String)> = sqlx::query_as(TERMINATED_TUS_PAGE)
        .bind(after.unwrap_or_default())
        .bind(i64::from(limit.clamp(1, MAX_CLEANUP_PAGE)))
        .fetch_all(connection)
        .await?;
    Ok(pending(rows))
}

pub async fn tus_staging_page(
    connection: &mut SqliteConnection,
    after: Option<&str>,
    limit: u32,
    now: Timestamp,
) -> Result<Vec<PendingCleanup>, TransferError> {
    let rows: Vec<(String, String)> = sqlx::query_as(TUS_STAGING_PAGE)
        .bind(after.unwrap_or_default())
        .bind(i64::from(limit.clamp(1, MAX_CLEANUP_PAGE)))
        .bind(now.to_string())
        .fetch_all(connection)
        .await?;
    Ok(pending(rows))
}

fn pending(rows: Vec<(String, String)>) -> Vec<PendingCleanup> {
    rows.into_iter()
        .map(|(id, transfer_session_file_id)| PendingCleanup {
            id,
            transfer_session_file_id,
        })
        .collect()
}
