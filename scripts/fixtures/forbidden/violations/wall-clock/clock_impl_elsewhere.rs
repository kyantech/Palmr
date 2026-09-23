use std::time::Instant;

use time::{OffsetDateTime, UtcDateTime};

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn monotonic(&self) -> Instant {
        Instant::now()
    }
}

pub fn today() -> UtcDateTime {
    UtcDateTime::now()
}
