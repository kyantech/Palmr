use sqlx::SqliteConnection;

use crate::features::quota::model::ReleaseReason;

use super::super::cancel::{self, Ending, Expired};
use super::super::error::TransferError;
use super::super::model::TransferSessionId;
use super::super::repo::{self, column};
use super::super::state::{has_expired, session_transition, Outcome, Trigger};
use super::{ReapReport, ReaperContext, PAGE_SIZE, PASS_LIMIT};
use crate::features::audit::actions::TransferScope;

const EXPIRE_TRANSACTION: &str = "transfers.expire_session";

pub(super) const LIVE_PAGE: &str = "SELECT id, expires_at
      FROM transfer_sessions
     WHERE state IN ('created','uploading','finalizing') AND expires_at <= ?1
       AND expires_at >= ?2 AND (expires_at > ?2 OR (expires_at = ?2 AND id > ?3))
     ORDER BY expires_at, id
     LIMIT ?4";

pub(super) const FAILED_PAGE: &str = "SELECT id, expires_at
      FROM transfer_sessions
     WHERE state = 'failed' AND expires_at <= ?1
       AND expires_at >= ?2 AND (expires_at > ?2 OR (expires_at = ?2 AND id > ?3))
     ORDER BY expires_at, id
     LIMIT ?4";

type Cursor = (String, String);

pub(super) async fn expire_due(
    context: &ReaperContext,
    report: &mut ReapReport,
) -> Result<bool, TransferError> {
    let mut capped = false;
    for sql in [LIVE_PAGE, FAILED_PAGE] {
        capped |= expire_pages(context, report, sql).await?;
    }
    Ok(capped)
}

async fn expire_pages(
    context: &ReaperContext,
    report: &mut ReapReport,
    sql: &'static str,
) -> Result<bool, TransferError> {
    let mut cursor: Cursor = (String::new(), String::new());
    let mut seen = 0_u32;
    loop {
        if context.stopping() {
            return Ok(false);
        }
        let want = PAGE_SIZE.min(PASS_LIMIT - seen);
        let page = {
            let mut connection = context.reader().await?;
            page(
                &mut connection,
                sql,
                &cursor,
                context.now()?.to_string(),
                want,
            )
            .await?
        };
        let Some(last) = page.last().cloned() else {
            return Ok(false);
        };
        for (id, _) in &page {
            let Ok(id) = id.parse::<TransferSessionId>() else {
                report.errors += 1;
                continue;
            };
            match expire_session(context, id).await {
                Ok(true) => report.sessions_expired += 1,
                Ok(false) => report.sessions_skipped += 1,
                Err(error) => {
                    report.errors += 1;
                    tracing::warn!(
                        error_kind = error.kind(),
                        "a transfer session could not be expired; the next pass retries it"
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

async fn page(
    connection: &mut SqliteConnection,
    sql: &'static str,
    cursor: &Cursor,
    now: String,
    limit: u32,
) -> Result<Vec<(String, String)>, TransferError> {
    let rows = sqlx::query(sql)
        .bind(now)
        .bind(&cursor.0)
        .bind(&cursor.1)
        .bind(i64::from(limit))
        .fetch_all(connection)
        .await?;
    rows.iter()
        .map(|row| Ok((column(row, "id")?, column(row, "expires_at")?)))
        .collect()
}

pub async fn expire_session(
    context: &ReaperContext,
    id: TransferSessionId,
) -> Result<bool, TransferError> {
    let now = context.now()?;
    let ended = context
        .pools
        .write_tx(context.clock.as_ref(), EXPIRE_TRANSACTION, async |tx| {
            let Some(session) = repo::find_session_unowned(tx.executor(), id).await? else {
                return Ok(None);
            };
            let due = has_expired(now, session.expires_at);
            let movable = matches!(
                session_transition(session.state, Trigger::Expire),
                Ok(Outcome::Moved(_))
            );
            if !due || !movable {
                return Ok(None);
            }
            let live = repo::live_items(tx.executor(), id).await?;
            let items_ended = cancel::end_items(
                tx,
                context.clock.as_ref(),
                session.provider,
                &live,
                Ending::Expired,
                now,
            )
            .await?;
            if !repo::expire_session(tx.executor(), id, session.state, now, true).await? {
                return Err(TransferError::Invariant {
                    what: "session_state",
                });
            }
            cancel::settle_if_held(tx, &context.quota, &session, ReleaseReason::Expired).await?;
            let owner = repo::session_owner(tx.executor(), id).await?;
            Ok::<_, TransferError>(Some((session, items_ended, owner)))
        })
        .await?;
    let Some((session, items_ended, owner)) = ended else {
        return Ok(false);
    };
    cancel::record_expired(
        &context.audit,
        &Expired {
            scope: TransferScope::Session,
            session: &id.to_string(),
            item_id: None,
            upload: None,
            provider: session.provider.as_str(),
            items_ended,
            completed_items_kept: u64::from(session.completed_file_count),
            session_closed: true,
            owner: owner.as_deref(),
        },
        now,
    );
    Ok(true)
}
