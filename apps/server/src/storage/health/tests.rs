use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};
use tempfile::TempDir;
use time::macros::datetime;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use super::consistency::{self, ConsistencyOutcome, SampledRow, SAMPLE_LIMIT};
use super::machine::{HealthMachine, DOWN_AFTER_FAILURES, RECOVER_AFTER_SUCCESSES};
use super::pattern::{byte_at, compare, Comparison, ProbePattern};
use super::probe_key::ProbeKey;
use super::{
    CheckName, CheckStatus, Diagnosis, FailureClass, HealthSignal, ProbeDepth, Schedule,
    SelfTestReport, SelfTestResult, StorageHealth, StorageMonitor, SubCheck, HEALTH_CHECK_PERIOD,
    PROBE_PREFIX,
};
use crate::domain::clock::{Clock, TestClock};
use crate::infra::db::migrate_tests::open_pools;
use crate::infra::db::{DbError, DbPools, MIGRATOR};
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::{Retryable, StorageError};
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{
    ListCursor, ListPage, ObjectBody, ObjectStat, PutHint, StorageDescriptor, StorageProvider,
};
use crate::storage::ProviderKind;

const NOW: OffsetDateTime = datetime!(2026-09-25 12:00 UTC);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Outcome {
    core: Option<Diagnosis>,
    capability: Option<Diagnosis>,
}

const PASS: Outcome = Outcome {
    core: None,
    capability: None,
};

const fn fail(diagnosis: Diagnosis) -> Outcome {
    Outcome {
        core: Some(diagnosis),
        capability: None,
    }
}

const fn degraded(diagnosis: Diagnosis) -> Outcome {
    Outcome {
        core: None,
        capability: Some(diagnosis),
    }
}

fn report(depth: ProbeDepth, outcome: Outcome) -> SelfTestReport {
    let check = |name, diagnosis: Option<Diagnosis>| SubCheck {
        name,
        status: if diagnosis.is_some() {
            CheckStatus::Failed
        } else {
            CheckStatus::Passed
        },
        duration: Duration::ZERO,
        diagnosis,
    };
    let mut checks = vec![check(CheckName::Write, outcome.core)];
    if depth == ProbeDepth::Full && outcome.core.is_none() {
        checks.push(check(CheckName::CorsPreflight, outcome.capability));
    }
    SelfTestReport {
        ran_at: NOW,
        depth,
        provider: ProviderKind::S3,
        duration: Duration::ZERO,
        checks,
        facts: Vec::new(),
    }
}

struct Scripted {
    kind: ProviderKind,
    outcomes: Mutex<VecDeque<Outcome>>,
    calls: Mutex<Vec<ProbeDepth>>,
    exists: Mutex<VecDeque<Result<bool, StorageError>>>,
    exists_calls: Mutex<usize>,
}

impl Scripted {
    fn new(outcomes: &[Outcome]) -> Arc<Self> {
        Self::of_kind(ProviderKind::S3, outcomes)
    }

    fn of_kind(kind: ProviderKind, outcomes: &[Outcome]) -> Arc<Self> {
        Arc::new(Self {
            kind,
            outcomes: Mutex::new(outcomes.iter().copied().collect()),
            calls: Mutex::new(Vec::new()),
            exists: Mutex::new(VecDeque::new()),
            exists_calls: Mutex::new(0),
        })
    }

