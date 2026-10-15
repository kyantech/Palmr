use std::collections::VecDeque;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt as _;
use url::Url;

use super::checksum_gate::ChecksumGate;
use super::finalize::Hooks;
use super::folders::{Member, HOST_A};
use super::transfers::sized;
use super::tus::pattern;
use super::*;
use crate::config::{S3Config, S3Profile, S3TlsVerification, StorageConfig};
use crate::domain::clock::Clock as _;
use crate::features::transfers::finalize::{FinalizeLimits, Finalizer, FinalizerParts, Point};
use crate::features::transfers::{ProxyLimits, TransferStorage};
use crate::infra::db::DbPools;
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::{Retryable, StorageError};
use crate::storage::health::{ProbeDepth, SelfTestReport, StorageHealth};
use crate::storage::key::ObjectKey;
use crate::storage::planning::UploadPlanner;
use crate::storage::provider::{
    ListCursor, ListPage, MultipartHandle, MultipartStorage, MultipartUploadPage, ObjectBody,
    ObjectStat, PartChecksum, PartPlanEntry, PendingMultipart, PresignStorage, PresignedRequest,
    PutHint, StorageDescriptor, StorageProvider, UploadedPart,
};
use crate::storage::s3::client::S3Clients;
use crate::storage::s3::profile::{ProviderProfile, MIB};
use crate::storage::s3::{classify_failure, Failure, Operation, S3Provider};

use crate::storage::s3::object_tests::minio::MinioServer;

const BUFFER_BYTES: u32 = 64 * 1024;
const SESSIONS: &str = "/api/v1/transfers/sessions";
const PART: usize = 8 * 1024 * 1024;
const PUBLIC_HOST: &str = "public.invalid";

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

pub(super) fn timeout() -> StorageError {
    StorageError::ProviderUnavailable(Retryable::new(crate::storage::s3::S3Failure::new(
        Operation::CompleteMultipartUpload,
        Failure::Timeout,
    )))
}

pub(super) fn upstream() -> StorageError {
    classify_failure(
        Operation::CompleteMultipartUpload,
        Failure::Service {
            status: 503,
            code: Some("ServiceUnavailable".to_owned()),
        },
    )
}

pub(super) enum PartFault {
    Before(fn() -> StorageError),
    After(fn() -> StorageError),
}

pub(super) struct PartGate {
    pub(super) entered: tokio::sync::Semaphore,
    pub(super) release: tokio::sync::Semaphore,
}

impl PartGate {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        })
    }
}

pub(super) enum CompleteFault {
    Before(fn() -> StorageError),
    After(fn() -> StorageError),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ListMode {
    #[default]
    Native,
    PrefixAware,
    ExactOnly,
    NotImplemented,
}

#[derive(Clone, Default)]
pub(super) struct Listing {
    pub(super) mode: ListMode,
    pub(super) page_size: Option<usize>,
    pub(super) fail_page: Option<usize>,
    pub(super) repeat_cursor: bool,
}

#[derive(Default)]
pub(super) struct Faults {
    pub(super) listing: Mutex<Listing>,
    pub(super) list_uploads_calls: AtomicUsize,
    pub(super) abort_after: Mutex<VecDeque<fn() -> StorageError>>,
    pub(super) abort_log: Mutex<Vec<(String, String)>>,
    pub(super) abort_gate: Mutex<Option<Arc<PartGate>>>,
    pub(super) complete: Mutex<VecDeque<CompleteFault>>,
    pub(super) list_parts: Mutex<VecDeque<fn() -> StorageError>>,
    pub(super) abort: Mutex<VecDeque<fn() -> StorageError>>,
    pub(super) part: Mutex<VecDeque<PartFault>>,
    pub(super) part_gate: Mutex<Option<Arc<PartGate>>>,
    pub(super) part_log: Mutex<Vec<(u32, u64, PartChecksum)>>,
    pub(super) part_calls: AtomicUsize,
    pub(super) stat_size: Mutex<Option<u64>>,
    pub(super) creates: AtomicUsize,
    pub(super) completes: AtomicUsize,
    pub(super) list_calls: AtomicUsize,
    pub(super) aborts: AtomicUsize,
    pub(super) signs: AtomicUsize,
    pub(super) puts: AtomicUsize,
    pub(super) bytes_read: AtomicU64,
    pub(super) overlapped: AtomicBool,
    probe: Mutex<Option<(DbPools, TestClock)>>,
}

impl Faults {
    async fn guard(&self) {
        let probe = self.probe.lock().unwrap().clone();
        let Some((pools, clock)) = probe else { return };
        let attempt = tokio::time::timeout(
            Duration::from_millis(400),
            pools.write_tx(&clock, "s3.test_probe", async |_tx| {
                Ok::<(), crate::infra::db::DbError>(())
            }),
        )
        .await;
        if attempt.is_err() {
            self.overlapped.store(true, Ordering::SeqCst);
        }
    }
}

pub(super) struct Faulty {
    inner: Arc<S3Provider>,
    faults: Arc<Faults>,
}

#[async_trait]
impl StorageProvider for Faulty {
    fn caps(&self) -> &StorageCapabilities {
        self.inner.caps()
    }

    fn describe(&self) -> StorageDescriptor {
        self.inner.describe()
    }

    async fn put_stream(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        self.faults.guard().await;
        self.faults.puts.fetch_add(1, Ordering::SeqCst);
        self.inner.put_stream(key, body, hint).await
    }

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.inner.open_read(key).await
    }

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.faults.guard().await;
        self.faults.bytes_read.fetch_add(len, Ordering::SeqCst);
        self.inner.open_range(key, start, len).await
    }

    async fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.faults.guard().await;
        let mut stat = self.inner.stat(key).await?;
        if let Some(size) = *self.faults.stat_size.lock().unwrap() {
            stat.size = size;
        }
        Ok(stat)
    }

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.inner.delete(key).await
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.inner.exists(key).await
    }

    async fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.inner.copy(src, dst).await
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        self.inner.list_page(prefix, cursor, page_size).await
    }

    async fn self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        self.inner.self_test(depth).await
    }

    fn as_multipart(&self) -> Option<&dyn MultipartStorage> {
        Some(self)
    }

    fn as_presign(&self) -> Option<&dyn PresignStorage> {
        self.inner.as_presign()
    }
}

#[async_trait]
impl MultipartStorage for Faulty {
    async fn create_multipart(
        &self,
        key: &ObjectKey,
        hint: PutHint,
    ) -> Result<MultipartHandle, StorageError> {
        self.faults.guard().await;
        self.faults.creates.fetch_add(1, Ordering::SeqCst);
        self.inner.create_multipart(key, hint).await
    }

    async fn sign_part_urls(
        &self,
        handle: &MultipartHandle,
        parts: &[PartPlanEntry],
        ttl: Duration,
    ) -> Result<Vec<PresignedRequest>, StorageError> {
        self.faults.signs.fetch_add(1, Ordering::SeqCst);
        self.inner.sign_part_urls(handle, parts, ttl).await
    }

