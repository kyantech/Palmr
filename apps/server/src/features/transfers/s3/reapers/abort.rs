use futures_util::stream::{self, StreamExt};

use crate::domain::time::Timestamp;
use crate::features::transfers::cleanup::abandoned_multipart_page;

use super::super::super::error::TransferError;
use super::super::abort::abort_at_provider;
use super::super::model::{MultipartId, MultipartState};
use super::hooks::Point;
use super::summary::Summary;
use super::{repo, S3ReaperContext, ABORT_CONCURRENCY, ABORT_GRACE, BATCH, FAILURE_LIMIT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Timing {
    Discovered,
    Targeted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AbortOutcome {
    Aborted,
    Terminal,
    NotAbandoned,
    TooRecent,
    Gone,
}

pub(super) async fn abort_one(
    context: &S3ReaperContext,
    id: MultipartId,
    timing: Timing,
) -> Result<AbortOutcome, TransferError> {
    let row = {
        let mut connection = context.reader().await?;
        repo::load_owned(&mut connection, id).await?
    };
    let Some(row) = row else {
        return Ok(AbortOutcome::Gone);
    };
    match row.s3.state {
        MultipartState::Abandoned => {}
        MultipartState::Aborted | MultipartState::Completed => return Ok(AbortOutcome::Terminal),
        MultipartState::Created | MultipartState::InProgress | MultipartState::Completing => {
            return Ok(AbortOutcome::NotAbandoned)
        }
    }
    if timing == Timing::Discovered {
        let settled = Timestamp::try_from(row.updated_at.get() + ABORT_GRACE)?;
        if settled > context.now()? {
            return Ok(AbortOutcome::TooRecent);
        }
    }
    context.reached(Point::BeforeProviderAbort).await?;
    abort_at_provider(
        &context.pools,
        context.clock.as_ref(),
        &context.storage,
        &row.s3,
    )
    .await?;
    Ok(AbortOutcome::Aborted)
}

pub(super) struct AbortPass {
    pub(super) next: Option<String>,
}

pub(super) async fn abort_abandoned(
    context: &S3ReaperContext,
    after: Option<String>,
    summary: &mut Summary,
) -> Result<AbortPass, TransferError> {
    let page = {
        let mut connection = context.reader().await?;
        abandoned_multipart_page(&mut connection, after.as_deref(), BATCH).await?
    };
    summary.scanned = summary
        .scanned
        .saturating_add(u64::try_from(page.len()).unwrap_or(u64::MAX));
    let full = page.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX);
    let next = page.last().map(|row| row.id.clone());
    let ids: Vec<MultipartId> = page
        .iter()
        .filter_map(|row| row.id.parse::<MultipartId>().ok())
        .collect();
    summary.failed += u64::try_from(page.len() - ids.len()).unwrap_or(u64::MAX);
    summary.matched += u64::try_from(ids.len()).unwrap_or(u64::MAX);
    let mut results = stream::iter(ids)
        .map(|id| {
            let context = context.clone();
            async move { abort_one(&context, id, Timing::Discovered).await }
        })
        .buffered(ABORT_CONCURRENCY);
    let mut consecutive = 0_u32;
    let mut tripped = false;
    while let Some(result) = results.next().await {
        if context.stopping() {
            return Ok(AbortPass { next: None });
        }
        match result {
            Ok(AbortOutcome::Aborted) => {
                summary.aborted += 1;
                consecutive = 0;
            }
            Ok(AbortOutcome::TooRecent | AbortOutcome::NotAbandoned) => {
                summary.skipped_live += 1;
            }
            Ok(AbortOutcome::Terminal | AbortOutcome::Gone) => {}
            Err(error) => {
                summary.failed += 1;
                tracing::warn!(
                    error_kind = error.kind(),
                    "an abandoned multipart upload could not be aborted; it stays discoverable and the next sweep retries it"
                );
                if matches!(
                    error,
                    TransferError::Storage(_) | TransferError::StorageUnavailable
                ) {
                    consecutive += 1;
                    if consecutive >= FAILURE_LIMIT {
                        tripped = true;
                        break;
                    }
                }
            }
        }
    }
    Ok(AbortPass {
        next: if full && !tripped { next } else { None },
    })
}