    fn with_exists(kind: ProviderKind, exists: Vec<Result<bool, StorageError>>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            outcomes: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            exists: Mutex::new(exists.into_iter().collect()),
            exists_calls: Mutex::new(0),
        })
    }

    fn push(&self, outcomes: &[Outcome]) {
        self.outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(outcomes);
    }

    fn take_calls(&self) -> Vec<ProbeDepth> {
        std::mem::take(&mut *self.calls.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn exists_calls(&self) -> usize {
        *self
            .exists_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl StorageProvider for Scripted {
    fn caps(&self) -> &StorageCapabilities {
        &StorageCapabilities::S3_DEFAULT
    }

    fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: self.kind,
            local: None,
        }
    }

    async fn put_stream(
        &self,
        _key: &ObjectKey,
        _body: ObjectBody,
        _hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        panic!("health code must never write product objects")
    }

    async fn open_read(&self, _key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        panic!("health code must never read product objects")
    }

    async fn open_range(
        &self,
        _key: &ObjectKey,
        _start: u64,
        _len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        panic!("health code must never read product objects")
    }

    async fn stat(&self, _key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        panic!("the consistency probe uses exists only")
    }

    async fn delete(&self, _key: &ObjectKey) -> Result<bool, StorageError> {
        panic!("health code must never delete product objects")
    }

    async fn exists(&self, _key: &ObjectKey) -> Result<bool, StorageError> {
        *self
            .exists_calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        self.exists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Ok(true))
    }

    async fn copy(&self, _src: &ObjectKey, _dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        panic!("health code must never copy product objects")
    }

    async fn list_page(
        &self,
        _prefix: &str,
        _cursor: Option<ListCursor>,
        _page_size: u32,
    ) -> Result<ListPage, StorageError> {
        panic!("health code must never list product objects")
    }

    async fn self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(depth);
        let outcome = self
            .outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(PASS);
        report(depth, outcome)
    }
}

#[derive(Clone, Default)]
struct Signals(Arc<Mutex<Vec<HealthSignal>>>);

impl Signals {
    fn last(&self) -> Option<HealthSignal> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .copied()
    }
}

fn supervised(provider: &Arc<Scripted>) -> (StorageMonitor, Signals) {
    let signals = Signals::default();
    let sink = signals.clone();
    let provider: Arc<dyn StorageProvider> = Arc::clone(provider) as Arc<dyn StorageProvider>;
    let monitor = StorageMonitor::new(
        provider,
        Arc::new(TestClock::new(NOW)),
        Arc::new(move |signal| {
            sink.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(signal);
        }),
    );
    (monitor, signals)
}

fn unavailable() -> StorageError {
    StorageError::ProviderUnavailable(Retryable::new(std::io::Error::from(
        std::io::ErrorKind::ConnectionRefused,
    )))
}

#[test]
fn unit_storage_health_vocabulary() {
    let names = [
        StorageHealth::Ok,
        StorageHealth::Degraded,
        StorageHealth::Down,
    ]
    .map(StorageHealth::as_str);
    assert_eq!(names, ["ok", "degraded", "down"]);
    assert_eq!((DOWN_AFTER_FAILURES, RECOVER_AFTER_SUCCESSES), (3, 2));
    assert_eq!(HEALTH_CHECK_PERIOD, Duration::from_secs(60));
    for diagnosis in [
        Diagnosis::PermissionDenied,
        Diagnosis::NotWritable,
        Diagnosis::Config,
        Diagnosis::ProviderMismatch,
    ] {
        assert_eq!(
            diagnosis.class(),
            FailureClass::NonTransient,
            "{diagnosis:?}"
        );
    }
    for diagnosis in [
        Diagnosis::Unreachable,
        Diagnosis::BucketMissing,
        Diagnosis::ClockSkew,
        Diagnosis::StorageFull,
    ] {
        assert_eq!(diagnosis.class(), FailureClass::Transient, "{diagnosis:?}");
    }
}

