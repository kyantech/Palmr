use crate::domain::time::Timestamp;

use super::super::super::error::TransferError;
use super::summary::Summary;
use super::{repo, S3ReaperContext, BATCH, TERMINAL_RETENTION};

const PRUNE_TRANSACTION: &str = "transfers.s3_prune_terminal";

pub(super) async fn prune_terminal(
    context: &S3ReaperContext,
    summary: &mut Summary,
) -> Result<bool, TransferError> {
    if context.stopping() {
        return Ok(false);
    }
    let cutoff = Timestamp::try_from(context.now()?.get() - TERMINAL_RETENTION)?;
    let deleted = context
        .pools
        .write_tx(context.clock.as_ref(), PRUNE_TRANSACTION, async |tx| {
            repo::prune_terminal(tx.executor(), cutoff, BATCH).await
        })
        .await?;
    summary.pruned = summary.pruned.saturating_add(deleted);
    Ok(deleted >= u64::from(BATCH))
}
