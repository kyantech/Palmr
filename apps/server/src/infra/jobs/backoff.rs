use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub const RETRY_BASE: Duration = Duration::from_secs(30);
pub const RETRY_CAP: Duration = Duration::from_secs(6 * 60 * 60);
pub const JITTER_DIVISOR: u64 = 5;

pub fn base_delay(attempts: u32) -> Duration {
    let base = millis(RETRY_BASE);
    let cap = millis(RETRY_CAP);
    let factor = 1_u64.checked_shl(attempts).unwrap_or(u64::MAX);
    Duration::from_millis(base.saturating_mul(factor).min(cap))
}

pub fn retry_delay(attempts: u32, sample: u32) -> Duration {
    let base = millis(base_delay(attempts));
    let spread = base / JITTER_DIVISOR;
    let span = u128::from(spread) * 2;
    let offset = span * u128::from(sample) / u128::from(u32::MAX);
    let offset = u64::try_from(offset).unwrap_or(spread * 2);
    Duration::from_millis(base - spread + offset)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Clone)]
pub struct Jitter(Arc<dyn Fn() -> u32 + Send + Sync>);

impl Jitter {
    pub fn os() -> Self {
        Self(Arc::new(|| {
            let mut bytes = [0_u8; 4];
            match getrandom::fill(&mut bytes) {
                Ok(()) => u32::from_ne_bytes(bytes),
                Err(_) => u32::MAX / 2,
            }
        }))
    }

    pub fn from_fn(sample: impl Fn() -> u32 + Send + Sync + 'static) -> Self {
        Self(Arc::new(sample))
    }

    pub fn sample(&self) -> u32 {
        (self.0)()
    }
}

impl fmt::Debug for Jitter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Jitter")
    }
}
