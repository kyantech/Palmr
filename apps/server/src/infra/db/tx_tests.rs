use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;
use time::macros::datetime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use super::{
    DbError, DbErrorKind, DbPools, ReadPool, WriteTx, DATABASE_FILE, SLOW_WRITE_TX_THRESHOLD,
};
use crate::config::{LogFormat, SqliteSynchronous};
use crate::domain::clock::{Clock, SystemClock, TestClock};
use crate::infra::telemetry::build_dispatch;

const NOT_ENTERED_WITHIN: Duration = Duration::from_millis(100);
const BURST_WRITERS: i64 = 32;
const BURST_INCREMENTS: i64 = 10;

async fn open(root: &TempDir) -> DbPools {
    DbPools::open(root.path(), 4, SqliteSynchronous::Full)
        .await
        .unwrap()
}

async fn create_ledger(pools: &DbPools, clock: &dyn Clock) {
    pools
        .write_tx(clock, "test.create_ledger", async |tx| {
            sqlx::raw_sql(
                "CREATE TABLE counter(id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL);
                 INSERT INTO counter(id, value) VALUES (1, 0);
                 CREATE TABLE commits(seq INTEGER PRIMARY KEY AUTOINCREMENT, writer TEXT NOT NULL);",
            )
            .execute(tx.executor())
            .await?;
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
}

async fn read_counter(tx: &mut WriteTx<'_>) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT value FROM counter WHERE id = 1")
        .fetch_one(tx.executor())
        .await?)
}

async fn counter_snapshot(db: &ReadPool) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT value FROM counter WHERE id = 1")
        .fetch_one(db.executor())
        .await?)
}

async fn commit_increment(
    tx: &mut WriteTx<'_>,
    observed: i64,
    writer: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE counter SET value = ? WHERE id = 1")
        .bind(observed + 1)
        .execute(tx.executor())
        .await?;
    sqlx::query("INSERT INTO commits(writer) VALUES (?)")
        .bind(writer)
        .execute(tx.executor())
        .await?;
    Ok(())
}

async fn external_connection(root: &TempDir) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(root.path().join(DATABASE_FILE))
            .busy_timeout(Duration::ZERO),
    )
    .await
    .unwrap()
}

