mod json;
mod presigned;
mod stderr;

use std::fmt;
use std::io::{self, IsTerminal};

use tracing::Dispatch;
use tracing_subscriber::filter::{EnvFilter, ParseError};
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

use json::JsonLayer;
pub use presigned::RedactedPresignedUrl;
pub use stderr::write_startup_failure;

pub const STARTUP_TRACING_INIT_FAILED: &str = "STARTUP_TRACING_INIT_FAILED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

#[derive(Debug)]
pub enum TelemetryInitError {
    InvalidFilter(ParseError),
    SubscriberAlreadyInstalled,
}

impl TelemetryInitError {
    pub const fn code(&self) -> &'static str {
        STARTUP_TRACING_INIT_FAILED
    }
}

impl fmt::Display for TelemetryInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{STARTUP_TRACING_INIT_FAILED}: ")?;
        match self {
            Self::InvalidFilter(source) => {
                write!(f, "PALMR_LOG_LEVEL is not a valid log filter: {source}")
            }
            Self::SubscriberAlreadyInstalled => {
                f.write_str("a global tracing subscriber is already installed")
            }
        }
    }
}

impl std::error::Error for TelemetryInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidFilter(source) => Some(source),
            Self::SubscriberAlreadyInstalled => None,
        }
    }
}

pub fn init(filter: &str, format: LogFormat) -> Result<(), TelemetryInitError> {
    let dispatch = build_dispatch(
        parse_filter(filter)?,
        format,
        io::stdout,
        SystemTime,
        io::stdout().is_terminal(),
    );
    tracing::dispatcher::set_global_default(dispatch)
        .map_err(|_| TelemetryInitError::SubscriberAlreadyInstalled)
}

fn parse_filter(directives: &str) -> Result<EnvFilter, TelemetryInitError> {
    EnvFilter::builder()
        .with_regex(false)
        .parse(directives)
        .map_err(TelemetryInitError::InvalidFilter)
}

fn build_dispatch<W, T>(
    filter: EnvFilter,
    format: LogFormat,
    make_writer: W,
    timer: T,
    ansi: bool,
) -> Dispatch
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    T: FormatTime + Send + Sync + 'static,
{
    let registry = Registry::default().with(filter);
    match format {
        LogFormat::Json => Dispatch::new(registry.with(JsonLayer::new(make_writer, timer))),
        LogFormat::Pretty => Dispatch::new(
            registry.with(
                tracing_subscriber::fmt::layer()
                    .pretty()
                    .with_ansi(ansi)
                    .with_timer(timer)
                    .with_writer(make_writer),
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
