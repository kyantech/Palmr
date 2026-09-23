use std::sync::Arc;

use super::health::Health;
use crate::domain::clock::Clock;

#[derive(Clone)]
pub struct AppState {
    clock: Arc<dyn Clock>,
    health: Health,
}

impl AppState {
    pub fn new(clock: Arc<dyn Clock>, health: Health) -> Self {
        Self { clock, health }
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub const fn health(&self) -> &Health {
        &self.health
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::macros::datetime;

    use super::AppState;
    use crate::app::health::{Health, StorageState};
    use crate::app::lifecycle::Readiness;
    use crate::domain::clock::TestClock;

    #[test]
    fn unit_app_state_clones_share_services() {
        let clock = TestClock::new(datetime!(2026-01-01 00:00 UTC));
        let state = AppState::new(Arc::new(clock), Health::new(Readiness::new()));
        let cloned = state.clone();

        assert!(Arc::ptr_eq(&state.clock, &cloned.clock));
        assert_eq!(cloned.clock().now(), datetime!(2026-01-01 00:00 UTC));

        state.health().checks().set_storage(StorageState::Degraded);
        assert_eq!(
            cloned.health().checks().snapshot().storage,
            Some(StorageState::Degraded)
        );
    }
}