#[test]
fn unit_health_machine_transitions() {
    let mut machine = HealthMachine::starting(None);
    assert_eq!(machine.state(), StorageHealth::Ok);
    let degrade = machine.record_failure(Diagnosis::Unreachable).unwrap();
    assert_eq!(
        (degrade.from, degrade.to),
        (StorageHealth::Ok, StorageHealth::Degraded)
    );
    assert!(machine.record_failure(Diagnosis::Unreachable).is_none());
    assert!(machine.failure_would_classify_down());
    let down = machine.record_failure(Diagnosis::Unreachable).unwrap();
    assert_eq!(down.to, StorageHealth::Down);
    assert!(!machine.failure_would_classify_down());
    assert!(machine.record_success().is_none());
    assert_eq!(machine.consecutive_successes(), 1);
    assert!(machine.record_failure(Diagnosis::Unreachable).is_none());
    assert_eq!(machine.consecutive_successes(), 0);
    assert!(machine.record_success().is_none());
    let up = machine.record_success().unwrap();
    assert_eq!(
        (up.from, up.to, up.cause),
        (StorageHealth::Down, StorageHealth::Ok, None)
    );
    assert_eq!(machine.consecutive_failures(), 0);

    let mut machine = HealthMachine::starting(None);
    let direct = machine.record_failure(Diagnosis::PermissionDenied).unwrap();
    assert_eq!(
        (direct.from, direct.to),
        (StorageHealth::Ok, StorageHealth::Down)
    );

    let booted_down = HealthMachine::starting(Some(Diagnosis::Unreachable));
    assert_eq!(booted_down.state(), StorageHealth::Down);
    assert_eq!(booted_down.cause(), Some(Diagnosis::Unreachable));
}

#[tokio::test]
async fn it_storage_health_hysteresis() {
    let provider = Scripted::new(&[PASS]);
    let (monitor, signals) = supervised(&provider);
    let health = |monitor: &StorageMonitor| monitor.status().snapshot().health;

    monitor.startup(None).await;
    assert_eq!(provider.take_calls(), [ProbeDepth::Full]);
    assert_eq!(health(&monitor), StorageHealth::Ok);
    assert_eq!(signals.last().unwrap().health, StorageHealth::Ok);

    provider.push(&[fail(Diagnosis::Unreachable)]);
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Degraded);
    assert!(!signals.last().unwrap().unreachable);

    provider.push(&[PASS]);
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Ok);
    assert_eq!(monitor.status().snapshot().consecutive_failures, 0);
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light, ProbeDepth::Light]
    );

    provider.push(&[
        fail(Diagnosis::Unreachable),
        fail(Diagnosis::Unreachable),
        fail(Diagnosis::Unreachable),
        fail(Diagnosis::Unreachable),
    ]);
    monitor.check().await;
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Degraded);
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light, ProbeDepth::Light]
    );
    monitor.check().await;
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light, ProbeDepth::Full],
        "the third consecutive failure is classified by one full self-test"
    );
    let snapshot = monitor.status().snapshot();
    assert_eq!(snapshot.health, StorageHealth::Down);
    assert_eq!(snapshot.cause, Some(Diagnosis::Unreachable));
    assert!(signals.last().unwrap().unreachable);

    provider.push(&[PASS]);
    monitor.check().await;
    assert_eq!(
        health(&monitor),
        StorageHealth::Down,
        "one success does not recover"
    );
    provider.push(&[fail(Diagnosis::Unreachable), PASS]);
    monitor.check().await;
    monitor.check().await;
    assert_eq!(
        health(&monitor),
        StorageHealth::Down,
        "a failure resets the recovery streak"
    );
    provider.push(&[PASS]);
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Ok);
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light; 4],
        "recovery never runs a full self-test once capabilities are known"
    );

    provider.push(&[fail(Diagnosis::Unreachable), PASS]);
    monitor.check().await;
    assert_eq!(
        health(&monitor),
        StorageHealth::Degraded,
        "the failure counter was reset by the recovery"
    );
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Ok);

    provider.push(&[fail(Diagnosis::PermissionDenied)]);
    monitor.check().await;
    let snapshot = monitor.status().snapshot();
    assert_eq!(snapshot.health, StorageHealth::Down);
    assert_eq!(snapshot.cause, Some(Diagnosis::PermissionDenied));
    assert!(!signals.last().unwrap().unreachable);
    assert_eq!(provider.take_calls().last(), Some(&ProbeDepth::Light));

    provider.push(&[PASS, PASS]);
    monitor.check().await;
    monitor.check().await;
    assert_eq!(health(&monitor), StorageHealth::Ok);

    provider.push(&[
        fail(Diagnosis::Unreachable),
        fail(Diagnosis::Unreachable),
        fail(Diagnosis::Unreachable),
        PASS,
    ]);
    provider.take_calls();
    for _ in 0..3 {
        monitor.check().await;
    }
    assert_eq!(
        provider.take_calls(),
        [
            ProbeDepth::Light,
            ProbeDepth::Light,
            ProbeDepth::Light,
            ProbeDepth::Full
        ]
    );
    assert_eq!(
        health(&monitor),
        StorageHealth::Ok,
        "a passing classification is a success, not an outage"
    );
}

