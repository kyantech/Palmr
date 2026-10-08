use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{self, QuotaDriftFacts};
use crate::features::audit::model::{Actor, AuditEvent, Outcome, Target, TargetType};
use crate::features::audit::service::AuditService;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::jobs::claim::{enqueue, Enqueued, MAX_ROWS_PER_TX};
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{
    ClaimedJob, DedupKey, Idempotency, JobKind, JobPayload, JobsError, NewJob, NonRetryable,
    Registry,
};

use super::arith::{add, bytes_from_column};
use super::error::QuotaError;
use super::repo::{self, ContentSums, UsageSnapshot};

pub const QUOTA_RECONCILE_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
pub const SETTLED_RESERVATION_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
pub const USER_PAGE_SIZE: u32 = 100;
pub const MAX_AUDITED_DRIFTS_PER_EXECUTION: u64 = 100;
const MAX_CURSOR_BYTES: usize = 128;
pub const CODE_ARITHMETIC: &str = "QUOTA_RECONCILE_ARITHMETIC";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct QuotaReconcileReport {
    pub reaped: u64,
    pub reap_transactions: u32,
    pub settled_deleted: u64,
    pub users_scanned: u64,
    pub drifted_users: u64,
    pub audited_drifts: u64,
    pub already_audited: u64,
    pub inactive_object_rows: u64,
    pub arithmetic_failures: u64,
    pub resume_after: Option<String>,
}

#[derive(Clone)]
pub struct QuotaReconcileContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    audit: AuditService,
}

impl QuotaReconcileContext {
    pub const fn new(pools: DbPools, clock: Arc<dyn Clock>, audit: AuditService) -> Self {
        Self {
            pools,
            clock,
            audit,
        }
    }

    fn now(&self) -> Result<Timestamp, QuotaError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }
}

pub fn register_jobs(registry: Registry, context: QuotaReconcileContext) -> Registry {
    registry.register(
        JobKind::QuotaReconcile,
        Idempotency::key("quota.reconcile reaps by expiry and reports drift without rewriting"),
        move |job: ClaimedJob| {
            let context = context.clone();
            async move { run(job, context).await }
        },
    )
}

pub async fn ensure_scheduled(pools: &DbPools, clock: &dyn Clock) -> Result<Enqueued, JobsError> {
    let recurring = Recurring::new(JobKind::QuotaReconcile, QUOTA_RECONCILE_PERIOD)
        .map_err(|_| JobsError::CorruptRow("quota_reconcile_period"))?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, "quota.schedule_reconcile", async |tx| {
            let live: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind = ?1 AND state IN ('pending', 'claimed'))",
            )
            .bind(JobKind::QuotaReconcile.as_str())
            .fetch_one(tx.executor())
            .await?;
            if live {
                return Ok(Enqueued::Deduplicated);
            }
            recurring.schedule_successor(tx, clock, &payload).await
        })
        .await
}

async fn run(job: ClaimedJob, context: QuotaReconcileContext) -> anyhow::Result<()> {
    let after = continuation_cursor(job.payload());
    let report = reconcile_from(&context, after.as_deref())
        .await
        .inspect_err(|error| {
            tracing::warn!(
                error_kind = error.kind(),
                "quota reconciliation failed; the job runtime retries it"
            );
        })?;
    let clock = context.clock.as_ref();
    let next = report.resume_after.clone();
    context
        .pools
        .write_tx(clock, "quota.reconcile_next", async |tx| match &next {
            Some(cursor) => enqueue_continuation(tx, clock, cursor).await,
            None => {
                let recurring = Recurring::new(JobKind::QuotaReconcile, QUOTA_RECONCILE_PERIOD)
                    .map_err(|_| JobsError::CorruptRow("quota_reconcile_period"))?;
                recurring
                    .schedule_successor(tx, clock, &JobPayload::empty())
                    .await
            }
        })
        .await?;
    if report.arithmetic_failures > 0 {
        return Err(NonRetryable::new(CODE_ARITHMETIC).into());
    }
    Ok(())
}

fn continuation_cursor(payload: &Value) -> Option<String> {
    payload
        .get("after")
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty() && cursor.len() <= MAX_CURSOR_BYTES)
        .map(str::to_owned)
}

