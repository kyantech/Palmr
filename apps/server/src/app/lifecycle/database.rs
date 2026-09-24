use std::path::Path;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant, MissedTickBehavior};

use sqlx::migrate::Migrator;

use crate::app::health::{DatabaseState, Health, MigrationState};
use crate::config::{OperatorConfig, SqliteSynchronous};
use crate::infra::db::{DbOpenError, DbPools, DbShutdown, MigrationError, WriterCheck};

pub const WRITER_CHECK_INTERVAL: Duration = Duration::from_secs(5);
pub const WRITER_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Database {
    pools: DbPools,
    monitor: JoinHandle<()>,
}

impl Database {
    pub async fn open(
        config: &OperatorConfig,
        data_root: &Path,
        health: &Health,
    ) -> Result<Self, DbOpenError> {
        let pools =
            DbPools::open(data_root, config.db_read_connections, config.db_synchronous).await?;
        tracing::debug!(
            read_connections = config.db_read_connections,
            synchronous = synchronous_name(config),
            "database pools opened"
        );
        publish_writer_state(&pools, health).await;
        let monitor = tokio::spawn(monitor_writer(pools.clone(), health.clone()));
        Ok(Self { pools, monitor })
    }

    pub async fn migrate(
        &self,
        migrator: &Migrator,
        health: &Health,
    ) -> Result<(), MigrationError> {
        health.checks().set_migrations(MigrationState::Pending);
        let status = self.pools.migrate(migrator).await?;
        health.checks().set_migrations(MigrationState::Current);
        tracing::info!(
            applied = status.applied,
            schema_version = status.version,
            "database migrations current"
        );
        Ok(())
    }

    pub const fn pools(&self) -> &DbPools {
        &self.pools
    }

    pub async fn close(self) -> DbShutdown {
        self.monitor.abort();
        let _ = self.monitor.await;
        self.pools.shutdown().await
    }
}

const fn synchronous_name(config: &OperatorConfig) -> &'static str {
    match config.db_synchronous {
        SqliteSynchronous::Full => "full",
        SqliteSynchronous::Normal => "normal",
    }
}

async fn publish_writer_state(pools: &DbPools, health: &Health) {
    let state = match pools.check_writer(WRITER_CHECK_TIMEOUT).await {
        WriterCheck::Available => DatabaseState::Writable,
        WriterCheck::Unavailable => DatabaseState::Unavailable,
    };
    health.checks().set_database(state);
}

async fn monitor_writer(pools: DbPools, health: Health) {
    let mut ticks = interval_at(
        Instant::now() + WRITER_CHECK_INTERVAL,
        WRITER_CHECK_INTERVAL,
    );
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        publish_writer_state(&pools, &health).await;
    }
}

pub fn log_database_closed(shutdown: &DbShutdown) {
    match &shutdown.checkpoint {
        Ok(checkpoint) if checkpoint.complete => tracing::debug!(
            wal_frames = checkpoint.wal_frames,
            checkpointed_frames = checkpoint.checkpointed_frames,
            "database closed after a WAL checkpoint"
        ),
        Ok(checkpoint) => tracing::warn!(
            wal_frames = checkpoint.wal_frames,
            checkpointed_frames = checkpoint.checkpointed_frames,
            "the WAL checkpoint could not complete before the database closed; SQLite replays the remaining WAL on the next start"
        ),
        Err(error) => tracing::warn!(
            error = %error,
            "the WAL checkpoint failed before the database closed; SQLite replays the remaining WAL on the next start"
        ),
    }
}
