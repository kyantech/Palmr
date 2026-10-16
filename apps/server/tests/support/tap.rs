#![allow(
    dead_code,
    reason = "each S3 test binary uses a different subset of the tap"
)]

use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use http::header::{CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING};
use http::StatusCode;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub range: Option<String>,
    pub served: Option<u64>,
}

#[derive(Clone)]
pub struct Recorder {
    upstream: String,
    client: reqwest::Client,
    seen: Arc<Mutex<Vec<Seen>>>,
}

pub struct Tap {
    address: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Tap {
    pub async fn start(upstream: &str) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Recorder {
            upstream: upstream.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
            seen: Arc::clone(&seen),
        };
        let app = axum::Router::new().fallback(relay).with_state(recorder);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address,
            seen,
            task,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn clear(&self) {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn all(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn object_reads(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|seen| seen.method == "GET" && seen.target.contains("/objects/"))
            .cloned()
            .collect()
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn relay(State(recorder): State<Recorder>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = http_body_util::BodyExt::collect(body)
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str)
        .to_owned();
    let mut upstream = recorder.client.request(
        parts.method.clone(),
        format!("{}{target}", recorder.upstream),
    );
    for (name, value) in &parts.headers {
        if name != CONTENT_LENGTH && name != CONNECTION && name != TRANSFER_ENCODING {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(answered) = upstream.body(bytes).send().await else {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap_or_default();
    };
    let mut response = Response::builder().status(answered.status());
    for (name, value) in answered.headers() {
        if name != CONNECTION && name != TRANSFER_ENCODING {
            response = response.header(name, value);
        }
    }
    let payload = answered.bytes().await.unwrap_or_default();
    recorder
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Seen {
            method: parts.method.to_string(),
            target,
            range: parts
                .headers
                .get("range")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            served: Some(payload.len() as u64),
        });
    response.body(Body::from(payload)).unwrap_or_default()
}