    async fn list_parts(
        &self,
        handle: &MultipartHandle,
    ) -> Result<Vec<UploadedPart>, StorageError> {
        self.faults.guard().await;
        self.faults.list_calls.fetch_add(1, Ordering::SeqCst);
        let fault = self.faults.list_parts.lock().unwrap().pop_front();
        if let Some(fault) = fault {
            return Err(fault());
        }
        self.inner.list_parts(handle).await
    }

    async fn upload_part_stream(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        len: u64,
        body: ObjectBody,
    ) -> Result<UploadedPart, StorageError> {
        self.inner
            .upload_part_stream(handle, part_number, len, body)
            .await
    }

    async fn upload_part_checksummed(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        len: u64,
        body: ObjectBody,
        checksum: PartChecksum,
    ) -> Result<UploadedPart, StorageError> {
        self.faults.guard().await;
        self.faults.part_calls.fetch_add(1, Ordering::SeqCst);
        self.faults
            .part_log
            .lock()
            .unwrap()
            .push((part_number, len, checksum));
        let gate = self.faults.part_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
        let fault = self.faults.part.lock().unwrap().pop_front();
        match fault {
            Some(PartFault::Before(error)) => Err(error()),
            Some(PartFault::After(error)) => {
                self.inner
                    .upload_part_checksummed(handle, part_number, len, body, checksum)
                    .await?;
                Err(error())
            }
            None => {
                self.inner
                    .upload_part_checksummed(handle, part_number, len, body, checksum)
                    .await
            }
        }
    }

    async fn upload_part_copy(
        &self,
        handle: &MultipartHandle,
        part_number: u32,
        src: &ObjectKey,
        range: RangeInclusive<u64>,
    ) -> Result<UploadedPart, StorageError> {
        self.inner
            .upload_part_copy(handle, part_number, src, range)
            .await
    }

    async fn complete_multipart(
        &self,
        handle: &MultipartHandle,
        parts: &[UploadedPart],
    ) -> Result<ObjectStat, StorageError> {
        self.faults.guard().await;
        self.faults.completes.fetch_add(1, Ordering::SeqCst);
        let fault = self.faults.complete.lock().unwrap().pop_front();
        match fault {
            Some(CompleteFault::Before(error)) => Err(error()),
            Some(CompleteFault::After(error)) => {
                self.inner.complete_multipart(handle, parts).await?;
                Err(error())
            }
            None => self.inner.complete_multipart(handle, parts).await,
        }
    }

    async fn abort_multipart(&self, handle: &MultipartHandle) -> Result<(), StorageError> {
        if handle
            .key()
            .as_str()
            .starts_with(crate::features::transfers::s3::reapers::SENTINEL_PREFIX)
        {
            return self.inner.abort_multipart(handle).await;
        }
        self.faults.guard().await;
        self.faults.aborts.fetch_add(1, Ordering::SeqCst);
        self.faults.abort_log.lock().unwrap().push((
            handle.key().as_str().to_owned(),
            handle.upload_id().to_owned(),
        ));
        let gate = self.faults.abort_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
        let fault = self.faults.abort.lock().unwrap().pop_front();
        if let Some(fault) = fault {
            return Err(fault());
        }
        self.inner.abort_multipart(handle).await?;
        let after = self.faults.abort_after.lock().unwrap().pop_front();
        match after {
            Some(fault) => Err(fault()),
            None => Ok(()),
        }
    }

    async fn list_multipart_uploads(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
    ) -> Result<MultipartUploadPage, StorageError> {
        self.faults.guard().await;
        self.faults
            .list_uploads_calls
            .fetch_add(1, Ordering::SeqCst);
        let listing = self.faults.listing.lock().unwrap().clone();
        match listing.mode {
            ListMode::NotImplemented => {
                return Err(classify_failure(
                    Operation::ListMultipartUploads,
                    Failure::Service {
                        status: 501,
                        code: Some("NotImplemented".to_owned()),
                    },
                ))
            }
            ListMode::ExactOnly if prefix.is_empty() => {
                return Ok(MultipartUploadPage {
                    uploads: Vec::new(),
                    next: None,
                })
            }
            ListMode::ExactOnly => return self.inner.list_multipart_uploads(prefix, cursor).await,
            ListMode::Native if listing.page_size.is_none() && !listing.repeat_cursor => {
                return self.inner.list_multipart_uploads(prefix, cursor).await
            }
            _ => {}
        }
        let mut all: Vec<PendingMultipart> = Vec::new();
        let mut inner_cursor = None;
        loop {
            let page = self.inner.list_multipart_uploads("", inner_cursor).await?;
            all.extend(page.uploads);
            match page.next {
                Some(next) => inner_cursor = Some(next),
                None => break,
            }
        }
        all.retain(|upload| match listing.mode {
            ListMode::PrefixAware => upload.handle.key().as_str().starts_with(prefix),
            _ => prefix.is_empty() || upload.handle.key().as_str() == prefix,
        });
        let size = listing.page_size.unwrap_or(1_000);
        let offset = cursor
            .as_ref()
            .map_or(0, |cursor| cursor.as_str().parse::<usize>().unwrap_or(0));
        if listing.fail_page == Some(offset / size + 1) {
            return Err(timeout());
        }
        let end = (offset + size).min(all.len());
        let uploads = all.get(offset..end).unwrap_or_default().to_vec();
        let next = if listing.repeat_cursor {
            Some(ListCursor::new("0"))
        } else {
            (end < all.len()).then(|| ListCursor::new(end.to_string()))
        };
        Ok(MultipartUploadPage { uploads, next })
    }
}

fn provider_for(server: &MinioServer, bucket: &str, public: Option<&str>) -> S3Provider {
    provider_with(
        server,
        server.endpoint(),
        bucket,
        public,
        (S3Profile::Minio, None),
    )
}

fn provider_with(
    server: &MinioServer,
    endpoint: &str,
    bucket: &str,
    public: Option<&str>,
    (profile, require_part_checksums): (S3Profile, Option<bool>),
) -> S3Provider {
    let config = S3Config {
        profile,
        endpoint: Url::parse(endpoint).unwrap(),
        public_endpoint: public.map(|public| Url::parse(public).unwrap()),
        region: "us-east-1".to_owned(),
        bucket: bucket.to_owned(),
        access_key: Secret::new(server.access_key().to_owned()),
        secret_key: Secret::new(server.secret_key().to_owned()),
        force_path_style: true,
        ca_file: None,
        tls_verification: S3TlsVerification::Enabled,
        multipart_ttl: Duration::from_secs(24 * 60 * 60),
        require_part_checksums,
    };
    let clients = S3Clients::build(&StorageConfig::S3(Box::new(config)))
        .unwrap()
        .unwrap();
    S3Provider::new(
        clients,
        BUFFER_BYTES,
        &Url::parse("https://files.example.test").unwrap(),
    )
    .unwrap()
}

pub(super) struct S3Rig {
    pub(super) stack: Stack,
    pub(super) storage: TransferStorage,
    pub(super) minio: MinioServer,
    pub(super) bucket: String,
    pub(super) provider: Arc<S3Provider>,
    pub(super) faults: Arc<Faults>,
    pub(super) hooks: Arc<Hooks>,
    pub(super) proxied: Arc<AtomicBool>,
    pub(super) gate: Option<ChecksumGate>,
    pub(super) alice: Member,
    pub(super) bob: Member,
    pub(super) http: reqwest::Client,
    _dir: TempDir,
}

