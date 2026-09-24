use std::fs::File;
use std::io::{self, Read};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rustix::process::{kill_process, Pid, Signal};

const TIMEOUT: Duration = Duration::from_secs(30);
const TEXT_LIMIT: u64 = 1024 * 1024;

pub struct ServerProcess {
    child: Child,
    port: u16,
    errors: PathBuf,
}

impl ServerProcess {
    pub fn spawn(data_dir: &Path) -> Result<Self> {
        let port = free_port()?;
        let errors = data_dir.with_extension("server.stderr");
        let child = palmr_command(data_dir, port)
            .env("PALMR_LOG_LEVEL", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&errors)?)
            .spawn()
            .context("spawn palmr serve")?;
        Ok(Self {
            child,
            port,
            errors,
        })
    }

    pub async fn is_ready(&self) -> bool {
        let url = format!("http://127.0.0.1:{}/health/ready", self.port);
        matches!(
            reqwest::get(&url).await,
            Ok(response) if response.status() == reqwest::StatusCode::OK
        )
    }

    pub async fn wait_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                bail!(
                    "palmr exited before becoming ready: {status}; {}",
                    read_text(&self.errors)?
                );
            }
            if self.is_ready().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("palmr did not become ready within {TIMEOUT:?}")
    }

    pub async fn stop(mut self) -> Result<ExitStatus> {
        let pid = Pid::from_raw(i32::try_from(self.child.id())?).context("child pid")?;
        kill_process(pid, Signal::TERM)?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        bail!("palmr did not exit within {TIMEOUT:?}")
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn palmr_command(data_dir: &Path, port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_palmr"));
    command
        .env_clear()
        .env("PALMR_DATA_DIR", data_dir)
        .env("PALMR_HOST", "127.0.0.1")
        .env("PALMR_PORT", port.to_string())
        .env("PALMR_BASE_URL", format!("http://127.0.0.1:{port}/"))
        .env("PALMR_LOG_FORMAT", "json");
    command
}

pub fn run_palmr(data_dir: &Path, port: u16, args: &[&str]) -> Result<Output> {
    palmr_command(data_dir, port)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run palmr {args:?}"))
}

pub fn free_port() -> Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

pub fn read_text(path: &Path) -> Result<String> {
    Ok(io::read_to_string(File::open(path)?.take(TEXT_LIMIT))?)
}
