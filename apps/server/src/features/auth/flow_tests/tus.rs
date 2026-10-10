use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64ct::{Base64, Encoding};

use super::folders::{Member, HOST_WORK};
use super::transfers::{request_body, sized, unsized_file};
use super::*;
use crate::features::transfers::TusLimits;
use crate::storage::error::StorageError;
use crate::storage::staging::{StagingAppend, StagingStorage, UploadId};

pub(super) const TUS: &str = "/api/v1/uploads/tus";
pub(super) const OCTET: &str = "application/offset+octet-stream";

pub(super) fn small_limits() -> TusLimits {
    TusLimits {
        buffer_bytes: 4_096,
        flush_bytes: 8_192,
        flush_interval: Duration::from_secs(2),
        idle_timeout: Duration::from_secs(60),
        lease_ttl: Duration::from_secs(60),
        upload_ttl: Duration::from_secs(24 * 60 * 60),
    }
}

pub(super) fn b64(text: &str) -> String {
    Base64::encode_string(text.as_bytes())
}

pub(super) fn metadata(
    session: &str,
    item: &str,
    filename: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut pairs = vec![
        format!("filename {}", b64(filename)),
        format!("transferSessionId {}", b64(session)),
        format!("itemId {}", b64(item)),
    ];
    for (key, value) in extra {
        pairs.push(format!("{key} {}", b64(value)));
    }
    pairs.join(",")
}

pub(super) struct TusReq<'a> {
    method: Method,
    path: String,
    member: Option<&'a Member>,
    headers: Vec<(String, String)>,
    body: Body,
    version: bool,
    csrf: bool,
    host: u8,
}

impl<'a> TusReq<'a> {
    pub(super) fn new(method: Method, path: &str, member: &'a Member) -> Self {
        Self {
            method,
            path: path.to_owned(),
            member: Some(member),
            headers: Vec::new(),
            body: Body::empty(),
            version: true,
            csrf: true,
            host: HOST_WORK,
        }
    }

    pub(super) fn anonymous(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            member: None,
            headers: Vec::new(),
            body: Body::empty(),
            version: true,
            csrf: true,
            host: HOST_WORK,
        }
    }

    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub(super) fn body(mut self, body: Body) -> Self {
        self.body = body;
        self.headers
            .push(("content-type".to_owned(), OCTET.to_owned()));
        self
    }

    pub(super) fn raw_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    pub(super) fn bytes(self, data: &[u8]) -> Self {
        let length = data.len().to_string();
        self.header("content-length", &length)
            .body(Body::from(data.to_vec()))
    }

    pub(super) fn no_version(mut self) -> Self {
        self.version = false;
        self
    }

    pub(super) fn no_csrf(mut self) -> Self {
        self.csrf = false;
        self
    }

    pub(super) fn via_peer(mut self, host: u8) -> Self {
        self.host = host;
        self
    }
}

pub(super) struct Planned {
    pub(super) session: String,
    pub(super) item: String,
    pub(super) name: String,
}

pub(super) struct Gate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

impl Gate {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        })
    }

    async fn pass(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }

    pub(super) async fn wait_until_entered(&self) {
        self.entered.acquire().await.unwrap().forget();
    }

    pub(super) fn open(&self) {
        self.release.add_permits(10_000);
    }
}

type SharedGate = Arc<Mutex<Option<Arc<Gate>>>>;

pub(super) struct TestStaging {
    inner: Arc<dyn StagingStorage>,
    pub(super) fail_ensure: AtomicBool,
    pub(super) fail_write_after: AtomicI64,
    pub(super) ensure_calls: AtomicUsize,
    pub(super) probe: Mutex<Option<crate::infra::db::DbPools>>,
    pub(super) overlapped_transaction: AtomicBool,
    pub(super) fail_remove: AtomicBool,
    pub(super) ensure_gate: SharedGate,
    pub(super) write_gate: SharedGate,
    pub(super) max_chunk: Arc<AtomicUsize>,
    pub(super) flushes: Arc<AtomicUsize>,
    clock: TestClock,
}

impl TestStaging {
    pub(super) fn new(root: &Path, clock: &TestClock) -> Arc<Self> {
        Arc::new(Self {
            inner: crate::storage::staging::temporary_local(root),
            fail_ensure: AtomicBool::new(false),
            fail_write_after: AtomicI64::new(-1),
            ensure_calls: AtomicUsize::new(0),
            probe: Mutex::new(None),
            overlapped_transaction: AtomicBool::new(false),
            fail_remove: AtomicBool::new(false),
            ensure_gate: Arc::new(Mutex::new(None)),
            write_gate: Arc::new(Mutex::new(None)),
            max_chunk: Arc::new(AtomicUsize::new(0)),
            flushes: Arc::new(AtomicUsize::new(0)),
            clock: clock.clone(),
        })
    }

