use std::sync::Arc;

use crate::storage::staging::{StagingStorage, UploadId};

use super::super::cleanup::tus_staging_page;
use super::super::error::TransferError;
use super::{ReapReport, ReaperContext, PAGE_SIZE, PASS_LIMIT};

const MARK_TRANSACTION: &str = "transfers.mark_staging_removed";

const MARK_REMOVED: &str = "UPDATE tus_uploads
      SET staging_removed_at = ?2, updated_at = ?2
    WHERE id IN (SELECT value FROM json_each(?1))
      AND state IN ('completed','terminated','expired') AND staging_removed_at IS NULL";

pub(super) async fn remove_pending(
    context: &ReaperContext,
    report: &mut ReapReport,
) -> Result<bool, TransferError> {
    let Some(staging) = context.staging.clone() else {
        return Ok(false);
    };
    let mut after: Option<String> = None;
    let mut seen = 0_u32;
    loop {
        if context.stopping() {
            return Ok(false);
        }
        let want = PAGE_SIZE.min(PASS_LIMIT - seen);
        let page = {
            let mut connection = context.reader().await?;
            tus_staging_page(&mut connection, after.as_deref(), want, context.now()?).await?
        };
        let Some(last) = page.last().map(|row| row.id.clone()) else {
            return Ok(false);
        };
        let mut removed: Vec<&str> = Vec::with_capacity(page.len());
        for row in &page {
            if context.stopping() {
                break;
            }
            match remove_one(&staging, &row.id).await {
                Ok(()) => removed.push(row.id.as_str()),
                Err(kind) => {
                    report.staging_failed += 1;
                    tracing::warn!(
                        kind,
                        "upload staging could not be removed; the next pass retries it"
                    );
                }
            }
        }
        mark_removed(context, &removed).await?;
        report.staging_removed += u64::try_from(removed.len()).unwrap_or(u64::MAX);
        seen = seen.saturating_add(u32::try_from(page.len()).unwrap_or(u32::MAX));
        if context.stopping() {
            return Ok(false);
        }
        if seen >= PASS_LIMIT {
            return Ok(true);
        }
        if page.len() < usize::try_from(want).unwrap_or(usize::MAX) {
            return Ok(false);
        }
        after = Some(last);
    }
}

async fn remove_one(staging: &Arc<dyn StagingStorage>, id: &str) -> Result<(), &'static str> {
    let upload = UploadId::parse(&id.replace('-', "")).map_err(|_| "invalid_upload_id")?;
    staging
        .remove(&upload)
        .await
        .map(|_| ())
        .map_err(|error| error.kind())
}

async fn mark_removed(context: &ReaperContext, ids: &[&str]) -> Result<(), TransferError> {
    if ids.is_empty() {
        return Ok(());
    }
    let list = serde_json::Value::from(ids.to_vec()).to_string();
    let now = context.now()?;
    context
        .pools
        .write_tx(context.clock.as_ref(), MARK_TRANSACTION, async |tx| {
            sqlx::query(MARK_REMOVED)
                .bind(&list)
                .bind(now.to_string())
                .execute(tx.executor())
                .await?;
            Ok::<_, TransferError>(())
        })
        .await
}
