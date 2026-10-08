use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};
use tempfile::TempDir;
use time::macros::datetime;
use tokio::net::TcpListener;

use super::{Application, Drain};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;

const GRACE: Duration = Duration::from_secs(10);
const FIRST_RUN: &str = "2026-10-09T00:00:00.000Z";
const SECOND_RUN: &str = "2026-10-10T00:00:00.000Z";

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobRow {
    id: String,
    kind: String,
    state: String,
    run_at: String,
    dedup_key: String,
}

fn config(data: &Path, address: SocketAddr) -> OperatorConfig {
    let port = address.port().to_string();
    let base_url = format!("http://{address}");
    let vars = [
        ("PALMR_HOST", "127.0.0.1"),
        ("PALMR_PORT", port.as_str()),
        ("PALMR_BASE_URL", base_url.as_str()),
        ("PALMR_DATA_DIR", data.to_str().unwrap()),
    ];
    OperatorConfig::load(&EnvironmentSource::from_vars(vars))
        .unwrap()
        .config
}

async fn start(data: &Path, clock: &TestClock) -> Application {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = config(data, listener.local_addr().unwrap());
    Application::start(listener, &config, Arc::new(clock.clone()))
        .await
        .unwrap_or_else(|error| panic!("startup failed: {error}"))
}

async fn connect(data: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(data.join("palmr.db")))
        .await
        .unwrap()
}

async fn jobs(data: &Path) -> Vec<JobRow> {
    let mut connection = connect(data).await;
    sqlx::query("SELECT id, kind, state, run_at, dedup_key FROM jobs ORDER BY run_at, id")
        .fetch_all(&mut connection)
        .await
        .unwrap()
        .iter()
        .map(|row| JobRow {
            id: row.get("id"),
            kind: row.get("kind"),
            state: row.get("state"),
            run_at: row.get("run_at"),
            dedup_key: row.get("dedup_key"),
        })
        .collect()
}

async fn eventually(data: &Path, what: &str, done: impl Fn(&[JobRow]) -> bool) -> Vec<JobRow> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let rows = jobs(data).await;
        if done(&rows) {
            return rows;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn key(run_at: time::OffsetDateTime) -> String {
    format!("quota.reconcile:{}", run_at.unix_timestamp())
}

fn live(rows: &[JobRow]) -> Vec<&JobRow> {
    rows.iter()
        .filter(|row| row.state == "pending" || row.state == "claimed")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_startup_schedules_quota_reconcile_once_and_the_chain_survives_restarts() {
    let data = TempDir::new().unwrap();
    let clock = TestClock::new(datetime!(2026-10-08 12:00 UTC));

    let first = start(data.path(), &clock).await;
    let rows = jobs(data.path()).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let original = rows[0].clone();
    assert_eq!(original.kind, "quota.reconcile");
    assert_eq!(original.state, "pending");
    assert_eq!(original.run_at, FIRST_RUN);
    assert_eq!(original.dedup_key, key(datetime!(2026-10-09 00:00 UTC)));

    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(
        jobs(data.path()).await,
        rows,
        "a future schedule is not claimed by the live workers"
    );
    assert_eq!(first.shutdown(GRACE).await, Drain::Completed);

    let second = start(data.path(), &clock).await;
    assert_eq!(jobs(data.path()).await, rows, "a restart changes nothing");
    assert_eq!(second.shutdown(GRACE).await, Drain::Completed);

    let third = start(data.path(), &clock).await;
    clock.advance(Duration::from_secs(12 * 60 * 60 + 30));
    let rows = eventually(data.path(), "the first reconcile to run", |rows| {
        rows.iter().any(|row| row.state == "succeeded")
    })
    .await;
    let rows = if live(&rows).is_empty() {
        eventually(data.path(), "the successor", |rows| !live(rows).is_empty()).await
    } else {
        rows
    };
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].id, original.id);
    assert_eq!(rows[0].state, "succeeded");
    assert_eq!(rows[1].state, "pending");
    assert_eq!(rows[1].run_at, SECOND_RUN);
    assert_eq!(rows[1].dedup_key, key(datetime!(2026-10-10 00:00 UTC)));
    assert_eq!(third.shutdown(GRACE).await, Drain::Completed);

    let fourth = start(data.path(), &clock).await;
    assert_eq!(
        jobs(data.path()).await,
        rows,
        "one chain after a further restart"
    );
    assert_eq!(fourth.shutdown(GRACE).await, Drain::Completed);

    let mut connection = connect(data.path()).await;
    sqlx::query(
        "UPDATE jobs SET state = 'claimed',
                         claimed_by = 'ffffffff-ffff-ffff-ffff-ffffffffffff#0',
                         lease_expires_at = '2026-10-12T00:00:00.000Z'
          WHERE state = 'pending'",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let fifth = start(data.path(), &clock).await;
    let after = jobs(data.path()).await;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(
        live(&after).len(),
        1,
        "the stale claim is requeued, not duplicated: {after:?}"
    );
    assert_eq!(after[1].id, rows[1].id);
    assert_eq!(after[1].state, "pending");
    assert!(after.iter().all(|row| row.kind == "quota.reconcile"));
    assert_eq!(fifth.shutdown(GRACE).await, Drain::Completed);
}