async fn enqueue_continuation(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    cursor: &str,
) -> Result<Enqueued, JobsError> {
    let recurring = Recurring::new(JobKind::QuotaReconcile, QUOTA_RECONCILE_PERIOD)
        .map_err(|_| JobsError::CorruptRow("quota_reconcile_period"))?;
    let bucket = recurring.bucket(Timestamp::try_from(clock.now())?)?;
    let key = DedupKey::new(format!(
        "{}:after:{cursor}",
        recurring.dedup_key(bucket)?.as_str()
    ))?;
    let payload = JobPayload::new(&json!({ "after": cursor }))
        .map_err(|_| JobsError::CorruptRow("quota_reconcile_cursor"))?;
    enqueue(
        tx,
        clock,
        &NewJob::new(JobKind::QuotaReconcile, payload).dedup_key(key),
    )
    .await
}

pub async fn reconcile(
    context: &QuotaReconcileContext,
) -> Result<QuotaReconcileReport, QuotaError> {
    reconcile_from(context, None).await
}

pub async fn reconcile_from(
    context: &QuotaReconcileContext,
    after: Option<&str>,
) -> Result<QuotaReconcileReport, QuotaError> {
    let mut report = QuotaReconcileReport::default();
    if after.is_none() {
        reap_stale(context, &mut report).await?;
    }
    report.resume_after = detect_drift(context, &mut report, after).await?;
    if after.is_none() {
        delete_settled(context, &mut report).await?;
    }
    tracing::info!(
        reaped = report.reaped,
        settled_deleted = report.settled_deleted,
        users_scanned = report.users_scanned,
        drifted_users = report.drifted_users,
        audited_drifts = report.audited_drifts,
        already_audited = report.already_audited,
        inactive_object_rows = report.inactive_object_rows,
        arithmetic_failures = report.arithmetic_failures,
        continues = report.resume_after.is_some(),
        "quota reconciliation finished"
    );
    Ok(report)
}

async fn reap_stale(
    context: &QuotaReconcileContext,
    report: &mut QuotaReconcileReport,
) -> Result<(), QuotaError> {
    loop {
        let now = context.now()?;
        let batch = context
            .pools
            .write_tx(context.clock.as_ref(), "quota.reconcile.reap", async |tx| {
                repo::reap_stale(tx.executor(), now, MAX_ROWS_PER_TX).await
            })
            .await?;
        for reservation in &batch {
            tracing::warn!(
                reservation_id = reservation.id,
                transfer_session_id = reservation.session_id,
                owner_id = reservation.owner_id,
                expires_at = reservation.expires_at,
                reserved_bytes = reservation.reserved.get(),
                "stale quota reservation released by the reaper; the owning transfer path did not settle it"
            );
        }
        let released = u64::try_from(batch.len()).unwrap_or(u64::MAX);
        report.reaped = report.reaped.saturating_add(released);
        report.reap_transactions = report.reap_transactions.saturating_add(1);
        if released < u64::from(MAX_ROWS_PER_TX) {
            return Ok(());
        }
    }
}

