use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use http::header::{CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Limited};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const CHECKSUM_HEADER: &str = "x-amz-checksum-sha256";
const FORWARD_CAP: usize = 64 * 1024 * 1024;
const MISSING_CHECKSUM: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>InvalidRequest</Code><Message>Missing required header for this request: x-amz-checksum-sha256</Message></Error>";

#[derive(Clone)]
struct Gate {
    upstream: String,
    client: reqwest::Client,
    rejected: Arc<AtomicUsize>,
    forwarded: Arc<Mutex<Vec<String>>>,
}

pub struct ChecksumGate {
    address: SocketAddr,
    rejected: Arc<AtomicUsize>,
    forwarded: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl ChecksumGate {
    pub async fn start(upstream: &str) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let rejected = Arc::new(AtomicUsize::new(0));
        let forwarded = Arc::new(Mutex::new(Vec::new()));
        let gate = Gate {
            upstream: upstream.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
            rejected: Arc::clone(&rejected),
            forwarded: Arc::clone(&forwarded),
        };
        let app = axum::Router::new().fallback(forward).with_state(gate);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address,
            rejected,
            forwarded,
            task,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }

    pub fn forwarded_checksums(&self) -> Vec<String> {
        self.forwarded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for ChecksumGate {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn is_part_upload(request: &http::request::Parts) -> bool {
    let query = request.uri.query().unwrap_or_default();
    let has = |name: &str| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    };
    request.method == Method::PUT
        && has("partNumber")
        && has("uploadId")
        && !request.headers.contains_key("x-amz-copy-source")
}

fn refusal(status: StatusCode, body: &'static str) -> Response {
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap_or_default()
}

async fn forward(State(gate): State<Gate>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(collected) = Limited::new(body, FORWARD_CAP).collect().await else {
        return refusal(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    };
    let bytes = collected.to_bytes();
    if is_part_upload(&parts) {
        match parts
            .headers
            .get(CHECKSUM_HEADER)
            .and_then(|value| value.to_str().ok())
        {
            None => {
                gate.rejected.fetch_add(1, Ordering::SeqCst);
                return refusal(StatusCode::BAD_REQUEST, MISSING_CHECKSUM);
            }
            Some(value) => gate
                .forwarded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(value.to_owned()),
        }
    }
    let target = format!(
        "{}{}",
        gate.upstream,
        parts
            .uri
            .path_and_query()
            .map_or("/", http::uri::PathAndQuery::as_str)
    );
    let mut upstream = gate.client.request(parts.method.clone(), target);
    for (name, value) in &parts.headers {
        if name != CONTENT_LENGTH && name != CONNECTION && name != TRANSFER_ENCODING {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(answered) = upstream.body(bytes).send().await else {
        return refusal(StatusCode::BAD_GATEWAY, "upstream unreachable");
    };
    let mut response = Response::builder().status(answered.status());
    for (name, value) in answered.headers() {
        if name != CONNECTION && name != TRANSFER_ENCODING {
            response = response.header(name, value);
        }
    }
    let Ok(payload) = answered.bytes().await else {
        return refusal(StatusCode::BAD_GATEWAY, "upstream truncated");
    };
    response.body(Body::from(payload)).unwrap_or_default()
}