#[tokio::test]
async fn it_startup_core_outage_initializes_down() {
    let provider = Scripted::new(&[fail(Diagnosis::Unreachable)]);
    let (monitor, signals) = supervised(&provider);
    let report = monitor.startup(None).await;

    assert_eq!(report.result(), SelfTestResult::Failed);
    assert_eq!(monitor.status().snapshot().health, StorageHealth::Down);
    assert_eq!(
        signals.last(),
        Some(HealthSignal {
            health: StorageHealth::Down,
            unreachable: true
        })
    );
    provider.take_calls();

    monitor.check().await;
    assert_eq!(monitor.status().snapshot().health, StorageHealth::Down);
    monitor.check().await;
    assert_eq!(monitor.status().snapshot().health, StorageHealth::Ok);
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light, ProbeDepth::Light, ProbeDepth::Full],
        "capabilities never verified at boot are verified once on recovery"
    );
    monitor.check().await;
    assert_eq!(provider.take_calls(), [ProbeDepth::Light]);

    let provider = Scripted::new(&[fail(Diagnosis::PermissionDenied)]);
    let (monitor, signals) = supervised(&provider);
    monitor.startup(None).await;
    assert_eq!(
        signals.last(),
        Some(HealthSignal {
            health: StorageHealth::Down,
            unreachable: false
        })
    );
}

#[tokio::test]
async fn it_full_test_degradation_is_not_cleared_by_light_success() {
    let provider = Scripted::new(&[degraded(Diagnosis::CorsMissingEtag)]);
    let (monitor, signals) = supervised(&provider);
    let report = monitor.startup(None).await;
    assert_eq!(report.result(), SelfTestResult::Degraded);
    let snapshot = monitor.status().snapshot();
    assert_eq!(snapshot.health, StorageHealth::Degraded);
    assert_eq!(snapshot.core, StorageHealth::Ok);
    assert_eq!(snapshot.degradations, [Diagnosis::CorsMissingEtag]);

    for _ in 0..5 {
        monitor.check().await;
    }
    let snapshot = monitor.status().snapshot();
    assert_eq!(snapshot.health, StorageHealth::Degraded);
    assert_eq!(snapshot.degradations, [Diagnosis::CorsMissingEtag]);
    assert_eq!(signals.last().unwrap().health, StorageHealth::Degraded);

    provider.push(&[fail(Diagnosis::Unreachable)]);
    monitor.verify().await;
    assert_eq!(
        monitor.status().snapshot().degradations,
        [Diagnosis::CorsMissingEtag],
        "a full test that never reached the capability checks keeps the diagnosis"
    );
    monitor.check().await;

    provider.push(&[PASS]);
    let verified = monitor.verify().await;
    assert_eq!(verified.result(), SelfTestResult::Passed);
    let snapshot = monitor.status().snapshot();
    assert_eq!(snapshot.health, StorageHealth::Ok);
    assert!(snapshot.degradations.is_empty());
}

