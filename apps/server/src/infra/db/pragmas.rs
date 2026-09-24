use std::fmt;
use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::SqliteConnection;

use crate::config::SqliteSynchronous;

pub const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
const TEMP_STORE: &str = "FILE";
const CACHE_SIZE_KIB: &str = "-16000";
const WAL_AUTOCHECKPOINT_PAGES: &str = "1000";

pub fn connect_options(path: &Path, synchronous: SqliteSynchronous) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .synchronous(driver_synchronous(synchronous))
        .pragma("temp_store", TEMP_STORE)
        .pragma("cache_size", CACHE_SIZE_KIB)
        .pragma("wal_autocheckpoint", WAL_AUTOCHECKPOINT_PAGES)
}

const fn driver_synchronous(synchronous: SqliteSynchronous) -> sqlx::sqlite::SqliteSynchronous {
    match synchronous {
        SqliteSynchronous::Full => sqlx::sqlite::SqliteSynchronous::Full,
        SqliteSynchronous::Normal => sqlx::sqlite::SqliteSynchronous::Normal,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionSetupError {
    ForeignKeysNotEnforced,
    JournalModeNotWal { observed: String },
}

impl fmt::Display for ConnectionSetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignKeysNotEnforced => {
                f.write_str("SQLite refused to enable foreign key enforcement on a new connection")
            }
            Self::JournalModeNotWal { observed } => write!(
                f,
                "SQLite kept journal mode {observed:?} instead of WAL; the database must live on a local filesystem that supports shared memory"
            ),
        }
    }
}

impl std::error::Error for ConnectionSetupError {}

pub async fn verify_connection(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?;
    if foreign_keys != 1 {
        return Err(setup_failed(ConnectionSetupError::ForeignKeysNotEnforced));
    }

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(setup_failed(ConnectionSetupError::JournalModeNotWal {
            observed: journal_mode,
        }));
    }
    Ok(())
}

fn setup_failed(error: ConnectionSetupError) -> sqlx::Error {
    sqlx::Error::Configuration(Box::new(error))
}