#[derive(Clone, Default)]
pub(super) struct RigOptions {
    pub(super) public: bool,
    pub(super) checksum_gate: bool,
    pub(super) proxy: Option<ProxyLimits>,
    pub(super) shutdown: Option<tokio_util::sync::CancellationToken>,
    pub(super) profile: Option<S3Profile>,
    pub(super) require_part_checksums: Option<bool>,
}

impl S3Rig {
    pub(super) async fn new() -> Self {
        Self::build(RigOptions::default()).await
    }

    pub(super) async fn with_public_host() -> Self {
        Self::build(RigOptions {
            public: true,
            ..RigOptions::default()
        })
        .await
    }

    pub(super) async fn checksum_demanding() -> Self {
        Self::build(RigOptions {
            checksum_gate: true,
            ..RigOptions::default()
        })
        .await
    }

    pub(super) async fn build(options: RigOptions) -> Self {
        let RigOptions {
            public,
            checksum_gate,
            proxy,
            shutdown,
            profile,
            require_part_checksums,
        } = options;
        let dir = TempDir::new().unwrap();
        let clock = TestClock::new(START);
        let minio = MinioServer::start().await.unwrap();
        let bucket = minio.create_bucket("s3mp").await.unwrap();
        let minio_port = Url::parse(minio.endpoint()).unwrap().port().unwrap();
        let public_endpoint = public.then(|| format!("http://{PUBLIC_HOST}:{minio_port}"));
        let gate = if checksum_gate {
            Some(ChecksumGate::start(minio.endpoint()).await)
        } else {
            None
        };
        let configured = profile.is_some() || require_part_checksums.is_some();
        let profile = profile.unwrap_or(if gate.is_some() {
            S3Profile::R2
        } else {
            S3Profile::Minio
        });
        let endpoint = gate
            .as_ref()
            .map_or_else(|| minio.endpoint().to_owned(), ChecksumGate::endpoint);
        let provider = Arc::new(provider_with(
            &minio,
            &endpoint,
            &bucket,
            public_endpoint.as_deref(),
            (profile, require_part_checksums),
        ));
        let faults = Arc::new(Faults::default());
        let faulty: Arc<dyn StorageProvider> = Arc::new(Faulty {
            inner: Arc::clone(&provider),
            faults: Arc::clone(&faults),
        });
        let proxied = Arc::new(AtomicBool::new(false));
        let planner = if gate.is_some() || configured {
            let capabilities = Arc::clone(&provider);
            UploadPlanner::s3(
                ProviderProfile::from_config(profile),
                Arc::new(move || capabilities.caps().requires_checksum_headers),
            )
        } else {
            let proxied = Arc::clone(&proxied);
            UploadPlanner::s3(
                ProviderProfile::Minio,
                Arc::new(move || proxied.load(Ordering::SeqCst)),
            )
        };
        let storage = TransferStorage::new(planner, Arc::new(|| StorageHealth::Ok))
            .with_object_store(faulty, bucket.clone(), Duration::from_secs(24 * 60 * 60));
        let reaper_storage = storage.clone();
        let hooks = Arc::new(Hooks::default());
        let setup = FinalizeSetup {
            hooks: Some(hooks.clone()),
            ..FinalizeSetup::default()
        };
        let stack = Stack::start_with_setup(
            dir.path(),
            &clock,
            Routes::new(),
            &[],
            Some(storage),
            TusSetup {
                finalize: Some(setup),
                proxy: proxy.unwrap_or_else(ProxyLimits::production),
                shutdown: shutdown.unwrap_or_default(),
                ..TusSetup::default()
            },
        )
        .await;
        *faults.probe.lock().unwrap() = Some((stack.pools.clone(), clock.clone()));
        let alice = stack.member("alice", HOST_A).await;
        let bob = stack.member("bob", HOST_A).await;
        Self {
            stack,
            storage: reaper_storage,
            minio,
            bucket,
            provider,
            faults,
            hooks,
            proxied,
            gate,
            alice,
            bob,
            http: {
                let builder = reqwest::Client::builder();
                let builder = if public {
                    builder.resolve(
                        PUBLIC_HOST,
                        std::net::SocketAddr::from(([127, 0, 0, 1], minio_port)),
                    )
                } else {
                    builder
                };
                builder.build().unwrap()
            },
            _dir: dir,
        }
    }

    pub(super) fn path(session: &str, item: &str, tail: &str) -> String {
        format!("{SESSIONS}/{session}/files/{item}/s3/multipart{tail}")
    }

    pub(super) async fn session(&self, files: &[Value]) -> (String, Vec<String>) {
        let opened = self.stack.opened(&self.alice, files).await;
        let session = opened["id"].as_str().unwrap().to_owned();
        let items = self.stack.item_ids(&session).await;
        (session, items)
    }

    pub(super) async fn one(&self, name: &str, size: u64) -> (String, String) {
        let (session, mut items) = self.session(&[sized("c1", name, size)]).await;
        (session, items.remove(0))
    }

    pub(super) async fn post(&self, who: &Member, path: &str, body: Option<&Value>) -> Fetched {
        self.stack.api(Method::POST, path, who, body).await
    }

    pub(super) async fn create(&self, session: &str, item: &str) -> Fetched {
        self.post(&self.alice, &Self::path(session, item, ""), None)
            .await
    }

    pub(super) async fn presign(&self, session: &str, item: &str, numbers: &[u32]) -> Fetched {
        self.post(
            &self.alice,
            &Self::path(session, item, "/presign"),
            Some(&json!({ "partNumbers": numbers })),
        )
        .await
    }

    pub(super) async fn report(&self, session: &str, item: &str, parts: Value) -> Fetched {
        self.post(
            &self.alice,
            &Self::path(session, item, "/parts"),
            Some(&json!({ "parts": parts })),
        )
        .await
    }

    pub(super) async fn parts(&self, session: &str, item: &str) -> Fetched {
        self.stack
            .api(
                Method::GET,
                &Self::path(session, item, "/parts"),
                &self.alice,
                None,
            )
            .await
    }

    pub(super) async fn complete(&self, session: &str, item: &str) -> Fetched {
        self.post(&self.alice, &Self::path(session, item, "/complete"), None)
            .await
    }

    pub(super) async fn abort(&self, session: &str, item: &str) -> Fetched {
        self.post(&self.alice, &Self::path(session, item, "/abort"), None)
            .await
    }

    pub(super) async fn put_part(&self, url: &str, bytes: Vec<u8>) -> String {
        let response = self.http.put(url).body(bytes).send().await.unwrap();
        let status = response.status();
        let etag = response
            .headers()
            .get("etag")
            .map(|value| value.to_str().unwrap().to_owned());
        assert!(status.is_success(), "the presigned PUT answered {status}");
        etag.expect("the provider returns an ETag")
    }

