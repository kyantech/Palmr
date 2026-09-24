use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, Mode, OFlags, RawMode};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};

use super::pool::STARTUP_DB_OPEN_FAILED;
use crate::domain::clock::Clock;
use crate::domain::id::Id;

pub const STARTUP_DATA_DIR_IN_USE: &str = "STARTUP_DATA_DIR_IN_USE";

pub const INSTANCE_LOCK_FILE: &str = "runtime/instance.lock";
const LOCK_FILE_MODE: RawMode = 0o640;
const METADATA_LIMIT: u64 = 4 * 1024;

pub enum Instance {}

pub type InstanceId = Id<Instance>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockOrigin {
    Fresh,
    Reclaimed { previous: Option<InstanceId> },
}

#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
    instance_id: InstanceId,
}

#[derive(Serialize, Deserialize)]
struct LockMetadata {
    instance_id: String,
    pid: u32,
}

impl InstanceLock {
    pub fn acquire(
        data_root: &Path,
        clock: &dyn Clock,
    ) -> Result<(Self, LockOrigin), InstanceLockError> {
        let path = data_root.join(INSTANCE_LOCK_FILE);
        let fd = rustix::fs::open(
            &path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(LOCK_FILE_MODE),
        )
        .map_err(|errno| InstanceLockError::unusable(&path, LockOperation::Open, errno.into()))?;
        match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => return Err(InstanceLockError::InUse { path }),
            Err(errno) => {
                return Err(InstanceLockError::unusable(
                    &path,
                    LockOperation::Lock,
                    errno.into(),
                ))
            }
        }

        let mut file = File::from(fd);
        let previous = read_previous(&mut file)
            .map_err(|error| InstanceLockError::unusable(&path, LockOperation::Read, error))?;
        let instance_id = InstanceId::generate(clock);
        write_metadata(&mut file, instance_id)
            .map_err(|error| InstanceLockError::unusable(&path, LockOperation::Write, error))?;

        let origin = match previous {
            None => LockOrigin::Fresh,
            Some(previous) => LockOrigin::Reclaimed { previous },
        };
        Ok((
            Self {
                file,
                path,
                instance_id,
            },
            origin,
        ))
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn release(self) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.sync_data()
    }
}

fn read_previous(file: &mut File) -> io::Result<Option<Option<InstanceId>>> {
    file.seek(SeekFrom::Start(0))?;
    let mut contents = Vec::new();
    file.take(METADATA_LIMIT).read_to_end(&mut contents)?;
    if contents.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_slice::<LockMetadata>(&contents)
            .ok()
            .and_then(|metadata| metadata.instance_id.parse().ok()),
    ))
}

fn write_metadata(file: &mut File, instance_id: InstanceId) -> io::Result<()> {
    let metadata = LockMetadata {
        instance_id: instance_id.to_string(),
        pid: std::process::id(),
    };
    let mut contents = serde_json::to_vec(&metadata)?;
    contents.push(b'\n');
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&contents)?;
    file.sync_data()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockOperation {
    Open,
    Lock,
    Read,
    Write,
}

impl LockOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Lock => "lock",
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug)]
pub enum InstanceLockError {
    InUse {
        path: PathBuf,
    },
    Unusable {
        path: PathBuf,
        operation: LockOperation,
        source: io::Error,
    },
}

impl InstanceLockError {
    fn unusable(path: &Path, operation: LockOperation, source: io::Error) -> Self {
        Self::Unusable {
            path: path.to_path_buf(),
            operation,
            source,
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::InUse { .. } => STARTUP_DATA_DIR_IN_USE,
            Self::Unusable { .. } => STARTUP_DB_OPEN_FAILED,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::InUse { path } | Self::Unusable { path, .. } => path,
        }
    }
}

impl fmt::Display for InstanceLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse { path } => write!(
                f,
                "{STARTUP_DATA_DIR_IN_USE}: another running Palmr process holds the lock on {}; a data directory can be used by only one Palmr process at a time. Stop the other process or give this one its own PALMR_DATA_DIR. Deleting the lock file does not release the lock",
                path.display()
            ),
            Self::Unusable {
                path,
                operation,
                source,
            } => write!(
                f,
                "{STARTUP_DB_OPEN_FAILED}: cannot use the single-instance lock file {} (operation: {}; reason: {source}). Check that PALMR_DATA_DIR is a local, writable filesystem that supports file locks",
                path.display(),
                operation.as_str()
            ),
        }
    }
}

impl std::error::Error for InstanceLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InUse { .. } => None,
            Self::Unusable { source, .. } => Some(source),
        }
    }
}
