use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use time::OffsetDateTime;

use super::{json_list, LifecycleContext, LifecycleError};
use crate::domain::clock::Clock;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{self, OrphanSweepCounts};
use crate::features::audit::model::{Actor, AuditEvent, Outcome, Target, TargetType};
use crate::infra::db::{DbError, DbPools};
use crate::infra::jobs::claim::MAX_ROWS_PER_TX;
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{ClaimedJob, JobKind, JobPayload};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{ListCursor, ListEntry, MAX_LIST_PAGE_SIZE};

pub const ORPHAN_SWEEP_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
pub const SWEEP_PAGE_SIZE: u32 = MAX_LIST_PAGE_SIZE;
pub const SWEEP_PREFIX: &str = "objects/";
pub const ORPHAN_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const STALE_TOMBSTONE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const DELETED_ROW_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const REAP_REFUSAL_PERCENT: u64 = 5;

const TRACKED_KEYS: &str = "SELECT object_key FROM storage_objects
                             WHERE object_key IN (SELECT value FROM json_each(?1))";

const LIVE_TRANSFER_KEYS: &str = "SELECT final_object_key FROM transfer_session_files
                                   WHERE final_object_key IN (SELECT value FROM json_each(?1))
                                     AND state IN ('pending','uploading','finalizing')";

const STALE_TOMBSTONES: &str = "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM storage_objects
                                 WHERE state = 'tombstoned' AND tombstoned_at <= ?1";

const REAP_DELETED_QUEUE: &str = "DELETE FROM file_deletion_queue
                                   WHERE storage_object_id IN (
                                         SELECT id FROM storage_objects
                                          WHERE state = 'deleted' AND deleted_at <= ?1
                                          ORDER BY deleted_at LIMIT ?2)";

const REAP_DELETED_OBJECTS: &str = "DELETE FROM storage_objects
                                     WHERE id IN (SELECT id FROM storage_objects
                                                   WHERE state = 'deleted' AND deleted_at <= ?1
                                                   ORDER BY deleted_at LIMIT ?2)";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SweepOutcome {
    #[default]
    Clean,
    ReportOnly,
    Reaped,
    Refused,
}

impl SweepOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::ReportOnly => "report_only",
            Self::Reaped => "reaped",
            Self::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanCounts {
    pub listed: u64,
    pub valid: u64,
    pub unparseable: u64,
    pub unparseable_bytes: u64,
    pub tracked: u64,
    pub live_transfer: u64,
    pub too_young: u64,
    pub candidates: u64,
    pub candidate_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub outcome: SweepOutcome,
    pub reap_enabled: bool,
    pub scan: ScanCounts,
    pub pages: u64,
    pub largest_page: u64,
    pub db_lookups: u64,
    pub reaped: u64,
    pub reaped_bytes: u64,
    pub reap_failures: u64,
    pub stale_tombstones: u64,
    pub stale_tombstone_bytes: u64,
    pub deleted_rows_reaped: u64,
}

impl SweepReport {
    pub const fn detected(&self) -> bool {
        self.scan.candidates > 0 || self.scan.unparseable > 0
    }

    fn counts(&self) -> OrphanSweepCounts {
        OrphanSweepCounts {
            outcome: self.outcome.as_str(),
            reap_enabled: self.reap_enabled,
            listed: self.scan.listed,
            unparseable: self.scan.unparseable,
            unparseable_bytes: self.scan.unparseable_bytes,
            candidates: self.scan.candidates,
            candidate_bytes: self.scan.candidate_bytes,
            too_young: self.scan.too_young,
            live_transfer: self.scan.live_transfer,
            reaped: self.reaped,
            reaped_bytes: self.reaped_bytes,
            stale_tombstones: self.stale_tombstones,
        }
    }
}

pub const fn refuses_reap(candidates: u64, valid: u64) -> bool {
    candidates.saturating_mul(100) > valid.saturating_mul(REAP_REFUSAL_PERCENT)
}

#[derive(Debug)]
pub enum SweepError {
    Storage(StorageError),
    Lifecycle(LifecycleError),
}

impl SweepError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Storage(error) => error.kind(),
            Self::Lifecycle(error) => error.kind(),
        }
    }
}

