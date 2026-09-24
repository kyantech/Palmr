use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

use super::pragmas::{connect_options, verify_connection};
use crate::config::SqliteSynchronous;

pub const STARTUP_DB_OPEN_FAILED: &str = "STARTUP_DB_OPEN_FAILED";
pub const DATABASE_FILE: &str = "palmr.db";

const WRITE_CONNECTIONS: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolRole {
    Write,
    Read,
}

impl PoolRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Read => "read",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DbPools {
    write: SqlitePool,
    read: SqlitePool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterCheck {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpoint {
    pub complete: bool,
    pub wal_frames: i64,
    pub checkpointed_frames: i64,
}

#[derive(Debug)]
pub struct DbShutdown {
    pub checkpoint: Result<WalCheckpoint, sqlx::Error>,
}

impl DbPools {
    pub async fn open(
        data_root: &Path,
        read_connections: u8,
        synchronous: SqliteSynchronous,
    ) -> Result<Self, DbOpenError> {
        let path = data_root.join(DATABASE_FILE);
        let options = connect_options(&path, synchronous);
        let failed = |cause| DbOpenError {
            path: path.clone(),
            cause,
        };

        let write = connect(PoolRole::Write, WRITE_CONNECTIONS, options.clone())
            .await
            .map_err(failed)?;
        let read = match connect(PoolRole::Read, u32::from(read_connections), options).await {
            Ok(read) => read,
            Err(cause) => {
                write.close().await;
                return Err(failed(cause));
            }
        };

        let pools = Self { write, read };
        if let Err(source) = pools.assert_fts5().await {
            pools.close().await;
            return Err(failed(DbOpenCause::Fts5Unavailable(source)));
        }
        Ok(pools)
    }

    pub const fn writer(&self) -> &SqlitePool {
        &self.write
    }

    pub const fn reader(&self) -> &SqlitePool {
        &self.read
    }

    pub async fn check_writer(&self, timeout: Duration) -> WriterCheck {
        let probe = sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&self.write);
        match tokio::time::timeout(timeout, probe).await {
            Ok(Ok(1)) => WriterCheck::Available,
            Ok(Ok(_) | Err(_)) | Err(_) => WriterCheck::Unavailable,
        }
    }

    pub async fn shutdown(self) -> DbShutdown {
        let checkpoint = self.checkpoint_wal().await;
        self.close().await;
        DbShutdown { checkpoint }
    }

    async fn assert_fts5(&self) -> Result<(), sqlx::Error> {
        sqlx::query_scalar::<_, String>("SELECT fts5_source_id()")
            .fetch_one(&self.write)
            .await
            .map(drop)
    }

    async fn checkpoint_wal(&self) -> Result<WalCheckpoint, sqlx::Error> {
        let (busy, wal_frames, checkpointed_frames): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(&self.write)
                .await?;
        Ok(WalCheckpoint {
            complete: busy == 0,
            wal_frames,
            checkpointed_frames,
        })
    }

    async fn close(&self) {
        self.read.close().await;
        self.write.close().await;
    }
}

async fn connect(
    role: PoolRole,
    max_connections: u32,
    options: SqliteConnectOptions,
) -> Result<SqlitePool, DbOpenCause> {
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .after_connect(|connection, _| Box::pin(verify_connection(connection)))
        .connect_with(options)
        .await
        .map_err(|source| DbOpenCause::Connect { pool: role, source })
}

#[derive(Debug)]
pub enum DbOpenCause {
    Connect { pool: PoolRole, source: sqlx::Error },
    Fts5Unavailable(sqlx::Error),
}

#[derive(Debug)]
pub struct DbOpenError {
    pub path: PathBuf,
    pub cause: DbOpenCause,
}

impl DbOpenError {
    pub const fn code(&self) -> &'static str {
        STARTUP_DB_OPEN_FAILED
    }

    pub const fn pool(&self) -> Option<PoolRole> {
        match self.cause {
            DbOpenCause::Connect { pool, .. } => Some(pool),
            DbOpenCause::Fts5Unavailable(_) => None,
        }
    }
}

impl fmt::Display for DbOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path.display();
        match &self.cause {
            DbOpenCause::Connect { pool, source } => write!(
                f,
                "{STARTUP_DB_OPEN_FAILED}: cannot open the {} connection pool for {path}: {source}. Check that the file is a Palmr v4 SQLite database on a local, writable filesystem",
                pool.as_str()
            ),
            DbOpenCause::Fts5Unavailable(source) => write!(
                f,
                "{STARTUP_DB_OPEN_FAILED}: the SQLite runtime opened for {path} does not provide FTS5 full-text search ({source}); this build of Palmr is unsupported"
            ),
        }
    }
}

impl std::error::Error for DbOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            DbOpenCause::Connect { source, .. } | DbOpenCause::Fts5Unavailable(source) => {
                Some(source)
            }
        }
    }
}
