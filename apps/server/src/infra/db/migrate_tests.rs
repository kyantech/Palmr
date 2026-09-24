use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;

use super::{DbPools, MigrationStatus, DATABASE_FILE, MIGRATOR};
use crate::config::SqliteSynchronous;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedMigration {
    pub version: i64,
    pub description: String,
    pub success: bool,
    pub checksum: Vec<u8>,
}

pub(crate) fn embedded_files() -> Vec<(String, String)> {
    MIGRATOR
        .iter()
        .map(|migration| {
            (
                format!(
                    "{:04}_{}.sql",
                    migration.version,
                    migration.description.replace(' ', "_")
                ),
                migration.sql.to_string(),
            )
        })
        .collect()
}

pub(crate) async fn fixture_migrator(files: &[(String, String)]) -> (TempDir, Migrator) {
    let directory = TempDir::new().unwrap();
    for (name, sql) in files {
        std::fs::write(directory.path().join(name), sql).unwrap();
    }
    let migrator = Migrator::new(directory.path().to_path_buf()).await.unwrap();
    (directory, migrator)
}

pub(crate) async fn open_pools(data_root: &Path) -> DbPools {
    DbPools::open(data_root, 1, SqliteSynchronous::Full)
        .await
        .unwrap()
}

pub(crate) async fn raw_connection(data_root: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(data_root.join(DATABASE_FILE))
            .create_if_missing(true),
    )
    .await
    .unwrap()
}

pub(crate) async fn recorded_migrations(
    connection: &mut SqliteConnection,
) -> Vec<RecordedMigration> {
    let rows: Vec<(i64, String, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version, description, success, checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.into_iter()
        .map(
            |(version, description, success, checksum)| RecordedMigration {
                version,
                description,
                success,
                checksum,
            },
        )
        .collect()
}

pub(crate) async fn schema_objects(connection: &mut SqliteConnection) -> Vec<(String, String)> {
    sqlx::query_as("SELECT type, name FROM sqlite_schema ORDER BY type, name")
        .fetch_all(&mut *connection)
        .await
        .unwrap()
}

fn embedded_records() -> Vec<RecordedMigration> {
    MIGRATOR
        .iter()
        .map(|migration| RecordedMigration {
            version: migration.version,
            description: migration.description.to_string(),
            success: true,
            checksum: migration.checksum.to_vec(),
        })
        .collect()
}

#[tokio::test]
async fn it_migrate_up_from_empty() {
    let files = embedded_files();
    assert_eq!(
        files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["0001_initial_schema.sql"]
    );
    assert_eq!(
        files[0].1,
        include_str!("../../../migrations/0001_initial_schema.sql")
    );

    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(pools.path(), data.path().join(DATABASE_FILE));

    let first = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        first,
        MigrationStatus {
            applied: 1,
            version: Some(1),
        }
    );
    let again = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        again,
        MigrationStatus {
            applied: 0,
            version: Some(1),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let reopened = open_pools(data.path()).await;
    assert_eq!(
        reopened.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 0,
            version: Some(1),
        }
    );
    reopened.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    assert_eq!(
        recorded_migrations(&mut connection).await,
        embedded_records()
    );
    assert_eq!(embedded_records()[0].description, "initial schema");
}
