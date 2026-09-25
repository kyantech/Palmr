pub mod caps;
pub mod error;
pub mod health;
pub mod key;
pub mod lifecycle;
pub mod local;
pub mod provider;
pub mod s3;
mod stored_key;

use std::fmt;
use std::sync::Arc;

use self::error::StorageError;
use self::local::LocalProvider;
use self::provider::StorageProvider;
use self::s3::client::S3Clients;
use self::s3::config::S3SetupError;
use self::s3::S3Provider;
use crate::config::{OperatorConfig, StorageConfig};
use crate::domain::clock::Clock;

#[derive(Debug)]
pub enum ProviderBuildError {
    LocalRoot(StorageError),
    S3Setup(S3SetupError),
    S3Build(StorageError),
}

impl ProviderBuildError {
    pub const fn provider(&self) -> ProviderKind {
        match self {
            Self::LocalRoot(_) => ProviderKind::Local,
            Self::S3Setup(_) | Self::S3Build(_) => ProviderKind::S3,
        }
    }
}

impl fmt::Display for ProviderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalRoot(error) => write!(
                f,
                "the local storage root under the data directory could not be opened: {error}"
            ),
            Self::S3Setup(error) => error.fmt(f),
            Self::S3Build(error) => {
                write!(f, "the S3 storage provider could not be built: {error}")
            }
        }
    }
}

impl std::error::Error for ProviderBuildError {}

#[cfg(test)]
pub(crate) fn test_runtime() -> (Arc<dyn StorageProvider>, health::StorageStatus) {
    let provider: Arc<dyn StorageProvider> = Arc::new(LocalProvider::temporary());
    let monitor = health::StorageMonitor::new(
        Arc::clone(&provider),
        Arc::new(crate::domain::clock::SystemClock),
        Arc::new(|_| {}),
    );
    (provider, monitor.status())
}

pub fn build_provider(
    config: &OperatorConfig,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn StorageProvider>, ProviderBuildError> {
    match &config.storage {
        StorageConfig::Local => {
            let local = LocalProvider::open(&config.data_dir, config.upload_buffer_bytes)
                .map_err(ProviderBuildError::LocalRoot)?
                .with_clock(clock);
            Ok(Arc::new(local))
        }
        StorageConfig::S3(_) => {
            let clients = S3Clients::build(&config.storage)
                .map_err(ProviderBuildError::S3Setup)?
                .ok_or_else(|| {
                    ProviderBuildError::S3Build(StorageError::Config(
                        "no S3 configuration is present".to_owned(),
                    ))
                })?;
            let s3 = S3Provider::new(clients, config.upload_buffer_bytes, config.base_url.url())
                .map_err(ProviderBuildError::S3Build)?
                .with_clock(clock);
            Ok(Arc::new(s3))
        }
    }
}

pub const fn configured_provider(config: &StorageConfig) -> ProviderKind {
    match config {
        StorageConfig::Local => ProviderKind::Local,
        StorageConfig::S3(_) => ProviderKind::S3,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Local,
    S3,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
