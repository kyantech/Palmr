mod system;

use std::time::Instant;

use time::OffsetDateTime;

pub use system::SystemClock;
#[cfg(test)]
pub use test_clock::TestClock;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> OffsetDateTime;

    // Measures elapsed durations only; never persisted or compared with `now`.
    fn monotonic(&self) -> Instant;
}

#[cfg(test)]
mod test_clock {
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex, MutexGuard, PoisonError},
        task::{Context, Poll, Waker},
        time::{Duration, Instant},
    };

    use time::{OffsetDateTime, UtcOffset};

    use super::Clock;

    #[derive(Debug, Clone)]
    pub struct TestClock {
        state: Arc<Mutex<State>>,
    }

    #[derive(Debug)]
    struct State {
        now: OffsetDateTime,
        monotonic: Instant,
        timers: Vec<(OffsetDateTime, Arc<Mutex<TimerSlot>>)>,
    }

    #[derive(Debug, Default)]
    struct TimerSlot {
        fired: bool,
        waker: Option<Waker>,
    }

    #[derive(Debug)]
    pub struct TestTimer {
        slot: Arc<Mutex<TimerSlot>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    impl TestClock {
        pub fn new(start: OffsetDateTime) -> Self {
            Self {
                state: Arc::new(Mutex::new(State {
                    now: start.to_offset(UtcOffset::UTC),
                    monotonic: Instant::now(),
                    timers: Vec::new(),
                })),
            }
        }

        pub fn advance(&self, by: Duration) {
            let mut state = lock(&self.state);
            state.now += by;
            state.monotonic += by;
        }

        // A wall-clock jump: monotonic time does not move.
        pub fn set(&self, at: OffsetDateTime) {
            lock(&self.state).now = at.to_offset(UtcOffset::UTC);
        }

        pub fn timer_at(&self, deadline: OffsetDateTime) -> TestTimer {
            let slot = Arc::new(Mutex::new(TimerSlot::default()));
            lock(&self.state).timers.push((deadline, Arc::clone(&slot)));
            TestTimer { slot }
        }

        // Moving the clock never fires a timer by itself, so a test decides
        // exactly when due work becomes runnable. Returns the number fired.
        pub fn settle(&self) -> usize {
            let due = {
                let mut state = lock(&self.state);
                let now = state.now;
                let (due, pending) = state
                    .timers
                    .drain(..)
                    .partition::<Vec<_>, _>(|(deadline, _)| *deadline <= now);
                state.timers = pending;
                due
            };
            for (_, slot) in &due {
                let mut slot = lock(slot);
                slot.fired = true;
                if let Some(waker) = slot.waker.take() {
                    waker.wake();
                }
            }
            due.len()
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> OffsetDateTime {
            lock(&self.state).now
        }

        fn monotonic(&self) -> Instant {
            lock(&self.state).monotonic
        }
    }

    impl Future for TestTimer {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let mut slot = lock(&self.slot);
            if slot.fired {
                Poll::Ready(())
            } else {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::{Context, Poll, Wake, Waker},
        time::Duration,
    };

    use time::{macros::datetime, UtcOffset};

    use super::{Clock, SystemClock, TestClock};

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn unit_test_clock_advance() {
        let clock = TestClock::new(datetime!(2026-09-23 17:42:31.123 UTC));
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        let started = shared.monotonic();

        clock.advance(Duration::from_millis(1_500));

        assert_eq!(shared.now(), datetime!(2026-09-23 17:42:32.623 UTC));
        assert_eq!(shared.monotonic() - started, Duration::from_millis(1_500));

        clock.advance(Duration::from_secs(86_400));

        assert_eq!(clock.now(), datetime!(2026-09-24 17:42:32.623 UTC));
        assert_eq!(
            clock.monotonic() - started,
            Duration::from_millis(86_401_500)
        );
    }

    #[test]
    fn unit_test_clock_new_normalizes_to_utc() {
        let clock = TestClock::new(datetime!(2026-09-23 14:42:31.123 -3));

        assert_eq!(clock.now(), datetime!(2026-09-23 17:42:31.123 UTC));
        assert_eq!(clock.now().offset(), UtcOffset::UTC);
    }

    #[test]
    fn unit_test_clock_set_jumps_wall_clock_only() {
        let clock = TestClock::new(datetime!(2026-09-23 17:42:31.123 UTC));
        let started = clock.monotonic();

        clock.set(datetime!(2026-09-23 17:00:00 UTC));
        assert_eq!(clock.now(), datetime!(2026-09-23 17:00:00 UTC));

        clock.set(datetime!(2027-01-01 03:00:00 +3));
        assert_eq!(clock.now(), datetime!(2027-01-01 00:00:00 UTC));
        assert_eq!(clock.now().offset(), UtcOffset::UTC);

        assert_eq!(clock.monotonic(), started);
    }

    #[test]
    fn unit_test_clock_settle_fires_only_due_timers() {
        let clock = TestClock::new(datetime!(2026-09-23 17:42:31.123 UTC));
        let wakes = Arc::new(WakeCounter::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);

        let mut early = pin!(clock.timer_at(datetime!(2026-09-23 17:42:36.123 UTC)));
        let mut late = pin!(clock.timer_at(datetime!(2026-09-23 17:42:41.123 UTC)));
        assert_eq!(early.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(late.as_mut().poll(&mut cx), Poll::Pending);

        clock.advance(Duration::from_secs(5));
        assert_eq!(early.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 0);

        assert_eq!(clock.settle(), 1);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        assert_eq!(early.as_mut().poll(&mut cx), Poll::Ready(()));
        assert_eq!(late.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(clock.settle(), 0);

        clock.set(datetime!(2026-09-23 18:00:00 UTC));
        assert_eq!(clock.settle(), 1);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 2);
        assert_eq!(late.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    #[test]
    fn unit_system_clock_reads_utc() {
        let clock: &dyn Clock = &SystemClock;
        let started = clock.monotonic();

        assert_eq!(clock.now().offset(), UtcOffset::UTC);
        assert!(clock.monotonic() >= started);
    }
}
