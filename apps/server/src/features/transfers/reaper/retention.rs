use crate::domain::time::Timestamp;

use super::super::error::TransferError;
use super::{
    before, ReapReport, ReaperContext, PASS_LIMIT, TERMINAL_SESSION_RETENTION,
    TERMINAL_UPLOAD_RETENTION,
};

const UPLOAD_BATCH: u32 = 200;
const SESSION_BATCH: u32 = 25;
const UPLOAD_TRANSACTION: &str = "transfers.prune_uploads";
const SESSION_TRANSACTION: &str = "transfers.prune_sessions";

pub(super) const PRUNE_UPLOADS: &str = "DELETE FROM tus_uploads
    WHERE id IN (SELECT id FROM tus_uploads
                  WHERE staging_removed_at IS NOT NULL AND staging_removed_at <= ?1
                    AND state IN ('completed','terminated','expired')
                  ORDER BY staging_removed_at
                  LIMIT ?2)";

pub(super) const PRUNE_SESSIONS: &str = "DELETE FROM transfer_sessions
    WHERE id IN (SELECT ts.id FROM transfer_sessions ts
                  WHERE ts.state IN ('completed','canceled','expired')
                    AND ts.completed_at IS NOT NULL AND ts.completed_at <= ?1
                    AND NOT EXISTS (SELECT 1 FROM transfer_session_files t
                                      JOIN tus_uploads tu ON tu.transfer_session_file_id = t.id
                                     WHERE t.transfer_session_id = ts.id)
                    AND NOT EXISTS (SELECT 1 FROM transfer_session_files t
                                      JOIN s3_multipart_uploads s ON s.transfer_session_file_id = t.id
                                     WHERE t.transfer_session_id = ts.id)
                    AND NOT EXISTS (SELECT 1 FROM quota_reservations q
                                     WHERE q.transfer_session_id = ts.id)
                  ORDER BY ts.completed_at
                  LIMIT ?2)";

pub(super) async fn prune(
    context: &ReaperContext,
    report: &mut ReapReport,
) -> Result<bool, TransferError> {
    let now = context.now()?;
    let uploads = prune_uploads(context, before(now, TERMINAL_UPLOAD_RETENTION)?).await?;
    report.uploads_pruned += uploads.0;
    let sessions = prune_sessions(context, before(now, TERMINAL_SESSION_RETENTION)?).await?;
    report.sessions_pruned += sessions.0;
    Ok(uploads.1 || sessions.1)
}

async fn prune_uploads(
    context: &ReaperContext,
    cutoff: Timestamp,
) -> Result<(u64, bool), TransferError> {
    batches(
        context,
        UPLOAD_TRANSACTION,
        PRUNE_UPLOADS,
        cutoff,
        UPLOAD_BATCH,
    )
    .await
}

async fn prune_sessions(
    context: &ReaperContext,
    cutoff: Timestamp,
) -> Result<(u64, bool), TransferError> {
    batches(
        context,
        SESSION_TRANSACTION,
        PRUNE_SESSIONS,
        cutoff,
        SESSION_BATCH,
    )
    .await
}

async fn batches(
    context: &ReaperContext,
    transaction: &'static str,
    sql: &'static str,
    cutoff: Timestamp,
    batch: u32,
) -> Result<(u64, bool), TransferError> {
    let mut total = 0_u64;
    while total < u64::from(PASS_LIMIT) {
        if context.stopping() {
            return Ok((total, false));
        }
        let deleted = context
            .pools
            .write_tx(context.clock.as_ref(), transaction, async |tx| {
                let result = sqlx::query(sql)
                    .bind(cutoff.to_string())
                    .bind(i64::from(batch))
                    .execute(tx.executor())
                    .await?;
                Ok::<_, TransferError>(result.rows_affected())
            })
            .await?;
        total = total.saturating_add(deleted);
        if deleted < u64::from(batch) {
            return Ok((total, false));
        }
    }
    Ok((total, true))
}
