use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use testcontainers::core::{CmdWaitFor, ExecCommand, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const IMAGE_NAME: &str = "cgr.dev/chainguard/minio@sha256";
const IMAGE_DIGEST: &str = "bd014394a80898e68c149f2311fdf8d5a2c2f3bb2c33b9327ae6d02b4b065ae1";
const API_PORT: u16 = 9000;
const DOMAIN: &str = "localhost";
const DATA_DIR: &str = "/tmp/palmr-minio";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const READY_LINE: &str = "API: ";

pub struct MinioServer {
    container: ContainerAsync<GenericImage>,
    endpoint: String,
    access_key: String,
    secret_key: String,
}

impl MinioServer {
    pub async fn start() -> Result<Self> {
        let access_key = format!("palmr{}", random_hex(8)?);
        let secret_key = random_hex(24)?;
        let container = GenericImage::new(IMAGE_NAME, IMAGE_DIGEST)
            .with_exposed_port(API_PORT.tcp())
            .with_wait_for(WaitFor::message_on_either_std(READY_LINE))
            .with_cmd(["server", DATA_DIR])
            .with_env_var("MINIO_ROOT_USER", &access_key)
            .with_env_var("MINIO_ROOT_PASSWORD", &secret_key)
            .with_env_var("MINIO_DOMAIN", DOMAIN)
            .with_env_var("MINIO_BROWSER", "off")
            .with_startup_timeout(STARTUP_TIMEOUT)
            .start()
            .await
            .context("start the MinIO test container; Docker must be available")?;

        let host = container.get_host().await?.to_string();
        if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
            bail!("virtual-hosted MinIO needs a local Docker host, found {host}");
        }
        let port = container.get_host_port_ipv4(API_PORT.tcp()).await?;
        let server = Self {
            container,
            endpoint: format!("http://{DOMAIN}:{port}"),
            access_key,
            secret_key,
        };
        server
            .mc(&[
                "alias",
                "set",
                "palmr",
                &format!("http://127.0.0.1:{API_PORT}"),
                &server.access_key,
                &server.secret_key,
            ])
            .await?;
        Ok(server)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn access_key(&self) -> &str {
        &self.access_key
    }

    pub fn secret_key(&self) -> &str {
        &self.secret_key
    }

    pub async fn create_bucket(&self, label: &str) -> Result<String> {
        let bucket = format!("palmr-{label}-{}", random_hex(4)?);
        self.mc(&["mb", &format!("palmr/{bucket}")]).await?;
        Ok(bucket)
    }

    async fn mc(&self, args: &[&str]) -> Result<()> {
        let mut command = vec!["mc", "--quiet"];
        command.extend_from_slice(args);
        let mut result = self
            .container
            .exec(ExecCommand::new(command).with_cmd_ready_condition(CmdWaitFor::exit()))
            .await?;
        match result.exit_code().await? {
            Some(0) => Ok(()),
            code => {
                let stderr = result.stderr_to_vec().await.unwrap_or_default();
                bail!(
                    "mc {} exited with {code:?}: {}",
                    args.first().copied().unwrap_or_default(),
                    String::from_utf8_lossy(&stderr).trim()
                )
            }
        }
    }
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).map_err(|error| anyhow::anyhow!("getrandom failed: {error}"))?;
    let mut text = String::with_capacity(bytes * 2);
    for byte in buffer {
        write!(text, "{byte:02x}")?;
    }
    Ok(text)
}
