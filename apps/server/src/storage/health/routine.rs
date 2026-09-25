use std::future::Future;
use std::io;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use time::OffsetDateTime;

use super::pattern::{compare, Comparison, ProbePattern};
use super::probe_key::ProbeKey;
use super::report::{
    CheckName, CheckScope, CheckStatus, Diagnosis, FactReport, ProbeDepth, SelfTestReport, SubCheck,
};
use crate::domain::clock::Clock;
use crate::storage::error::StorageError;
use crate::storage::provider::{ObjectBody, ObjectStat};
use crate::storage::ProviderKind;

pub const PROBE_BYTES: u64 = 4 * 1024;
pub const RANGE_START: u64 = 1024;
pub const RANGE_LEN: u64 = 512;
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
pub const STALE_PROBE_AGE: Duration = Duration::from_secs(60 * 60);
pub const CLEANUP_PAGE: u32 = 100;

#[derive(Debug, Clone)]
pub(in crate::storage) struct ListedProbe {
    pub key: ProbeKey,
    pub modified_at: OffsetDateTime,
}

#[async_trait]
pub(in crate::storage) trait ProbeStore: Send + Sync {
    async fn put_probe(
        &self,
        key: &ProbeKey,
        body: ObjectBody,
        len: u64,
    ) -> Result<ObjectStat, StorageError>;

    async fn stat_probe(&self, key: &ProbeKey) -> Result<ObjectStat, StorageError>;

    async fn read_probe(&self, key: &ProbeKey) -> Result<ObjectBody, StorageError>;

    async fn read_probe_range(
        &self,
        key: &ProbeKey,
        start: u64,
        len: u64,
    ) -> Result<ObjectBody, StorageError>;

    async fn delete_probe(&self, key: &ProbeKey) -> Result<(), StorageError>;

    async fn probe_exists(&self, key: &ProbeKey) -> Result<bool, StorageError>;

    async fn list_probes(&self, limit: u32) -> Result<Vec<ListedProbe>, StorageError>;

    fn diagnose(&self, error: &StorageError) -> Diagnosis {
        diagnose(error)
    }
}

pub(in crate::storage) fn diagnose(error: &StorageError) -> Diagnosis {
    match error {
        StorageError::PermissionDenied => Diagnosis::PermissionDenied,
        StorageError::QuotaOnDevice => Diagnosis::StorageFull,
        StorageError::ProviderUnavailable(_) => Diagnosis::Unreachable,
        StorageError::Config(_) => Diagnosis::Config,
        StorageError::ProviderMismatch { .. } => Diagnosis::ProviderMismatch,
        StorageError::SizeMismatch { .. } => Diagnosis::SizeMismatch,
        StorageError::RangeNotSatisfiable { .. } => Diagnosis::RangeMismatch,
        StorageError::Io(source) if source.kind() == io::ErrorKind::ReadOnlyFilesystem => {
            Diagnosis::NotWritable
        }
        StorageError::Io(source) if source.kind() == io::ErrorKind::TimedOut => {
            Diagnosis::Unreachable
        }
        StorageError::NotFound
        | StorageError::AlreadyExists
        | StorageError::InvalidKey
        | StorageError::Io(_)
        | StorageError::S3(_) => Diagnosis::ProviderError,
    }
}

pub(in crate::storage) struct ProbeRun<'c> {
    clock: &'c dyn Clock,
    provider: ProviderKind,
    depth: ProbeDepth,
    ran_at: OffsetDateTime,
    started: Instant,
    checks: Vec<SubCheck>,
    facts: Vec<FactReport>,
}

impl<'c> ProbeRun<'c> {
    pub(in crate::storage) fn new(
        clock: &'c dyn Clock,
        provider: ProviderKind,
        depth: ProbeDepth,
    ) -> Self {
        Self {
            clock,
            provider,
            depth,
            ran_at: clock.now(),
            started: clock.monotonic(),
            checks: Vec::new(),
            facts: Vec::new(),
        }
    }

    pub(in crate::storage) fn core_intact(&self) -> bool {
        !self.checks.iter().any(|check| {
            check.name.scope() == CheckScope::Core && check.status == CheckStatus::Failed
        })
    }

    pub(in crate::storage) fn started(&self) -> Instant {
        self.clock.monotonic()
    }

    pub(in crate::storage) fn elapsed(&self, since: Instant) -> Duration {
        self.clock.monotonic().saturating_duration_since(since)
    }

    pub(in crate::storage) async fn run<T, F>(&mut self, name: CheckName, work: F) -> Option<T>
    where
        F: Future<Output = Result<T, Diagnosis>>,
    {
        if !self.core_intact() {
            self.skip(name);
            return None;
        }
        self.attempt(name, work).await
    }

    pub(in crate::storage) async fn attempt<T, F>(&mut self, name: CheckName, work: F) -> Option<T>
    where
        F: Future<Output = Result<T, Diagnosis>>,
    {
        let started = self.started();
        let outcome = tokio::time::timeout(CHECK_TIMEOUT, work)
            .await
            .unwrap_or(Err(Diagnosis::Unreachable));
        let duration = self.elapsed(started);
        match outcome {
            Ok(value) => {
                self.record(name, CheckStatus::Passed, None, duration);
                Some(value)
            }
            Err(diagnosis) => {
                self.record(name, CheckStatus::Failed, Some(diagnosis), duration);
                None
            }
        }
    }

