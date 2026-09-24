use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::error::CliError;
use crate::app::lifecycle::data_dir::{apply_process_umask, DataDir};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::infra::db::{InstanceLock, InstanceLockError, DATABASE_FILE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Exclusive,
    ReadOnly { allow_concurrent: bool },
}

pub struct DataAccess {
    root: PathBuf,
    lock: Option<InstanceLock>,
}

impl DataAccess {
    pub fn claim(
        config: &OperatorConfig,
        access: Access,
        clock: &dyn Clock,
    ) -> Result<Self, CliError> {
        apply_process_umask();
        if let Access::ReadOnly {
            allow_concurrent: true,
        } = access
        {
            return Ok(Self {
                root: config.data_dir.clone(),
                lock: None,
            });
        }

        let data_dir = DataDir::prepare(&config.data_dir).map_err(StartupError::from)?;
        match InstanceLock::acquire(data_dir.root(), clock) {
            Ok((lock, _)) => Ok(Self {
                root: data_dir.root().to_path_buf(),
                lock: Some(lock),
            }),
            Err(source @ InstanceLockError::InUse { .. }) => Err(CliError::DataDirInUse {
                source,
                allows_concurrent: matches!(access, Access::ReadOnly { .. }),
            }),
            Err(error) => Err(StartupError::from(error).into()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn existing_database(&self) -> Result<PathBuf, CliError> {
        let path = self.root.join(DATABASE_FILE);
        match path.symlink_metadata() {
            Ok(metadata) if metadata.is_file() => Ok(path),
            Ok(_) | Err(_) => Err(CliError::DatabaseNotFound { path }),
        }
    }

    pub fn release(self) {
        let Some(lock) = self.lock else {
            return;
        };
        let path = lock.path().to_path_buf();
        if let Err(error) = lock.release() {
            let _ = writeln!(
                io::stderr().lock(),
                "warning: the lock file {} could not be cleared ({error}); the lock itself is released and the next start reclaims the file",
                path.display()
            );
        }
    }
}