    pub(super) async fn upload_parts(
        &self,
        session: &str,
        item: &str,
        data: &[u8],
        numbers: &[u32],
        report: bool,
    ) {
        for batch in numbers.chunks(16) {
            let presigned = self.presign(session, item, batch).await;
            assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
            let mut reports = Vec::new();
            for entry in presigned.json()["parts"].as_array().unwrap() {
                let number = usize::try_from(entry["partNumber"].as_u64().unwrap()).unwrap();
                let start = (number - 1) * PART;
                let end = (number * PART).min(data.len());
                let etag = self
                    .put_part(entry["url"].as_str().unwrap(), data[start..end].to_vec())
                    .await;
                reports.push(json!({
                    "partNumber": number,
                    "etag": etag,
                    "sizeBytes": end - start,
                }));
            }
            if report {
                let reported = self.report(session, item, json!(reports)).await;
                assert_eq!(
                    reported.status,
                    StatusCode::NO_CONTENT,
                    "{}",
                    reported.text()
                );
            }
        }
    }

    pub(super) async fn upload_all(&self, session: &str, item: &str, data: &[u8]) {
        let count = u32::try_from(data.len().div_ceil(PART)).unwrap();
        let numbers: Vec<u32> = (1..=count).collect();
        self.upload_parts(session, item, data, &numbers, true).await;
    }

    pub(super) async fn open(&self, session: &str, item: &str, data: &[u8]) {
        let created = self.create(session, item).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        self.upload_all(session, item, data).await;
    }

    pub(super) async fn final_key(&self, item: &str) -> String {
        sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
            .bind(item)
            .fetch_one(self.stack.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn item_row(&self, item: &str) -> (String, String, Option<String>) {
        sqlx::query_as(
            "SELECT state, finalize_stage, error_code FROM transfer_session_files WHERE id = ?1",
        )
        .bind(item)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn multipart_state(&self, item: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT state FROM s3_multipart_uploads WHERE transfer_session_file_id = ?1",
        )
        .bind(item)
        .fetch_optional(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn objects(&self) -> i64 {
        self.stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await
    }

    pub(super) async fn files(&self) -> i64 {
        self.stack.scalar_i64("SELECT COUNT(*) FROM files").await
    }

    pub(super) async fn used(&self) -> i64 {
        self.stack.used_bytes(self.alice.id).await
    }

    pub(super) async fn uploads_audited(&self) -> i64 {
        self.stack
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
            .await
    }

    pub(super) async fn read_object(&self, key: &str) -> (u64, Vec<u8>) {
        let key = ObjectKey::parse(key).unwrap();
        let (_, mut body) = self.provider.open_read(&key).await.unwrap();
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = body.read(&mut buffer).await.unwrap();
            if read == 0 {
                return (total, hasher.finalize().to_vec());
            }
            hasher.update(&buffer[..read]);
            total += read as u64;
        }
    }

    pub(super) async fn object_exists(&self, key: &str) -> bool {
        self.provider
            .exists(&ObjectKey::parse(key).unwrap())
            .await
            .unwrap()
    }

    pub(super) async fn open_uploads(&self, key: &str) -> usize {
        self.provider
            .list_multipart_uploads(key, None)
            .await
            .unwrap()
            .uploads
            .len()
    }

    pub(super) async fn persisted_text(&self) -> String {
        let mut all = String::new();
        for table in [
            "s3_multipart_uploads",
            "s3_multipart_parts",
            "transfer_session_files",
            "transfer_sessions",
            "audit_events",
            "jobs",
            "storage_objects",
            "files",
        ] {
            let columns: Vec<String> =
                sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                    .fetch_all(self.stack.pools.reader().executor())
                    .await
                    .unwrap();
            let expression = columns
                .iter()
                .map(|column| format!("quote({column})"))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            let rows: Vec<String> =
                sqlx::query_scalar(&format!("SELECT {expression} FROM {table}"))
                    .fetch_all(self.stack.pools.reader().executor())
                    .await
                    .unwrap();
            for row in rows {
                all.push_str(&row);
                all.push('\n');
            }
        }
        all
    }

    pub(super) fn assert_no_overlap(&self) {
        assert!(
            !self.faults.overlapped.load(Ordering::SeqCst),
            "a provider call ran while a write transaction was open"
        );
    }

    pub(super) fn second_finalizer(&self) -> Finalizer {
        Finalizer::new(FinalizerParts {
            pools: self.stack.pools.clone(),
            clock: Arc::new(self.stack.clock.clone()),
            settings: self.stack.settings.handle(),
            storage: self.stack.transfer_storage(),
            audit: self.stack.audit.clone(),
            holder: "restarted".to_owned(),
            stop: tokio_util::sync::CancellationToken::new(),
            limits: FinalizeLimits {
                heartbeat: Duration::from_millis(50),
                busy_wait: Duration::from_secs(10),
                busy_poll: Duration::from_millis(5),
                attempts: 3,
                backoff: Duration::from_millis(10),
                ..FinalizeLimits::production()
            },
            hooks: None,
        })
    }

    pub(super) async fn restart(&self) -> Finalizer {
        let finalizer = self.second_finalizer();
        finalizer.start_redrive().await.unwrap();
        finalizer.idle().await;
        finalizer
    }

    pub(super) fn leaked(&self, bodies: &[String], item_key: &str, upload_id: &str) {
        for body in bodies {
            for secret in [
                item_key,
                upload_id,
                self.bucket.as_str(),
                self.minio.access_key(),
                self.minio.secret_key(),
            ] {
                if !secret.is_empty() {
                    assert!(
                        !body.contains(secret),
                        "a response leaks a storage identity: {body}"
                    );
                }
            }
        }
    }
}

fn presented(fetched: &Fetched) -> String {
    fetched.text()
}

#[tokio::test]
async fn it_s3mp_minio_end_to_end_presigned_put_round_trip() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 321;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("round-trip.bin", size).await;
    assert_eq!(rig.objects().await, 0, "no object row at admission");

    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let body = created.json();
    assert_eq!(body["partSizeBytes"], 8 * MIB);
    assert_eq!(body["partCount"], 3);
    assert_eq!(body["maxPresignBatch"], 16);
    assert_eq!(body["presignTtlSeconds"], 900);
    assert_eq!(body["checksumHeadersRequired"], false);
    let mut keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "checksumHeadersRequired",
            "maxPresignBatch",
            "multipartId",
            "partCount",
            "partSizeBytes",
            "presignTtlSeconds"
        ]
    );
    assert_eq!(rig.objects().await, 0, "no object row at creation");
    let upload_id: String = sqlx::query_scalar("SELECT s3_upload_id FROM s3_multipart_uploads")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    let key = rig.final_key(&item).await;
    let stored_key: String = sqlx::query_scalar("SELECT object_key FROM s3_multipart_uploads")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        stored_key, key,
        "the multipart targets the recorded identity"
    );
    assert_eq!(rig.open_uploads(&key).await, 1);

    let mut bodies = vec![created.text()];
    let count = 3_u32;
    let numbers: Vec<u32> = (1..=count).collect();
    let presigned = rig.presign(&session, &item, &numbers).await;
    assert_eq!(presigned.status, StatusCode::OK, "{}", presigned.text());
    assert!(!presigned.text().contains(rig.minio.secret_key()));
    assert!(
        presigned.json().get("bucket").is_none() && presigned.json().get("objectKey").is_none()
    );
    let mut reports = Vec::new();
    for entry in presigned.json()["parts"].as_array().unwrap() {
        let number = usize::try_from(entry["partNumber"].as_u64().unwrap()).unwrap();
        let start = (number - 1) * PART;
        let end = (number * PART).min(data.len());
        let etag = rig
            .put_part(entry["url"].as_str().unwrap(), data[start..end].to_vec())
            .await;
        assert!(etag.starts_with('"'), "the ETag is kept verbatim: {etag}");
        reports.push(json!({ "partNumber": number, "etag": etag, "sizeBytes": end - start }));
    }
    let reported = rig.report(&session, &item, json!(reports)).await;
    assert_eq!(
        reported.status,
        StatusCode::NO_CONTENT,
        "{}",
        reported.text()
    );
    assert_eq!(reported.text(), "");

    let parts = rig.parts(&session, &item).await;
    assert_eq!(parts.status, StatusCode::OK, "{}", parts.text());
    let listed = parts.json();
    assert_eq!(listed["missingPartNumbers"], json!([]));
    assert_eq!(listed["uploadedBytes"], size);
    assert_eq!(listed["completedParts"].as_array().unwrap().len(), 3);
    assert_eq!(listed["partCount"], 3);
    bodies.push(parts.text());
    assert_eq!(rig.objects().await, 0, "ListParts creates no object row");

    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let result = completed.json();
    assert_eq!(result["sizeBytes"], size);
    assert_eq!(result["state"], "completed");
    assert_eq!(result["contentType"], "application/octet-stream");
    bodies.push(completed.text());

    assert_eq!(rig.files().await, 1);
    assert_eq!(rig.objects().await, 1);
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(rig.uploads_audited().await, 1);
    let (object_id, object_key, object_size): (String, String, i64) = sqlx::query_as(
        "SELECT o.id, o.object_key, o.size_bytes FROM storage_objects o WHERE o.state = 'active'",
    )
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    let recorded: (String, String) = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files WHERE id = ?1",
    )
    .bind(&item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        (object_id, object_key),
        recorded,
        "the preallocated identity"
    );
    assert_eq!(object_size, i64::try_from(size).unwrap());
    assert_eq!(rig.read_object(&key).await, (size, sha256(&data)));
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("completed")
    );
    assert_eq!(rig.item_row(&item).await.0, "completed");
    assert_eq!(
        rig.open_uploads(&key).await,
        0,
        "the multipart upload was consumed"
    );
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(i64::try_from(size).unwrap()))
    );
    rig.leaked(&bodies, &key, &upload_id);
    rig.assert_no_overlap();

    let again = rig.complete(&session, &item).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        again.json()["fileId"],
        result["fileId"],
        "the original file"
    );
    assert_eq!(rig.files().await, 1);
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    let _ = presented(&again);
}

