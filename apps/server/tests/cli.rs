pub mod support;

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use tempfile::{Builder, TempDir};

use support::{read_text, run_palmr, ServerProcess};

const LOCK_FILE: &str = "runtime/instance.lock";
const DATA_DIR_IN_USE: &str = "STARTUP_DATA_DIR_IN_USE";
const EX_CONFIG: i32 = 78;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

type MigrationRow = (i64, String, Vec<u8>, String, bool);
type SchemaRow = (String, String, Option<String>);

struct DataRoot {
    _temp: TempDir,
    data: PathBuf,
    out: PathBuf,
    held: TcpListener,
}

impl DataRoot {
    fn new(prefix: &str) -> Result<Self> {
        let temp = Builder::new().prefix(prefix).tempdir()?;
        let data = temp.path().join("data");
        let out = temp.path().join("backups");
        fs::create_dir(&data)?;
        fs::create_dir(&out)?;
        Ok(Self {
            _temp: temp,
            data,
            out,
            held: TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?,
        })
    }

    fn palmr(&self, args: &[&str]) -> Result<Output> {
        run_palmr(&self.data, self.held.local_addr()?.port(), args)
    }

    fn database(&self) -> PathBuf {
        self.data.join("palmr.db")
    }

    fn out_text(&self) -> Result<&str> {
        self.out.to_str().context("utf-8 temp path")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status {}; stdout {}; stderr {}",
        output.status,
        text(&output.stdout),
        text(&output.stderr)
    );
}

fn assert_refused_in_use(output: &Output) {
    assert_eq!(output.status.code(), Some(EX_CONFIG), "{output:?}");
    let stderr = text(&output.stderr);
    assert!(
        stderr.starts_with(&format!("FATAL: {DATA_DIR_IN_USE}: ")),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

fn applied_count(output: &Output) -> Result<u64> {
    let stdout = text(&output.stdout);
    let applied = stdout
        .strip_prefix("database migrations current: applied ")
        .and_then(|rest| rest.split(',').next())
        .with_context(|| format!("unexpected migrate output {stdout:?}"))?;
    Ok(applied.parse()?)
}

async fn open_read_only(path: &Path) -> Result<SqliteConnection> {
    Ok(SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await?)
}

async fn open_backup_file(path: &Path) -> Result<SqliteConnection> {
    Ok(SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true)
        .connect()
        .await?)
}

async fn open_writer(path: &Path) -> Result<SqliteConnection> {
    Ok(SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        .connect()
        .await?)
}

async fn migrations(connection: &mut SqliteConnection) -> Result<Vec<MigrationRow>> {
    Ok(sqlx::query_as(
        "SELECT version, description, checksum, installed_on, success \
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(connection)
    .await?)
}

async fn schema(connection: &mut SqliteConnection) -> Result<Vec<SchemaRow>> {
    Ok(
        sqlx::query_as("SELECT type, name, sql FROM sqlite_schema ORDER BY type, name")
            .fetch_all(connection)
            .await?,
    )
}

async fn integrity(connection: &mut SqliteConnection) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(connection)
        .await?)
}

async fn freelist_pages(connection: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(connection)
        .await?)
}

async fn ledger(connection: &mut SqliteConnection) -> Result<(i64, i64)> {
    Ok(
        sqlx::query_as("SELECT (SELECT count(*) FROM probe_ledger), (SELECT n FROM probe_total)")
            .fetch_one(connection)
            .await?,
    )
}

