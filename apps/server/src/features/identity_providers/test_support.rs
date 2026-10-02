use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
    pub sized: bool,
}

impl Reply {
    pub fn json(value: &Value) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            body: value.to_string().into_bytes(),
            delay: Duration::ZERO,
            sized: true,
        }
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            delay: Duration::ZERO,
            sized: true,
        }
    }

    pub fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".to_owned(), location.to_owned())],
            body: Vec::new(),
            delay: Duration::ZERO,
            sized: true,
        }
    }

    pub fn raw(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            body,
            delay: Duration::ZERO,
            sized: true,
        }
    }

    #[must_use]
    pub fn unsized_body(mut self) -> Self {
        self.sized = false;
        self
    }

    #[must_use]
    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Routes = Arc<Mutex<HashMap<String, Reply>>>;
type Log = Arc<Mutex<Vec<Recorded>>>;

pub struct FakeIdp {
    address: SocketAddr,
    routes: Routes,
    log: Log,
    task: JoinHandle<()>,
}

impl Drop for FakeIdp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeIdp {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let routes: Routes = Arc::default();
        let log: Log = Arc::default();
        let task = tokio::spawn(serve(listener, Arc::clone(&routes), Arc::clone(&log)));
        Self {
            address,
            routes,
            log,
            task,
        }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.address.port())
    }

    pub fn set(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.log.lock().unwrap().clone()
    }

    pub fn requests_to(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.path == path)
            .count()
    }

    pub fn issuer(&self) -> String {
        format!("{}/realm", self.base())
    }

    pub fn discovery_document(&self) -> Value {
        let base = self.base();
        json!({
            "issuer": self.issuer(),
            "authorization_endpoint": format!("{base}/realm/authorize"),
            "token_endpoint": format!("{base}/realm/token"),
            "userinfo_endpoint": format!("{base}/realm/userinfo"),
            "jwks_uri": format!("{base}/realm/jwks"),
            "scopes_supported": ["openid", "email", "profile"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        })
    }

    pub fn serve_healthy_oidc(&self) {
        self.set(
            "/realm/.well-known/openid-configuration",
            Reply::json(&self.discovery_document()),
        );
        self.set(
            "/realm/jwks",
            Reply::json(
                &json!({ "keys": [{ "kty": "RSA", "kid": "k1", "n": "AQAB", "e": "AQAB" }] }),
            ),
        );
        self.set("/realm/token", Reply::status(400));
        self.set("/realm/authorize", Reply::status(400));
        self.set("/realm/userinfo", Reply::status(401));
    }
}

async fn serve(listener: TcpListener, routes: Routes, log: Log) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let routes = Arc::clone(&routes);
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            }
            let head = String::from_utf8_lossy(&buffer).into_owned();
            let mut lines = head.split("\r\n");
            let mut request_line = lines.next().unwrap_or_default().split(' ');
            let method = request_line.next().unwrap_or_default().to_owned();
            let target = request_line.next().unwrap_or_default().to_owned();
            let path = target.split('?').next().unwrap_or_default().to_owned();
            let headers = lines
                .take_while(|line| !line.is_empty())
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
                .collect();
            log.lock().unwrap().push(Recorded {
                method,
                path: path.clone(),
                headers,
            });
            let reply = routes
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .unwrap_or_else(|| Reply::status(404));
            if !reply.delay.is_zero() {
                tokio::time::sleep(reply.delay).await;
            }
            let mut response = format!("HTTP/1.1 {} Reply\r\nConnection: close\r\n", reply.status);
            if reply.sized {
                response.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
            }
            for (name, value) in &reply.headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&reply.body).await;
            let _ = stream.shutdown().await;
        });
    }
}
