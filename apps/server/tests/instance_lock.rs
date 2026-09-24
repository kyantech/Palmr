use std::fs::{self, File};
use std::io::{self, Read};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use rustix::fs::FlockOperation;
use rustix::process::{kill_process, Pid, Signal};
use serde_json::Value;
use tempfile::{Builder, TempDir};

const LOCK_FILE: &str = "runtime/instance.lock";
const DATA_DIR_IN_USE: &str = "STARTUP_DATA_DIR_IN_USE";
const EX_CONFIG: i32 = 78;
const TIMEOUT: Duration = Duration::from_secs(30);
const TEXT_LIMIT: u64 = 1024 * 1024;

struct Server {
    child: Child,
    port: u16,
    logs: PathBuf,
    errors: PathBuf,
}

impl Server {
    fn spawn(data_dir: &Path, name: &str) -> Result<Self> {
        let port = free_port()?;
        let logs = data_dir.with_extension(format!("{name}.stdout"));
        let errors = data_dir.with_extension(format!("{name}.stderr"));
        let child = Command::new(env!("CARGO_BIN_EXE_palmr"))
            .env_clear()
            .env("PALMR_DATA_DIR", data_dir)
            .env("PALMR_HOST", "127.0.0.1")
            .env("PALMR_PORT", port.to_string())
            .env("PALMR_BASE_URL", format!("http://127.0.0.1:{port}/"))
            .env("PALMR_LOG_FORMAT", "json")
            .env("PALMR_LOG_LEVEL", "info")
            .stdin(Stdio::null())
            .stdout(File::create(&logs)?)
            .stderr(File::create(&errors)?)
            .spawn()
            .context("spawn palmr")?;
        Ok(Self {
            child,
            port,
            logs,
            errors,
        })
    }

