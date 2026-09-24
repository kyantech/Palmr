use std::time::Duration;

use sqlx::{Connection, Sqlite, SqliteConnection, SqlitePool, Transaction};
use tracing::Instrument;

use super::error::DbError;
use super::DbPools;
use crate::domain::clock::Clock;

pub const SLOW_WRITE_TX_THRESHOLD: Duration = Duration::from_millis(250);
const BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

#[derive(Debug, Clone)]
pub struct ReadPool(SqlitePool);

impl ReadPool {
    pub(super) const fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }

    pub const fn executor(&self) -> &SqlitePool {
        &self.0
    }
}

#[derive(Debug)]
pub struct WriteTx<'c> {
    tx: Transaction<'c, Sqlite>,
}

impl WriteTx<'_> {
    pub fn executor(&mut self) -> &mut SqliteConnection {
        &mut self.tx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Committed,
    RolledBack,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl DbPools {
    pub async fn write_tx<T, E, F>(
        &self,
        clock: &dyn Clock,
        name: &'static str,
        work: F,
    ) -> Result<T, E>
    where
        F: AsyncFnOnce(&mut WriteTx<'_>) -> Result<T, E>,
        E: From<DbError>,
    {
        run_write_tx(self.writer(), clock, name, work)
            .instrument(tracing::debug_span!("write_tx", transaction = name))
            .await
    }
}

async fn run_write_tx<T, E, F>(
    pool: &SqlitePool,
    clock: &dyn Clock,
    name: &'static str,
    work: F,
) -> Result<T, E>
where
    F: AsyncFnOnce(&mut WriteTx<'_>) -> Result<T, E>,
    E: From<DbError>,
{
    let queued = clock.monotonic();
    let mut connection = pool.acquire().await.map_err(DbError::from)?;
    let pool_wait = clock.monotonic().saturating_duration_since(queued);

    let finished = run_immediate(&mut connection, clock, name, work).await?;
    if finished.discard_connection {
        connection.close_on_drop();
    }
    report(name, pool_wait, finished.elapsed, finished.outcome);
    finished.result
}

struct Finished<T, E> {
    outcome: Outcome,
    elapsed: Duration,
    discard_connection: bool,
    result: Result<T, E>,
}

async fn run_immediate<T, E, F>(
    connection: &mut SqliteConnection,
    clock: &dyn Clock,
    name: &'static str,
    work: F,
) -> Result<Finished<T, E>, DbError>
where
    F: AsyncFnOnce(&mut WriteTx<'_>) -> Result<T, E>,
    E: From<DbError>,
{
    let mut tx = WriteTx {
        tx: connection.begin_with(BEGIN_IMMEDIATE).await?,
    };
    let started = clock.monotonic();
    let work_result = work(&mut tx).await;

    let (outcome, result, discard_connection) = match work_result {
        Ok(value) => match tx.tx.commit().await {
            Ok(()) => (Outcome::Committed, Ok(value), false),
            Err(source) => (
                Outcome::RolledBack,
                Err(E::from(DbError::from(source))),
                false,
            ),
        },
        Err(error) => match tx.tx.rollback().await {
            Ok(()) => (Outcome::RolledBack, Err(error), false),
            Err(source) => {
                tracing::warn!(
                    transaction = name,
                    error_kind = DbError::from(source).kind().as_str(),
                    "write transaction rollback failed; discarding its connection"
                );
                (Outcome::RolledBack, Err(error), true)
            }
        },
    };

    Ok(Finished {
        outcome,
        elapsed: clock.monotonic().saturating_duration_since(started),
        discard_connection,
        result,
    })
}

fn report(name: &'static str, pool_wait: Duration, elapsed: Duration, outcome: Outcome) {
    let pool_wait_ms = millis(pool_wait);
    let elapsed_ms = millis(elapsed);
    if elapsed > SLOW_WRITE_TX_THRESHOLD {
        tracing::warn!(
            transaction = name,
            elapsed_ms,
            pool_wait_ms,
            threshold_ms = millis(SLOW_WRITE_TX_THRESHOLD),
            outcome = outcome.as_str(),
            "write transaction exceeded its duration budget"
        );
    } else {
        tracing::debug!(
            transaction = name,
            elapsed_ms,
            pool_wait_ms,
            outcome = outcome.as_str(),
            "write transaction finished"
        );
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
