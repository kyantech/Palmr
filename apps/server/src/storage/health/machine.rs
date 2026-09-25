use super::report::{Diagnosis, FailureClass};
use super::StorageHealth;

pub const DOWN_AFTER_FAILURES: u32 = 3;
pub const RECOVER_AFTER_SUCCESSES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: StorageHealth,
    pub to: StorageHealth,
    pub cause: Option<Diagnosis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthMachine {
    state: StorageHealth,
    failures: u32,
    successes: u32,
    cause: Option<Diagnosis>,
}

impl HealthMachine {
    pub const fn starting(core_failure: Option<Diagnosis>) -> Self {
        match core_failure {
            None => Self {
                state: StorageHealth::Ok,
                failures: 0,
                successes: 0,
                cause: None,
            },
            Some(cause) => Self {
                state: StorageHealth::Down,
                failures: 1,
                successes: 0,
                cause: Some(cause),
            },
        }
    }

    pub const fn state(&self) -> StorageHealth {
        self.state
    }

    pub const fn cause(&self) -> Option<Diagnosis> {
        self.cause
    }

    pub const fn consecutive_failures(&self) -> u32 {
        self.failures
    }

    pub const fn consecutive_successes(&self) -> u32 {
        self.successes
    }

    pub const fn failure_would_classify_down(&self) -> bool {
        matches!(self.state, StorageHealth::Degraded)
            && self.failures.saturating_add(1) >= DOWN_AFTER_FAILURES
    }

    pub fn record_success(&mut self) -> Option<Transition> {
        let from = self.state;
        self.failures = 0;
        match self.state {
            StorageHealth::Ok | StorageHealth::Degraded => {
                self.successes = 0;
                self.state = StorageHealth::Ok;
                self.cause = None;
            }
            StorageHealth::Down => {
                self.successes = self.successes.saturating_add(1);
                if self.successes >= RECOVER_AFTER_SUCCESSES {
                    self.successes = 0;
                    self.state = StorageHealth::Ok;
                    self.cause = None;
                }
            }
        }
        self.transition(from)
    }

    pub fn record_failure(&mut self, diagnosis: Diagnosis) -> Option<Transition> {
        let from = self.state;
        self.successes = 0;
        self.failures = self.failures.saturating_add(1);
        self.cause = Some(diagnosis);
        self.state = if diagnosis.class() == FailureClass::NonTransient {
            StorageHealth::Down
        } else {
            match self.state {
                StorageHealth::Ok => StorageHealth::Degraded,
                StorageHealth::Degraded if self.failures >= DOWN_AFTER_FAILURES => {
                    StorageHealth::Down
                }
                state => state,
            }
        };
        self.transition(from)
    }

    fn transition(&self, from: StorageHealth) -> Option<Transition> {
        (from != self.state).then_some(Transition {
            from,
            to: self.state,
            cause: self.cause,
        })
    }
}