async fn delete_settled(
    context: &QuotaReconcileContext,
    report: &mut QuotaReconcileReport,
) -> Result<(), QuotaError> {
    let cutoff = Timestamp::try_from(context.clock.now() - SETTLED_RESERVATION_RETENTION)?;
    loop {
        let deleted = context
            .pools
            .write_tx(
                context.clock.as_ref(),
                "quota.reconcile.delete_settled",
                async |tx| repo::delete_settled(tx.executor(), cutoff, MAX_ROWS_PER_TX).await,
            )
            .await?;
        report.settled_deleted = report.settled_deleted.saturating_add(deleted);
        if deleted < u64::from(MAX_ROWS_PER_TX) {
            return Ok(());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Expected {
    pub(super) my_files: ByteSize,
    pub(super) received: ByteSize,
    pub(super) total: ByteSize,
}

pub(super) fn expected_usage(snapshot: &UsageSnapshot) -> Result<Expected, QuotaError> {
    let my_files = snapshot.files.limbs.total("live_my_files_bytes")?;
    let received = snapshot.received.limbs.total("live_received_bytes")?;
    let total = add(my_files, received, "live_my_files_plus_received")?;
    Ok(Expected {
        my_files,
        received,
        total,
    })
}

fn inactive_rows(sums: &ContentSums, other: &ContentSums) -> u64 {
    sums.inactive_objects.saturating_add(other.inactive_objects)
}

async fn detect_drift(
    context: &QuotaReconcileContext,
    report: &mut QuotaReconcileReport,
    after: Option<&str>,
) -> Result<Option<String>, QuotaError> {
    let mut cursor = after.unwrap_or_default().to_owned();
    loop {
        let page =
            repo::usage_page(context.pools.reader().executor(), &cursor, USER_PAGE_SIZE).await?;
        for snapshot in &page {
            report.users_scanned = report.users_scanned.saturating_add(1);
            inspect(context, report, snapshot).await?;
            if report.audited_drifts >= MAX_AUDITED_DRIFTS_PER_EXECUTION {
                return Ok(Some(snapshot.user_id.clone()));
            }
        }
        match page.last() {
            Some(last) if page.len() >= usize::try_from(USER_PAGE_SIZE).unwrap_or(usize::MAX) => {
                cursor.clone_from(&last.user_id);
            }
            _ => return Ok(None),
        }
    }
}

async fn inspect(
    context: &QuotaReconcileContext,
    report: &mut QuotaReconcileReport,
    snapshot: &UsageSnapshot,
) -> Result<(), QuotaError> {
    let broken = inactive_rows(&snapshot.files, &snapshot.received);
    if broken > 0 {
        report.inactive_object_rows = report.inactive_object_rows.saturating_add(broken);
        tracing::error!(
            user_id = snapshot.user_id,
            inactive_object_rows = broken,
            "live content rows reference storage objects that are not active"
        );
    }
    match expected_usage(snapshot) {
        Ok(expected) if snapshot.used == expected.total.to_i64() => Ok(()),
        Ok(_) => confirm_drift(context, report, &snapshot.user_id).await,
        Err(error) => {
            note_arithmetic_failure(report, &snapshot.user_id, &error);
            Ok(())
        }
    }
}

pub(super) async fn confirm_drift(
    context: &QuotaReconcileContext,
    report: &mut QuotaReconcileReport,
    user_id: &str,
) -> Result<(), QuotaError> {
    let fresh = context
        .pools
        .write_tx(
            context.clock.as_ref(),
            "quota.reconcile.confirm_drift",
            async |tx| repo::usage_for(tx.executor(), user_id).await,
        )
        .await?;
    let Some(fresh) = fresh else {
        return Ok(());
    };
    let expected = match expected_usage(&fresh) {
        Ok(expected) => expected,
        Err(error) => {
            note_arithmetic_failure(report, user_id, &error);
            return Ok(());
        }
    };
    let used = bytes_from_column(fresh.used, "users.used_bytes")?;
    if used == expected.total {
        return Ok(());
    }
    report.drifted_users = report.drifted_users.saturating_add(1);
    let difference = expected.total.to_i64().checked_sub(used.to_i64());
    tracing::warn!(
        user_id,
        used_bytes = used.get(),
        expected_bytes = expected.total.get(),
        difference_bytes = difference,
        my_files_bytes = expected.my_files.get(),
        received_bytes = expected.received.get(),
        "quota usage drift detected; users.used_bytes was left unchanged"
    );
    let spec = actions::quota_drift_detected(&QuotaDriftFacts {
        used_bytes: used.get(),
        expected_bytes: expected.total.get(),
        difference_bytes: difference,
        my_files_bytes: expected.my_files.get(),
        received_bytes: expected.received.get(),
        file_count: fresh.files.rows,
        received_file_count: fresh.received.rows,
        inactive_object_rows: inactive_rows(&fresh.files, &fresh.received),
    });
    let since = Timestamp::try_from(context.clock.now() - QUOTA_RECONCILE_PERIOD)?;
    if repo::drift_audited_since(
        context.pools.reader().executor(),
        user_id,
        since,
        spec.metadata().as_str(),
    )
    .await?
    {
        report.already_audited = report.already_audited.saturating_add(1);
        return Ok(());
    }
    report.audited_drifts = report.audited_drifts.saturating_add(1);
    context.audit.record_async(
        AuditEvent::new(spec, Actor::system(), Outcome::Success, context.now()?)
            .with_target(Target::new(TargetType::User).id(user_id)),
    );
    Ok(())
}

fn note_arithmetic_failure(report: &mut QuotaReconcileReport, user_id: &str, error: &QuotaError) {
    report.arithmetic_failures = report.arithmetic_failures.saturating_add(1);
    tracing::error!(
        user_id,
        error_kind = error.kind(),
        "live usage could not be summed in 64-bit arithmetic; users.used_bytes was left unchanged"
    );
}
