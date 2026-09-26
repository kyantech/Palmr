use std::fmt;
use std::io;
use std::path::PathBuf;

use super::admin::RecoveryError;
use crate::app::lifecycle::StartupError;
use crate::features::users::model::UserId;
use crate::infra::db::InstanceLockError;
use crate::infra::jobs::JobsError;

pub const CLI_DATABASE_NOT_FOUND: &str = "CLI_DATABASE_NOT_FOUND";
pub const CLI_DB_INTEGRITY_FAILED: &str = "CLI_DB_INTEGRITY_FAILED";
pub const CLI_DB_CHECK_FAILED: &str = "CLI_DB_CHECK_FAILED";
pub const CLI_BACKUP_DESTINATION_INVALID: &str = "CLI_BACKUP_DESTINATION_INVALID";
pub const CLI_BACKUP_EXISTS: &str = "CLI_BACKUP_EXISTS";
pub const CLI_BACKUP_FAILED: &str = "CLI_BACKUP_FAILED";
pub const CLI_JOBS_FAILED: &str = "CLI_JOBS_FAILED";
pub const CLI_USER_NOT_FOUND: &str = "CLI_USER_NOT_FOUND";
pub const CLI_RECOVERY_FAILED: &str = "CLI_RECOVERY_FAILED";
pub const CLI_OUTPUT_FAILED: &str = "CLI_OUTPUT_FAILED";

const EX_FAILURE: u8 = 1;
const EX_DATAERR: u8 = 65;
const EX_NOINPUT: u8 = 66;
const EX_NOUSER: u8 = 67;
const EX_IOERR: u8 = 74;
const EX_CANTCREAT: u8 = 73;
const EX_CONFIG: u8 = 78;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationProblem {
    Missing,
    NotADirectory,
    NotUtf8,
    Unreadable,
}

impl DestinationProblem {
    const fn describe(self) -> &'static str {
        match self {
            Self::Missing => "the directory does not exist; create it first",
            Self::NotADirectory => "the path is not a directory",
            Self::NotUtf8 => "the path is not valid UTF-8",
            Self::Unreadable => "the directory cannot be resolved",
        }
    }
}

#[derive(Debug)]
pub enum BackupFailure {
    Sqlite(sqlx::Error),
    Io(io::Error),
}

impl fmt::Display for BackupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
        }
    }
}

#[derive(Debug)]
pub enum CliError {
    Runtime(io::Error),
    Startup(StartupError),
    DataDirInUse {
        source: InstanceLockError,
        allows_concurrent: bool,
    },
    DatabaseNotFound {
        path: PathBuf,
    },
    IntegrityFailed {
        path: PathBuf,
        problems: usize,
    },
    CheckFailed {
        path: PathBuf,
        source: sqlx::Error,
    },
    BackupDestination {
        path: PathBuf,
        problem: DestinationProblem,
    },
    BackupExists {
        path: PathBuf,
    },
    BackupFailed {
        path: PathBuf,
        source: BackupFailure,
    },
    Jobs {
        source: JobsError,
    },
    UserNotFound {
        user: String,
    },
    RecoveryFailed {
        source: RecoveryError,
    },
    CredentialNotDelivered {
        user: UserId,
    },
}

impl CliError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Startup(error) => error.exit_code(),
            Self::DataDirInUse { .. } => EX_CONFIG,
            Self::DatabaseNotFound { .. } => EX_NOINPUT,
            Self::UserNotFound { .. } => EX_NOUSER,
            Self::CredentialNotDelivered { .. } => EX_IOERR,
            Self::IntegrityFailed { .. } => EX_DATAERR,
            Self::BackupDestination { .. } | Self::BackupExists { .. } => EX_CANTCREAT,
            Self::Jobs { .. }
            | Self::RecoveryFailed { .. }
            | Self::Runtime(_)
            | Self::CheckFailed { .. }
            | Self::BackupFailed { .. } => EX_FAILURE,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => write!(f, "the async runtime could not be started: {error}"),
            Self::Startup(error) => error.fmt(f),
            Self::DataDirInUse {
                source,
                allows_concurrent,
            } => {
                source.fmt(f)?;
                if *allows_concurrent {
                    f.write_str(". Pass --allow-concurrent to run this read-only command alongside the running server")?;
                }
                Ok(())
            }
            Self::DatabaseNotFound { path } => write!(
                f,
                "{CLI_DATABASE_NOT_FOUND}: there is no Palmr database at {}. Check PALMR_DATA_DIR",
                path.display()
            ),
            Self::IntegrityFailed { path, problems } => write!(
                f,
                "{CLI_DB_INTEGRITY_FAILED}: the SQLite integrity check reported {problems} problem(s) in {}. Stop Palmr and restore the database from a backup",
                path.display()
            ),
            Self::CheckFailed { path, source } => write!(
                f,
                "{CLI_DB_CHECK_FAILED}: the SQLite integrity check could not run on {}: {source}",
                path.display()
            ),
            Self::BackupDestination { path, problem } => write!(
                f,
                "{CLI_BACKUP_DESTINATION_INVALID}: cannot write a backup into {}: {}",
                path.display(),
                problem.describe()
            ),
            Self::BackupExists { path } => write!(
                f,
                "{CLI_BACKUP_EXISTS}: {} already exists and is never overwritten; run the backup again or move the existing file",
                path.display()
            ),
            Self::BackupFailed { path, source } => write!(
                f,
                "{CLI_BACKUP_FAILED}: the database backup to {} failed: {source}. No backup file was left behind",
                path.display()
            ),
            Self::Jobs { source } => write!(f, "{CLI_JOBS_FAILED}: {source}"),
            Self::UserNotFound { user } => write!(
                f,
                "{CLI_USER_NOT_FOUND}: no account matches {user:?}; nothing was changed"
            ),
            Self::RecoveryFailed { source } => write!(
                f,
                "{CLI_RECOVERY_FAILED}: the recovery was rolled back and nothing was changed: {source}"
            ),
            Self::CredentialNotDelivered { user } => write!(
                f,
                "{CLI_OUTPUT_FAILED}: the password of user {user} was reset but the temporary password could not be written to standard output; run the command again to issue a new one"
            ),
        }
    }
}

impl std::error::Error for CliError {}

impl From<StartupError> for CliError {
    fn from(error: StartupError) -> Self {
        Self::Startup(error)
    }
}