fn query_of(url: &str) -> std::collections::BTreeMap<String, String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.into_owned()))
        .collect()
}

#[tokio::test]
async fn it_s3mp_presign_batch_limits() {
    let rig = S3Rig::new().await;
    let size = 17 * 8 * MIB;
    let (session, item) = rig.one("many.bin", size).await;
    let before_create = rig.presign(&session, &item, &[1]).await;
    assert_eq!(before_create.status, StatusCode::NOT_FOUND);
    assert_eq!(before_create.error_code(), "MULTIPART_NOT_FOUND");
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(created.json()["partCount"], 17);

    let signs = rig.faults.signs.load(Ordering::SeqCst);
    let sixteen: Vec<u32> = (1..=16).collect();
    let accepted = rig.presign(&session, &item, &sixteen).await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.text());
    let body = accepted.json();
    assert_eq!(body["ttlSeconds"], 900);
    let parts = body["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 16);
    for (index, part) in parts.iter().enumerate() {
        assert_eq!(part["partNumber"], index + 1);
        let query = query_of(part["url"].as_str().unwrap());
        assert_eq!(query["x-amz-expires"], "900");
        assert_eq!(query["x-amz-signedheaders"], "host");
        assert_eq!(query["partnumber"], (index + 1).to_string());
        for name in query.keys() {
            assert!(!name.starts_with("x-amz-checksum"), "{name}");
            assert_ne!(name, "x-amz-sdk-checksum-algorithm");
            assert!(!name.contains("content-type"), "{name}");
        }
    }
    assert_eq!(rig.faults.signs.load(Ordering::SeqCst), signs + 1);

    let all: Vec<u32> = (1..=17).collect();
    let refused = rig.presign(&session, &item, &all).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refused.error_code(), "PRESIGN_BATCH_TOO_LARGE");
    assert_eq!(refused.json()["error"]["details"]["maxItems"], 16);
    assert_eq!(
        rig.faults.signs.load(Ordering::SeqCst),
        signs + 1,
        "a refused batch signs nothing"
    );

    let path = S3Rig::path(&session, &item, "/presign");
    for (numbers, code) in [
        (json!([0]), "MULTIPART_PART_INVALID"),
        (json!([18]), "MULTIPART_PART_INVALID"),
        (json!([-1]), "MULTIPART_PART_INVALID"),
        (json!([1, 1]), "MULTIPART_PART_INVALID"),
        (json!(["1"]), "MULTIPART_PART_INVALID"),
        (json!([1.5]), "MULTIPART_PART_INVALID"),
        (json!([null]), "MULTIPART_PART_INVALID"),
        (json!([4_294_967_297_u64]), "MULTIPART_PART_INVALID"),
        (json!([]), "VALIDATION_ERROR"),
        (json!("1"), "VALIDATION_ERROR"),
    ] {
        let fetched = rig
            .post(&rig.alice, &path, Some(&json!({ "partNumbers": numbers })))
            .await;
        assert_eq!(
            fetched.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{numbers}"
        );
        assert_eq!(fetched.error_code(), code, "{numbers}");
    }
    for body in [json!({}), json!({ "partNumbers": [1], "key": "x" })] {
        let fetched = rig.post(&rig.alice, &path, Some(&body)).await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(fetched.error_code(), "VALIDATION_ERROR");
    }
    assert_eq!(rig.faults.signs.load(Ordering::SeqCst), signs + 1);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_presign_signs_the_public_endpoint_and_servers_use_the_internal_one() {
    let rig = S3Rig::with_public_host().await;
    let size = 9 * MIB;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("public.bin", size).await;
    rig.open(&session, &item, &data).await;
    let presigned = rig.presign(&session, &item, &[1]).await;
    let url = presigned.json()["parts"][0]["url"]
        .as_str()
        .unwrap()
        .to_owned();
    let parsed = Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some(PUBLIC_HOST));
    assert!(!url.contains("localhost"));
    let parts = rig.parts(&session, &item).await;
    assert_eq!(parts.status, StatusCode::OK, "{}", parts.text());
    assert_eq!(parts.json()["missingPartNumbers"], json!([]));
    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    let key = rig.final_key(&item).await;
    assert_eq!(rig.read_object(&key).await, (size, sha256(&data)));
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-091")]
async fn regression_R091_presigned_url_lifetime() {
    let rig = S3Rig::new().await;
    let size = 9 * MIB;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("lifetime.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    let first = rig.presign(&session, &item, &[1]).await;
    let second = rig.presign(&session, &item, &[1]).await;
    let third = rig.presign(&session, &item, &[1, 2]).await;
    for fetched in [&first, &second, &third] {
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        assert_eq!(fetched.json()["ttlSeconds"], 900);
        for part in fetched.json()["parts"].as_array().unwrap() {
            let query = query_of(part["url"].as_str().unwrap());
            assert!(query["x-amz-expires"].parse::<u64>().unwrap() <= 900);
        }
    }
    let url_of = |fetched: &Fetched| {
        fetched.json()["parts"][0]["url"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        rig.faults.signs.load(Ordering::SeqCst),
        3,
        "every request signs afresh; nothing is cached server-side"
    );

    let etag = rig.put_part(&url_of(&third), data[..PART].to_vec()).await;
    assert!(etag.starts_with('"'));
    let again = rig.presign(&session, &item, &[1]).await;
    assert_eq!(
        again.status,
        StatusCode::OK,
        "a part already signed or uploaded can be signed again"
    );

    let stale_clock = Arc::new(TestClock::new(
        crate::domain::clock::SystemClock.now() - time::Duration::minutes(30),
    ));
    let stale = provider_for(&rig.minio, &rig.bucket, None).with_clock(stale_clock);
    let handle = MultipartHandle::new(
        ObjectKey::parse(&rig.final_key(&item).await).unwrap(),
        sqlx::query_scalar::<_, String>("SELECT s3_upload_id FROM s3_multipart_uploads")
            .fetch_one(rig.stack.pools.reader().executor())
            .await
            .unwrap(),
    );
    let expired = stale
        .sign_part_urls(
            &handle,
            &[PartPlanEntry {
                part_number: 2,
                len: MIB,
            }],
            Duration::from_secs(900),
        )
        .await
        .unwrap();
    let refused = rig
        .http
        .put(expired[0].url().as_str())
        .body(data[PART..].to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        reqwest::StatusCode::FORBIDDEN,
        "the provider refuses an expired URL"
    );
    let fresh = rig.presign(&session, &item, &[2]).await;
    let etag = rig.put_part(&url_of(&fresh), data[PART..].to_vec()).await;
    assert!(etag.starts_with('"'), "re-signing recovers the part");

    let persisted: Vec<String> = sqlx::query_scalar(
        "SELECT group_concat(COALESCE(etag, '') || '|' || COALESCE(signed_at, '') || '|' || COALESCE(signed_expires_at, ''), ';')
           FROM s3_multipart_parts",
    )
    .fetch_all(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    let dump = rig.persisted_text().await;
    for text in persisted.iter().chain(std::iter::once(&dump)) {
        assert!(!text.contains("X-Amz-Signature"), "no URL is persisted");
        assert!(!text.contains("x-amz-signature"));
        assert!(!text.contains(PUBLIC_HOST));
    }
    let signed: i64 = rig
        .stack
        .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts WHERE signed_at IS NOT NULL")
        .await;
    assert_eq!(signed, 2);
    assert!(!again.text().contains(rig.minio.secret_key()));
}

async fn part_cache(rig: &S3Rig) -> Vec<(i64, String, Option<String>, i64)> {
    sqlx::query_as(
        "SELECT part_number, state, etag, size_bytes FROM s3_multipart_parts ORDER BY part_number",
    )
    .fetch_all(rig.stack.pools.reader().executor())
    .await
    .unwrap()
}

fn numbers(fetched: &Fetched, field: &str) -> Vec<u64> {
    fetched.json()[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|part| {
            part["partNumber"]
                .as_u64()
                .unwrap_or_else(|| part.as_u64().unwrap())
        })
        .collect()
}

#[tokio::test]
async fn it_s3mp_list_parts_is_source_of_truth() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("truth.bin", size).await;
    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    rig.upload_parts(&session, &item, &data, &[1], true).await;
    rig.upload_parts(&session, &item, &data, &[2], false).await;
    let cache = part_cache(&rig).await;
    assert_eq!(cache[0].1, "uploaded", "a reported part is only a hint");
    assert_eq!(
        cache[1].1, "signed",
        "the lost browser response left no trace"
    );
    assert_eq!(cache[2].1, "planned");

    let parts = rig.parts(&session, &item).await;
    assert_eq!(parts.status, StatusCode::OK, "{}", parts.text());
    assert_eq!(numbers(&parts, "completedParts"), [1, 2]);
    assert_eq!(numbers(&parts, "missingPartNumbers"), [3]);
    assert_eq!(parts.json()["uploadedBytes"], 16 * MIB);
    assert_eq!(parts.json()["partSizeBytes"], 8 * MIB);
    assert_eq!(parts.json()["partCount"], 3);
    let cache = part_cache(&rig).await;
    assert_eq!(cache[1].1, "verified", "the part ListParts found is cached");
    assert!(cache[1].2.is_some());

    let lie = rig
        .report(
            &session,
            &item,
            json!([{ "partNumber": 3, "etag": "\"0123456789abcdef0123456789abcdef\"", "sizeBytes": 4 * MIB }]),
        )
        .await;
    assert_eq!(lie.status, StatusCode::NO_CONTENT, "{}", lie.text());
    assert_eq!(part_cache(&rig).await[2].1, "uploaded");
    let duplicate = rig
        .report(
            &session,
            &item,
            json!([{ "partNumber": 3, "etag": "\"0123456789abcdef0123456789abcdef\"", "sizeBytes": 4 * MIB }]),
        )
        .await;
    assert_eq!(
        duplicate.status,
        StatusCode::NO_CONTENT,
        "a repeated report is safe"
    );

    let truth = rig.parts(&session, &item).await;
    assert_eq!(
        numbers(&truth, "missingPartNumbers"),
        [3],
        "the lie is not believed"
    );
    assert_eq!(truth.json()["uploadedBytes"], 16 * MIB);
    let cache = part_cache(&rig).await;
    assert_eq!(
        (cache[2].1.as_str(), cache[2].2.clone(), cache[2].3),
        ("planned", None, i64::try_from(4 * MIB).unwrap()),
        "the unconfirmed hint is dropped and the planned size restored"
    );

    let completes = rig.faults.completes.load(Ordering::SeqCst);
    let incomplete = rig.complete(&session, &item).await;
    assert_eq!(incomplete.status, StatusCode::CONFLICT);
    assert_eq!(incomplete.error_code(), "MULTIPART_INCOMPLETE");
    assert_eq!(
        incomplete.json()["error"]["details"]["missingPartNumbers"],
        json!([3])
    );
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), completes);
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("in_progress")
    );
    assert_eq!(rig.stack.reservation(&session).await.0, "held");

    let short = rig.presign(&session, &item, &[3]).await;
    let url = short.json()["parts"][0]["url"].as_str().unwrap().to_owned();
    rig.put_part(&url, data[2 * PART..2 * PART + MIB as usize].to_vec())
        .await;
    let held = rig.parts(&session, &item).await;
    assert_eq!(numbers(&held, "completedParts"), [1, 2, 3]);
    assert_eq!(
        held.json()["uploadedBytes"],
        17 * MIB,
        "honest provider sizes"
    );
    assert_eq!(held.json()["completedParts"][2]["sizeBytes"], MIB);
    let invalid = rig.complete(&session, &item).await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid.error_code(), "MULTIPART_PART_INVALID");
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), completes);
    assert_eq!(rig.item_row(&item).await.0, "uploading");

    let first_etag = held.json()["completedParts"][2]["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    rig.upload_parts(&session, &item, &data, &[3], false).await;
    rig.stack
        .execute("UPDATE s3_multipart_parts SET state = 'planned', etag = NULL, uploaded_at = NULL")
        .await;
    let restarted = rig.parts(&session, &item).await;
    assert_eq!(numbers(&restarted, "missingPartNumbers"), Vec::<u64>::new());
    assert_eq!(restarted.json()["uploadedBytes"], 20 * MIB);
    assert_ne!(
        restarted.json()["completedParts"][2]["etag"]
            .as_str()
            .unwrap(),
        first_etag,
        "the changed ETag is the one reported"
    );
    assert!(part_cache(&rig).await.iter().all(|row| row.1 == "verified"));

    let done = rig.complete(&session, &item).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    let key = rig.final_key(&item).await;
    assert_eq!(rig.read_object(&key).await, (size, sha256(&data)));
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_complete_uses_head_object_size() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 7;
    let data = pattern(usize::try_from(size).unwrap());

    let (session, item) = rig.one("measured.bin", size).await;
    rig.open(&session, &item, &data).await;
    *rig.faults.stat_size.lock().unwrap() = Some(size + 1);
    let mismatch = rig.complete(&session, &item).await;
    assert_eq!(
        mismatch.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        mismatch.text()
    );
    assert_eq!(mismatch.error_code(), "UPLOAD_LENGTH_MISMATCH");
    assert_eq!(
        (rig.files().await, rig.used().await),
        (0, 0),
        "a measured size that disagrees exposes no row"
    );
    let (state, stage, code) = rig.item_row(&item).await;
    assert_eq!(
        (state.as_str(), stage.as_str(), code.as_deref()),
        ("failed", "placing", Some("UPLOAD_LENGTH_MISMATCH"))
    );
    let tombstone: (String, i64) = sqlx::query_as("SELECT state, size_bytes FROM storage_objects")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        tombstone,
        ("tombstoned".to_owned(), i64::try_from(size + 1).unwrap()),
        "the HeadObject measurement is the recorded size"
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob'")
            .await,
        1
    );
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("abandoned")
    );
    *rig.faults.stat_size.lock().unwrap() = None;

    let (session, item) = rig.one("measured-ok.bin", size).await;
    rig.open(&session, &item, &data).await;
    let read = rig.faults.bytes_read.load(Ordering::SeqCst);
    let done = rig.complete(&session, &item).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert_eq!(done.json()["sizeBytes"], size);
    let key = rig.final_key(&item).await;
    let head = rig
        .provider
        .stat(&ObjectKey::parse(&key).unwrap())
        .await
        .unwrap();
    assert_eq!(head.size, size);
    let committed: i64 = sqlx::query_scalar("SELECT size_bytes FROM files")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(committed, i64::try_from(head.size).unwrap());
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(
        rig.faults.bytes_read.load(Ordering::SeqCst) - read,
        8_192,
        "the MIME prefix is one bounded ranged read"
    );
    assert_eq!(rig.read_object(&key).await, (size, sha256(&data)));
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_zero_byte_put_object() {
    let rig = S3Rig::new().await;
    let (session, item) = rig.one("empty.txt", 0).await;
    let kind: String = sqlx::query_scalar("SELECT upload_kind FROM transfer_session_files")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(kind, "s3_single");

    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CONFLICT);
    assert_eq!(created.error_code(), "TRANSFER_SESSION_STATE_INVALID");
    let presigned = rig.presign(&session, &item, &[1]).await;
    assert_eq!(presigned.status, StatusCode::NOT_FOUND);
    assert_eq!(presigned.error_code(), "MULTIPART_NOT_FOUND");
    assert_eq!(
        rig.faults.creates.load(Ordering::SeqCst),
        0,
        "no fake multipart upload"
    );
    assert_eq!(rig.objects().await, 0);

    let completed = rig.complete(&session, &item).await;
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());
    assert_eq!(completed.json()["sizeBytes"], 0);
    assert_eq!(completed.json()["contentType"], "text/plain");
    assert_eq!(
        rig.faults.puts.load(Ordering::SeqCst),
        1,
        "one server-side PutObject"
    );
    assert_eq!(rig.faults.creates.load(Ordering::SeqCst), 0);
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (1, 1, 0)
    );
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        0
    );
    let key = rig.final_key(&item).await;
    assert_eq!(rig.read_object(&key).await, (0, sha256(b"")));
    let object: (String, i64) = sqlx::query_as("SELECT state, size_bytes FROM storage_objects")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(object, ("active".to_owned(), 0));
    assert_eq!(rig.item_row(&item).await.0, "completed");
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(0))
    );
    assert_eq!(rig.uploads_audited().await, 1);

    let repeated = rig.complete(&session, &item).await;
    assert_eq!(repeated.status, StatusCode::OK);
    assert_eq!(repeated.json()["fileId"], completed.json()["fileId"]);
    assert_eq!(rig.faults.puts.load(Ordering::SeqCst), 1, "no second write");

    let (second_session, second_item) = rig.one("empty.txt", 0).await;
    let twin = rig.complete(&second_session, &second_item).await;
    assert_eq!(twin.status, StatusCode::OK, "{}", twin.text());
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM files ORDER BY created_at")
        .fetch_all(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(names.len(), 2);
    assert_ne!(
        names[0], names[1],
        "keep-both naming applies to empty files too"
    );
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_s3mp_non_owner_404() {
    let rig = S3Rig::new().await;
    let size = 9 * MIB;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("private.bin", size).await;
    rig.open(&session, &item, &data).await;
    let key = rig.final_key(&item).await;
    let baseline = (
        rig.faults.creates.load(Ordering::SeqCst),
        rig.faults.signs.load(Ordering::SeqCst),
        rig.faults.list_calls.load(Ordering::SeqCst),
        rig.faults.completes.load(Ordering::SeqCst),
        rig.faults.aborts.load(Ordering::SeqCst),
    );

    let ghost_session = "0192f3a1-0000-7000-8000-00000000f001";
    let ghost_item = "0192f3a1-0000-7000-8000-00000000f002";
    let calls: [(Method, &str, Option<Value>); 6] = [
        (Method::POST, "", None),
        (
            Method::POST,
            "/presign",
            Some(json!({ "partNumbers": [1] })),
        ),
        (
            Method::POST,
            "/parts",
            Some(json!({ "parts": [{ "partNumber": 1, "etag": "\"x\"", "sizeBytes": PART }] })),
        ),
        (Method::GET, "/parts", None),
        (Method::POST, "/complete", None),
        (Method::POST, "/abort", None),
    ];
    for (method, tail, body) in calls {
        let foreign = rig
            .stack
            .api(
                method.clone(),
                &S3Rig::path(&session, &item, tail),
                &rig.bob,
                body.as_ref(),
            )
            .await;
        let missing = rig
            .stack
            .api(
                method.clone(),
                &S3Rig::path(ghost_session, ghost_item, tail),
                &rig.bob,
                body.as_ref(),
            )
            .await;
        let crossed = rig
            .stack
            .api(
                method.clone(),
                &S3Rig::path(&session, ghost_item, tail),
                &rig.alice,
                body.as_ref(),
            )
            .await;
        assert_eq!(
            foreign.status,
            StatusCode::NOT_FOUND,
            "{method} {tail}: {}",
            foreign.text()
        );
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{method} {tail}");
        assert_eq!(crossed.status, StatusCode::NOT_FOUND, "{method} {tail}");
        let code = if tail.is_empty() {
            "TRANSFER_SESSION_NOT_FOUND"
        } else {
            "MULTIPART_NOT_FOUND"
        };
        for fetched in [&foreign, &missing, &crossed] {
            assert_eq!(fetched.error_code(), code, "{method} {tail}");
        }
        assert_eq!(
            foreign.error_without_request_id(),
            missing.error_without_request_id(),
            "a foreign upload is indistinguishable from an unknown one"
        );
    }
    assert_eq!(
        baseline,
        (
            rig.faults.creates.load(Ordering::SeqCst),
            rig.faults.signs.load(Ordering::SeqCst),
            rig.faults.list_calls.load(Ordering::SeqCst),
            rig.faults.completes.load(Ordering::SeqCst),
            rig.faults.aborts.load(Ordering::SeqCst),
        ),
        "no provider call is made for a stranger"
    );
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("in_progress")
    );
    assert_eq!(rig.item_row(&item).await.0, "uploading");
    assert_eq!(
        rig.open_uploads(&key).await,
        1,
        "the owner's upload is untouched"
    );
    let own = rig.complete(&session, &item).await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text());
}

