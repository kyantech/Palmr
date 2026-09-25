pub mod caps;
pub mod error;
pub mod health;
pub mod key;
pub mod local;
pub mod provider;
pub mod s3;

use std::fmt;

use crate::config::StorageConfig;

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
