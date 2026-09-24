use std::fs;
use std::path::Path;

use sqlx::pool::PoolConnection;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Sqlite, SqliteConnection, SqlitePool};
use tempfile::TempDir;

use super::{DbPools, DATABASE_FILE};
use crate::config::{EnvironmentSource, OperatorConfig, SqliteSynchronous};

const SYNCHRONOUS_FULL: i64 = 2;
const SYNCHRONOUS_NORMAL: i64 = 1;
const TEMP_STORE_FILE: i64 = 1;

#[derive(Debug, PartialEq, Eq)]
struct ObservedPragmas {
    journal_mode: String,
    foreign_keys: i64,
    busy_timeout: i64,
    synchronous: i64,
    temp_store: i64,
    cache_size: i64,
    wal_autocheckpoint: i64,
}

impl ObservedPragmas {
    fn canonical(synchronous: i64) -> Self {
        Self {
            journal_mode: "wal".to_owned(),
            foreign_keys: 1,
            busy_timeout: 5000,
            synchronous,
            temp_store: TEMP_STORE_FILE,
            cache_size: -16000,
            wal_autocheckpoint: 1000,
        }
    }

    async fn read(connection: &mut SqliteConnection) -> Self {
        Self {
            journal_mode: pragma(connection, "journal_mode").await,
            foreign_keys: pragma(connection, "foreign_keys").await,
            busy_timeout: pragma(connection, "busy_timeout").await,
            synchronous: pragma(connection, "synchronous").await,
            temp_store: pragma(connection, "temp_store").await,
            cache_size: pragma(connection, "cache_size").await,
            wal_autocheckpoint: pragma(connection, "wal_autocheckpoint").await,
        }
    }
}

async fn pragma<T>(connection: &mut SqliteConnection, name: &str) -> T
where
    T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite> + Send + Unpin,
{
    sqlx::query_scalar(&format!("PRAGMA {name}"))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

fn config(vars: &[(&str, &str)]) -> OperatorConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
}

async fn open(root: &Path, config: &OperatorConfig) -> DbPools {
    DbPools::open(root, config.db_read_connections, config.db_synchronous)
        .await
        .unwrap()
}

async fn hold_every_connection(pool: &SqlitePool) -> Vec<PoolConnection<Sqlite>> {
    let mut held = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        held.push(pool.acquire().await.unwrap());
    }
    held
}

async fn replace_every_connection(held: Vec<PoolConnection<Sqlite>>) {
    for connection in held {
        connection.close().await.unwrap();
    }
}

fn is_foreign_key_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|error| error.is_foreign_key_violation())
}

async fn assert_foreign_keys_enforced_in_temp_schema(connection: &mut SqliteConnection) {
    assert_eq!(pragma::<i64>(connection, "foreign_keys").await, 1);
    sqlx::raw_sql(
        "CREATE TEMP TABLE fk_parent(id INTEGER PRIMARY KEY);
         CREATE TEMP TABLE fk_child(parent_id INTEGER NOT NULL REFERENCES fk_parent(id));",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    let orphan = sqlx::query("INSERT INTO fk_child(parent_id) VALUES (42)")
        .execute(&mut *connection)
        .await
        .unwrap_err();
    assert!(is_foreign_key_violation(&orphan), "{orphan}");
}

async fn assert_foreign_keys_enforced_in_main_schema(connection: &mut SqliteConnection) {
    assert_eq!(pragma::<i64>(connection, "foreign_keys").await, 1);
    let orphan = sqlx::query("INSERT INTO fk_child(parent_id) VALUES (42)")
        .execute(&mut *connection)
        .await
        .unwrap_err();
    assert!(is_foreign_key_violation(&orphan), "{orphan}");
}

#[tokio::test]
async fn it_sqlite_runtime_defaults() {
    let root = TempDir::new().unwrap();
    let defaults = config(&[]);
    assert_eq!(defaults.db_synchronous, SqliteSynchronous::Full);
    let pools = open(root.path(), &defaults).await;
    assert!(root.path().join(DATABASE_FILE).is_file());
    assert_eq!(pools.writer().options().get_max_connections(), 1);
    assert_eq!(pools.reader().executor().options().get_max_connections(), 4);

    let mut writers = hold_every_connection(pools.writer()).await;
    let mut readers = hold_every_connection(pools.reader().executor()).await;
    assert_eq!((writers.len(), readers.len()), (1, 4));
    for connection in writers.iter_mut().chain(readers.iter_mut()) {
        assert_eq!(
            ObservedPragmas::read(connection).await,
            ObservedPragmas::canonical(SYNCHRONOUS_FULL)
        );
    }
    drop((writers, readers));
    pools.shutdown().await.checkpoint.unwrap();

    let root = TempDir::new().unwrap();
    let explicit = config(&[
        ("PALMR_DB_SYNCHRONOUS", "normal"),
        ("PALMR_DB_READ_CONNECTIONS", "7"),
    ]);
    let pools = open(root.path(), &explicit).await;
    assert_eq!(pools.writer().options().get_max_connections(), 1);
    assert_eq!(pools.reader().executor().options().get_max_connections(), 7);

    let mut writers = hold_every_connection(pools.writer()).await;
    let mut readers = hold_every_connection(pools.reader().executor()).await;
    assert_eq!((writers.len(), readers.len()), (1, 7));
    for connection in writers.iter_mut().chain(readers.iter_mut()) {
        assert_eq!(
            ObservedPragmas::read(connection).await,
            ObservedPragmas::canonical(SYNCHRONOUS_NORMAL)
        );
    }
    drop((writers, readers));
    pools.shutdown().await.checkpoint.unwrap();
}

#[tokio::test]
async fn it_foreign_keys_on_every_connection() {
    let root = TempDir::new().unwrap();
    let pools = open(root.path(), &config(&[("PALMR_DB_READ_CONNECTIONS", "3")])).await;
    sqlx::raw_sql(
        "CREATE TABLE fk_parent(id INTEGER PRIMARY KEY);
         CREATE TABLE fk_child(parent_id INTEGER NOT NULL REFERENCES fk_parent(id));",
    )
    .execute(pools.writer())
    .await
    .unwrap();

    for _generation in 0..2 {
        let mut writers = hold_every_connection(pools.writer()).await;
        let mut readers = hold_every_connection(pools.reader().executor()).await;
        assert_eq!((writers.len(), readers.len()), (1, 3));
        for connection in &mut writers {
            assert_foreign_keys_enforced_in_main_schema(connection).await;
        }
        for connection in &mut readers {
            assert_foreign_keys_enforced_in_temp_schema(connection).await;
        }
        replace_every_connection(writers).await;
        replace_every_connection(readers).await;
        assert_eq!(
            (pools.writer().size(), pools.reader().executor().size()),
            (0, 0)
        );
    }

    let mut writer = pools.writer().acquire().await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO fk_parent(id) VALUES (1);
         INSERT INTO fk_child(parent_id) VALUES (1);",
    )
    .execute(&mut *writer)
    .await
    .unwrap();
    let blocked = sqlx::query("DELETE FROM fk_parent WHERE id = 1")
        .execute(&mut *writer)
        .await
        .unwrap_err();
    assert!(is_foreign_key_violation(&blocked), "{blocked}");
    drop(writer);
    pools.shutdown().await.checkpoint.unwrap();
}