#[tokio::test]
async fn it_storage_object_created_only_at_finalization() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 5;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("late.bin", size).await;
    assert_eq!(rig.objects().await, 0, "not at session admission");
    let identity: (String, String) = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files WHERE id = ?1",
    )
    .bind(&item)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();

    let created = rig.create(&session, &item).await;
    assert_eq!(created.status, StatusCode::CREATED);
    assert_eq!(rig.objects().await, 0, "not at CreateMultipartUpload");
    let presigned = rig.presign(&session, &item, &[1, 2, 3]).await;
    assert_eq!(presigned.status, StatusCode::OK);
    assert_eq!(rig.objects().await, 0, "not at presign");
    rig.upload_all(&session, &item, &data).await;
    assert_eq!(
        rig.objects().await,
        0,
        "not at UploadPart or the part report"
    );
    let parts = rig.parts(&session, &item).await;
    assert_eq!(parts.status, StatusCode::OK);
    assert_eq!(rig.objects().await, 0, "not at ListParts");

    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    let (completed, ()) = tokio::join!(rig.complete(&session, &item), async {
        gate.wait_until_entered().await;
        assert!(
            rig.object_exists(&identity.1).await,
            "the provider already holds the completed object"
        );
        assert_eq!(
            rig.objects().await,
            0,
            "no row while only the object is placed"
        );
        assert_eq!((rig.files().await, rig.used().await), (0, 0));
        let (state, stage, _) = rig.item_row(&item).await;
        assert_eq!((state.as_str(), stage.as_str()), ("finalizing", "placing"));
        assert_eq!(
            rig.multipart_state(&item).await.as_deref(),
            Some("completing")
        );
        gate.open();
    });
    assert_eq!(completed.status, StatusCode::OK, "{}", completed.text());

    let rows: Vec<(String, String, String, i64, i64)> =
        sqlx::query_as("SELECT id, object_key, state, size_bytes, refcount FROM storage_objects")
            .fetch_all(rig.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        rows,
        [(
            identity.0.clone(),
            identity.1.clone(),
            "active".to_owned(),
            i64::try_from(size).unwrap(),
            1
        )],
        "the row carries the identity allocated at admission"
    );
    let file_object: String = sqlx::query_scalar("SELECT storage_object_id FROM files")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(file_object, identity.0);
    rig.assert_no_overlap();
}

