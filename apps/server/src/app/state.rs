use std::sync::Arc;

use crate::domain::clock::Clock;

#[derive(Clone)]
pub struct AppState {
    clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self { clock }
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::macros::datetime;

    use super::AppState;
    use crate::domain::clock::TestClock;

    #[test]
    fn unit_app_state_clones_share_services() {
        let clock = TestClock::new(datetime!(2026-01-01 00:00 UTC));
        let state = AppState::new(Arc::new(clock));
        let cloned = state.clone();

        assert!(Arc::ptr_eq(&state.clock, &cloned.clock));
        assert_eq!(cloned.clock().now(), datetime!(2026-01-01 00:00 UTC));
    }
}
