use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use sqlx::migrate::Migrator;
use sqlx::Connection;
use tempfile::TempDir;
use time::macros::datetime;
use tokio::net::TcpStream;

use super::database::Database;
use super::{Application, Readiness, StartupError, EX_CONFIG};
use crate::app::health::{Health, MigrationState};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::infra::db::migrate_tests::{
    embedded_files, fixture_migrator, open_pools, raw_connection, recorded_migrations,
    schema_objects,
};
use crate::infra::db::{
    DB_SCHEMA_AHEAD_OF_BINARY, MIGRATOR, STARTUP_MIGRATION_CHECKSUM_MISMATCH,
    STARTUP_MIGRATION_FAILED,
};
use crate::infra::telemetry::write_startup_failure;

fn config(data_root: &Path, port: u16) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_HOST", "127.0.0.1"),
        ("PALMR_PORT", port.to_string().as_str()),
        ("PALMR_DATA_DIR", data_root.to_str().unwrap()),
    ]))
    .unwrap()
    .config
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn startup_failure(data_root: &Path, migrator: &Migrator) -> StartupError {
    let port = unused_port();
    let clock = Arc::new(TestClock::new(datetime!(2026-09-23 12:00 UTC)));
    let error = Application::bind_with(&config(data_root, port), clock, migrator)
        .await
        .err()
        .unwrap();
    assert!(
        TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .await
            .is_err(),
        "the listener must never be bound when migrations fail"
    );
    assert_eq!(error.exit_code(), EX_CONFIG);
    assert!(matches!(error, StartupError::Migration(_)), "{error}");
    error
}

async fn seed(data_root: &Path, migrator: &Migrator) {
    let pools = open_pools(data_root).await;
    pools.migrate(migrator).await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();
}

fn with_file(mut files: Vec<(String, String)>, name: &str, sql: &str) -> Vec<(String, String)> {
    files.push((name.to_owned(), sql.to_owned()));
    files
}

