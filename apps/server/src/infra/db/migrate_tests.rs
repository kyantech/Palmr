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
        [
            "0001_initial_schema.sql",
            "0002_add_identity_provider_email_linking.sql"
        ]
    );
    assert_eq!(
        files[0].1,
        include_str!("../../../migrations/0001_initial_schema.sql")
    );
    assert_eq!(
        files[1].1,
        include_str!("../../../migrations/0002_add_identity_provider_email_linking.sql")
    );

    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(pools.path(), data.path().join(DATABASE_FILE));

    let first = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        first,
        MigrationStatus {
            applied: 2,
            version: Some(2),
        }
    );
    let again = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        again,
        MigrationStatus {
            applied: 0,
            version: Some(2),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let reopened = open_pools(data.path()).await;
    assert_eq!(
        reopened.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 0,
            version: Some(2),
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

const PROVIDER_ROWS: [(&str, &str); 3] = [
    ("0192f3a1-0000-7000-8000-000000000001", "oidc"),
    ("0192f3a1-0000-7000-8000-000000000002", "oauth2"),
    ("0192f3a1-0000-7000-8000-000000000003", "oidc"),
];

#[tokio::test]
async fn it_migrate_from_0001_derives_email_linking_by_protocol() {
    let files = embedded_files();
    let (_directory, frozen) = fixture_migrator(&files[..1]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&frozen).await.unwrap(),
        MigrationStatus {
            applied: 1,
            version: Some(1),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    for (id, kind) in PROVIDER_ROWS {
        let endpoints = if kind == "oauth2" {
            "'https://idp.example.test/authorize', 'https://idp.example.test/token'"
        } else {
            "NULL, NULL"
        };
        sqlx::query(&format!(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, authorization_endpoint, token_endpoint,
                  client_id, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, {endpoints}, 'client', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .bind(id)
        .bind(kind)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 1,
            version: Some(2),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let migrated: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT id, kind, allow_email_linking FROM identity_providers ORDER BY id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        migrated,
        PROVIDER_ROWS
            .iter()
            .map(|(id, kind)| (
                (*id).to_owned(),
                (*kind).to_owned(),
                i64::from(*kind == "oidc")
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        recorded_migrations(&mut connection)
            .await
            .iter()
            .map(|record| (record.version, record.success))
            .collect::<Vec<_>>(),
        [(1, true), (2, true)]
    );
    connection.close().await.unwrap();
}

#[tokio::test]
async fn it_migrate_email_linking_rejects_non_boolean_values() {
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&MIGRATOR).await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    for (index, value) in ["2", "-1", "NULL"].into_iter().enumerate() {
        let id = format!("0192f3a1-0000-7000-8000-00000000010{index}");
        let outcome = sqlx::query(&format!(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, client_id, allow_email_linking, created_at, updated_at)
             VALUES (?1, ?1, ?1, 'oidc', 'client', {value}, '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .bind(id)
        .execute(&mut connection)
        .await;
        assert!(
            outcome.is_err(),
            "allow_email_linking = {value} was accepted"
        );
    }
    sqlx::query(
        "INSERT INTO identity_providers
             (id, key, display_name, kind, client_id, created_at, updated_at)
         VALUES ('0192f3a1-0000-7000-8000-000000000200', 'defaulted', 'defaulted', 'oidc',
                 'client', '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let stored: i64 = sqlx::query_scalar(
        "SELECT allow_email_linking FROM identity_providers WHERE key = 'defaulted'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(stored, 0, "an unspecified policy must fail closed");
    connection.close().await.unwrap();
}