impl fmt::Display for SweepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "orphan sweep listing failed: {}", error.kind()),
            Self::Lifecycle(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SweepError {}

impl From<StorageError> for SweepError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<LifecycleError> for SweepError {
    fn from(error: LifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<sqlx::Error> for SweepError {
    fn from(error: sqlx::Error) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

impl From<DbError> for SweepError {
    fn from(error: DbError) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

impl From<InvalidTimestamp> for SweepError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Lifecycle(LifecycleError::from(error))
    }
}

struct Candidate {
    key: ObjectKey,
    size: u64,
}

pub async fn sweep(context: &LifecycleContext) -> Result<SweepReport, SweepError> {
    let clock = context.clock.as_ref();
    let now = clock.now();
    let mut report = SweepReport {
        reap_enabled: context.orphan_reap,
        ..SweepReport::default()
    };

    let (count, bytes) = stale_tombstones(&context.pools, now).await?;
    report.stale_tombstones = count;
    report.stale_tombstone_bytes = bytes;

    let mut cursor = None;
    loop {
        let page = list(context, cursor).await?;
        report.pages += 1;
        report.largest_page = report
            .largest_page
            .max(u64::try_from(page.entries.len()).unwrap_or(u64::MAX));
        scan_page(
            &context.pools,
            &page.entries,
            now,
            &mut report.scan,
            &mut report.db_lookups,
        )
        .await?;
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }

    report.outcome = if report.scan.candidates == 0 {
        SweepOutcome::Clean
    } else if !context.orphan_reap {
        SweepOutcome::ReportOnly
    } else if refuses_reap(report.scan.candidates, report.scan.valid) {
        SweepOutcome::Refused
    } else {
        reap(context, now, &mut report).await?;
        SweepOutcome::Reaped
    };

    report.deleted_rows_reaped = reap_deleted_rows(&context.pools, clock).await?;
    announce(context, &report)?;
    Ok(report)
}

async fn list(
    context: &LifecycleContext,
    cursor: Option<ListCursor>,
) -> Result<crate::storage::provider::ListPage, SweepError> {
    let page = context
        .provider
        .list_page(SWEEP_PREFIX, cursor, SWEEP_PAGE_SIZE)
        .await?;
    debug_assert!(page.entries.len() <= SWEEP_PAGE_SIZE as usize);
    Ok(page)
}

async fn reap(
    context: &LifecycleContext,
    now: OffsetDateTime,
    report: &mut SweepReport,
) -> Result<(), SweepError> {
    let mut cursor = None;
    loop {
        let page = list(context, cursor).await?;
        let mut rescan = ScanCounts::default();
        let candidates = scan_page(
            &context.pools,
            &page.entries,
            now,
            &mut rescan,
            &mut report.db_lookups,
        )
        .await?;
        for candidate in candidates {
            match context.provider.delete(&candidate.key).await {
                Ok(_) | Err(StorageError::NotFound) => {
                    report.reaped += 1;
                    report.reaped_bytes = report.reaped_bytes.saturating_add(candidate.size);
                }
                Err(error) => {
                    report.reap_failures += 1;
                    tracing::warn!(
                        storage_error = error.kind(),
                        "an orphan candidate could not be deleted; the next sweep retries it"
                    );
                }
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            return Ok(());
        }
    }
}

async fn scan_page(
    pools: &DbPools,
    entries: &[ListEntry],
    now: OffsetDateTime,
    counts: &mut ScanCounts,
    db_lookups: &mut u64,
) -> Result<Vec<Candidate>, SweepError> {
    let mut valid = Vec::with_capacity(entries.len());
    for entry in entries {
        counts.listed += 1;
        match ObjectKey::parse(&entry.key) {
            Ok(key) => valid.push((key, entry)),
            Err(_) => {
                counts.unparseable += 1;
                counts.unparseable_bytes = counts.unparseable_bytes.saturating_add(entry.size);
            }
        }
    }
    counts.valid += u64::try_from(valid.len()).unwrap_or(u64::MAX);
    if valid.is_empty() {
        return Ok(Vec::new());
    }

    let list = json_list(valid.iter().map(|(key, _)| key.as_str()));
    let tracked: HashSet<String> = sqlx::query_scalar(TRACKED_KEYS)
        .bind(&list)
        .fetch_all(pools.reader().executor())
        .await
        .map_err(LifecycleError::from)?
        .into_iter()
        .collect();
    let live: HashSet<String> = sqlx::query_scalar(LIVE_TRANSFER_KEYS)
        .bind(&list)
        .fetch_all(pools.reader().executor())
        .await
        .map_err(LifecycleError::from)?
        .into_iter()
        .collect();
    *db_lookups += 2;

    let mut candidates = Vec::new();
    for (key, entry) in valid {
        if tracked.contains(key.as_str()) {
            counts.tracked += 1;
        } else if live.contains(key.as_str()) {
            counts.live_transfer += 1;
        } else if now - entry.modified_at < ORPHAN_MIN_AGE {
            counts.too_young += 1;
        } else {
            counts.candidates += 1;
            counts.candidate_bytes = counts.candidate_bytes.saturating_add(entry.size);
            candidates.push(Candidate {
                key,
                size: entry.size,
            });
        }
    }
    Ok(candidates)
}

async fn stale_tombstones(pools: &DbPools, now: OffsetDateTime) -> Result<(u64, u64), SweepError> {
    let cutoff = Timestamp::try_from(now - STALE_TOMBSTONE_AGE)?;
    let (count, bytes): (i64, i64) = sqlx::query_as(STALE_TOMBSTONES)
        .bind(cutoff.to_string())
        .fetch_one(pools.reader().executor())
        .await?;
    Ok((
        u64::try_from(count).unwrap_or(0),
        u64::try_from(bytes).unwrap_or(0),
    ))
}

pub async fn reap_deleted_rows(pools: &DbPools, clock: &dyn Clock) -> Result<u64, SweepError> {
    let cutoff = Timestamp::try_from(clock.now() - DELETED_ROW_RETENTION)?.to_string();
    let mut reaped = 0_u64;
    loop {
        let batch = pools
            .write_tx(clock, "storage.orphan_sweep.reap_deleted", async |tx| {
                sqlx::query(REAP_DELETED_QUEUE)
                    .bind(&cutoff)
                    .bind(i64::from(MAX_ROWS_PER_TX))
                    .execute(tx.executor())
                    .await?;
                let deleted = sqlx::query(REAP_DELETED_OBJECTS)
                    .bind(&cutoff)
                    .bind(i64::from(MAX_ROWS_PER_TX))
                    .execute(tx.executor())
                    .await?;
                Ok::<u64, LifecycleError>(deleted.rows_affected())
            })
            .await?;
        reaped = reaped.saturating_add(batch);
        if batch < u64::from(MAX_ROWS_PER_TX) {
            return Ok(reaped);
        }
    }
}

fn announce(context: &LifecycleContext, report: &SweepReport) -> Result<(), SweepError> {
    if report.stale_tombstones > 0 {
        tracing::warn!(
            stale_tombstones = report.stale_tombstones,
            stale_tombstone_bytes = report.stale_tombstone_bytes,
            "storage objects have been tombstoned for more than 24 h; physical deletion is failing"
        );
    }
    if report.outcome == SweepOutcome::Refused {
        tracing::error!(
            candidates = report.scan.candidates,
            valid = report.scan.valid,
            refusal_percent = REAP_REFUSAL_PERCENT,
            "orphan reaping refused: too large a share of storage has no database record; check for a restored database, a wrong bucket or a wrong volume"
        );
    }
    if !report.detected() {
        tracing::info!(
            listed = report.scan.listed,
            pages = report.pages,
            deleted_rows_reaped = report.deleted_rows_reaped,
            "orphan sweep found no orphans"
        );
        return Ok(());
    }
    tracing::warn!(
        outcome = report.outcome.as_str(),
        reap_enabled = report.reap_enabled,
        listed = report.scan.listed,
        valid = report.scan.valid,
        unparseable = report.scan.unparseable,
        unparseable_bytes = report.scan.unparseable_bytes,
        candidates = report.scan.candidates,
        candidate_bytes = report.scan.candidate_bytes,
        too_young = report.scan.too_young,
        live_transfer = report.scan.live_transfer,
        reaped = report.reaped,
        reaped_bytes = report.reaped_bytes,
        reap_failures = report.reap_failures,
        "storage orphans detected"
    );
    let occurred_at = Timestamp::try_from(context.clock.now())?;
    let event = AuditEvent::new(
        actions::storage_orphan_detected(&report.counts()),
        Actor::system(),
        Outcome::Success,
        occurred_at,
    )
    .with_target(Target::new(TargetType::System).label(context.configured.as_str()));
    context.audit.record_async(event);
    Ok(())
}

pub(super) async fn run(_job: ClaimedJob, context: LifecycleContext) -> anyhow::Result<()> {
    let report = sweep(&context).await.inspect_err(|error| {
        tracing::warn!(
            error_kind = error.kind(),
            "orphan sweep failed; the job runtime retries it"
        );
    })?;
    tracing::debug!(
        outcome = report.outcome.as_str(),
        pages = report.pages,
        "orphan sweep finished"
    );
    let recurring = Recurring::new(JobKind::StorageOrphanSweep, ORPHAN_SWEEP_PERIOD)?;
    let payload = JobPayload::empty();
    let clock = context.clock.as_ref();
    context
        .pools
        .write_tx(clock, "storage.orphan_sweep_successor", async |tx| {
            recurring.schedule_successor(tx, clock, &payload).await
        })
        .await?;
    Ok(())
}

pub async fn ensure_scheduled(pools: &DbPools, clock: &dyn Clock) -> anyhow::Result<()> {
    let recurring = Recurring::new(JobKind::StorageOrphanSweep, ORPHAN_SWEEP_PERIOD)?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, "storage.schedule_orphan_sweep", async |tx| {
            recurring.schedule(tx, clock, &payload).await
        })
        .await?;
    Ok(())
}