fn failure_line(error: &StartupError) -> String {
    let mut out = Vec::new();
    write_startup_failure(&mut out, error).unwrap();
    String::from_utf8(out).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R093_versioned_migrations_gate_startup() {
    let (_broken_dir, broken) = fixture_migrator(&with_file(
        embedded_files(),
        "0002_partial_then_broken.sql",
        "CREATE TABLE r093_partial (id INTEGER PRIMARY KEY);\n\
         INSERT INTO r093_partial (id) VALUES (1);\n\
         INSERT INTO r093_missing_table (id) VALUES (1);\n",
    ))
    .await;

    let fresh = TempDir::new().unwrap();
    let error = startup_failure(fresh.path(), &broken).await;
    assert_eq!(error.code(), Some(STARTUP_MIGRATION_FAILED));
    let text = error.to_string();
    assert!(
        text.starts_with("STARTUP_MIGRATION_FAILED: schema migration 2 (partial then broken)"),
        "{text}"
    );
    assert!(text.contains("r093_missing_table"), "{text}");
    assert!(failure_line(&error).starts_with("FATAL: STARTUP_MIGRATION_FAILED: "));
    let mut connection = raw_connection(fresh.path()).await;
    let recorded: Vec<i64> = recorded_migrations(&mut connection)
        .await
        .into_iter()
        .map(|migration| migration.version)
        .collect();
    assert_eq!(recorded, [1]);
    assert!(!schema_objects(&mut connection)
        .await
        .iter()
        .any(|(_, name)| name == "r093_partial"));
    connection.close().await.unwrap();

    let current = TempDir::new().unwrap();
    seed(current.path(), &MIGRATOR).await;
    let mut connection = raw_connection(current.path()).await;
    let schema_before = schema_objects(&mut connection).await;
    let recorded_before = recorded_migrations(&mut connection).await;
    connection.close().await.unwrap();
    let error = startup_failure(current.path(), &broken).await;
    assert_eq!(error.code(), Some(STARTUP_MIGRATION_FAILED));
    let mut connection = raw_connection(current.path()).await;
    assert_eq!(schema_objects(&mut connection).await, schema_before);
    assert_eq!(recorded_migrations(&mut connection).await, recorded_before);
    connection.close().await.unwrap();

    let readiness = Readiness::new();
    let health = Health::new(readiness.clone());
    let pending = TempDir::new().unwrap();
    let database = Database::open(&config(pending.path(), 5487), pending.path(), &health)
        .await
        .unwrap();
    assert!(database.migrate(&broken, &health).await.is_err());
    assert_eq!(
        health.checks().snapshot().migrations,
        Some(MigrationState::Pending)
    );
    assert!(!readiness.is_ready());
    database.migrate(&MIGRATOR, &health).await.unwrap();
    assert_eq!(
        health.checks().snapshot().migrations,
        Some(MigrationState::Current)
    );
    database.close().await.checkpoint.unwrap();

    let (_edited_dir, edited) = fixture_migrator(&[(
        "0001_initial_schema.sql".to_owned(),
        "-- Palmr v4.0.0 initial schema, edited after release\n".to_owned(),
    )])
    .await;
    let tampered = TempDir::new().unwrap();
    seed(tampered.path(), &edited).await;
    let mut connection = raw_connection(tampered.path()).await;
    let recorded_before = recorded_migrations(&mut connection).await;
    connection.close().await.unwrap();
    assert_ne!(
        recorded_before[0].checksum,
        MIGRATOR.iter().next().unwrap().checksum.to_vec()
    );
    let error = startup_failure(tampered.path(), &MIGRATOR).await;
    assert_eq!(error.code(), Some(STARTUP_MIGRATION_CHECKSUM_MISMATCH));
    assert!(
        error
            .to_string()
            .starts_with("STARTUP_MIGRATION_CHECKSUM_MISMATCH: schema migration 1 recorded in "),
        "{error}"
    );
    let mut connection = raw_connection(tampered.path()).await;
    assert_eq!(recorded_migrations(&mut connection).await, recorded_before);
    connection.close().await.unwrap();

    let (_future_dir, future) = fixture_migrator(&with_file(
        embedded_files(),
        "0002_future_release.sql",
        "CREATE TABLE r093_future (id INTEGER PRIMARY KEY);\n",
    ))
    .await;
    let ahead = TempDir::new().unwrap();
    seed(ahead.path(), &future).await;
    let error = startup_failure(ahead.path(), &MIGRATOR).await;
    assert_eq!(error.code(), Some(DB_SCHEMA_AHEAD_OF_BINARY));
    let text = error.to_string();
    assert!(text.starts_with("DB_SCHEMA_AHEAD_OF_BINARY: "), "{text}");
    assert!(
        text.contains("records schema migration 2, which this Palmr binary does not contain (its latest is 1)"),
        "{text}"
    );
    let mut connection = raw_connection(ahead.path()).await;
    let recorded: Vec<i64> = recorded_migrations(&mut connection)
        .await
        .into_iter()
        .map(|migration| migration.version)
        .collect();
    assert_eq!(recorded, [1, 2]);
    assert!(schema_objects(&mut connection)
        .await
        .iter()
        .any(|(_, name)| name == "r093_future"));
    connection.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn it_foreign_database_aborts_with_clean_install_message() {
    let data = TempDir::new().unwrap();
    let mut connection = raw_connection(data.path()).await;
    sqlx::raw_sql(
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL);\n\
         INSERT INTO notes (body) VALUES ('left exactly as found');\n",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let schema_before = schema_objects(&mut connection).await;
    connection.close().await.unwrap();

    let error = startup_failure(data.path(), &MIGRATOR).await;

    assert_eq!(error.code(), Some(STARTUP_MIGRATION_FAILED));
    let line = failure_line(&error);
    assert!(
        line.starts_with("FATAL: STARTUP_MIGRATION_FAILED: the data directory contains a database Palmr v4 does not recognize ("),
        "{line}"
    );
    assert!(
        line.contains(
            "Palmr v4 does not upgrade Palmr v3 data; point PALMR_DATA_DIR at an empty directory"
        ),
        "{line}"
    );
    assert!(!line.contains("ADR"), "{line}");
    assert!(!line.contains("SPEC"), "{line}");

    let mut connection = raw_connection(data.path()).await;
    assert_eq!(schema_objects(&mut connection).await, schema_before);
    let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM notes")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(bodies, ["left exactly as found"]);
    connection.close().await.unwrap();
}
