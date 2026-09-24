use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use governor::middleware::NoOpMiddleware;
use governor::nanos::Nanos;
use governor::state::keyed::ShrinkableKeyedStateStore;
use governor::state::StateStore;

use super::class::{BucketSpec, Dimension, RateLimitClass, Stage};
use super::error::{RetryAfter, Throttled};
use super::key::{RateLimitKey, Subject};
use crate::domain::clock::Clock;

pub const MAX_TRACKED_KEYS: usize = 16_384;
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
const EVICTION_LOW_WATER: usize = MAX_TRACKED_KEYS - MAX_TRACKED_KEYS / 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    Throttled(Throttled),
    MissingIdentity(Dimension),
}

#[derive(Clone)]
struct LimiterClock {
    clock: Arc<dyn Clock>,
    origin: Instant,
}

impl LimiterClock {
    fn new(clock: Arc<dyn Clock>) -> Self {
        let origin = clock.monotonic();
        Self { clock, origin }
    }
}

impl governor::clock::Clock for LimiterClock {
    type Instant = Duration;

    fn now(&self) -> Duration {
        self.clock
            .monotonic()
            .saturating_duration_since(self.origin)
    }
}

struct BoundedStore {
    entries: Mutex<HashMap<RateLimitKey, u64>>,
    capacity: usize,
    low_water: usize,
}

impl BoundedStore {
    fn new(capacity: usize, low_water: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            low_water,
        }
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<RateLimitKey, u64>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn evict_least_constrained(entries: &mut HashMap<RateLimitKey, u64>, keep: usize) {
    let excess = entries.len().saturating_sub(keep);
    if excess == 0 {
        return;
    }
    let mut arrivals: Vec<u64> = entries.values().copied().collect();
    let (_, threshold, _) = arrivals.select_nth_unstable(excess - 1);
    let threshold = *threshold;
    let strictly_older = arrivals.iter().filter(|tat| **tat < threshold).count();
    let mut ties_to_drop = excess - strictly_older;
    entries.retain(|_, tat| {
        if *tat < threshold {
            false
        } else if *tat == threshold && ties_to_drop > 0 {
            ties_to_drop -= 1;
            false
        } else {
            true
        }
    });
}

impl StateStore for BoundedStore {
    type Key = RateLimitKey;

    fn measure_and_replace<T, F, E>(&self, key: &RateLimitKey, f: F) -> Result<T, E>
    where
        F: Fn(Option<Nanos>) -> Result<(T, Nanos), E>,
    {
        let mut entries = self.entries();
        if let Some(tat) = entries.get_mut(key) {
            let (outcome, next) = f(Some(Nanos::from(*tat)))?;
            *tat = next.into();
            return Ok(outcome);
        }
        let (outcome, next) = f(None)?;
        if entries.len() >= self.capacity {
            evict_least_constrained(&mut entries, self.low_water);
        }
        entries.insert(*key, next.into());
        Ok(outcome)
    }
}

impl ShrinkableKeyedStateStore<RateLimitKey> for BoundedStore {
    fn retain_recent(&self, drop_below: Nanos) {
        let drop_below = drop_below.as_u64();
        self.entries().retain(|_, tat| *tat > drop_below);
    }

    fn shrink_to_fit(&self) {
        self.entries().shrink_to_fit();
    }

    fn len(&self) -> usize {
        self.entries().len()
    }

    fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }
}

type Gcra =
    governor::RateLimiter<RateLimitKey, BoundedStore, LimiterClock, NoOpMiddleware<Duration>>;

struct Bucket {
    spec: BucketSpec,
    gcra: Gcra,
    clock: LimiterClock,
    last_sweep: AtomicU64,
}

impl Bucket {
    fn new(spec: BucketSpec, clock: LimiterClock) -> Self {
        Self {
            spec,
            gcra: governor::RateLimiter::new(
                spec.quota(),
                BoundedStore::new(MAX_TRACKED_KEYS, EVICTION_LOW_WATER),
                clock.clone(),
            ),
            clock,
            last_sweep: AtomicU64::new(0),
        }
    }

    fn check(&self, key: &RateLimitKey) -> Result<(), Duration> {
        self.sweep_if_due();
        self.gcra.check_key(key).map_err(|not_until| {
            let now = governor::clock::Clock::now(&self.clock);
            not_until.wait_time_from(now)
        })
    }

    fn sweep_if_due(&self) {
        let now = nanos(governor::clock::Clock::now(&self.clock));
        let last = self.last_sweep.load(Ordering::Acquire);
        if now.saturating_sub(last) < nanos(SWEEP_INTERVAL) {
            return;
        }
        if self
            .last_sweep
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.gcra.retain_recent();
            self.gcra.shrink_to_fit();
        }
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

pub struct RateLimiter {
    classes: Vec<Vec<Bucket>>,
}

impl RateLimiter {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let clock = LimiterClock::new(clock);
        let classes = RateLimitClass::ALL
            .into_iter()
            .map(|class| {
                class
                    .buckets()
                    .iter()
                    .map(|spec| Bucket::new(*spec, clock.clone()))
                    .collect()
            })
            .collect();
        Self { classes }
    }

    fn buckets(&self, class: RateLimitClass) -> &[Bucket] {
        self.classes.get(class.index()).map_or(&[], Vec::as_slice)
    }

    pub fn admit(
        &self,
        class: RateLimitClass,
        stage: Stage,
        subject: &Subject,
    ) -> Result<(), Denial> {
        for bucket in self
            .buckets(class)
            .iter()
            .filter(|bucket| bucket.spec.dimension().stage() == stage)
        {
            let dimension = bucket.spec.dimension();
            let key = subject
                .key_for(dimension)
                .ok_or(Denial::MissingIdentity(dimension))?;
            bucket.check(&key).map_err(|wait| {
                Denial::Throttled(Throttled::new(class, RetryAfter::covering(wait)))
            })?;
        }
        Ok(())
    }

    pub fn tracked_keys(&self, class: RateLimitClass) -> usize {
        self.buckets(class)
            .iter()
            .map(|bucket| bucket.gcra.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{evict_least_constrained, BoundedStore};
    use crate::infra::ratelimit::key::RateLimitKey;

    fn ip_key(index: u32) -> RateLimitKey {
        RateLimitKey::ResolvedIp(std::net::Ipv4Addr::from(index).into())
    }

    #[test]
    fn unit_eviction_drops_exactly_the_least_constrained_entries() {
        let mut entries: HashMap<RateLimitKey, u64> = (0..10)
            .map(|index| (ip_key(index), u64::from(index % 5)))
            .collect();
        evict_least_constrained(&mut entries, 3);
        let mut kept: Vec<u64> = entries.values().copied().collect();
        kept.sort_unstable();
        assert_eq!(kept, [3, 4, 4]);
    }

    #[test]
    fn unit_bounded_store_never_exceeds_capacity() {
        use governor::nanos::Nanos;
        use governor::state::keyed::ShrinkableKeyedStateStore;
        use governor::state::StateStore;

        let store = BoundedStore::new(8, 6);
        for index in 0..100 {
            let admitted: Result<(), ()> = store
                .measure_and_replace(&ip_key(index), |_| Ok(((), Nanos::from(u64::from(index)))));
            assert!(admitted.is_ok());
            assert!(store.len() <= 8);
        }
        store.retain_recent(Nanos::from(99));
        assert_eq!(store.len(), 0);
    }
}
