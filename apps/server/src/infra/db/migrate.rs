use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::SqliteConnection;

use super::pool::DbPools;

pub const STARTUP_MIGRATION_FAILED: &str = "STARTUP_MIGRATION_FAILED";
pub const STARTUP_MIGRATION_CHECKSUM_MISMATCH: &str = "STARTUP_MIGRATION_CHECKSUM_MISMATCH";
pub const DB_SCHEMA_AHEAD_OF_BINARY: &str = "DB_SCHEMA_AHEAD_OF_BINARY";

const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationStatus {
    pub applied: usize,
    pub version: Option<i64>,
}

#[derive(Debug)]
pub enum MigrationFailure {
    UnrecognizedDatabase,
    Failed {
        migration: Option<(i64, String)>,
        source: MigrateError,
    },
    ChecksumMismatch {
        version: i64,
    },
    SchemaAhead {
        recorded: i64,
        known: Option<i64>,
    },
}

#[derive(Debug)]
pub struct MigrationError {
    pub path: PathBuf,
    pub failure: MigrationFailure,
}

impl MigrationError {
    pub const fn code(&self) -> &'static str {
        match self.failure {
            MigrationFailure::UnrecognizedDatabase | MigrationFailure::Failed { .. } => {
                STARTUP_MIGRATION_FAILED
            }
            MigrationFailure::ChecksumMismatch { .. } => STARTUP_MIGRATION_CHECKSUM_MISMATCH,
            MigrationFailure::SchemaAhead { .. } => DB_SCHEMA_AHEAD_OF_BINARY,
        }
    }

    pub fn version(&self) -> Option<i64> {
        match &self.failure {
            MigrationFailure::UnrecognizedDatabase => None,
            MigrationFailure::Failed { migration, .. } => {
                migration.as_ref().map(|(version, _)| *version)
            }
            MigrationFailure::ChecksumMismatch { version } => Some(*version),
            MigrationFailure::SchemaAhead { recorded, .. } => Some(*recorded),
        }
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path.display();
        match &self.failure {
            MigrationFailure::UnrecognizedDatabase => write!(
                f,
                "{STARTUP_MIGRATION_FAILED}: the data directory contains a database Palmr v4 does not recognize ({path}). Palmr v4 does not upgrade Palmr v3 data; point PALMR_DATA_DIR at an empty directory"
            ),
            MigrationFailure::Failed {
                migration: Some((version, description)),
                source,
            } => write!(
                f,
                "{STARTUP_MIGRATION_FAILED}: schema migration {version} ({description}) failed on {path}: {source}. The failed migration was rolled back and the database remains at its last applied migration"
            ),
            MigrationFailure::Failed {
                migration: None,
                source,
            } => write!(
                f,
                "{STARTUP_MIGRATION_FAILED}: cannot migrate the database {path}: {source}"
            ),
            MigrationFailure::ChecksumMismatch { version } => write!(
                f,
                "{STARTUP_MIGRATION_CHECKSUM_MISMATCH}: schema migration {version} recorded in {path} differs from the migration built into this Palmr binary. The recorded checksum is never bypassed; run the Palmr release that created this database or restore a backup"
            ),
            MigrationFailure::SchemaAhead { recorded, known } => {
                write!(
                    f,
                    "{DB_SCHEMA_AHEAD_OF_BINARY}: {path} records schema migration {recorded}, which this Palmr binary does not contain"
                )?;
                if let Some(known) = known {
                    write!(f, " (its latest is {known})")?;
                }
                f.write_str(". The database was upgraded by a newer Palmr release; run that release or restore a backup taken before the upgrade")
            }
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.failure {
            MigrationFailure::Failed { source, .. } => Some(source),
            MigrationFailure::UnrecognizedDatabase
            | MigrationFailure::ChecksumMismatch { .. }
            | MigrationFailure::SchemaAhead { .. } => None,
        }
    }
}

impl DbPools {
    pub async fn migrate(&self, migrator: &Migrator) -> Result<MigrationStatus, MigrationError> {
        let failed = |failure| MigrationError {
            path: self.path().to_path_buf(),
            failure,
        };
        let mut connection = self
            .writer()
            .acquire()
            .await
            .map_err(|source| failed(unclassified(source)))?;

        let recorded = recorded_versions(&mut connection)
            .await
            .map_err(|source| failed(unclassified(source)))?
            .ok_or_else(|| failed(MigrationFailure::UnrecognizedDatabase))?;
        let pending = migrator
            .iter()
            .filter(|migration| !recorded.contains(&migration.version))
            .count();

        migrator
            .run(&mut *connection)
            .await
            .map_err(|error| failed(classify(error, migrator)))?;

        Ok(MigrationStatus {
            applied: pending,
            version: latest_version(migrator),
        })
    }
}

async fn recorded_versions(
    connection: &mut SqliteConnection,
) -> Result<Option<HashSet<i64>>, sqlx::Error> {
    let (versioned, populated): (bool, bool) = sqlx::query_as(
        "SELECT \
             EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1), \
             EXISTS (SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\')",
    )
    .bind(MIGRATIONS_TABLE)
    .fetch_one(&mut *connection)
    .await?;

    match (versioned, populated) {
        (false, false) => Ok(Some(HashSet::new())),
        (false, true) => Ok(None),
        (true, _) => {
            let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations")
                .fetch_all(&mut *connection)
                .await?;
            Ok(Some(versions.into_iter().collect()))
        }
    }
}

fn latest_version(migrator: &Migrator) -> Option<i64> {
    migrator.iter().map(|migration| migration.version).max()
}

const fn unclassified(source: sqlx::Error) -> MigrationFailure {
    MigrationFailure::Failed {
        migration: None,
        source: MigrateError::Execute(source),
    }
}

fn classify(error: MigrateError, migrator: &Migrator) -> MigrationFailure {
    let known = latest_version(migrator);
    match error {
        MigrateError::VersionMismatch(version) => MigrationFailure::ChecksumMismatch { version },
        MigrateError::VersionMissing(recorded) if known.is_none_or(|known| recorded > known) => {
            MigrationFailure::SchemaAhead { recorded, known }
        }
        MigrateError::ExecuteMigration(_, version) => MigrationFailure::Failed {
            migration: migrator
                .iter()
                .find(|migration| migration.version == version)
                .map(|migration| (version, migration.description.to_string())),
            source: error,
        },
        source => MigrationFailure::Failed {
            migration: None,
            source,
        },
    }
}