#[tokio::test(start_paused = true)]
async fn it_health_monitor_interval_and_cancellation() {
    let provider = Scripted::new(&[]);
    let (monitor, _) = supervised(&provider);
    monitor.startup(None).await;
    provider.take_calls();
    let monitor = Arc::new(monitor);
    let cancel = CancellationToken::new();
    let task = monitor.spawn(Schedule::Every(HEALTH_CHECK_PERIOD), cancel.clone());
    let settle = || async {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    };

    tokio::time::sleep(Duration::from_secs(59)).await;
    settle().await;
    assert!(provider.take_calls().is_empty());
    tokio::time::sleep(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(provider.take_calls(), [ProbeDepth::Light]);
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(provider.take_calls(), [ProbeDepth::Light]);

    tokio::time::advance(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(
        provider.take_calls(),
        [ProbeDepth::Light],
        "a stalled process runs one probe, not a burst of missed ones"
    );

    cancel.cancel();
    task.await.unwrap();
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert!(provider.take_calls().is_empty());
    assert_eq!(Arc::strong_count(&monitor), 1);
}

#[test]
fn unit_probe_key_is_outside_object_key_grammar() {
    let key = ProbeKey::generate();
    assert!(key.as_str().starts_with(PROBE_PREFIX));
    assert_eq!(key.oid().len(), 32);
    assert!(ObjectKey::parse(key.as_str()).is_err());
    assert!(!key.as_str().starts_with("objects/") && !key.as_str().starts_with("branding/"));
    assert_ne!(ProbeKey::generate(), key);
    assert_eq!(ProbeKey::from_listed(key.as_str()), Some(key.clone()));
    assert_eq!(ProbeKey::from_listed_name(key.oid()), Some(key.clone()));
    assert!(!format!("{key:?}").contains(key.oid()));

    let objects = ObjectKey::allocate(KeyNamespace::Objects);
    for hostile in [
        objects.as_str().to_owned(),
        format!("{PROBE_PREFIX}../../objects/ab/cd/{}", key.oid()),
        format!("{PROBE_PREFIX}{}", key.oid().to_uppercase()),
        format!("{PROBE_PREFIX}{}/x", key.oid()),
        format!("{PROBE_PREFIX}0192f3c8d7e94a1b8f0c2d5e6a7b8c9d"),
        PROBE_PREFIX.to_owned(),
        format!("/{}", key.as_str()),
        format!("_palmr/probe//{}", key.oid()),
        format!("_PALMR/probe/{}", key.oid()),
        "objects/_palmr/probe".to_owned(),
    ] {
        assert_eq!(ProbeKey::from_listed(&hostile), None, "{hostile}");
    }
    for name in ["..", ".tmp-x", "notes.txt", "", &key.oid()[1..]] {
        assert_eq!(ProbeKey::from_listed_name(name), None, "{name}");
    }
}

#[test]
fn unit_probe_key_has_no_public_constructor() {
    let source = include_str!("probe_key.rs");
    for line in source.lines().map(str::trim) {
        assert!(
            !line.starts_with("pub fn") && !line.starts_with("pub(crate)"),
            "{line}"
        );
        assert!(!line.starts_with("impl From"), "{line}");
        assert!(!line.contains("FromStr"), "{line}");
        assert!(!line.contains("Deserialize"), "{line}");
    }
    assert!(source.contains("pub(in crate::storage) struct ProbeKey {\n    text: String,\n}"));
    let key_source = include_str!("../key.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(!key_source.contains("_palmr"));
    assert!(!key_source.contains("from_unchecked"));
}

#[test]
fn unit_health_code_never_mutates_the_catalog() {
    for source in [
        include_str!("routine.rs"),
        include_str!("monitor.rs"),
        include_str!("consistency.rs"),
        include_str!("../local/probe.rs"),
        include_str!("../s3/selftest.rs"),
    ] {
        let upper = source.to_uppercase();
        for statement in [
            "INSERT ",
            "UPDATE ",
            "DELETE FROM",
            "REPLACE INTO",
            "WRITE_TX",
        ] {
            assert!(!upper.contains(statement), "{statement}");
        }
        assert!(
            !source.contains(".delete(&"),
            "health code deletes only probes"
        );
        assert!(!source.contains("abort_multipart("));
        assert!(!source.contains("list_multipart_uploads(\""));
    }
}

#[tokio::test]
async fn unit_probe_pattern_is_deterministic() {
    let key = ProbeKey::generate();
    let seed = key.seed();
    assert_eq!(byte_at(seed, 7), byte_at(seed, 7));
    let distinct = (0..256)
        .map(|index| byte_at(seed, index))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(distinct.len() > 100);
    let whole = ProbePattern::new(seed, 0, 4096).body();
    assert_eq!(
        compare(whole, seed, 0, 4096).await.unwrap(),
        Comparison::Equal
    );
    let slice = ProbePattern::new(seed, 1024, 512).body();
    assert_eq!(
        compare(slice, seed, 1024, 512).await.unwrap(),
        Comparison::Equal
    );
    let shifted = ProbePattern::new(seed, 1, 512).body();
    assert_eq!(
        compare(shifted, seed, 0, 512).await.unwrap(),
        Comparison::Different
    );
    let short = ProbePattern::new(seed, 0, 4095).body();
    assert_eq!(
        compare(short, seed, 0, 4096).await.unwrap(),
        Comparison::Different
    );
    let long = ProbePattern::new(seed, 0, 4097).body();
    assert_eq!(
        compare(long, seed, 0, 4096).await.unwrap(),
        Comparison::Different
    );
}

fn rows(count: usize, provider: &str) -> Vec<SampledRow> {
    (0..count)
        .map(|index| SampledRow {
            id: format!("row-{index}"),
            object_key: ObjectKey::allocate(KeyNamespace::Objects)
                .as_str()
                .to_owned(),
            provider: provider.to_owned(),
        })
        .collect()
}

fn existence(present: usize, missing: usize) -> Vec<Result<bool, StorageError>> {
    std::iter::repeat_with(|| Ok(true))
        .take(present)
        .chain(std::iter::repeat_with(|| Ok(false)).take(missing))
        .collect()
}

#[tokio::test]
async fn unit_consistency_probe_classification() {
    let replaced = Scripted::with_exists(ProviderKind::S3, existence(39, 11));
    let report = consistency::probe(replaced.as_ref(), &rows(50, "s3")).await;
    assert_eq!(report.outcome, ConsistencyOutcome::StorageReplaced);
    assert_eq!((report.present, report.missing), (39, 11));
    assert_eq!(
        report.outcome.degradation(),
        Some(Diagnosis::StorageReplaced)
    );

    let boundary = Scripted::with_exists(ProviderKind::S3, existence(40, 10));
    let report = consistency::probe(boundary.as_ref(), &rows(50, "s3")).await;
    assert_eq!(report.outcome, ConsistencyOutcome::Consistent);

    let bounded = Scripted::with_exists(ProviderKind::S3, Vec::new());
    let report = consistency::probe(bounded.as_ref(), &rows(80, "s3")).await;
    assert_eq!(bounded.exists_calls(), SAMPLE_LIMIT);
    assert_eq!(report.sampled, SAMPLE_LIMIT);

    let mut outage = existence(3, 0);
    outage.push(Err(unavailable()));
    let outage = Scripted::with_exists(ProviderKind::S3, outage);
    let report = consistency::probe(outage.as_ref(), &rows(10, "s3")).await;
    assert_eq!(
        report.outcome,
        ConsistencyOutcome::Inconclusive(Diagnosis::Unreachable)
    );
    assert_eq!((report.present, report.missing, report.unprobed), (3, 0, 7));
    assert_eq!(
        outage.exists_calls(),
        4,
        "probing stops at the first outage"
    );
    assert_eq!(report.outcome.degradation(), None);

    let denied = Scripted::with_exists(ProviderKind::S3, vec![Err(StorageError::PermissionDenied)]);
    let report = consistency::probe(denied.as_ref(), &rows(10, "s3")).await;
    assert_eq!(
        report.outcome,
        ConsistencyOutcome::Inconclusive(Diagnosis::PermissionDenied)
    );
    assert_eq!(report.missing, 0);

    let not_found = Scripted::with_exists(
        ProviderKind::S3,
        std::iter::repeat_with(|| Err(StorageError::NotFound))
            .take(10)
            .collect(),
    );
    let report = consistency::probe(not_found.as_ref(), &rows(10, "s3")).await;
    assert_eq!(report.outcome, ConsistencyOutcome::StorageReplaced);
    assert_eq!(report.missing, 10);

    let mut mixed = rows(9, "s3");
    mixed.extend(rows(1, "local"));
    let mismatch = Scripted::with_exists(ProviderKind::S3, Vec::new());
    let report = consistency::probe(mismatch.as_ref(), &mixed).await;
    assert_eq!(report.outcome, ConsistencyOutcome::ProviderMismatch);
    assert_eq!(report.mismatched, 1);
    assert_eq!(mismatch.exists_calls(), 9);
    assert_eq!(
        report.outcome.degradation(),
        Some(Diagnosis::ProviderMismatch)
    );

    let empty = Scripted::with_exists(ProviderKind::S3, Vec::new());
    let report = consistency::probe(empty.as_ref(), &[]).await;
    assert_eq!(report.outcome, ConsistencyOutcome::Empty);
}

fn spread_key(rng: &mut StdRng) -> ObjectKey {
    let oid: String = (0..16)
        .map(|_| format!("{:02x}", rng.random::<u8>()))
        .collect();
    ObjectKey::parse(&format!("objects/{}/{}/{oid}", &oid[..2], &oid[2..4])).unwrap()
}

async fn seed_catalog(pools: &DbPools, active: usize, tombstoned: usize) {
    let clock = TestClock::new(NOW);
    let mut rng = StdRng::seed_from_u64(0x5eed);
    pools
        .write_tx(&clock, "test.seed_storage_objects", async |tx| {
            for index in 0..active + tombstoned {
                clock.advance(Duration::from_secs(3_600));
                let id = crate::domain::id::Id::<()>::generate(&clock).to_string();
                let key = if index % 2 == 0 {
                    spread_key(&mut rng)
                } else {
                    ObjectKey::allocate(KeyNamespace::Objects)
                };
                let at = "2026-09-25T12:00:00Z";
                let live = index < active;
                sqlx::query(
                    "INSERT INTO storage_objects
                        (id, object_key, provider, size_bytes, state, refcount,
                         created_at, updated_at, finalized_at, tombstoned_at)
                     VALUES (?1, ?2, 'local', 1, ?3, ?4, ?5, ?5, ?5, ?6)",
                )
                .bind(id)
                .bind(key.as_str())
                .bind(if live { "active" } else { "tombstoned" })
                .bind(i64::from(live))
                .bind(at)
                .bind((!live).then_some(at))
                .execute(tx.executor())
                .await?;
            }
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();
}

async fn catalog_fingerprint(pools: &DbPools) -> Vec<(String, String, String, i64)> {
    sqlx::query_as("SELECT id, object_key, state, refcount FROM storage_objects ORDER BY id")
        .fetch_all(pools.reader().executor())
        .await
        .unwrap()
}

#[tokio::test]
async fn it_consistency_sample_is_bounded_stratified_and_report_only() {
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&MIGRATOR).await.unwrap();
    seed_catalog(&pools, 400, 40).await;
    let before = catalog_fingerprint(&pools).await;

    let sampled = consistency::sample(pools.reader()).await.unwrap();
    assert!(sampled.len() <= SAMPLE_LIMIT);
    assert!(sampled.len() >= 40, "{}", sampled.len());
    let active: std::collections::BTreeSet<String> = before
        .iter()
        .filter(|row| row.2 == "active")
        .map(|row| row.0.clone())
        .collect();
    assert!(sampled.iter().all(|row| active.contains(&row.id)));
    let shards: std::collections::BTreeSet<&str> = sampled
        .iter()
        .map(|row| &row.object_key["objects/".len().."objects/".len() + 2])
        .collect();
    assert!(shards.len() >= 15, "{shards:?}");
    let mut ids: Vec<&str> = sampled.iter().map(|row| row.id.as_str()).collect();
    ids.sort_unstable();
    let oldest = before.first().unwrap().0.as_str();
    let newest_active = before
        .iter()
        .rfind(|row| row.2 == "active")
        .unwrap()
        .0
        .as_str();
    assert!(ids.contains(&oldest));
    assert!(*ids.last().unwrap() > before[before.len() / 2].0.as_str());
    assert!(newest_active >= *ids.last().unwrap());

    let provider = Scripted::with_exists(ProviderKind::Local, existence(0, SAMPLE_LIMIT));
    let (monitor, signals) = supervised(&provider);
    monitor.startup(Some(pools.reader().clone())).await;
    let snapshot = monitor.status().snapshot();
    let consistency = snapshot.consistency.unwrap();
    assert_eq!(consistency.outcome, ConsistencyOutcome::StorageReplaced);
    assert!(consistency.sampled <= SAMPLE_LIMIT);
    assert_eq!(provider.exists_calls(), consistency.sampled);
    assert_eq!(snapshot.health, StorageHealth::Degraded);
    assert_eq!(snapshot.core, StorageHealth::Ok);
    assert_eq!(snapshot.degradations, [Diagnosis::StorageReplaced]);
    assert_eq!(signals.last().unwrap().health, StorageHealth::Degraded);

    monitor.check().await;
    assert_eq!(
        monitor.status().snapshot().degradations,
        [Diagnosis::StorageReplaced],
        "the startup consistency probe runs once and its diagnosis persists"
    );
    assert_eq!(provider.exists_calls(), consistency.sampled);
    assert_eq!(catalog_fingerprint(&pools).await, before);
    pools.shutdown().await.checkpoint.unwrap();
}

#[tokio::test]
async fn it_consistency_probe_waits_for_reachable_storage() {
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&MIGRATOR).await.unwrap();
    seed_catalog(&pools, 20, 0).await;

    let provider = Scripted::of_kind(ProviderKind::Local, &[fail(Diagnosis::Unreachable)]);
    let (monitor, _) = supervised(&provider);
    monitor.startup(Some(pools.reader().clone())).await;
    assert_eq!(
        provider.exists_calls(),
        0,
        "an unreachable provider is never sampled"
    );
    assert!(monitor.status().snapshot().consistency.is_none());

    monitor.check().await;
    assert_eq!(provider.exists_calls(), 0);
    monitor.check().await;
    let consistency = monitor.status().snapshot().consistency.unwrap();
    assert_eq!(consistency.outcome, ConsistencyOutcome::Consistent);
    assert_eq!(provider.exists_calls(), 20);
    pools.shutdown().await.checkpoint.unwrap();
}

#[test]
fn unit_self_test_report_classification() {
    let passed = report(ProbeDepth::Full, PASS);
    assert_eq!(passed.result(), SelfTestResult::Passed);
    assert!(passed.capabilities_verified());
    let degraded = report(ProbeDepth::Full, degraded(Diagnosis::CorsMissingEtag));
    assert_eq!(degraded.result(), SelfTestResult::Degraded);
    assert_eq!(degraded.core_failure(), None);
    let failed = report(ProbeDepth::Full, fail(Diagnosis::Unreachable));
    assert_eq!(failed.result(), SelfTestResult::Failed);
    assert_eq!(failed.core_failure(), Some(Diagnosis::Unreachable));
    assert!(!failed.capabilities_verified());
    assert!(!report(ProbeDepth::Light, PASS).capabilities_verified());
    let clock = TestClock::new(NOW);
    assert_eq!(clock.now(), passed.ran_at);
}
