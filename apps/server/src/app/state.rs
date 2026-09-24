use std::sync::Arc;

use super::health::Health;
use super::openapi::ApiDocs;
use crate::domain::clock::Clock;

#[derive(Clone)]
pub struct AppState {
    clock: Arc<dyn Clock>,
    health: Health,
    api_docs: ApiDocs,
}

impl AppState {
    pub fn new(clock: Arc<dyn Clock>, health: Health, api_docs: ApiDocs) -> Self {
        Self {
            clock,
            health,
            api_docs,
        }
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub const fn health(&self) -> &Health {
        &self.health
    }

    pub const fn api_docs(&self) -> &ApiDocs {
        &self.api_docs
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::macros::datetime;

    use super::AppState;
    use crate::app::health::{Health, StorageState};
    use crate::app::lifecycle::Readiness;
    use crate::app::openapi::ApiDocs;
    use crate::app::router::application_routes;
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::clock::TestClock;

    #[test]
    fn unit_app_state_clones_share_services() {
        let clock = TestClock::new(datetime!(2026-01-01 00:00 UTC));
        let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
            &str,
            &str,
        )>()))
        .unwrap()
        .config;
        let docs = ApiDocs::new(
            application_routes().build().unwrap().openapi,
            &config.base_url,
        )
        .unwrap();
        let state = AppState::new(Arc::new(clock), Health::new(Readiness::new()), docs);
        let cloned = state.clone();

        assert!(Arc::ptr_eq(&state.clock, &cloned.clock));
        assert!(std::ptr::eq(
            state.api_docs().document().as_ptr(),
            cloned.api_docs().document().as_ptr()
        ));
        assert_eq!(cloned.clock().now(), datetime!(2026-01-01 00:00 UTC));

        state.health().checks().set_storage(StorageState::Degraded);
        assert_eq!(
            cloned.health().checks().snapshot().storage,
            Some(StorageState::Degraded)
        );
    }
}