    async fn wait_ready(&mut self) -> Result<()> {
        let url = format!("http://127.0.0.1:{}/health/ready", self.port);
        let client = reqwest::Client::new();
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "palmr exited before becoming ready: {status}; {}",
                    read_text(&self.errors)?
                );
            }
            if let Ok(response) = client.get(&url).send().await {
                if response.status() == reqwest::StatusCode::OK {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("palmr did not become ready within {TIMEOUT:?}")
    }

    async fn wait_exit(&mut self) -> Result<ExitStatus> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.child.kill()?;
        bail!("palmr did not exit within {TIMEOUT:?}")
    }

    async fn stop(&mut self, signal: Signal) -> Result<ExitStatus> {
        let pid = Pid::from_raw(i32::try_from(self.child.id())?).context("child pid")?;
        kill_process(pid, signal)?;
        self.wait_exit().await
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn log_events(&self) -> Result<Vec<Value>> {
        read_text(&self.logs)?
            .lines()
            .map(|line| serde_json::from_str(line).with_context(|| format!("log line {line}")))
            .collect()
    }

    fn logged(&self, message: &str) -> Result<Option<Value>> {
        Ok(self
            .log_events()?
            .into_iter()
            .find(|event| event["message"] == message))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn read_text(path: &Path) -> Result<String> {
    Ok(io::read_to_string(File::open(path)?.take(TEXT_LIMIT))?)
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

fn data_dir() -> Result<TempDir> {
    Ok(Builder::new().prefix("palmr-lock-").tempdir()?)
}

fn lock_contents(data_dir: &Path) -> Result<String> {
    read_text(&data_dir.join(LOCK_FILE))
}

fn lock_metadata(data_dir: &Path) -> Result<(String, u64)> {
    let metadata: Value = serde_json::from_str(&lock_contents(data_dir)?)?;
    let instance_id = metadata["instance_id"]
        .as_str()
        .context("instance_id")?
        .to_owned();
    let pid = metadata["pid"].as_u64().context("pid")?;
    ensure!(
        metadata.as_object().map(serde_json::Map::len) == Some(2),
        "unexpected lock metadata {metadata}"
    );
    Ok((instance_id, pid))
}

fn assert_uuid_v7(text: &str) {
    assert_eq!(text.len(), 36, "{text}");
    assert_eq!(&text[14..15], "7", "{text}");
    assert!(
        text.chars()
            .all(|char| matches!(char, '0'..='9' | 'a'..='f' | '-')),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn it_second_instance_refused() -> Result<()> {
    let root = data_dir()?;
    let data = root.path().join("data");
    fs::create_dir(&data)?;

    let mut first = Server::spawn(&data, "first")?;
    first.wait_ready().await?;
    let (first_id, first_pid) = lock_metadata(&data)?;
    assert_uuid_v7(&first_id);
    assert_eq!(first_pid, u64::from(first.pid()));

    let mut second = Server::spawn(&data, "second")?;
    let status = second.wait_exit().await?;
    assert_eq!(status.code(), Some(EX_CONFIG));
    let stderr = read_text(&second.errors)?;
    assert!(
        stderr.starts_with(&format!("FATAL: {DATA_DIR_IN_USE}: ")),
        "{stderr}"
    );
    let failure = second
        .log_events()?
        .into_iter()
        .find(|event| event["startup_error"] == DATA_DIR_IN_USE)
        .context("structured startup failure")?;
    assert_eq!(failure["level"], "ERROR");
    for later in ["database migrations current", "startup.completed"] {
        assert!(first.logged(later)?.is_some(), "{later}");
        assert!(second.logged(later)?.is_none(), "{later}");
    }
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, second.port)).is_err());
    assert_eq!(lock_metadata(&data)?, (first_id.clone(), first_pid));

    assert!(first.stop(Signal::TERM).await?.success());
    assert_eq!(lock_contents(&data)?, "");

    let mut third = Server::spawn(&data, "third")?;
    third.wait_ready().await?;
    let (third_id, third_pid) = lock_metadata(&data)?;
    assert_uuid_v7(&third_id);
    assert_ne!(third_id, first_id);
    assert_eq!(third_pid, u64::from(third.pid()));
    assert!(third.stop(Signal::TERM).await?.success());
    assert!(third.logged("instance_lock.reclaimed")?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_stale_lock_reclaimed() -> Result<()> {
    let root = data_dir()?;
    let data = root.path().join("data");
    fs::create_dir(&data)?;

    let mut crashed = Server::spawn(&data, "crashed")?;
    crashed.wait_ready().await?;
    let (stale_id, stale_pid) = lock_metadata(&data)?;
    let status = crashed.stop(Signal::KILL).await?;
    assert_eq!(status.code(), None);

    assert_eq!(lock_metadata(&data)?, (stale_id.clone(), stale_pid));
    {
        let probe = File::options()
            .read(true)
            .write(true)
            .open(data.join(LOCK_FILE))?;
        rustix::fs::flock(probe.as_fd(), FlockOperation::NonBlockingLockExclusive)
            .context("no live process may own the lock after its holder was killed")?;
    }

    let mut restarted = Server::spawn(&data, "restarted")?;
    restarted.wait_ready().await?;
    let (instance_id, pid) = lock_metadata(&data)?;
    assert_uuid_v7(&instance_id);
    assert_ne!(instance_id, stale_id);
    assert_eq!(pid, u64::from(restarted.pid()));

    let notice = restarted
        .logged("instance_lock.reclaimed")?
        .context("reclaim notice")?;
    assert_eq!(notice["level"], "WARN");
    assert_eq!(notice["previous_instance_id"], stale_id.as_str());
    assert_eq!(notice["instance_id"], instance_id.as_str());

    let mut refused = Server::spawn(&data, "refused")?;
    assert_eq!(refused.wait_exit().await?.code(), Some(EX_CONFIG));
    assert_eq!(lock_metadata(&data)?.0, instance_id);

    assert!(restarted.stop(Signal::TERM).await?.success());
    Ok(())
}
