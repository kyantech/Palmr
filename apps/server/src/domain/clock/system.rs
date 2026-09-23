use std::time::Instant;

use time::OffsetDateTime;

use super::Clock;

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[expect(
        clippy::disallowed_methods,
        reason = "SystemClock is the only production reader of the OS wall clock"
    )]
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn monotonic(&self) -> Instant {
        Instant::now()
    }
}