    async fn probe_for_open_transaction(&self) {
        let pools = self.probe.lock().unwrap().clone();
        let Some(pools) = pools else { return };
        let attempt = tokio::time::timeout(
            Duration::from_millis(400),
            pools.write_tx(&self.clock, "tus.test_probe", async |_tx| {
                Ok::<(), crate::infra::db::DbError>(())
            }),
        )
        .await;
        if attempt.is_err() {
            self.overlapped_transaction.store(true, Ordering::SeqCst);
        }
    }
}

struct TestAppend {
    gate: SharedGate,
    inner: Box<dyn StagingAppend>,
    budget: Arc<AtomicI64>,
    max_chunk: Arc<AtomicUsize>,
    flushes: Arc<AtomicUsize>,
}

#[async_trait]
impl StagingAppend for TestAppend {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        self.max_chunk.fetch_max(chunk.len(), Ordering::SeqCst);
        let limit = self.budget.load(Ordering::SeqCst);
        if limit >= 0 {
            let remaining = limit - i64::try_from(chunk.len()).unwrap();
            if remaining < 0 {
                return Err(StorageError::QuotaOnDevice);
            }
            self.budget.store(remaining, Ordering::SeqCst);
        }
        self.inner.write_chunk(chunk).await
    }

    async fn flush(&mut self) -> Result<(), StorageError> {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        self.inner.flush().await
    }

    async fn len(&mut self) -> Result<u64, StorageError> {
        self.inner.len().await
    }
}

#[async_trait]
impl StagingStorage for TestStaging {
    async fn ensure(&self, id: &UploadId) -> Result<Box<dyn StagingAppend>, StorageError> {
        self.ensure_calls.fetch_add(1, Ordering::SeqCst);
        self.probe_for_open_transaction().await;
        let gate = self.ensure_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        if self.fail_ensure.load(Ordering::SeqCst) {
            return Err(StorageError::PermissionDenied);
        }
        let budget = Arc::new(AtomicI64::new(self.fail_write_after.load(Ordering::SeqCst)));
        let inner = self.inner.ensure(id).await?;
        Ok(Box::new(TestAppend {
            gate: Arc::clone(&self.write_gate),
            inner,
            budget,
            max_chunk: Arc::clone(&self.max_chunk),
            flushes: Arc::clone(&self.flushes),
        }))
    }

    async fn write_hint(&self, id: &UploadId, hint: &[u8]) -> Result<(), StorageError> {
        self.inner.write_hint(id, hint).await
    }

    async fn staged_len(&self, id: &UploadId) -> Result<Option<u64>, StorageError> {
        self.inner.staged_len(id).await
    }

    async fn remove(&self, id: &UploadId) -> Result<bool, StorageError> {
        self.probe_for_open_transaction().await;
        if self.fail_remove.load(Ordering::SeqCst) {
            return Err(StorageError::PermissionDenied);
        }
        self.inner.remove(id).await
    }
}

pub(super) struct TusRow {
    pub(super) state: String,
    pub(super) offset: i64,
    pub(super) length: Option<i64>,
    pub(super) defer: i64,
    pub(super) staging_path: String,
    pub(super) locked_by: Option<String>,
    pub(super) expires_at: String,
}

impl Stack {
    pub(super) async fn tus_send(&self, request: TusReq<'_>) -> Fetched {
        let mut builder = Request::builder()
            .method(request.method.clone())
            .uri(request.path)
            .header(ORIGIN, BASE_URL);
        if request.version {
            builder = builder.header("tus-resumable", "1.0.0");
        }
        if let Some(member) = request.member {
            let creds = &member.creds;
            builder = builder.header(
                COOKIE,
                format!("palmr_session={}; palmr_csrf={}", creds.session, creds.csrf),
            );
            let state_changing = matches!(
                request.method,
                Method::POST | Method::PUT | Method::PATCH | Method::DELETE
            );
            if request.csrf && state_changing {
                builder = builder.header(CSRF_HEADER, &creds.csrf);
            }
        }
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        self.send(with_peer(builder.body(request.body).unwrap(), request.host))
            .await
    }

    pub(super) async fn planned(&self, member: &Member, name: &str, size: Option<u64>) -> Planned {
        let file = match size {
            Some(size) => sized("c1", name, size),
            None => unsized_file("c1", name),
        };
        let created = self.open(member, &request_body(None, &[file])).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        let session = created.json()["id"].as_str().unwrap().to_owned();
        let item = self.item_ids(&session).await.remove(0);
        Planned {
            session,
            item,
            name: name.to_owned(),
        }
    }

