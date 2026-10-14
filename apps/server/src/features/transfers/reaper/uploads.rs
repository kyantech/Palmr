use sqlx::sqlite::SqliteRow;
use sqlx::SqliteConnection;

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::quota::model::ReleaseReason;
use crate::infra::db::WriteTx;

use super::super::cancel::{self, Ending, Expired};
use super::super::error::TransferError;
use super::super::model::{TransferProvider, TransferSessionId};
use super::super::repo::{
    self, bytes, column, invariant, optional_bytes, optional_parsed, parsed, FinalizeStage,
    LiveItem,
};
use super::super::state::{
    has_expired, item_transition, session_transition, ItemState, Outcome, TransferSessionState,
    Trigger,
};
use super::super::tus::repo::{TusState, TusUploadId};
use super::{ReapReport, ReaperContext, PAGE_SIZE, PASS_LIMIT};
use crate::features::audit::actions::TransferScope;

const EXPIRE_TRANSACTION: &str = "transfers.expire_upload";

pub(super) const DUE_PAGE: &str = "SELECT id, expires_at
      FROM tus_uploads
     WHERE state IN ('created','in_progress') AND expires_at <= ?1
       AND expires_at >= ?2 AND (expires_at > ?2 OR (expires_at = ?2 AND id > ?3))
     ORDER BY expires_at, id
     LIMIT ?4";

const LOAD_UPLOAD: &str =
    "SELECT tu.state AS tus_state, tu.expires_at, tu.locked_by, tu.owner_user_id,
        tu.lock_expires_at, tu.upload_offset, tu.upload_length,
        t.id AS item_id, t.transfer_session_id, t.state AS item_state, t.finalize_stage,
        t.reserved_bytes, t.final_object_id, t.final_object_key,
        ts.state AS session_state, ts.expires_at AS session_expires_at, ts.provider
      FROM tus_uploads tu
      JOIN transfer_session_files t ON t.id = tu.transfer_session_file_id
      JOIN transfer_sessions ts ON ts.id = t.transfer_session_id
     WHERE tu.id = ?1";

const EXPIRE_STRAY_UPLOAD: &str = "UPDATE tus_uploads SET state = 'expired', updated_at = ?2
    WHERE id = ?1 AND state IN ('created','in_progress')";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOutcome {
    Expired,
    Gone,
    NotDue,
    Leased,
    AwaitingFinalization,
}

struct Loaded {
    tus_state: TusState,
    expires_at: Timestamp,
    leased_until: Option<Timestamp>,
    offset: ByteSize,
    length: Option<ByteSize>,
    item: LiveItem,
    session: TransferSessionId,
    session_state: TransferSessionState,
    session_expires_at: Timestamp,
    provider: TransferProvider,
    owner: Option<String>,
}

fn loaded_from(row: &SqliteRow) -> Result<Loaded, TransferError> {
    let tus_state: String = column(row, "tus_state")?;
    let item_state: String = column(row, "item_state")?;
    let stage: String = column(row, "finalize_stage")?;
    let session_state: String = column(row, "session_state")?;
    let provider: String = column(row, "provider")?;
    let holder: Option<String> = column(row, "locked_by")?;
    let leased_until =
        optional_parsed::<Timestamp>(row, "lock_expires_at")?.filter(|_| holder.is_some());
    Ok(Loaded {
        tus_state: TusState::parse(&tus_state).ok_or_else(|| invariant("tus_state"))?,
        expires_at: parsed(row, "expires_at")?,
        leased_until,
        offset: bytes(row, "upload_offset")?,
        length: optional_bytes(row, "upload_length")?,
        item: LiveItem {
            id: parsed(row, "item_id")?,
            state: ItemState::parse(&item_state).ok_or_else(|| invariant("item_state"))?,
            finalize_stage: FinalizeStage::parse(&stage)
                .ok_or_else(|| invariant("finalize_stage"))?,
            reserved: bytes(row, "reserved_bytes")?,
            final_object_id: parsed(row, "final_object_id")?,
            final_object_key: column(row, "final_object_key")?,
        },
        session: parsed(row, "transfer_session_id")?,
        session_state: TransferSessionState::parse(&session_state)
            .ok_or_else(|| invariant("session_state"))?,
        session_expires_at: parsed(row, "session_expires_at")?,
        provider: TransferProvider::parse(&provider).ok_or_else(|| invariant("provider"))?,
        owner: column(row, "owner_user_id")?,
    })
}

impl Loaded {
    fn lease_live(&self, now: Timestamp) -> bool {
        self.leased_until.is_some_and(|until| until > now)
    }

    fn awaits_finalization(&self, now: Timestamp) -> bool {
        if self.session_state.is_terminal() || has_expired(now, self.session_expires_at) {
            return false;
        }
        match self.item.state {
            ItemState::Finalizing => true,
            ItemState::Uploading => self.length == Some(self.offset),
            _ => false,
        }
    }
}

