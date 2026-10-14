mod retention;
mod schedule;
mod sessions;
mod staging;
mod uploads;

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::service::AuditService;
use crate::features::quota::service::QuotaService;
use crate::features::settings::SettingsHandle;
use crate::infra::db::{DbError, DbPools};
use crate::storage::staging::StagingStorage;

use super::error::TransferError;

pub use schedule::{ensure_scheduled, register_jobs};
#[cfg(test)]
pub use uploads::{expire_upload, UploadOutcome};

pub const EXPIRE_STALE_PERIOD: Duration = Duration::from_secs(15 * 60);
pub const PAGE_SIZE: u32 = 200;
pub const PASS_LIMIT: u32 = 1_000;
pub const MAX_CONTINUATIONS: u32 = 50;
pub const TERMINAL_UPLOAD_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const TERMINAL_SESSION_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone)]
pub struct ReaperContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    quota: QuotaService,
    audit: AuditService,
    staging: Option<Arc<dyn StagingStorage>>,
    stop: CancellationToken,
}

impl ReaperContext {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        audit: AuditService,
        staging: Option<Arc<dyn StagingStorage>>,
        stop: CancellationToken,
    ) -> Self {
        let quota = QuotaService::new(settings, Arc::clone(&clock));
        Self {
            pools,
            clock,
            quota,
            audit,
            staging,
            stop,
        }
    }

    fn now(&self) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    fn stopping(&self) -> bool {
        self.stop.is_cancelled()
    }

    async fn reader(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, TransferError> {
        Ok(self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReapReport {
    pub sessions_expired: u64,
    pub sessions_skipped: u64,
    pub uploads_expired: u64,
    pub uploads_skipped: u64,
    pub staging_removed: u64,
    pub staging_failed: u64,
    pub uploads_pruned: u64,
    pub sessions_pruned: u64,
    pub errors: u64,
    pub more: bool,
}

pub async fn run_pass(context: &ReaperContext) -> Result<ReapReport, TransferError> {
    let mut report = ReapReport::default();
    let capped = sessions::expire_due(context, &mut report).await?;
    report.more |= capped;
    let capped = uploads::expire_due(context, &mut report).await?;
    report.more |= capped;
    let capped = staging::remove_pending(context, &mut report).await?;
    report.more |= capped;
    let capped = retention::prune(context, &mut report).await?;
    report.more |= capped;
    Ok(report)
}

fn before(at: Timestamp, window: Duration) -> Result<Timestamp, TransferError> {
    Ok(Timestamp::try_from(at.get() - window)?)
}

#[cfg(test)]
const DATE: &str = "'2026-10-01T00:00:00.000Z'";

#[cfg(test)]
pub(crate) const PLANNED_QUERIES: [(&str, &str, [&str; 4], &str); 7] = [
    (
        "live sessions past expiry",
        sessions::LIVE_PAGE,
        [DATE, "''", "''", "200"],
        "ix_transfer_sessions_expiry",
    ),
    (
        "failed sessions past expiry",
        sessions::FAILED_PAGE,
        [DATE, "''", "''", "200"],
        "ix_transfer_sessions_failed_expiry",
    ),
    (
        "uploads past expiry",
        uploads::DUE_PAGE,
        [DATE, "''", "''", "200"],
        "ix_tus_uploads_expiry",
    ),
    (
        "terminal uploads awaiting staging removal",
        super::cleanup::TUS_STAGING_PAGE,
        ["''", "200", DATE, "''"],
        "ix_tus_uploads_cleanup",
    ),
    (
        "terminal uploads past retention",
        retention::PRUNE_UPLOADS,
        [DATE, "200", "''", "''"],
        "ix_tus_uploads_retention",
    ),
    (
        "terminal sessions past retention",
        retention::PRUNE_SESSIONS,
        [DATE, "200", "''", "''"],
        "ix_transfer_sessions_retention",
    ),
    (
        "abandoned multipart rows",
        super::cleanup::ABANDONED_MULTIPART_PAGE,
        ["''", "200", "''", "''"],
        "ix_s3mp_abandoned",
    ),
];

#[cfg(test)]
mod tests {
    const FORBIDDEN_IN_TRANSACTIONS: [&str; 8] = [
        "StagingStorage",
        ".remove(",
        "staging.",
        "std::fs",
        "tokio::fs",
        "read_dir",
        "unlink",
        "rename(",
    ];

    fn production(source: &str) -> &str {
        source.split("#[cfg(test)]").next().unwrap_or(source)
    }

    #[test]
    fn unit_reaper_database_modules_never_touch_storage() {
        for (file, source) in [
            ("sessions.rs", include_str!("reaper/sessions.rs")),
            ("uploads.rs", include_str!("reaper/uploads.rs")),
            ("retention.rs", include_str!("reaper/retention.rs")),
            ("schedule.rs", include_str!("reaper/schedule.rs")),
        ] {
            let source = production(source);
            for token in FORBIDDEN_IN_TRANSACTIONS {
                assert!(!source.contains(token), "{file} names {token}");
            }
        }
    }

    #[test]
    fn unit_reaper_staging_removal_runs_outside_every_transaction() {
        let source = production(include_str!("reaper/staging.rs"));
        let (_, marker) = source
            .split_once("async fn mark_removed")
            .expect("the marking transaction");
        assert!(!marker.contains(".remove("), "{marker}");
        let (loop_body, _) = source
            .split_once("async fn remove_one")
            .expect("the removal step");
        assert!(!loop_body.contains("write_tx"), "{loop_body}");
        for token in [
            "format!(", ".join(", "PathBuf", "Path::", "filename", "relative",
        ] {
            assert!(
                !source.contains(token),
                "staging removal derives a path via {token}"
            );
        }
        assert!(source.contains("UploadId::parse"));
    }

    #[test]
    fn unit_reaper_names_no_unbounded_selects() {
        for source in [
            include_str!("reaper/sessions.rs"),
            include_str!("reaper/uploads.rs"),
            include_str!("reaper/retention.rs"),
            include_str!("cleanup.rs"),
        ] {
            let source = production(source);
            for statement in source.split("\";\n").filter(|part| part.contains("SELECT")) {
                assert!(
                    statement.contains("LIMIT") || statement.contains("WHERE tu.id = ?1"),
                    "{statement}"
                );
            }
        }
    }
}
