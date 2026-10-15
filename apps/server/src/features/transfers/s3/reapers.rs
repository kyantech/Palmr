mod abort;
mod expire;
mod hooks;
mod orphan;
mod payload;
mod prune;
mod repo;
mod rows;
mod scan;
mod schedule;
mod summary;

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::service::AuditService;
use crate::features::quota::service::QuotaService;
use crate::features::settings::SettingsHandle;
use crate::infra::db::{DbError, DbPools};
use crate::infra::jobs::{ClaimedJob, Idempotency, JobKind, JobsError, NonRetryable, Registry};

use super::super::admission::TransferStorage;
use super::super::error::TransferError;
use super::model::MultipartId;
use super::proxied::ProxyUploads;
use abort::{abort_abandoned, abort_one, Timing};
use expire::{expire_due, expire_one};
use orphan::{abort_orphan, OrphanTarget};
use payload::{parse, AbortJob, ReconcileJob};
use prune::prune_terminal;
use rows::reconcile_rows;
use scan::{scan_provider, ScanStatus};
use schedule::{enqueue_continuation, schedule_successor};

#[cfg(feature = "e2e-fixture")]
pub use hooks::PauseHook;
pub use hooks::{Point, ReaperHooks};
#[cfg(test)]
pub use scan::{Coverage, Strategy, SENTINEL_PREFIX};
pub use schedule::schedule_startup;
pub use summary::Summary;

pub const BATCH: u32 = 100;
pub const ABORT_CONCURRENCY: usize = 4;
pub const FAILURE_LIMIT: u32 = 5;
pub const MAX_CONTINUATIONS: u32 = 1_000;
pub const MAX_SCAN_PAGES: u32 = 200;
pub const ABORT_GRACE: Duration = Duration::from_secs(2 * 60);
pub const ORPHAN_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const TERMINAL_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const CODE_NO_STORAGE: &str = "S3_REAPER_NO_MULTIPART_STORAGE";
const CODE_INVARIANT: &str = "S3_REAPER_INVARIANT";
const CODE_REJECTED: &str = "S3_REAPER_PROVIDER_REJECTED";

pub struct S3ReaperParts {
    pub pools: DbPools,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsHandle,
    pub audit: AuditService,
    pub storage: TransferStorage,
    pub proxy: Option<ProxyUploads>,
    pub stop: CancellationToken,
    pub hooks: Option<Arc<dyn ReaperHooks>>,
}

#[derive(Clone)]
pub struct S3ReaperContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    quota: QuotaService,
    audit: AuditService,
    storage: TransferStorage,
    proxy: Option<ProxyUploads>,
    stop: CancellationToken,
    hooks: Option<Arc<dyn ReaperHooks>>,
    status: ScanStatus,
}

impl S3ReaperContext {
    pub fn new(parts: S3ReaperParts) -> Self {
        let quota = QuotaService::new(parts.settings, Arc::clone(&parts.clock));
        Self {
            pools: parts.pools,
            clock: parts.clock,
            quota,
            audit: parts.audit,
            storage: parts.storage,
            proxy: parts.proxy,
            stop: parts.stop,
            hooks: parts.hooks,
            status: ScanStatus::default(),
        }
    }

    #[cfg(test)]
    pub fn scan_status(&self) -> ScanStatus {
        self.status.clone()
    }

    fn now(&self) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    fn stopping(&self) -> bool {
        self.stop.is_cancelled()
    }

