use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use palmr_server::lifecycle::{Application, Drain};
use palmr_server::{Clock, EnvironmentSource, OperatorConfig, TestClock};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::{Client, RequestBuilder};
use tempfile::{Builder, TempDir};
use time::macros::datetime;
use tokio::net::TcpListener;
use url::Url;

const CSRF_COOKIE: &str = "palmr_csrf";
const CSRF_HEADER: &str = "X-Palmr-CSRF";

pub struct TestApplication {
    application: Application,
    data_dir: TempDir,
    clock: TestClock,
    base_url: Url,
    client: Client,
    cookies: Arc<Jar>,
}

impl TestApplication {
    pub async fn start(test_name: &str) -> Result<Self> {
        let data_dir = Builder::new()
            .prefix("palmr-it-")
            .tempdir()
            .context("create isolated data directory")?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind ephemeral test listener")?;
        let address = listener
            .local_addr()
            .context("read test listener address")?;
        let base_url = Url::parse(&format!("http://{address}/"))?;
        let port = address.port().to_string();
        let data_path = data_dir.path().as_os_str().to_owned();
        let source = EnvironmentSource::from_vars([
            (OsString::from("PALMR_HOST"), OsString::from("127.0.0.1")),
            (OsString::from("PALMR_PORT"), OsString::from(port)),
            (
                OsString::from("PALMR_BASE_URL"),
                OsString::from(base_url.as_str()),
            ),
            (OsString::from("PALMR_DATA_DIR"), data_path),
        ]);
        let config = OperatorConfig::load(&source)
            .context("load test application configuration")?
            .config;
        let clock = TestClock::new(datetime!(2026-01-01 00:00 UTC));
        let injected_clock: Arc<dyn Clock> = Arc::new(clock.clone());
        let application = Application::start(listener, &config, injected_clock)
            .with_context(|| format!("start application for {test_name}"))?;
        let cookies = Arc::new(Jar::default());
        let client = Client::builder()
            .cookie_provider(Arc::clone(&cookies))
            .build()
            .context("build test HTTP client")?;
        Ok(Self {
            application,
            data_dir,
            clock,
            base_url,
            client,
            cookies,
        })
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn clock(&self) -> &TestClock {
        &self.clock
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn url(&self, path: &str) -> std::result::Result<Url, url::ParseError> {
        self.base_url.join(path.trim_start_matches('/'))
    }

    pub fn csrf_token(&self) -> Option<String> {
        self.cookies
            .cookies(&self.base_url)
            .and_then(|cookies| cookies.to_str().ok().map(str::to_owned))
            .and_then(|cookies| {
                cookies.split(';').find_map(|cookie| {
                    let (name, value) = cookie.trim().split_once('=')?;
                    (name == CSRF_COOKIE).then(|| value.to_owned())
                })
            })
    }

    pub fn with_csrf(&self, request: RequestBuilder) -> RequestBuilder {
        match self.csrf_token() {
            Some(token) => request.header(CSRF_HEADER, token),
            None => request,
        }
    }

    pub async fn shutdown(self) -> Drain {
        let Self {
            application,
            data_dir,
            clock: _,
            base_url: _,
            client: _,
            cookies: _,
        } = self;
        let drain = application.shutdown(Duration::from_secs(5)).await;
        drop(data_dir);
        drain
    }
}