#[tokio::test]
async fn it_fts5_available() {
    let root = TempDir::new().unwrap();
    let pools = open(root.path(), &config(&[])).await;

    let source_id: String = sqlx::query_scalar("SELECT fts5_source_id()")
        .fetch_one(pools.reader().executor())
        .await
        .unwrap();
    assert!(!source_id.trim().is_empty());

    let mut connection = pools.reader().executor().acquire().await.unwrap();
    sqlx::raw_sql(
        "CREATE VIRTUAL TABLE temp.search_probe USING fts5(
             name,
             description,
             tokenize = 'unicode61 remove_diacritics 2',
             prefix = '2 3 4'
         );
         INSERT INTO temp.search_probe(name, description)
             VALUES ('Férias 2026.pdf', 'Relatório anual'), ('notes.txt', NULL);",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    for (query, expected) in [
        ("ferias", 1_i64),
        ("relat*", 1),
        ("notes", 1),
        ("absent", 0),
    ] {
        let hits: i64 =
            sqlx::query_scalar("SELECT count(*) FROM temp.search_probe WHERE search_probe MATCH ?")
                .bind(query)
                .fetch_one(&mut *connection)
                .await
                .unwrap();
        assert_eq!(hits, expected, "{query}");
    }
    drop(connection);

    let main_schema_objects: i64 = sqlx::query_scalar("SELECT count(*) FROM main.sqlite_master")
        .fetch_one(pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(main_schema_objects, 0);
    pools.shutdown().await.checkpoint.unwrap();
}

#[tokio::test]
async fn it_shutdown_checkpoints_wal() {
    const ROWS: i64 = 200;

    let root = TempDir::new().unwrap();
    let database = root.path().join(DATABASE_FILE);
    let wal = root.path().join(format!("{DATABASE_FILE}-wal"));
    let pools = open(root.path(), &config(&[("PALMR_DB_READ_CONNECTIONS", "2")])).await;

    sqlx::raw_sql("CREATE TABLE wal_activity(id INTEGER PRIMARY KEY, payload BLOB NOT NULL)")
        .execute(pools.writer())
        .await
        .unwrap();
    for _ in 0..ROWS {
        sqlx::query("INSERT INTO wal_activity(payload) VALUES (zeroblob(2048))")
            .execute(pools.writer())
            .await
            .unwrap();
    }
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM wal_activity")
        .fetch_one(pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(visible, ROWS);

    let mut bystander =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&database))
            .await
            .unwrap();
    let observed: i64 = sqlx::query_scalar("SELECT count(*) FROM wal_activity")
        .fetch_one(&mut bystander)
        .await
        .unwrap();
    assert_eq!(observed, ROWS);
    let wal_before = fs::metadata(&wal).unwrap().len();
    assert!(wal_before > 0);

    let writer = pools.writer().clone();
    let reader = pools.reader().executor().clone();
    let shutdown = pools.shutdown().await;
    let checkpoint = shutdown.checkpoint.unwrap();

    assert!(checkpoint.complete, "{checkpoint:?}");
    assert_eq!(
        checkpoint.checkpointed_frames, checkpoint.wal_frames,
        "{checkpoint:?}"
    );
    assert!(writer.is_closed());
    assert!(reader.is_closed());
    assert_eq!(fs::metadata(&wal).unwrap().len(), 0);

    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM wal_activity")
        .fetch_one(&mut bystander)
        .await
        .unwrap();
    assert_eq!(persisted, ROWS);
    bystander.close().await.unwrap();
}