    fn proxy_active(&self, upload: MultipartId) -> bool {
        self.proxy
            .as_ref()
            .is_some_and(|proxy| proxy.in_flight(upload))
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

    async fn reached(&self, point: Point) -> Result<(), TransferError> {
        match &self.hooks {
            Some(hooks) if !hooks.proceeds(point).await => Err(TransferError::Invariant {
                what: "simulated_crash",
            }),
            _ => Ok(()),
        }
    }
}

fn jobs_error(error: JobsError) -> TransferError {
    match error {
        JobsError::Db(error) => TransferError::Db(error),
        JobsError::Time(error) => TransferError::Time(error),
        JobsError::Dedup(_) | JobsError::CorruptRow(_) => TransferError::Invariant {
            what: "job_enqueue",
        },
    }
}

fn job_failure(error: TransferError) -> anyhow::Error {
    match error {
        TransferError::StorageUnavailable => NonRetryable::new(CODE_NO_STORAGE).into(),
        TransferError::Invariant { .. } => NonRetryable::new(CODE_INVARIANT).into(),
        TransferError::Storage(error) if !error.is_retryable() => {
            NonRetryable::new(CODE_REJECTED).into()
        }
        other => anyhow::anyhow!("{}", other.kind()),
    }
}

fn scheduling(error: JobsError) -> anyhow::Error {
    anyhow::anyhow!("{}", error.kind())
}

pub fn register_jobs(registry: Registry, context: S3ReaperContext) -> Registry {
    let abort = context.clone();
    registry
        .register(
            JobKind::S3AbortAbandonedMultipart,
            Idempotency::key(
                "s3.abort_abandoned_multipart moves every row by a guarded transition, treats NoSuchUpload as success, re-selects its candidates from the database and keys targeted work by the multipart id or the provider identity",
            ),
            move |job: ClaimedJob| {
                let context = abort.clone();
                async move { run_abort(job, context).await }
            },
        )
        .register(
            JobKind::S3ReconcileMultipart,
            Idempotency::key(
                "s3.reconcile_multipart only reads the provider, rewrites the part cache under a guarded state and queues provider orphans under a dedup key derived from their identity",
            ),
            move |job: ClaimedJob| {
                let context = context.clone();
                async move { run_reconcile(job, context).await }
            },
        )
}

fn continuation(payload: &AbortJob) -> u32 {
    match payload {
        AbortJob::Expire { continuation, .. }
        | AbortJob::Abort { continuation, .. }
        | AbortJob::Prune { continuation } => *continuation,
        _ => 0,
    }
}

async fn run_abort(job: ClaimedJob, context: S3ReaperContext) -> anyhow::Result<()> {
    let payload: AbortJob = parse(job.payload())?;
    let mut summary = Summary {
        retried: job.attempts(),
        ..Summary::default()
    };
    let clock = context.clock.as_ref();
    let kind = JobKind::S3AbortAbandonedMultipart;
    match &payload {
        AbortJob::Sweep {} => {
            schedule_successor(&context.pools, clock, kind)
                .await
                .map_err(scheduling)?;
            if context.storage.multipart().is_none() {
                tracing::debug!(
                    "no multipart storage is configured; the S3 abort sweep has nothing to do"
                );
                return Ok(());
            }
            let next = expire_due(&context, None, &mut summary)
                .await
                .map_err(job_failure)?;
            if let Some(after) = next {
                enqueue_continuation(
                    &context.pools,
                    clock,
                    kind,
                    "expire",
                    1,
                    &AbortJob::Expire {
                        after: Some(after),
                        continuation: 1,
                    },
                )
                .await
                .map_err(scheduling)?;
            }
            abort_phase(&context, None, 0, &mut summary).await?;
            prune_phase(&context, 0, &mut summary).await?;
        }
        AbortJob::Expire {
            after,
            continuation: sequence,
        } => {
            let next = expire_due(&context, after.clone(), &mut summary)
                .await
                .map_err(job_failure)?;
            if let Some(after) = next.filter(|_| *sequence < MAX_CONTINUATIONS) {
                let sequence = sequence + 1;
                enqueue_continuation(
                    &context.pools,
                    clock,
                    kind,
                    "expire",
                    sequence,
                    &AbortJob::Expire {
                        after: Some(after),
                        continuation: sequence,
                    },
                )
                .await
                .map_err(scheduling)?;
            }
        }
        AbortJob::Abort {
            after,
            continuation: sequence,
        } => abort_phase(&context, after.clone(), *sequence, &mut summary).await?,
        AbortJob::Prune {
            continuation: sequence,
        } => prune_phase(&context, *sequence, &mut summary).await?,
        AbortJob::Multipart { multipart_id } => {
            let id: MultipartId = multipart_id
                .parse()
                .map_err(|_| NonRetryable::new(payload::PAYLOAD_INVALID))?;
            expire_one(&context, id).await.map_err(job_failure)?;
            let outcome = abort_one(&context, id, Timing::Targeted)
                .await
                .map_err(job_failure)?;
            summary.matched += 1;
            if outcome == abort::AbortOutcome::Aborted {
                summary.aborted += 1;
            }
        }
        AbortJob::Orphan {
            bucket,
            key,
            upload_id,
            initiated_at,
        } => {
            let target =
                OrphanTarget::parse(bucket, key, upload_id, initiated_at).map_err(job_failure)?;
            summary.matched += 1;
            match abort_orphan(&context, &target).await.map_err(job_failure)? {
                orphan::OrphanOutcome::Aborted => summary.aborted += 1,
                orphan::OrphanOutcome::TooYoung => summary.provider_orphans_too_young += 1,
                orphan::OrphanOutcome::Known => summary.skipped_live += 1,
            }
        }
    }
    summary.log(
        JobKind::S3AbortAbandonedMultipart.as_str(),
        continuation(&payload),
    );
    if summary.failed > 0 {
        anyhow::bail!(
            "s3.abort_abandoned_multipart left {} candidates unprocessed",
            summary.failed
        );
    }
    Ok(())
}

async fn abort_phase(
    context: &S3ReaperContext,
    after: Option<String>,
    sequence: u32,
    summary: &mut Summary,
) -> anyhow::Result<()> {
    let pass = abort_abandoned(context, after, summary)
        .await
        .map_err(job_failure)?;
    if let Some(after) = pass.next.filter(|_| sequence < MAX_CONTINUATIONS) {
        let sequence = sequence + 1;
        enqueue_continuation(
            &context.pools,
            context.clock.as_ref(),
            JobKind::S3AbortAbandonedMultipart,
            "abort",
            sequence,
            &AbortJob::Abort {
                after: Some(after),
                continuation: sequence,
            },
        )
        .await
        .map_err(scheduling)?;
    }
    Ok(())
}

async fn prune_phase(
    context: &S3ReaperContext,
    sequence: u32,
    summary: &mut Summary,
) -> anyhow::Result<()> {
    let more = prune_terminal(context, summary)
        .await
        .map_err(job_failure)?;
    if more && sequence < MAX_CONTINUATIONS {
        let sequence = sequence + 1;
        enqueue_continuation(
            &context.pools,
            context.clock.as_ref(),
            JobKind::S3AbortAbandonedMultipart,
            "prune",
            sequence,
            &AbortJob::Prune {
                continuation: sequence,
            },
        )
        .await
        .map_err(scheduling)?;
    }
    Ok(())
}

async fn run_reconcile(job: ClaimedJob, context: S3ReaperContext) -> anyhow::Result<()> {
    let payload: ReconcileJob = parse(job.payload())?;
    let mut summary = Summary {
        retried: job.attempts(),
        ..Summary::default()
    };
    let kind = JobKind::S3ReconcileMultipart;
    let (after, sequence) = match &payload {
        ReconcileJob::Sweep {} => {
            schedule_successor(&context.pools, context.clock.as_ref(), kind)
                .await
                .map_err(scheduling)?;
            if context.storage.multipart().is_none() {
                tracing::debug!(
                    "no multipart storage is configured; the S3 reconciliation has nothing to do"
                );
                return Ok(());
            }
            scan_provider(&context, &mut summary).await;
            (None, 0)
        }
        ReconcileJob::Rows {
            after,
            continuation,
        } => (after.clone(), *continuation),
    };
    let next = reconcile_rows(&context, after, &mut summary)
        .await
        .map_err(job_failure)?;
    if let Some(after) = next.filter(|_| sequence < MAX_CONTINUATIONS) {
        let sequence = sequence + 1;
        enqueue_continuation(
            &context.pools,
            context.clock.as_ref(),
            kind,
            "rows",
            sequence,
            &ReconcileJob::Rows {
                after: Some(after),
                continuation: sequence,
            },
        )
        .await
        .map_err(scheduling)?;
    }
    summary.log(kind.as_str(), sequence);
    if summary.failed > 0 {
        anyhow::bail!(
            "s3.reconcile_multipart left {} rows unreconciled",
            summary.failed
        );
    }
    Ok(())
}

#[cfg(test)]
const DATE: &str = "'2026-10-01T00:00:00.000Z'";

#[cfg(test)]
pub(crate) const PLANNED_QUERIES: [(&str, &str, [&str; 4], &str); 3] = [
    (
        "multipart rows past expiry",
        repo::EXPIRY_PAGE,
        [DATE, "''", "''", "100"],
        "ix_s3mp_expiry",
    ),
    (
        "open multipart rows for reconciliation",
        repo::OPEN_PAGE,
        [DATE, "''", "''", "100"],
        "ix_s3mp_expiry",
    ),
    (
        "known provider identities of one listing page",
        repo::KNOWN_PAIRS,
        ["'bucket'", "'[\"objects/aa\"]'", "''", "''"],
        "ux_s3mp_",
    ),
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::orphan::OrphanTarget;
    use super::payload::{AbortJob, ReconcileJob};
    use super::scan::SENTINEL_PREFIX;
    use crate::domain::time::Timestamp;
    use crate::storage::key::{KeyNamespace, ObjectKey};

    const ABORT: &str = include_str!("reapers/abort.rs");
    const EXPIRE: &str = include_str!("reapers/expire.rs");
    const ORPHAN: &str = include_str!("reapers/orphan.rs");
    const PRUNE: &str = include_str!("reapers/prune.rs");
    const ROWS: &str = include_str!("reapers/rows.rs");
    const SCAN: &str = include_str!("reapers/scan.rs");
    const SCHEDULE: &str = include_str!("reapers/schedule.rs");

    fn production(source: &str) -> &str {
        source
            .split("\n#[cfg(test)]\nmod ")
            .next()
            .unwrap_or(source)
    }

    fn production_schedule() -> &'static str {
        production(SCHEDULE)
    }