#[tokio::test]
async fn it_finalize_redrive_commits_placed_object_once() {
    let rig = S3Rig::new().await;
    let size = 20 * MIB + 5;
    let data = pattern(usize::try_from(size).unwrap());
    let (session, item) = rig.one("recover.bin", size).await;
    rig.open(&session, &item, &data).await;
    let key = rig.final_key(&item).await;
    let upload_id: String = sqlx::query_scalar("SELECT s3_upload_id FROM s3_multipart_uploads")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();

    rig.hooks.crash_at(Point::BeforeCommit);
    let crashed = rig.complete(&session, &item).await;
    assert_eq!(
        crashed.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        crashed.text()
    );
    assert_eq!(rig.faults.completes.load(Ordering::SeqCst), 1);
    let (state, stage, _) = rig.item_row(&item).await;
    assert_eq!((state.as_str(), stage.as_str()), ("finalizing", "placing"));
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("completing")
    );
    assert!(rig.object_exists(&key).await, "the object is placed");
    assert_eq!(
        rig.open_uploads(&key).await,
        0,
        "the multipart upload no longer exists"
    );
    assert_eq!(
        (rig.files().await, rig.objects().await, rig.used().await),
        (0, 0, 0)
    );
    assert_eq!(rig.stack.reservation(&session).await.0, "held");
    let lists = rig.faults.list_calls.load(Ordering::SeqCst);
    let identity: (String, String) =
        sqlx::query_as("SELECT final_object_id, final_object_key FROM transfer_session_files")
            .fetch_one(rig.stack.pools.reader().executor())
            .await
            .unwrap();

    let finalizer = rig.restart().await;
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(
        rig.used().await,
        i64::try_from(size).unwrap(),
        "one increment"
    );
    assert_eq!(rig.uploads_audited().await, 1, "one audit event");
    assert_eq!(rig.item_row(&item).await.0, "completed");
    assert_eq!(
        rig.multipart_state(&item).await.as_deref(),
        Some("completed")
    );
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(i64::try_from(size).unwrap())),
        "one settlement"
    );
    assert_eq!(
        rig.faults.completes.load(Ordering::SeqCst),
        1,
        "no second CompleteMultipartUpload is attempted"
    );
    assert_eq!(
        rig.faults.list_calls.load(Ordering::SeqCst),
        lists,
        "an existing object is adopted without listing parts"
    );
    let adopted: (String, String) =
        sqlx::query_as("SELECT id, object_key FROM storage_objects WHERE state = 'active'")
            .fetch_one(rig.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(adopted, identity, "the recorded identity is reused");
    assert_eq!(rig.read_object(&key).await, (size, sha256(&data)));

    finalizer.start_redrive().await.unwrap();
    finalizer.idle().await;
    let again = rig.restart().await;
    again.idle().await;
    assert_eq!((rig.files().await, rig.objects().await), (1, 1));
    assert_eq!(rig.used().await, i64::try_from(size).unwrap());
    assert_eq!(rig.uploads_audited().await, 1);
    let retried = rig.complete(&session, &item).await;
    assert_eq!(retried.status, StatusCode::OK);
    let file: String = sqlx::query_scalar("SELECT id FROM files")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(retried.json()["fileId"], file);
    rig.leaked(&[crashed.text(), retried.text()], &key, &upload_id);
    rig.assert_no_overlap();
}