fn spawn_queued_writer(
    pools: &DbPools,
    writer: &'static str,
    entered: mpsc::UnboundedSender<&'static str>,
) -> tokio::task::JoinHandle<Result<(), DbError>> {
    let pools = pools.clone();
    tokio::spawn(async move {
        pools
            .write_tx(&SystemClock, "test.queued_increment", async |tx| {
                entered.send(writer).unwrap();
                let observed = read_counter(tx).await?;
                commit_increment(tx, observed, writer).await
            })
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_concurrent_writes_serialize_without_busy() {
    let root = TempDir::new().unwrap();
    let pools = open(&root).await;
    create_ledger(&pools, &SystemClock).await;

    let (holding, held) = oneshot::channel();
    let (release, released) = oneshot::channel::<()>();
    let first = {
        let pools = pools.clone();
        tokio::spawn(async move {
            pools
                .write_tx(&SystemClock, "test.holding_increment", async |tx| {
                    let observed = read_counter(tx).await?;
                    holding.send(observed).unwrap();
                    released.await.unwrap();
                    commit_increment(tx, observed, "first").await
                })
                .await
        })
    };
    assert_eq!(held.await.unwrap(), 0);

    let mut external = external_connection(&root).await;
    let refused = sqlx::raw_sql("BEGIN IMMEDIATE")
        .execute(&mut external)
        .await
        .unwrap_err();
    assert_eq!(DbError::from(refused).kind(), DbErrorKind::Busy);
    assert_eq!(counter_snapshot(pools.reader()).await.unwrap(), 0);

    let (entered, mut entries) = mpsc::unbounded_channel();
    let second = spawn_queued_writer(&pools, "second", entered.clone());
    assert!(tokio::time::timeout(NOT_ENTERED_WITHIN, entries.recv())
        .await
        .is_err());
    let third = spawn_queued_writer(&pools, "third", entered);
    assert!(tokio::time::timeout(NOT_ENTERED_WITHIN, entries.recv())
        .await
        .is_err());

    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    third.await.unwrap().unwrap();
    assert_eq!(entries.recv().await, Some("second"));
    assert_eq!(entries.recv().await, Some("third"));

    let order: Vec<String> = sqlx::query_scalar("SELECT writer FROM commits ORDER BY seq")
        .fetch_all(pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(order, ["first", "second", "third"]);

    let mut burst = JoinSet::new();
    for _ in 0..BURST_WRITERS {
        let pools = pools.clone();
        burst.spawn(async move {
            for _ in 0..BURST_INCREMENTS {
                pools
                    .write_tx(&SystemClock, "test.burst_increment", async |tx| {
                        let observed = read_counter(tx).await?;
                        tokio::task::yield_now().await;
                        commit_increment(tx, observed, "burst").await
                    })
                    .await?;
            }
            Ok::<_, DbError>(())
        });
    }
    while let Some(outcome) = burst.join_next().await {
        outcome.unwrap().unwrap();
    }

    let value = counter_snapshot(pools.reader()).await.unwrap();
    assert_eq!(value, 3 + BURST_WRITERS * BURST_INCREMENTS);
    let commits: i64 = sqlx::query_scalar("SELECT count(*) FROM commits")
        .fetch_one(pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(commits, value);

    external.close().await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn json_lines(&self) -> Vec<Map<String, Value>> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn it_slow_write_tx_warns() {
    let root = TempDir::new().unwrap();
    let pools = open(&root).await;
    let clock = TestClock::new(datetime!(2026-09-23 12:00 UTC));
    create_ledger(&pools, &clock).await;

    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("info"),
        LogFormat::Json,
        output.clone(),
        SystemTime,
        false,
    );
    let guard = tracing::dispatcher::set_default(&dispatch);

    pools
        .write_tx(&clock, "test.within_budget", async |tx| {
            let observed = read_counter(tx).await?;
            clock.advance(SLOW_WRITE_TX_THRESHOLD);
            commit_increment(tx, observed, "within_budget").await
        })
        .await
        .unwrap();

    pools
        .write_tx(&clock, "test.over_budget", async |tx| {
            let observed = read_counter(tx).await?;
            clock.advance(SLOW_WRITE_TX_THRESHOLD + Duration::from_millis(1));
            commit_increment(tx, observed, "over_budget").await
        })
        .await
        .unwrap();

    let failed = pools
        .write_tx(&clock, "test.over_budget_rolled_back", async |tx| {
            let observed = read_counter(tx).await?;
            commit_increment(tx, observed, "rolled_back").await?;
            clock.advance(Duration::from_millis(400));
            sqlx::query("INSERT INTO counter(id, value) VALUES (2, 0)")
                .execute(tx.executor())
                .await?;
            Ok::<_, DbError>(())
        })
        .await
        .unwrap_err();
    assert_eq!(failed.kind(), DbErrorKind::CheckViolation);
    drop(guard);

    let warnings: Vec<_> = output
        .json_lines()
        .into_iter()
        .filter(|line| line["level"] == "WARN")
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");

    let over = &warnings[0];
    assert_eq!(over["transaction"], "test.over_budget");
    assert_eq!(over["elapsed_ms"], 251);
    assert_eq!(over["threshold_ms"], 250);
    assert_eq!(over["pool_wait_ms"], 0);
    assert_eq!(over["outcome"], "committed");
    assert_eq!(
        over["message"],
        "write transaction exceeded its duration budget"
    );

    let rolled_back = &warnings[1];
    assert_eq!(rolled_back["transaction"], "test.over_budget_rolled_back");
    assert_eq!(rolled_back["elapsed_ms"], 400);
    assert_eq!(rolled_back["outcome"], "rolled_back");

    let order: Vec<String> = sqlx::query_scalar("SELECT writer FROM commits ORDER BY seq")
        .fetch_all(pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(order, ["within_budget", "over_budget"]);
    pools.shutdown().await.checkpoint.unwrap();
}
