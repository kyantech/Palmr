use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rustix::fs::{RenameFlags, CWD};
use rustix::io::Errno;
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

use super::error::{BackupFailure, CliError, DestinationProblem};
use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::infra::db::SnapshotReader;

pub async fn check(
    config: &OperatorConfig,
    allow_concurrent: bool,
    clock: &dyn Clock,
) -> Result<(), CliError> {
    let access = DataAccess::claim(config, Access::ReadOnly { allow_concurrent }, clock)?;
    let outcome = check_database(config, &access).await;
    access.release();
    outcome
}

async fn check_database(config: &OperatorConfig, access: &DataAccess) -> Result<(), CliError> {
    access.existing_database()?;
    let reader = SnapshotReader::open(access.root(), config.db_synchronous)
        .await
        .map_err(StartupError::from)?;
    let report = reader.integrity_check().await;
    let path = reader.path().to_path_buf();
    reader.close().await;
    let report = report.map_err(|source| CliError::CheckFailed {
        path: path.clone(),
        source,
    })?;

    let mut stdout = io::stdout().lock();
    for row in report.problems() {
        let _ = writeln!(stdout, "{}", printable(row));
    }
    let _ = stdout.flush();
    if report.is_ok() {
        Ok(())
    } else {
        Err(CliError::IntegrityFailed {
            path,
            problems: report.problems().len(),
        })
    }
}

pub async fn backup(
    config: &OperatorConfig,
    out: &Path,
    allow_concurrent: bool,
    clock: &dyn Clock,
) -> Result<PathBuf, CliError> {
    let directory = destination_directory(out)?;
    let access = DataAccess::claim(config, Access::ReadOnly { allow_concurrent }, clock)?;
    let outcome = backup_database(config, &access, &directory, clock.now()).await;
    access.release();
    outcome
}

async fn backup_database(
    config: &OperatorConfig,
    access: &DataAccess,
    directory: &Path,
    now: OffsetDateTime,
) -> Result<PathBuf, CliError> {
    access.existing_database()?;
    let name = backup_file_name(now);
    let target = directory.join(&name);
    let staging = directory.join(format!(".{name}.partial"));
    for path in [&target, &staging] {
        if path.symlink_metadata().is_ok() {
            return Err(CliError::BackupExists { path: path.clone() });
        }
    }
    let Some(staging_text) = staging.to_str() else {
        return Err(CliError::BackupDestination {
            path: directory.to_path_buf(),
            problem: DestinationProblem::NotUtf8,
        });
    };

    let reader = SnapshotReader::open(access.root(), config.db_synchronous)
        .await
        .map_err(StartupError::from)?;
    let vacuumed = reader.vacuum_into(staging_text).await;
    reader.close().await;

    let published = vacuumed
        .map_err(BackupFailure::Sqlite)
        .and_then(|()| publish(&staging, &target, directory).map_err(BackupFailure::Io));
    match published {
        Ok(()) => Ok(target),
        Err(source) => {
            let _ = fs::remove_file(&staging);
            match source {
                BackupFailure::Io(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    Err(CliError::BackupExists { path: target })
                }
                source => Err(CliError::BackupFailed {
                    path: target,
                    source,
                }),
            }
        }
    }
}

fn destination_directory(out: &Path) -> Result<PathBuf, CliError> {
    let invalid = |problem| CliError::BackupDestination {
        path: out.to_path_buf(),
        problem,
    };
    let metadata = fs::metadata(out).map_err(|error| {
        invalid(if error.kind() == io::ErrorKind::NotFound {
            DestinationProblem::Missing
        } else {
            DestinationProblem::Unreadable
        })
    })?;
    if !metadata.is_dir() {
        return Err(invalid(DestinationProblem::NotADirectory));
    }
    let directory = out
        .canonicalize()
        .map_err(|_| invalid(DestinationProblem::Unreadable))?;
    if directory.to_str().is_none() {
        return Err(invalid(DestinationProblem::NotUtf8));
    }
    Ok(directory)
}

pub fn backup_file_name(now: OffsetDateTime) -> String {
    let stamp = now
        .to_offset(UtcOffset::UTC)
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap_or_default();
    format!("palmr-{stamp}.db")
}

fn publish(staging: &Path, target: &Path, directory: &Path) -> io::Result<()> {
    File::open(staging)?.sync_all()?;
    match rustix::fs::renameat_with(CWD, staging, CWD, target, RenameFlags::NOREPLACE) {
        Ok(()) => {}
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {
            fs::hard_link(staging, target)?;
            fs::remove_file(staging)?;
        }
        Err(errno) => return Err(errno.into()),
    }
    File::open(directory)?.sync_all()
}

fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use time::macros::datetime;

    use super::{backup, backup_file_name};
    use crate::cli::error::CliError;
    use crate::cli::migrate::migrate;
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::clock::TestClock;

    #[test]
    fn unit_backup_file_name_is_utc_and_sortable() {
        assert_eq!(
            backup_file_name(datetime!(2026-09-22 03:15:00 UTC)),
            "palmr-20260922T031500Z.db"
        );
        assert_eq!(
            backup_file_name(datetime!(2026-09-22 05:15:00 +02:00)),
            "palmr-20260922T031500Z.db"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unit_backup_never_overwrites_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let out = temp.path().join("out");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&out).unwrap();
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_DATA_DIR",
            data.to_str().unwrap(),
        )]))
        .unwrap()
        .config;
        let clock = TestClock::new(datetime!(2026-09-22 03:15:00 UTC));
        migrate(&config, &clock).await.unwrap();

        let first = backup(&config, &out, false, &clock).await.unwrap();
        assert_eq!(
            first,
            out.canonicalize()
                .unwrap()
                .join("palmr-20260922T031500Z.db")
        );
        fs::write(&first, b"operator copy").unwrap();

        let error = backup(&config, &out, false, &clock).await.unwrap_err();
        assert!(
            matches!(&error, CliError::BackupExists { path } if *path == first),
            "{error:?}"
        );
        assert_eq!(error.exit_code(), 73);
        assert_eq!(
            fs::read_dir(&out).unwrap().count(),
            1,
            "no staging file is left behind"
        );
        assert_eq!(
            fs::File::open(&first).unwrap().metadata().unwrap().len(),
            13
        );
    }
}