async fn append_entry(connection: &mut SqliteConnection) -> Result<()> {
    let mut tx = connection.begin().await?;
    sqlx::query("INSERT INTO probe_ledger (payload) VALUES (randomblob(512))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE probe_total SET n = n + 1")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn seed_ledger(path: &Path) -> Result<()> {
    let mut connection = open_writer(path).await?;
    sqlx::raw_sql(
        "CREATE TABLE probe_ledger (id INTEGER PRIMARY KEY, payload BLOB NOT NULL); \
         CREATE TABLE probe_total (n INTEGER NOT NULL); \
         INSERT INTO probe_total (n) VALUES (0); \
         WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 8000) \
         INSERT INTO probe_ledger (payload) SELECT randomblob(1024) FROM seq; \
         UPDATE probe_total SET n = (SELECT count(*) FROM probe_ledger); \
         DELETE FROM probe_ledger WHERE id % 4 <> 0; \
         UPDATE probe_total SET n = (SELECT count(*) FROM probe_ledger);",
    )
    .execute(&mut connection)
    .await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut connection)
        .await?;
    ensure!(freelist_pages(&mut connection).await? > 0);
    connection.close().await?;
    Ok(())
}

fn directory_entries(path: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(path)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

fn is_backup_name(name: &str) -> bool {
    let Some(stamp) = name
        .strip_prefix("palmr-")
        .and_then(|rest| rest.strip_suffix("Z.db"))
    else {
        return false;
    };
    let bytes = stamp.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'T'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_cli_migrate_idempotent() -> Result<()> {
    let root = DataRoot::new("palmr-cli-migrate-")?;

    let first = root.palmr(&["migrate"])?;
    assert_success(&first);
    let applied = applied_count(&first)?;
    assert!(applied >= 1, "{first:?}");
    assert!(first.stderr.is_empty(), "{first:?}");
    assert_eq!(read_text(&root.data.join(LOCK_FILE))?, "");

    let mut connection = open_read_only(&root.database()).await?;
    let recorded = migrations(&mut connection).await?;
    let tables = schema(&mut connection).await?;
    connection.close().await?;
    assert_eq!(u64::try_from(recorded.len())?, applied);
    assert!(recorded.iter().all(|row| row.4));

    let second = root.palmr(&["migrate"])?;
    assert_success(&second);
    assert_eq!(applied_count(&second)?, 0);

    let mut connection = open_read_only(&root.database()).await?;
    assert_eq!(migrations(&mut connection).await?, recorded);
    assert_eq!(schema(&mut connection).await?, tables);
    connection.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_cli_mutating_refuses_while_server_holds_lock() -> Result<()> {
    let root = DataRoot::new("palmr-cli-lock-")?;
    let mut server = ServerProcess::spawn(&root.data)?;
    server.wait_ready().await?;
    let lock = read_text(&root.data.join(LOCK_FILE))?;
    assert!(!lock.is_empty());
    let schema_before = {
        let mut connection = open_read_only(&root.database()).await?;
        let rows = schema(&mut connection).await?;
        connection.close().await?;
        rows
    };

    assert_refused_in_use(&root.palmr(&["migrate"])?);

    let jobs = root.palmr(&["jobs", "run-once", "--kind", "tokens.prune"])?;
    assert_refused_in_use(&jobs);

    let backup = root.palmr(&["db", "backup", "--out", root.out_text()?])?;
    assert_refused_in_use(&backup);
    assert!(text(&backup.stderr).contains("--allow-concurrent"));
    assert!(directory_entries(&root.out)?.is_empty());

    let check = root.palmr(&["db", "check"])?;
    assert_refused_in_use(&check);
    assert!(text(&check.stderr).contains("--allow-concurrent"));

    let concurrent = root.palmr(&["db", "check", "--allow-concurrent"])?;
    assert_success(&concurrent);
    assert_eq!(text(&concurrent.stdout), "ok\n");
    assert!(concurrent.stderr.is_empty(), "{concurrent:?}");

    assert_eq!(read_text(&root.data.join(LOCK_FILE))?, lock);
    assert!(server.is_ready().await);
    assert!(server.stop().await?.success());

    let mut connection = open_read_only(&root.database()).await?;
    assert_eq!(schema(&mut connection).await?, schema_before);
    connection.close().await?;

    let released = root.palmr(&["db", "check"])?;
    assert_success(&released);
    assert_eq!(text(&released.stdout), "ok\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_cli_jobs_run_once_is_noop_on_empty_queue() -> Result<()> {
    let root = DataRoot::new("palmr-cli-jobs-")?;
    assert_success(&root.palmr(&["migrate"])?);

    let first = root.palmr(&["jobs", "run-once", "--kind", "tokens.prune"])?;
    assert_success(&first);
    assert_eq!(
        text(&first.stdout),
        "jobs run-once: executed 0 job(s) of kind tokens.prune\n"
    );
    assert!(first.stderr.is_empty(), "{first:?}");

    let second = root.palmr(&["jobs", "run-once", "--kind", "tokens.prune"])?;
    assert_success(&second);
    assert_eq!(text(&second.stdout), text(&first.stdout));

    let unknown = root.palmr(&["jobs", "run-once", "--kind", "run"])?;
    assert_eq!(unknown.status.code(), Some(2), "{unknown:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_cli_db_backup_vacuum_into_consistent() -> Result<()> {
    let root = DataRoot::new("palmr-cli-backup-")?;
    assert_success(&root.palmr(&["migrate"])?);
    seed_ledger(&root.database()).await?;

    let mut server = ServerProcess::spawn(&root.data)?;
    server.wait_ready().await?;

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let path = root.database();
        tokio::spawn(async move {
            let mut connection = open_writer(&path).await?;
            let mut commits = 0_u64;
            while !stop.load(Ordering::SeqCst) {
                append_entry(&mut connection).await?;
                commits += 1;
            }
            connection.close().await?;
            anyhow::Ok(commits)
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;

    let data = root.data.clone();
    let port = root.held.local_addr()?.port();
    let out = root.out_text()?.to_owned();
    let output = tokio::task::spawn_blocking(move || {
        run_palmr(
            &data,
            port,
            &["db", "backup", "--out", &out, "--allow-concurrent"],
        )
    })
    .await??;
    stop.store(true, Ordering::SeqCst);
    let commits = writer.await??;
    assert!(commits > 0);
    assert_success(&output);
    assert!(text(&output.stderr).contains("instance.key"));

    let stdout = text(&output.stdout);
    let reported = PathBuf::from(stdout.strip_suffix('\n').context("one line")?);
    assert_eq!(reported.parent(), Some(root.out.canonicalize()?.as_path()));
    let name = reported
        .file_name()
        .and_then(|name| name.to_str())
        .context("file name")?
        .to_owned();
    assert!(is_backup_name(&name), "{name}");
    assert_eq!(directory_entries(&root.out)?, std::slice::from_ref(&name));

    let mut header = [0_u8; 16];
    std::io::Read::read_exact(&mut fs::File::open(&reported)?, &mut header)?;
    assert_eq!(&header, SQLITE_MAGIC);

    let mut backup = open_backup_file(&reported).await?;
    assert_eq!(integrity(&mut backup).await?, ["ok"]);
    let (rows, total) = ledger(&mut backup).await?;
    assert_eq!(rows, total, "the backup is a single consistent snapshot");
    assert!(rows >= 2000);
    assert_eq!(freelist_pages(&mut backup).await?, 0);
    let backup_migrations = migrations(&mut backup).await?;
    backup.close().await?;

    assert!(server.is_ready().await);
    assert!(server.stop().await?.success());

    let mut source = open_read_only(&root.database()).await?;
    assert_eq!(integrity(&mut source).await?, ["ok"]);
    assert!(freelist_pages(&mut source).await? > 0);
    let (source_rows, source_total) = ledger(&mut source).await?;
    assert_eq!(source_rows, source_total);
    assert!(source_rows >= rows);
    assert_eq!(migrations(&mut source).await?, backup_migrations);
    source.close().await?;

    assert_eq!(directory_entries(&root.out)?, [name]);
    Ok(())
}
