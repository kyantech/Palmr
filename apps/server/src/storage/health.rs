mod consistency;
mod machine;
mod monitor;
mod pattern;
mod probe_key;
mod report;
mod routine;

pub use self::monitor::{
    HealthSignal, Schedule, StorageMonitor, StorageStatus, HEALTH_CHECK_PERIOD,
};
pub(in crate::storage) use self::pattern::{compare, Comparison, ProbePattern};
pub(in crate::storage) use self::probe_key::{ProbeKey, PROBE_DIRS, PROBE_PREFIX};
pub use self::report::{
    CheckName, CheckScope, CheckStatus, Diagnosis, Fact, FactReport, FailureClass, ProbeDepth,
    SelfTestReport, SelfTestResult, SubCheck,
};
pub use self::routine::PROBE_BYTES;
pub(in crate::storage) use self::routine::{
    diagnose, remove_and_verify, skip_removal, sweep_stale, write_and_verify, ListedProbe,
    ProbeRun, ProbeStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageHealth {
    Ok,
    Degraded,
    Down,
}

impl StorageHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Down => "down",
        }
    }
}

#[cfg(test)]
mod tests;