pub(super) async fn expire_due(
    context: &ReaperContext,
    report: &mut ReapReport,
) -> Result<bool, TransferError> {
    let mut cursor = (String::new(), String::new());
    let mut seen = 0_u32;
    loop {
        if context.stopping() {
            return Ok(false);
        }
        let want = PAGE_SIZE.min(PASS_LIMIT - seen);
        let page = {
            let mut connection = context.reader().await?;
            due_page(&mut connection, &cursor, context.now()?, want).await?
        };
        let Some(last) = page.last().cloned() else {
            return Ok(false);
        };
        for (id, _) in &page {
            match expire_upload(context, id).await {
                Ok(UploadOutcome::Expired) => report.uploads_expired += 1,
                Ok(_) => report.uploads_skipped += 1,
                Err(error) => {
                    report.errors += 1;
                    tracing::warn!(
                        error_kind = error.kind(),
                        "a stale upload could not be expired; the next pass retries it"
                    );
                }
            }
        }
        seen = seen.saturating_add(u32::try_from(page.len()).unwrap_or(u32::MAX));
        if seen >= PASS_LIMIT {
            return Ok(true);
        }
        if page.len() < usize::try_from(want).unwrap_or(usize::MAX) {
            return Ok(false);
        }
        cursor = (last.1, last.0);
    }
}

async fn due_page(
    connection: &mut SqliteConnection,
    cursor: &(String, String),
    now: Timestamp,
    limit: u32,
) -> Result<Vec<(String, String)>, TransferError> {
    let rows = sqlx::query(DUE_PAGE)
        .bind(now.to_string())
        .bind(&cursor.0)
        .bind(&cursor.1)
        .bind(i64::from(limit))
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|row| Ok((column(row, "id")?, column(row, "expires_at")?)))
        .collect()
}

pub async fn expire_upload(
    context: &ReaperContext,
    id: &str,
) -> Result<UploadOutcome, TransferError> {
    let parsed: TusUploadId = id.parse().map_err(|_| invariant("tus_upload_id"))?;
    let now = context.now()?;
    let (outcome, ended) = context
        .pools
        .write_tx(context.clock.as_ref(), EXPIRE_TRANSACTION, async |tx| {
            let row = sqlx::query(LOAD_UPLOAD)
                .bind(parsed.to_string())
                .fetch_optional(tx.executor())
                .await?;
            let Some(row) = row else {
                return Ok((UploadOutcome::Gone, None));
            };
            let upload = loaded_from(&row)?;
            if !matches!(upload.tus_state, TusState::Created | TusState::InProgress) {
                return Ok((UploadOutcome::Gone, None));
            }
            if !has_expired(now, upload.expires_at) {
                return Ok((UploadOutcome::NotDue, None));
            }
            if upload.lease_live(now) {
                return Ok((UploadOutcome::Leased, None));
            }
            if upload.awaits_finalization(now) {
                return Ok((UploadOutcome::AwaitingFinalization, None));
            }
            let item = &upload.item;
            if item_transition(item.state, Trigger::Expire)
                .ok()
                .is_none_or(|outcome| !outcome.moved())
            {
                sqlx::query(EXPIRE_STRAY_UPLOAD)
                    .bind(parsed.to_string())
                    .bind(now.to_string())
                    .execute(tx.executor())
                    .await?;
                return Ok((UploadOutcome::Expired, None));
            }
            let items_ended = cancel::end_items(
                tx,
                context.clock.as_ref(),
                upload.provider,
                std::slice::from_ref(item),
                Ending::Expired,
                now,
            )
            .await?;
            let settled = settle_share(context, tx, &upload, now).await?;
            Ok::<_, TransferError>((
                UploadOutcome::Expired,
                Some((
                    upload,
                    items_ended,
                    settled.session_closed,
                    settled.completed_kept,
                )),
            ))
        })
        .await?;
    if let Some((upload, items_ended, session_closed, completed_kept)) = ended {
        cancel::record_expired(
            &context.audit,
            &Expired {
                scope: TransferScope::Upload,
                session: &upload.session.to_string(),
                item_id: Some(&upload.item.id.to_string()),
                upload: Some(&parsed.to_string()),
                provider: upload.provider.as_str(),
                items_ended,
                completed_items_kept: completed_kept,
                session_closed,
                owner: upload.owner.as_deref(),
            },
            now,
        );
    }
    Ok(outcome)
}

struct Settled {
    session_closed: bool,
    completed_kept: u64,
}

async fn settle_share(
    context: &ReaperContext,
    tx: &mut WriteTx<'_>,
    upload: &Loaded,
    now: Timestamp,
) -> Result<Settled, TransferError> {
    let mut settled = Settled {
        session_closed: false,
        completed_kept: 0,
    };
    if upload.session_state.is_terminal() {
        return Ok(settled);
    }
    let session = repo::find_session_unowned(tx.executor(), upload.session)
        .await?
        .ok_or(TransferError::SessionNotFound)?;
    settled.completed_kept = u64::from(session.completed_file_count);
    if repo::reservation_held(tx.executor(), upload.session).await? {
        if upload.item.state.holds_reservation() {
            context
                .quota
                .release_share(tx, upload.session, upload.item.reserved)
                .await?;
        }
        if repo::remaining_items(tx.executor(), upload.session).await? == 0 {
            if let Ok(Outcome::Moved(_)) = session_transition(session.state, Trigger::Expire) {
                let moved =
                    repo::expire_session(tx.executor(), upload.session, session.state, now, false)
                        .await?;
                if !moved {
                    return Err(invariant("session_state"));
                }
                cancel::settle_session(tx, &context.quota, &session, ReleaseReason::Expired)
                    .await?;
                settled.session_closed = true;
                return Ok(settled);
            }
        }
    }
    repo::touch_session(tx.executor(), upload.session, now).await?;
    Ok(settled)
}