    fn transaction_bodies(source: &str) -> Vec<&str> {
        let mut bodies = Vec::new();
        let mut rest = production(source);
        while let Some(start) = rest.find("async |tx|") {
            let after = &rest[start..];
            let Some(open) = after.find('{') else { break };
            let mut depth = 0_usize;
            let mut end = open;
            for (offset, character) in after[open..].char_indices() {
                match character {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            bodies.push(&after[open..=end]);
            rest = &after[end..];
        }
        bodies
    }

    #[test]
    fn unit_s3_reaper_payloads_have_a_closed_camel_case_shape() {
        let cases = [
            (json!({"mode": "sweep"}), AbortJob::Sweep {}),
            (
                json!({"mode": "expire", "after": ["2026-10-01T00:00:00.000Z", "id"], "continuation": 2}),
                AbortJob::Expire {
                    after: Some(("2026-10-01T00:00:00.000Z".to_owned(), "id".to_owned())),
                    continuation: 2,
                },
            ),
            (
                json!({"mode": "abort", "after": null, "continuation": 1}),
                AbortJob::Abort {
                    after: None,
                    continuation: 1,
                },
            ),
            (
                json!({"mode": "prune", "continuation": 3}),
                AbortJob::Prune { continuation: 3 },
            ),
            (
                json!({"mode": "multipart", "multipartId": "m"}),
                AbortJob::Multipart {
                    multipart_id: "m".to_owned(),
                },
            ),
            (
                json!({"mode": "orphan", "bucket": "b", "key": "k", "uploadId": "u", "initiatedAt": "t"}),
                AbortJob::Orphan {
                    bucket: "b".to_owned(),
                    key: "k".to_owned(),
                    upload_id: "u".to_owned(),
                    initiated_at: "t".to_owned(),
                },
            ),
        ];
        for (wire, job) in cases {
            assert_eq!(
                serde_json::from_value::<AbortJob>(wire.clone()).unwrap(),
                job
            );
            assert_eq!(serde_json::to_value(&job).unwrap(), wire);
        }
        for hostile in [
            json!({}),
            json!({"mode": "delete_everything"}),
            json!({"mode": "sweep", "extra": 1}),
            json!({"mode": "multipart", "multipartId": "m", "bucket": "b"}),
            json!({"mode": "orphan", "key": "k"}),
            json!({"mode": "orphan", "bucket": "b", "key": "k", "upload_id": "u", "initiatedAt": "t"}),
        ] {
            assert!(
                serde_json::from_value::<AbortJob>(hostile.clone()).is_err(),
                "{hostile}"
            );
        }
        assert_eq!(
            serde_json::from_value::<ReconcileJob>(json!({"mode": "sweep"})).unwrap(),
            ReconcileJob::Sweep {}
        );
        assert!(serde_json::from_value::<ReconcileJob>(json!({"mode": "orphan"})).is_err());
    }

    #[test]
    fn unit_orphan_target_accepts_only_server_shaped_identities() {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let at = "2026-10-01T00:00:00.000Z";
        let parsed = OrphanTarget::parse("bucket", key.as_str(), "upload", at).unwrap();
        assert_eq!(parsed.key, key);
        let branding = "branding/logo/0123456789abcdef0123456789abcdef";
        for (bucket, key, upload, initiated) in [
            ("bucket", branding, "upload", at),
            ("bucket", "objects/zz/zz/not-an-oid", "upload", at),
            ("bucket", "../../objects", "upload", at),
            ("", key.as_str(), "upload", at),
            ("bucket", key.as_str(), "", at),
            ("bucket", key.as_str(), "line\nbreak", at),
            ("bucket", key.as_str(), "tab\tbreak", at),
            ("bucket", key.as_str(), "ok", "yesterday"),
        ] {
            assert!(
                OrphanTarget::parse(bucket, key, upload, initiated).is_err(),
                "{bucket} {key} {upload:?} {initiated}"
            );
        }
        let long = "u".repeat(1_025);
        assert!(OrphanTarget::parse("bucket", key.as_str(), &long, at).is_err());
        assert!(OrphanTarget::parse("bucket", key.as_str(), &"u".repeat(1_024), at).is_ok());
    }

    #[test]
    fn unit_orphan_dedup_key_is_the_provider_identity() {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let at = "2026-10-01T00:00:00.000Z";
        let one = OrphanTarget::parse("bucket", key.as_str(), "upload-1", at).unwrap();
        let again = OrphanTarget::parse(
            "bucket",
            key.as_str(),
            "upload-1",
            "2026-11-01T00:00:00.000Z",
        )
        .unwrap();
        let other = OrphanTarget::parse("bucket", key.as_str(), "upload-2", at).unwrap();
        let foreign = OrphanTarget::parse("other", key.as_str(), "upload-1", at).unwrap();
        let huge = OrphanTarget::parse("bucket", key.as_str(), &"u".repeat(1_024), at).unwrap();
        assert_eq!(one.dedup_key().unwrap(), again.dedup_key().unwrap());
        assert_ne!(one.dedup_key().unwrap(), other.dedup_key().unwrap());
        assert_ne!(one.dedup_key().unwrap(), foreign.dedup_key().unwrap());
        assert!(huge.dedup_key().unwrap().as_str().len() <= 256);
    }

    #[test]
    fn unit_orphan_age_guard_is_inclusive_at_twenty_four_hours() {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let initiated = "2026-10-01T00:00:00.000Z";
        let target = OrphanTarget::parse("bucket", key.as_str(), "u", initiated).unwrap();
        let at = |text: &str| text.parse::<Timestamp>().unwrap();
        assert!(!target.old_enough(at("2026-10-01T23:59:59.999Z")).unwrap());
        assert!(target.old_enough(at("2026-10-02T00:00:00.000Z")).unwrap());
        assert!(target.old_enough(at("2026-10-02T00:00:00.001Z")).unwrap());
        assert!(!target.old_enough(at("2026-09-30T00:00:00.000Z")).unwrap());
    }

    #[test]
    fn unit_sentinel_keys_are_valid_objects_keys_that_sort_before_every_real_key() {
        let key = ObjectKey::parse(&format!("{SENTINEL_PREFIX}{}", "0".repeat(28))).unwrap();
        assert_eq!(key.namespace(), KeyNamespace::Objects);
        let real = ObjectKey::allocate(KeyNamespace::Objects);
        assert!(key.as_str() < real.as_str());
    }

    #[test]
    fn unit_s3_reapers_never_call_the_provider_inside_a_transaction() {
        let forbidden = [
            ".abort_multipart(",
            ".create_multipart(",
            ".list_parts(",
            ".list_multipart_uploads(",
            ".stat(",
            ".multipart()",
            "abort_at_provider",
            "provider_handle",
            "storage.",
        ];
        let mut inspected = 0;
        for (name, source) in [
            ("expire.rs", EXPIRE),
            ("abort.rs", ABORT),
            ("orphan.rs", ORPHAN),
            ("prune.rs", PRUNE),
            ("rows.rs", ROWS),
            ("scan.rs", SCAN),
            ("schedule.rs", SCHEDULE),
        ] {
            for body in transaction_bodies(source) {
                inspected += 1;
                for token in forbidden {
                    assert!(
                        !body.contains(token),
                        "{name} calls {token} inside a transaction"
                    );
                }
            }
        }
        assert!(
            inspected >= 7,
            "{inspected} transaction closures were inspected"
        );
    }

    #[test]
    fn unit_provider_abort_calls_are_confined_to_three_places() {
        let calls = |source: &str| production(source).matches(".abort_multipart(").count();
        assert_eq!(calls(SCAN), 1, "the enumeration probe");
        assert_eq!(calls(ORPHAN), 1, "the provider-only orphan");
        for (name, source) in [
            ("expire.rs", EXPIRE),
            ("rows.rs", ROWS),
            ("prune.rs", PRUNE),
            ("schedule.rs", SCHEDULE),
            ("abort.rs", ABORT),
        ] {
            assert_eq!(calls(source), 0, "{name}");
        }
        assert_eq!(
            production(ABORT).matches("abort_at_provider(").count(),
            1,
            "known rows reuse the M14-T07 abort"
        );
    }

    #[test]
    fn unit_startup_reconciliation_has_no_path_to_the_storage_provider() {
        let reconcile = include_str!("../../../app/reconcile.rs");
        let production = production(reconcile);
        for token in [
            "abort_multipart",
            "TransferStorage",
            "StorageProvider",
            "MultipartStorage",
            "multipart()",
        ] {
            assert!(
                !production.contains(token),
                "startup reconciliation names {token}"
            );
        }
        let (_, step) = production
            .split_once("fn s3_reapers_step")
            .expect("the S3 scheduling step");
        let step = step.split("\n}\n").next().unwrap();
        assert!(step.contains("schedule_startup"));
        let schedule = production_schedule();
        for token in ["abort_multipart", "multipart()", "provider"] {
            assert!(!schedule.contains(token), "{token}");
        }
    }

    #[test]
    fn unit_startup_reconciles_only_after_the_exclusive_instance_lock() {
        let lifecycle = production(include_str!("../../../app/lifecycle.rs"));
        let initialize = lifecycle
            .split("async fn initialize(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("initialize");
        let lock = initialize.find("InstanceLock::acquire(").expect("the lock");
        let reconcile = initialize
            .find("ReconcileRegistry::production()")
            .expect("reconciliation");
        let jobs = initialize.find("start_jobs(").expect("the job runtime");
        assert!(
            lock < reconcile && reconcile < jobs,
            "{lock} {reconcile} {jobs}"
        );
    }

    #[test]
    fn unit_every_reaper_select_is_bounded() {
        for source in [EXPIRE, ABORT, ROWS, SCAN, PRUNE] {
            for statement in production(source)
                .split("\";\n")
                .filter(|part| part.contains("SELECT"))
            {
                assert!(statement.contains("LIMIT"), "{statement}");
            }
        }
        let repo = production(include_str!("reapers/repo.rs"));
        for (name, query) in [
            ("EXPIRY_PAGE", "EXPIRY_PAGE"),
            ("OPEN_PAGE", "OPEN_PAGE"),
            ("PRUNE_TERMINAL", "PRUNE_TERMINAL"),
        ] {
            let (_, statement) = repo.split_once(query).expect(name);
            assert!(
                statement.split("\";").next().unwrap().contains("LIMIT"),
                "{name}"
            );
        }
    }
}
