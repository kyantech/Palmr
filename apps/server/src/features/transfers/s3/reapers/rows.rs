use crate::domain::error_code::ErrorCode;
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::MultipartHandle;

use super::super::super::error::TransferError;
use super::super::super::repo::{self as transfer_repo, ErrorParts, FinalizeStage, SessionMove};
use super::super::super::state::{
    has_expired, session_transition, ItemState, Outcome as Moved, Trigger,
};
use super::super::model::{MultipartId, MultipartState};
use super::super::reconcile::{analyze, PartLayout};
use super::super::repo as s3_repo;
use super::summary::Summary;
use super::{repo, S3ReaperContext, BATCH};

const SNAPSHOT_TRANSACTION: &str = "transfers.s3_reconcile_row";
const LOST_TRANSACTION: &str = "transfers.s3_multipart_lost";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowOutcome {
    Reconciled,
    Lost,
    Skipped,
    Completing,
}

pub(super) async fn reconcile_one(
    context: &S3ReaperContext,
    id: MultipartId,
) -> Result<RowOutcome, TransferError> {
    let Some(multipart) = context.storage.multipart() else {
        return Err(TransferError::StorageUnavailable);
    };
    let row = {
        let mut connection = context.reader().await?;
        repo::load_owned(&mut connection, id).await?
    };
    let Some(row) = row else {
        return Ok(RowOutcome::Skipped);
    };
    match row.s3.state {
        MultipartState::Created | MultipartState::InProgress => {}
        MultipartState::Completing => return Ok(RowOutcome::Completing),
        _ => return Ok(RowOutcome::Skipped),
    }
    let started = context.now()?;
    if has_expired(started, row.expires_at)
        || row.item.state == ItemState::Finalizing
        || context.proxy_active(id)
    {
        return Ok(RowOutcome::Skipped);
    }
    if context.storage.bucket() != Some(row.s3.bucket.as_str()) {
        return Err(TransferError::Invariant {
            what: "multipart_bucket",
        });
    }
    let layout = PartLayout::of(&row.s3).ok_or(TransferError::Invariant { what: "part_plan" })?;
    let key = ObjectKey::parse(&row.s3.object_key).map_err(|_| TransferError::Invariant {
        what: "multipart_object_key",
    })?;
    let handle = MultipartHandle::new(key.clone(), row.s3.upload_id.clone());
    match multipart.list_parts(&handle).await {
        Ok(listed) => {
            let analysis = analyze(&layout, &listed);
            let present = analysis.present_numbers();
            let applied = context
                .pools
                .write_tx(context.clock.as_ref(), SNAPSHOT_TRANSACTION, async |tx| {
                    let promote = !analysis.held.is_empty();
                    if !repo::mark_reconciled_passive(tx.executor(), id, started, promote).await? {
                        return Ok::<_, TransferError>(false);
                    }
                    s3_repo::verify_present(tx.executor(), id, &analysis.held, started).await?;
                    s3_repo::demote_absent(tx.executor(), id, &layout, &present, started).await?;
                    Ok(true)
                })
                .await?;
            Ok(if applied {
                RowOutcome::Reconciled
            } else {
                RowOutcome::Skipped
            })
        }
        Err(StorageError::NotFound) => confirm_lost(context, id, &key).await,
        Err(error) => Err(TransferError::Storage(error)),
    }
}