    pub(super) fn create_req<'a>(
        &self,
        member: &'a Member,
        planned: &Planned,
        length: Option<u64>,
    ) -> TusReq<'a> {
        let request = TusReq::new(Method::POST, TUS, member).header(
            "upload-metadata",
            &metadata(&planned.session, &planned.item, &planned.name, &[]),
        );
        match length {
            Some(length) => request.header("upload-length", &length.to_string()),
            None => request.header("upload-defer-length", "1"),
        }
    }

    pub(super) async fn tus_create(
        &self,
        member: &Member,
        planned: &Planned,
        length: Option<u64>,
    ) -> Fetched {
        self.tus_send(self.create_req(member, planned, length))
            .await
    }

    pub(super) async fn tus_created(
        &self,
        member: &Member,
        planned: &Planned,
        length: Option<u64>,
    ) -> String {
        let created = self.tus_create(member, planned, length).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        upload_id_of(&created)
    }

    pub(super) async fn tus_row(&self, id: &str) -> TusRow {
        let (state, offset, length, defer, staging_path, locked_by, expires_at): (
            String,
            i64,
            Option<i64>,
            i64,
            String,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT state, upload_offset, upload_length, upload_defer_length, staging_path,
                    locked_by, expires_at FROM tus_uploads WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        TusRow {
            state,
            offset,
            length,
            defer,
            staging_path,
            locked_by,
            expires_at,
        }
    }

    pub(super) fn staged_hint(&self, id: &str) -> String {
        use std::io::Read as _;
        let path = self
            .data_dir
            .join("uploads")
            .join(id.replace('-', ""))
            .join("meta.json");
        std::io::read_to_string(std::fs::File::open(path).unwrap().take(8_192)).unwrap()
    }

    pub(super) fn staging_dirs(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.data_dir.join("uploads"))
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    pub(super) fn staged_bytes(&self, id: &str) -> Option<Vec<u8>> {
        let path = self
            .data_dir
            .join("uploads")
            .join(id.replace('-', ""))
            .join("blob");
        use std::io::Read as _;
        let file = std::fs::File::open(path).ok()?;
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    pub(super) async fn tus_count(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM tus_uploads").await
    }

    pub(super) async fn item_state(&self, item: &str) -> String {
        sqlx::query_scalar("SELECT state FROM transfer_session_files WHERE id = ?1")
            .bind(item)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }
}

pub(super) fn upload_id_of(created: &Fetched) -> String {
    created
        .headers
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned()
}

pub(super) fn header<'a>(fetched: &'a Fetched, name: &str) -> &'a str {
    fetched
        .headers
        .get(name)
        .unwrap_or_else(|| panic!("missing {name}: {:?}", fetched.headers))
        .to_str()
        .unwrap()
}

pub(super) fn assert_head_error(fetched: &Fetched, status: StatusCode) {
    assert_eq!(fetched.status, status);
    assert!(fetched.body.is_empty(), "a HEAD response has no body");
    assert_eq!(header(fetched, "tus-resumable"), "1.0.0");
    assert!(fetched.headers.contains_key("x-request-id"));
    for leaked in [
        "upload-offset",
        "upload-length",
        "upload-defer-length",
        "location",
    ] {
        assert!(!fetched.headers.contains_key(leaked), "{leaked}");
    }
}

pub(super) fn assert_tus_error(fetched: &Fetched, status: StatusCode, code: &str) {
    assert_eq!(fetched.status, status, "{}", fetched.text());
    assert_eq!(fetched.error_code(), code, "{}", fetched.text());
    assert_eq!(header(fetched, "tus-resumable"), "1.0.0");
    assert!(fetched.headers.contains_key("x-request-id"));
    let body = fetched.json();
    assert_eq!(
        body["error"]["requestId"].as_str().unwrap(),
        header(fetched, "x-request-id")
    );
}

pub(super) fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect()
}

pub(super) fn frames(data: &[u8], sizes: &[usize]) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut at = 0;
    for size in sizes {
        out.push(Bytes::copy_from_slice(&data[at..at + size]));
        at += size;
    }
    assert_eq!(at, data.len());
    out
}

pub(super) fn stream_of(frames: Vec<Bytes>) -> Body {
    Body::from_stream(futures_util::stream::iter(
        frames.into_iter().map(Ok::<_, std::io::Error>),
    ))
}

pub(super) fn broken_stream(frames: Vec<Bytes>) -> Body {
    let items: Vec<Result<Bytes, std::io::Error>> = frames
        .into_iter()
        .map(Ok)
        .chain(std::iter::once(Err(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        ))))
        .collect();
    Body::from_stream(futures_util::stream::iter(items))
}

pub(super) type FrameSender = tokio::sync::mpsc::UnboundedSender<Result<Bytes, std::io::Error>>;

pub(super) fn channel_body() -> (FrameSender, Body) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    (sender, Body::from_stream(stream))
}

pub(super) async fn wait_for_offset(stack: &Stack, id: &str, offset: i64) {
    for _ in 0..500 {
        if stack.tus_row(id).await.offset >= offset {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the offset never reached {offset}");
}