    pub(in crate::storage) fn record(
        &mut self,
        name: CheckName,
        status: CheckStatus,
        diagnosis: Option<Diagnosis>,
        duration: Duration,
    ) {
        self.checks.push(SubCheck {
            name,
            status,
            duration,
            diagnosis,
        });
    }

    pub(in crate::storage) fn skip(&mut self, name: CheckName) {
        self.record(name, CheckStatus::Skipped, None, Duration::ZERO);
    }

    pub(in crate::storage) fn fact(&mut self, fact: FactReport) {
        self.facts.push(fact);
    }

    pub(in crate::storage) fn finish(self) -> SelfTestReport {
        let duration = self.elapsed(self.started);
        SelfTestReport {
            ran_at: self.ran_at,
            depth: self.depth,
            provider: self.provider,
            duration,
            checks: self.checks,
            facts: self.facts,
        }
    }
}

pub(in crate::storage) async fn write_and_verify<S>(
    run: &mut ProbeRun<'_>,
    store: &S,
    key: &ProbeKey,
) -> bool
where
    S: ProbeStore + ?Sized,
{
    let seed = key.seed();
    let written = run
        .run(CheckName::Write, async {
            let body = ProbePattern::new(seed, 0, PROBE_BYTES).body();
            let stat = store
                .put_probe(key, body, PROBE_BYTES)
                .await
                .map_err(|error| store.diagnose(&error))?;
            expect_size(&stat)
        })
        .await
        .is_some();
    if !written {
        for name in [CheckName::Stat, CheckName::Read, CheckName::Range] {
            run.skip(name);
        }
        return false;
    }
    run.run(CheckName::Stat, async {
        let stat = store
            .stat_probe(key)
            .await
            .map_err(|error| store.diagnose(&error))?;
        expect_size(&stat)
    })
    .await;
    run.run(CheckName::Read, async {
        let body = store
            .read_probe(key)
            .await
            .map_err(|error| store.diagnose(&error))?;
        verify(
            store,
            body,
            seed,
            0,
            PROBE_BYTES,
            Diagnosis::ContentMismatch,
        )
        .await
    })
    .await;
    run.run(CheckName::Range, async {
        let body = store
            .read_probe_range(key, RANGE_START, RANGE_LEN)
            .await
            .map_err(|error| store.diagnose(&error))?;
        verify(
            store,
            body,
            seed,
            RANGE_START,
            RANGE_LEN,
            Diagnosis::RangeMismatch,
        )
        .await
    })
    .await;
    true
}

pub(in crate::storage) fn skip_removal(run: &mut ProbeRun<'_>) {
    run.skip(CheckName::Delete);
    run.skip(CheckName::Absent);
}

pub(in crate::storage) async fn remove_and_verify<S>(
    run: &mut ProbeRun<'_>,
    store: &S,
    key: &ProbeKey,
) where
    S: ProbeStore + ?Sized,
{
    let deleted = run
        .attempt(CheckName::Delete, async {
            store
                .delete_probe(key)
                .await
                .map_err(|error| store.diagnose(&error))
        })
        .await;
    if deleted.is_none() {
        run.skip(CheckName::Absent);
        return;
    }
    run.attempt(CheckName::Absent, async {
        match store.probe_exists(key).await {
            Ok(false) => Ok(()),
            Ok(true) => Err(Diagnosis::DeleteIneffective),
            Err(error) => Err(store.diagnose(&error)),
        }
    })
    .await;
}

pub(in crate::storage) async fn sweep_stale<S>(
    run: &mut ProbeRun<'_>,
    store: &S,
    in_use: &[&ProbeKey],
) where
    S: ProbeStore + ?Sized,
{
    if !run.core_intact() {
        run.skip(CheckName::Cleanup);
        return;
    }
    let started = run.started();
    let cutoff = run.ran_at - STALE_PROBE_AGE;
    let swept = tokio::time::timeout(CHECK_TIMEOUT, async {
        let listed = store.list_probes(CLEANUP_PAGE).await?;
        let mut removed = 0_usize;
        for probe in listed {
            if probe.modified_at < cutoff && !in_use.contains(&&probe.key) {
                store.delete_probe(&probe.key).await?;
                removed += 1;
            }
        }
        Ok::<usize, StorageError>(removed)
    })
    .await;
    let duration = run.elapsed(started);
    match swept {
        Ok(Ok(removed)) => {
            if removed > 0 {
                tracing::info!(
                    removed,
                    "stale storage self-test probes from an interrupted run were removed"
                );
            }
            run.record(CheckName::Cleanup, CheckStatus::Passed, None, duration);
        }
        Ok(Err(_)) | Err(_) => run.record(
            CheckName::Cleanup,
            CheckStatus::Warning,
            Some(Diagnosis::CleanupFailed),
            duration,
        ),
    }
}

fn expect_size(stat: &ObjectStat) -> Result<(), Diagnosis> {
    if stat.size == PROBE_BYTES {
        Ok(())
    } else {
        Err(Diagnosis::SizeMismatch)
    }
}

async fn verify<S>(
    store: &S,
    body: ObjectBody,
    seed: u64,
    start: u64,
    len: u64,
    mismatch: Diagnosis,
) -> Result<(), Diagnosis>
where
    S: ProbeStore + ?Sized,
{
    match compare(body, seed, start, len).await {
        Ok(Comparison::Equal) => Ok(()),
        Ok(Comparison::Different) => Err(mismatch),
        Err(error) => Err(store.diagnose(&StorageError::from(error))),
    }
}