async fn confirm_lost(
    context: &S3ReaperContext,
    id: MultipartId,
    key: &ObjectKey,
) -> Result<RowOutcome, TransferError> {
    let provider = context
        .storage
        .provider_handle()
        .ok_or(TransferError::StorageUnavailable)?;
    match provider.stat(key).await {
        Ok(_) => return Ok(RowOutcome::Skipped),
        Err(StorageError::NotFound) => {}
        Err(error) => return Err(TransferError::Storage(error)),
    }
    let now = context.now()?;
    let clock = context.clock.as_ref();
    context
        .pools
        .write_tx(clock, LOST_TRANSACTION, async |tx| {
            let Some(row) = repo::load_owned(tx.executor(), id).await? else {
                return Ok::<_, TransferError>(RowOutcome::Skipped);
            };
            let open = matches!(
                row.s3.state,
                MultipartState::Created | MultipartState::InProgress
            );
            let untouched = row.item.state == ItemState::Uploading
                && row.item.finalize_stage == FinalizeStage::None
                && !row.session_state.is_terminal();
            let Some(owner) = row.owner.filter(|_| open && untouched) else {
                return Ok(RowOutcome::Skipped);
            };
            if has_expired(now, row.expires_at) {
                return Ok(RowOutcome::Skipped);
            }
            if !repo::mark_lost(tx.executor(), id, now).await? {
                return Ok(RowOutcome::Skipped);
            }
            let code = ErrorCode::MultipartNotFound.as_str();
            if !repo::fail_lost_item(tx.executor(), row.item.id, code, now).await? {
                return Err(TransferError::Invariant { what: "item_state" });
            }
            if transfer_repo::reservation_held(tx.executor(), row.session).await? {
                context
                    .quota
                    .release_share(tx, row.session, row.item.reserved)
                    .await?;
            }
            match session_transition(row.session_state, Trigger::Failed) {
                Ok(Moved::Moved(to)) => {
                    let moved = transfer_repo::move_session(
                        tx.executor(),
                        owner,
                        row.session,
                        &SessionMove {
                            from: row.session_state,
                            to,
                            now,
                            finished: false,
                            cancel_requested: false,
                        },
                    )
                    .await?;
                    if !moved {
                        return Err(TransferError::Invariant {
                            what: "session_state",
                        });
                    }
                }
                Ok(Moved::Unchanged(_)) | Err(_) => {
                    transfer_repo::touch_session(tx.executor(), row.session, now).await?;
                }
            }
            let failure = ErrorParts {
                code: code.to_owned(),
                request_id: None,
            };
            transfer_repo::record_session_error(tx.executor(), row.session, &failure).await?;
            Ok(RowOutcome::Lost)
        })
        .await
}

pub(super) async fn reconcile_rows(
    context: &S3ReaperContext,
    after: Option<(String, String)>,
    summary: &mut Summary,
) -> Result<Option<(String, String)>, TransferError> {
    let now = context.now()?.to_string();
    let (lower, id) = after.unwrap_or_else(|| (now.clone(), String::new()));
    let page = {
        let mut connection = context.reader().await?;
        repo::page(&mut connection, repo::OPEN_PAGE, (&now, &lower, &id), BATCH).await?
    };
    summary.scanned = summary
        .scanned
        .saturating_add(u64::try_from(page.len()).unwrap_or(u64::MAX));
    let full = page.len() >= usize::try_from(BATCH).unwrap_or(usize::MAX);
    let next = page
        .last()
        .map(|(id, expires)| (expires.clone(), id.clone()));
    let mut consecutive = 0_u32;
    for (id, _) in &page {
        if context.stopping() {
            return Ok(None);
        }
        let Ok(multipart) = id.parse::<MultipartId>() else {
            summary.failed += 1;
            continue;
        };
        summary.matched += 1;
        match reconcile_one(context, multipart).await {
            Ok(RowOutcome::Reconciled) => {
                summary.reconciled += 1;
                consecutive = 0;
            }
            Ok(RowOutcome::Lost) => {
                summary.provider_missing += 1;
                consecutive = 0;
            }
            Ok(RowOutcome::Completing) => summary.skipped_completing += 1,
            Ok(RowOutcome::Skipped) => summary.skipped_live += 1,
            Err(error) => {
                summary.failed += 1;
                tracing::warn!(
                    error_kind = error.kind(),
                    "a multipart upload could not be reconciled; the next run retries it"
                );
                if matches!(error, TransferError::Storage(_)) {
                    consecutive += 1;
                    if consecutive >= super::FAILURE_LIMIT {
                        return Ok(None);
                    }
                }
            }
        }
    }
    Ok(if full { next } else { None })
}
