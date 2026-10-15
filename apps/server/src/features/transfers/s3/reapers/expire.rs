use std::slice;

use crate::features::audit::actions::TransferScope;
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{DedupKey, JobKind, JobPayload, NewJob};

use super::super::super::cancel::{self, Ending, Expired};
use super::super::super::error::TransferError;
use super::super::super::repo::FinalizeStage;
use super::super::super::state::{has_expired, item_transition, ItemState, Trigger};
use super::super::model::{MultipartId, MultipartState};
use super::hooks::Point;
use super::payload::AbortJob;
use super::summary::Summary;
use super::{jobs_error, repo, S3ReaperContext, BATCH};

const EXPIRE_TRANSACTION: &str = "transfers.s3_expire_multipart";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpireOutcome {
    Expired,
    NotDue,
    Gone,
    Completing,
    Finalizing,
    ProxyActive,
}

pub(super) fn abort_dedup_key(id: MultipartId) -> Result<DedupKey, TransferError> {
    DedupKey::new(format!("s3mp:{id}")).map_err(|_| TransferError::Invariant {
        what: "multipart_job_key",
    })
}

pub(super) async fn expire_one(
    context: &S3ReaperContext,
    id: MultipartId,
) -> Result<ExpireOutcome, TransferError> {
    context.reached(Point::ExpireDiscovered).await?;
    if context.proxy_active(id) {
        return Ok(ExpireOutcome::ProxyActive);
    }
    let now = context.now()?;
    let clock = context.clock.as_ref();
    let ended = context
        .pools
        .write_tx(clock, EXPIRE_TRANSACTION, async |tx| {
            let Some(row) = repo::load_owned(tx.executor(), id).await? else {
                return Ok::<_, TransferError>((ExpireOutcome::Gone, None));
            };
            match row.s3.state {
                MultipartState::Created | MultipartState::InProgress => {}
                MultipartState::Completing => return Ok((ExpireOutcome::Completing, None)),
                _ => return Ok((ExpireOutcome::Gone, None)),
            }
            if !has_expired(now, row.expires_at) {
                return Ok((ExpireOutcome::NotDue, None));
            }
            if row.item.state == ItemState::Finalizing
                || row.item.finalize_stage != FinalizeStage::None
            {
                return Ok((ExpireOutcome::Finalizing, None));
            }
            if row.item.state == ItemState::Completed {
                return Ok((ExpireOutcome::Gone, None));
            }
            if !repo::mark_expired(tx.executor(), id, now).await? {
                return Ok((ExpireOutcome::NotDue, None));
            }
            let live = item_transition(row.item.state, Trigger::Expire)
                .ok()
                .is_some_and(|outcome| outcome.moved());
            let mut items_ended = 0;
            let mut settled = cancel::ExpiredShare::default();
            if live {
                items_ended = cancel::end_items(
                    tx,
                    clock,
                    row.provider,
                    slice::from_ref(&row.item),
                    Ending::Expired,
                    now,
                )
                .await?;
                settled = cancel::settle_expired_item(
                    tx,
                    &context.quota,
                    (row.session, row.session_state),
                    &row.item,
                    now,
                )
                .await?;
            }
            let payload = JobPayload::new(&AbortJob::Multipart {
                multipart_id: id.to_string(),
            })
            .map_err(|_| TransferError::Invariant {
                what: "multipart_job_payload",
            })?;
            enqueue(
                tx,
                clock,
                &NewJob::new(JobKind::S3AbortAbandonedMultipart, payload)
                    .dedup_key(abort_dedup_key(id)?),
            )
            .await
            .map_err(jobs_error)?;
            Ok((ExpireOutcome::Expired, Some((row, items_ended, settled))))
        })
        .await?;
    let (outcome, details) = ended;
    if let Some((row, items_ended, settled)) = details {
        cancel::record_expired(
            &context.audit,
            &Expired {
                scope: TransferScope::Upload,
                session: &row.session.to_string(),
                item_id: Some(&row.item.id.to_string()),
                upload: Some(&id.to_string()),
                provider: row.provider.as_str(),
                items_ended,
                completed_items_kept: settled.completed_kept,
                session_closed: settled.session_closed,
                owner: row.owner.map(|owner| owner.to_string()).as_deref(),
            },
            now,
        );
    }
    Ok(outcome)
}

pub(super) async fn expire_due(
    context: &S3ReaperContext,
    after: Option<(String, String)>,
    summary: &mut Summary,
) -> Result<Option<(String, String)>, TransferError> {
    let now = context.now()?.to_string();
    let (lower, id) = after.unwrap_or_default();
    let page = {
        let mut connection = context.reader().await?;
        repo::page(
            &mut connection,
            repo::EXPIRY_PAGE,
            (&now, &lower, &id),
            BATCH,
        )
        .await?
    };
    summary.scanned = summary
        .scanned
        .saturating_add(u64::try_from(page.len()).unwrap_or(u64::MAX));
    let full = page.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX);
    let next = page
        .last()
        .map(|(id, expires)| (expires.clone(), id.clone()));
    for (id, _) in &page {
        if context.stopping() {
            return Ok(None);
        }
        let Ok(multipart) = id.parse::<MultipartId>() else {
            summary.failed += 1;
            continue;
        };
        summary.matched += 1;
        match expire_one(context, multipart).await {
            Ok(ExpireOutcome::Expired) => {
                summary.expired += 1;
                summary.abandoned += 1;
            }
            Ok(ExpireOutcome::Completing | ExpireOutcome::Finalizing) => {
                summary.skipped_completing += 1;
            }
            Ok(ExpireOutcome::NotDue | ExpireOutcome::ProxyActive) => {
                summary.skipped_live += 1;
            }
            Ok(ExpireOutcome::Gone) => {}
            Err(error) => {
                summary.failed += 1;
                tracing::warn!(
                    error_kind = error.kind(),
                    "a multipart upload could not be expired; the next sweep retries it"
                );
            }
        }
    }
    Ok(if full { next } else { None })
}
