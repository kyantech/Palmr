#[cfg(not(unix))]
compile_error!("Palmr runs on Unix-like operating systems only");

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "route policy primitives are consumed as feature routes are registered"
    )
)]
mod app;
pub mod cli;
#[expect(
    dead_code,
    unused_imports,
    reason = "configuration fields are consumed by later startup steps"
)]
mod config;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "domain primitives are consumed by feature modules"
    )
)]
mod domain;
mod features;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "infrastructure primitives are consumed by feature modules"
    )
)]
mod infra;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "storage primitives are consumed by the storage providers"
    )
)]
mod storage;

pub use app::{lifecycle, openapi};
pub use config::{EnvironmentSource, OperatorConfig};
pub use domain::clock::{Clock, TestClock};
pub use infra::telemetry::write_startup_failure;
