use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{sleep_until, timeout, Instant};
use tokio_util::sync::CancellationToken;

use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;
use crate::storage::error::StorageError;
use crate::storage::provider::PartChecksum;
use crate::storage::s3::is_checksum_rejection;
use crate::storage::s3::plan::PROXY_PART_SIZE;

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferSessionId};
use super::super::repo as transfer_repo;
use super::model::{part_bytes, CompletedPartView, MultipartId, MAX_ETAG_CHARS};
use super::repo;
use super::service::{part_invalid, Open, S3Service};

pub const S3_PROXY_CONCURRENCY: usize = 8;

const PROXY_TRANSACTION: &str = "transfers.s3_proxy_part";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyLimits {
    pub concurrency: usize,
    pub permit_wait: Duration,
    pub idle_timeout: Duration,
}

impl ProxyLimits {
    pub const PERMIT_WAIT: Duration = Duration::from_secs(5);
    pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

    pub const fn production() -> Self {
        Self {
            concurrency: S3_PROXY_CONCURRENCY,
            permit_wait: Self::PERMIT_WAIT,
            idle_timeout: Self::IDLE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferSnapshot {
    pub active_permits: usize,
    pub active_buffers: usize,
    pub active_capacity_bytes: u64,
    pub peak_permits: usize,
    pub peak_buffers: usize,
    pub peak_capacity_bytes: u64,
}

#[derive(Debug, Default)]
struct BufferGauge {
    permits: AtomicUsize,
    buffers: AtomicUsize,
    capacity: AtomicU64,
    peak_permits: AtomicUsize,
    peak_buffers: AtomicUsize,
    peak_capacity: AtomicU64,
}

impl BufferGauge {
    fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            active_permits: self.permits.load(Ordering::SeqCst),
            active_buffers: self.buffers.load(Ordering::SeqCst),
            active_capacity_bytes: self.capacity.load(Ordering::SeqCst),
            peak_permits: self.peak_permits.load(Ordering::SeqCst),
            peak_buffers: self.peak_buffers.load(Ordering::SeqCst),
            peak_capacity_bytes: self.peak_capacity.load(Ordering::SeqCst),
        }
    }
}

#[derive(Debug)]
struct Admitted {
    _permit: OwnedSemaphorePermit,
    gauge: Arc<BufferGauge>,
}

impl Admitted {
    fn new(permit: OwnedSemaphorePermit, gauge: Arc<BufferGauge>) -> Self {
        let held = gauge.permits.fetch_add(1, Ordering::SeqCst) + 1;
        gauge.peak_permits.fetch_max(held, Ordering::SeqCst);
        Self {
            _permit: permit,
            gauge,
        }
    }
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.gauge.permits.fetch_sub(1, Ordering::SeqCst);
    }
}

struct BufferLease {
    gauge: Arc<BufferGauge>,
    capacity: u64,
}

impl BufferLease {
    fn new(gauge: &Arc<BufferGauge>, capacity: usize) -> Self {
        let capacity = capacity as u64;
        let buffers = gauge.buffers.fetch_add(1, Ordering::SeqCst) + 1;
        gauge.peak_buffers.fetch_max(buffers, Ordering::SeqCst);
        let bytes = gauge.capacity.fetch_add(capacity, Ordering::SeqCst) + capacity;
        gauge.peak_capacity.fetch_max(bytes, Ordering::SeqCst);
        Self {
            gauge: Arc::clone(gauge),
            capacity,
        }
    }
}

impl Drop for BufferLease {
    fn drop(&mut self) {
        self.gauge.buffers.fetch_sub(1, Ordering::SeqCst);
        self.gauge
            .capacity
            .fetch_sub(self.capacity, Ordering::SeqCst);
    }
}

struct PartClaim {
    claimed: Arc<Mutex<HashSet<(MultipartId, u32)>>>,
    key: (MultipartId, u32),
}

impl Drop for PartClaim {
    fn drop(&mut self) {
        self.claimed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

#[derive(Clone)]
pub struct ProxyUploads {
    limits: ProxyLimits,
    permits: Arc<Semaphore>,
    claimed: Arc<Mutex<HashSet<(MultipartId, u32)>>>,
    gauge: Arc<BufferGauge>,
    shutdown: CancellationToken,
}

impl ProxyUploads {
    pub fn new(limits: ProxyLimits, shutdown: CancellationToken) -> Self {
        Self {
            limits,
            permits: Arc::new(Semaphore::new(limits.concurrency)),
            claimed: Arc::default(),
            gauge: Arc::default(),
            shutdown,
        }
    }

    fn claim(&self, upload: MultipartId, part: u32) -> Option<PartClaim> {
        let key = (upload, part);
        let inserted = self
            .claimed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key);
        inserted.then(|| PartClaim {
            claimed: Arc::clone(&self.claimed),
            key,
        })
    }

    pub fn in_flight(&self, upload: MultipartId) -> bool {
        self.claimed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|(claimed, _)| *claimed == upload)
    }

    async fn admit(&self) -> Result<Admitted, ProxyError> {
        match timeout(
            self.limits.permit_wait,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(Admitted::new(permit, Arc::clone(&self.gauge))),
            Ok(Err(_closed)) => Err(ProxyError::ShuttingDown),
            Err(_elapsed) => {
                let held = self.gauge.snapshot();
                tracing::debug!(
                    in_flight_parts = held.active_permits,
                    buffered_bytes = held.active_capacity_bytes,
                    "proxied part uploads are saturated"
                );
                Err(ProxyError::Backpressure {
                    retry_after: self.limits.permit_wait.as_secs().max(1),
                })
            }
        }
    }
}

#[derive(Debug)]
pub enum ProxyError {
    Transfer(TransferError),
    PartLocked,
    Backpressure { retry_after: u64 },
    IdleTimeout,
    ShuttingDown,
    ChecksumRejected,
}

impl ProxyError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Transfer(error) => error.kind(),
            Self::PartLocked => "s3_proxy_part_locked",
            Self::Backpressure { .. } => "s3_proxy_backpressure",
            Self::IdleTimeout => "s3_proxy_idle_timeout",
            Self::ShuttingDown => "s3_proxy_shutting_down",
            Self::ChecksumRejected => "s3_proxy_checksum_rejected",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Transfer(error) => error.api_error(),
            Self::PartLocked => ApiError::new(ErrorCode::UploadLocked),
            Self::Backpressure { .. } => ApiError::new(ErrorCode::TransferBackpressure),
            Self::IdleTimeout => ApiError::new(ErrorCode::TransferIdleTimeout),
            Self::ShuttingDown => ApiError::new(ErrorCode::ServiceUnavailable),
            Self::ChecksumRejected => ApiError::new(ErrorCode::StorageUpstreamError)
                .with_detail("reason", "checksum_rejected"),
        }
    }

    pub const fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Backpressure { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transfer(error) => write!(f, "{error}"),
            Self::PartLocked => f.write_str("another request is uploading this part"),
            Self::Backpressure { .. } => {
                f.write_str("the server is handling the maximum number of proxied part uploads")
            }
            Self::IdleTimeout => f.write_str("the part body stalled or took too long"),
            Self::ShuttingDown => f.write_str("the server is shutting down"),
            Self::ChecksumRejected => f.write_str("the storage provider rejected the checksum"),
        }
    }
}

impl std::error::Error for ProxyError {}

impl From<TransferError> for ProxyError {
    fn from(error: TransferError) -> Self {
        Self::Transfer(error)
    }
}

impl From<DbError> for ProxyError {
    fn from(error: DbError) -> Self {
        Self::Transfer(TransferError::Db(error))
    }
}

impl From<crate::domain::time::InvalidTimestamp> for ProxyError {
    fn from(error: crate::domain::time::InvalidTimestamp) -> Self {
        Self::Transfer(TransferError::Time(error))
    }
}

fn provider_failure(error: StorageError) -> ProxyError {
    match error {
        StorageError::NotFound => ProxyError::Transfer(TransferError::MultipartNotFound),
        error if is_checksum_rejection(&error) => ProxyError::ChecksumRejected,
        error => ProxyError::Transfer(TransferError::Storage(error)),
    }
}

pub struct ProxiedPart {
    pub part_number: String,
    pub content_length: Option<u64>,
    pub body: Body,
}

struct Admission {
    open: Open,
    session: TransferSessionId,
    item: SessionItemId,
    number: u32,
    expected: usize,
}

enum Step {
    Shutdown,
    Idle,
    Frame(Option<Result<http_body::Frame<Bytes>, axum::Error>>),
}

fn part_number(raw: &str) -> Option<u32> {
    let number: u32 = raw.parse().ok()?;
    (number.to_string() == raw).then_some(number)
}

async fn receive(
    shutdown: &CancellationToken,
    limits: ProxyLimits,
    body: &mut Body,
    expected: usize,
) -> Result<(Bytes, PartChecksum), ProxyError> {
    let mut buffer = BytesMut::with_capacity(expected);
    let mut hasher = Sha256::new();
    let mut last_frame = Instant::now();
    loop {
        let step = tokio::select! {
            biased;
            () = shutdown.cancelled() => Step::Shutdown,
            frame = body.frame() => Step::Frame(frame),
            () = sleep_until(last_frame + limits.idle_timeout) => Step::Idle,
        };
        match step {
            Step::Shutdown => return Err(ProxyError::ShuttingDown),
            Step::Idle => return Err(ProxyError::IdleTimeout),
            Step::Frame(None) => break,
            Step::Frame(Some(Err(_))) => {
                return Err(part_invalid("body_interrupted").into());
            }
            Step::Frame(Some(Ok(frame))) => {
                last_frame = Instant::now();
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                if data.len() > expected - buffer.len() {
                    return Err(part_invalid("body_too_long").into());
                }
                hasher.update(&data);
                buffer.extend_from_slice(&data);
            }
        }
    }
    if buffer.len() != expected {
        return Err(part_invalid("body_too_short").into());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    Ok((buffer.freeze(), PartChecksum::sha256(digest)))
}

impl S3Service {
    pub async fn upload_part_proxied(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
        input: ProxiedPart,
    ) -> Result<CompletedPartView, ProxyError> {
        if self.proxy.shutdown.is_cancelled() {
            return Err(ProxyError::ShuttingDown);
        }
        let ProxiedPart {
            part_number,
            content_length,
            body,
        } = input;
        let admission = self
            .admit_part(owner, session, item, &part_number, content_length)
            .await?;
        let claim = self
            .proxy
            .claim(admission.open.s3.id, admission.number)
            .ok_or(ProxyError::PartLocked)?;
        let admitted = self.proxy.admit().await?;
        let service = self.clone();
        tokio::spawn(async move {
            let _claim = claim;
            let _admitted = admitted;
            service.transfer_part(owner, admission, body).await
        })
        .await
        .map_err(|_| {
            ProxyError::Transfer(TransferError::Invariant {
                what: "proxied_part_task",
            })
        })?
    }

    async fn admit_part(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
        raw_part: &str,
        content_length: Option<u64>,
    ) -> Result<Admission, ProxyError> {
        let open = self.open(owner, session, item).await?;
        if !self.storage.is_available() {
            return Err(TransferError::StorageUnavailable.into());
        }
        if !self.storage.requires_proxied_parts() {
            return Err(part_invalid("direct_part_upload").into());
        }
        if open.layout.part_size > PROXY_PART_SIZE {
            return Err(part_invalid("part_size_exceeds_proxy_buffer").into());
        }
        let number = part_number(raw_part)
            .filter(|number| open.layout.expected(*number).is_some())
            .ok_or_else(|| part_invalid("part_number"))?;
        let expected = open
            .layout
            .expected(number)
            .and_then(|len| usize::try_from(len).ok())
            .ok_or_else(|| part_invalid("part_number"))?;
        if content_length.is_some_and(|declared| declared != expected as u64) {
            return Err(part_invalid("content_length_mismatch").into());
        }
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        if transfer_repo::owner_is_active(&mut connection, owner).await? != Some(true) {
            return Err(TransferError::AccountInactive.into());
        }
        Ok(Admission {
            open,
            session,
            item,
            number,
            expected,
        })
    }

    async fn transfer_part(
        &self,
        owner: UserId,
        admission: Admission,
        mut body: Body,
    ) -> Result<CompletedPartView, ProxyError> {
        let Admission {
            open,
            session,
            item,
            number,
            expected,
        } = admission;
        let lease = BufferLease::new(&self.proxy.gauge, expected);
        let (bytes, checksum) =
            receive(&self.proxy.shutdown, self.proxy.limits, &mut body, expected).await?;
        drop(body);
        let multipart = self
            .storage
            .multipart()
            .ok_or(TransferError::StorageUnavailable)?;
        let uploaded = multipart
            .upload_part_checksummed(
                &open.handle,
                number,
                expected as u64,
                Box::pin(std::io::Cursor::new(bytes)),
                checksum,
            )
            .await
            .map_err(provider_failure)?;
        drop(lease);
        self.record_part(
            (owner, session, item),
            &open,
            number,
            uploaded.etag.as_str(),
            uploaded.size,
        )
        .await?;
        Ok(CompletedPartView {
            part_number: number,
            etag: uploaded.etag.as_str().to_owned(),
            size_bytes: part_bytes(uploaded.size),
        })
    }

    async fn record_part(
        &self,
        (owner, session, item): (UserId, TransferSessionId, SessionItemId),
        open: &Open,
        number: u32,
        etag: &str,
        size: u64,
    ) -> Result<(), ProxyError> {
        let now = Timestamp::try_from(self.clock.now()).map_err(TransferError::from)?;
        let expires_at = Timestamp::try_from(now.get() + self.storage.multipart_ttl())
            .map_err(TransferError::from)?;
        let recordable = etag.chars().count() <= MAX_ETAG_CHARS;
        let alive = self
            .pools
            .write_tx(self.clock.as_ref(), PROXY_TRANSACTION, async |tx| {
                if !repo::touch_open(tx.executor(), open.s3.id, now, expires_at, true).await? {
                    return Ok::<_, TransferError>(false);
                }
                if recordable {
                    repo::record_proxied_part(tx.executor(), open.s3.id, number, etag, size, now)
                        .await?;
                }
                Ok(true)
            })
            .await
            .map_err(ProxyError::from)?;
        if alive {
            return Ok(());
        }
        Err(self.after_race(owner, session, item).await.into())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use bytes::Bytes;
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    use super::{
        part_number, provider_failure, receive, BufferLease, BufferSnapshot, ProxyError,
        ProxyLimits, ProxyUploads, S3_PROXY_CONCURRENCY,
    };
    use crate::domain::clock::TestClock;
    use crate::domain::error_code::ErrorCode;
    use crate::features::transfers::error::TransferError;
    use crate::features::transfers::s3::model::MultipartId;
    use crate::storage::error::StorageError;
    use crate::storage::provider::PartChecksum;
    use crate::storage::s3::{classify_failure, Failure, Operation};

    impl ProxyUploads {
        fn snapshot(&self) -> BufferSnapshot {
            self.gauge.snapshot()
        }
    }

    impl super::S3Service {
        pub fn proxy_snapshot(&self) -> BufferSnapshot {
            self.proxy.snapshot()
        }
    }

    fn limits(concurrency: usize) -> ProxyLimits {
        ProxyLimits {
            concurrency,
            permit_wait: Duration::from_millis(30),
            idle_timeout: Duration::from_secs(60),
        }
    }

    fn uploads(concurrency: usize) -> ProxyUploads {
        ProxyUploads::new(limits(concurrency), CancellationToken::new())
    }

    #[test]
    fn unit_the_proxy_concurrency_is_the_documented_eight() {
        assert_eq!(S3_PROXY_CONCURRENCY, 8);
        assert_eq!(ProxyLimits::production().concurrency, 8);
        assert_eq!(
            ProxyLimits::production().idle_timeout,
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn unit_proxy_permits_are_one_global_pool_that_returns_on_drop() {
        let proxy = uploads(2);
        let other_handle = proxy.clone();
        let first = proxy.admit().await.unwrap();
        let second = other_handle.admit().await.unwrap();
        assert_eq!(proxy.snapshot().active_permits, 2);
        match proxy.admit().await {
            Err(ProxyError::Backpressure { retry_after }) => assert!(retry_after >= 1),
            other => panic!("expected backpressure, got {other:?}"),
        }
        assert_eq!(
            other_handle.snapshot().active_permits,
            2,
            "a refused wait takes nothing"
        );
        drop(first);
        let third = proxy.admit().await.unwrap();
        drop((second, third));
        let snapshot = proxy.snapshot();
        assert_eq!(snapshot.active_permits, 0);
        assert_eq!(snapshot.peak_permits, 2);
    }

    #[tokio::test]
    async fn unit_closed_pool_reports_shutdown_not_backpressure() {
        let proxy = uploads(1);
        proxy.permits.close();
        assert!(matches!(proxy.admit().await, Err(ProxyError::ShuttingDown)));
    }

    #[test]
    fn unit_part_claims_are_exclusive_per_part_and_released_on_drop() {
        let proxy = uploads(8);
        let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
        let upload = MultipartId::generate(&clock);
        let other = MultipartId::generate(&clock);
        let held = proxy.claim(upload, 1).unwrap();
        assert!(
            proxy.claim(upload, 1).is_none(),
            "the same part is exclusive"
        );
        assert!(
            proxy.claim(upload, 2).is_some(),
            "other parts are independent"
        );
        assert!(
            proxy.claim(other, 1).is_some(),
            "other uploads are independent"
        );
        drop(held);
        assert!(proxy.claim(upload, 1).is_some());
        assert!(
            proxy.claimed.lock().unwrap().is_empty(),
            "no claim outlives its request"
        );
    }

    #[test]
    fn unit_buffer_leases_track_count_capacity_and_peaks() {
        let proxy = uploads(8);
        let a = BufferLease::new(&proxy.gauge, 8 << 20);
        let b = BufferLease::new(&proxy.gauge, 5 << 20);
        let held = proxy.snapshot();
        assert_eq!(
            (held.active_buffers, held.active_capacity_bytes),
            (2, 13 << 20)
        );
        drop(a);
        drop(b);
        let end = proxy.snapshot();
        assert_eq!((end.active_buffers, end.active_capacity_bytes), (0, 0));
        assert_eq!((end.peak_buffers, end.peak_capacity_bytes), (2, 13 << 20));
    }

    #[test]
    fn unit_part_numbers_are_canonical_decimal_integers() {
        assert_eq!(part_number("1"), Some(1));
        assert_eq!(part_number("9900"), Some(9_900));
        for refused in [
            "",
            "0x1",
            "01",
            "+1",
            "-1",
            "1.0",
            " 1",
            "1 ",
            "1e0",
            "4294967296",
            "abc",
        ] {
            assert_eq!(
                part_number(refused).filter(|number| number.to_string() == refused),
                None,
                "{refused:?}"
            );
        }
        assert_eq!(
            part_number("0"),
            Some(0),
            "zero is refused by the plan lookup"
        );
    }

    #[test]
    fn unit_proxy_errors_use_catalogue_codes_and_never_leak() {
        let table = [
            (ProxyError::PartLocked, ErrorCode::UploadLocked, 423),
            (
                ProxyError::Backpressure { retry_after: 5 },
                ErrorCode::TransferBackpressure,
                503,
            ),
            (ProxyError::IdleTimeout, ErrorCode::TransferIdleTimeout, 408),
            (ProxyError::ShuttingDown, ErrorCode::ServiceUnavailable, 503),
            (
                ProxyError::ChecksumRejected,
                ErrorCode::StorageUpstreamError,
                502,
            ),
        ];
        for (error, code, status) in table {
            let api = error.api_error();
            assert_eq!(api.code(), code, "{error}");
            assert_eq!(api.status().as_u16(), status, "{error}");
            assert_eq!(
                error.retry_after().is_some(),
                matches!(error, ProxyError::Backpressure { .. })
            );
            assert!(error.kind().starts_with("s3_proxy_"));
        }
        let wrapped = ProxyError::from(TransferError::MultipartNotFound).api_error();
        assert_eq!(wrapped.code(), ErrorCode::MultipartNotFound);
    }

    #[test]
    fn unit_provider_failures_are_classified_by_structure_not_message() {
        let service = |status, code: Option<&str>| {
            classify_failure(
                Operation::UploadPart,
                Failure::Service {
                    status,
                    code: code.map(str::to_owned),
                },
            )
        };
        assert!(matches!(
            provider_failure(StorageError::NotFound),
            ProxyError::Transfer(TransferError::MultipartNotFound)
        ));
        for code in [
            "BadDigest",
            "InvalidDigest",
            "XAmzContentSHA256Mismatch",
            "XAmzContentChecksumMismatch",
        ] {
            assert!(
                matches!(
                    provider_failure(service(400, Some(code))),
                    ProxyError::ChecksumRejected
                ),
                "{code}"
            );
        }
        for (status, code) in [
            (400, Some("InvalidRequest")),
            (400, Some("InvalidPart")),
            (403, Some("AccessDenied")),
            (503, Some("SlowDown")),
            (400, None),
        ] {
            assert!(
                matches!(
                    provider_failure(service(status, code)),
                    ProxyError::Transfer(TransferError::Storage(_))
                ),
                "{status} {code:?}"
            );
        }
        let timed_out = classify_failure(Operation::UploadPart, Failure::Timeout);
        assert!(matches!(
            provider_failure(timed_out),
            ProxyError::Transfer(TransferError::Storage(StorageError::ProviderUnavailable(_)))
        ));
    }

    async fn received(
        frames: Vec<Result<Bytes, std::io::Error>>,
        expected: usize,
    ) -> Result<(Bytes, PartChecksum), ProxyError> {
        let mut body = Body::from_stream(futures_util::stream::iter(frames));
        receive(&CancellationToken::new(), limits(8), &mut body, expected).await
    }

    fn reason(error: &ProxyError) -> &'static str {
        match error {
            ProxyError::Transfer(TransferError::MultipartPartInvalid { reason }) => reason,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn unit_receive_accepts_exactly_the_expected_bytes_and_hashes_them() {
        let data: Vec<u8> = (0..10_000_u32)
            .map(|n| u8::try_from(n % 251).unwrap())
            .collect();
        let frames = data
            .chunks(1_337)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect();
        let (bytes, checksum) = received(frames, data.len()).await.unwrap();
        assert_eq!(bytes.as_ref(), data.as_slice());
        assert_eq!(
            checksum,
            PartChecksum::sha256(Sha256::digest(&data).into()),
            "the checksum covers exactly the received bytes"
        );
    }

    #[tokio::test]
    async fn unit_receive_refuses_every_length_violation_without_a_result() {
        let frame = |len: usize| Ok::<_, std::io::Error>(Bytes::from(vec![9_u8; len]));
        let long = received(vec![frame(10), frame(1)], 10).await.unwrap_err();
        assert_eq!(reason(&long), "body_too_long");
        let long_first = received(vec![frame(11)], 10).await.unwrap_err();
        assert_eq!(reason(&long_first), "body_too_long");
        let short = received(vec![frame(9)], 10).await.unwrap_err();
        assert_eq!(reason(&short), "body_too_short");
        let empty = received(Vec::new(), 10).await.unwrap_err();
        assert_eq!(reason(&empty), "body_too_short");
        let broken = received(
            vec![
                frame(4),
                Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
            ],
            10,
        )
        .await
        .unwrap_err();
        assert_eq!(reason(&broken), "body_interrupted");
    }

    fn feed() -> (
        tokio::sync::mpsc::UnboundedSender<Result<Bytes, std::io::Error>>,
        Body,
    ) {
        let (sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
        let body = Body::from_stream(futures_util::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
        ));
        (sender, body)
    }

    #[tokio::test(start_paused = true)]
    async fn unit_receive_ends_on_an_idle_gap_and_survives_within_it() {
        let (sender, mut body) = feed();
        let task = tokio::spawn(async move {
            let outcome = receive(&CancellationToken::new(), limits(8), &mut body, 100).await;
            (outcome, body)
        });
        sender.send(Ok(Bytes::from_static(&[1; 10]))).unwrap();
        tokio::time::advance(Duration::from_secs(59)).await;
        sender.send(Ok(Bytes::from_static(&[1; 10]))).unwrap();
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(
            !task.is_finished(),
            "progress within the idle window keeps the part alive"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        let (outcome, _body) = task.await.unwrap();
        assert!(matches!(outcome, Err(ProxyError::IdleTimeout)));
    }

    #[tokio::test(start_paused = true)]
    async fn unit_receive_has_no_absolute_deadline_while_frames_keep_arriving() {
        const FORMER_DEADLINE: Duration = Duration::from_secs(15 * 60);
        let expected = 1_000;
        let (sender, mut body) = feed();
        let started = tokio::time::Instant::now();
        let task = tokio::spawn(async move {
            receive(&CancellationToken::new(), limits(8), &mut body, expected).await
        });
        let frame = Bytes::from_static(&[7; 10]);
        for _ in 0..expected / frame.len() {
            sender.send(Ok(frame.clone())).unwrap();
            tokio::time::advance(Duration::from_secs(59)).await;
        }
        drop(sender);
        let (bytes, checksum) = task.await.unwrap().unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed > 3 * FORMER_DEADLINE,
            "the part took {elapsed:?}, far beyond the removed 15 minute deadline"
        );
        assert_eq!(bytes.len(), expected);
        assert_eq!(
            checksum,
            PartChecksum::sha256(Sha256::digest(&bytes).into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unit_receive_idle_gap_still_fails_after_a_long_progressing_stream() {
        let (sender, mut body) = feed();
        let task = tokio::spawn(async move {
            receive(&CancellationToken::new(), limits(8), &mut body, 1_000).await
        });
        for _ in 0..40 {
            sender.send(Ok(Bytes::from_static(&[1; 10]))).unwrap();
            tokio::time::advance(Duration::from_secs(30)).await;
        }
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(matches!(task.await.unwrap(), Err(ProxyError::IdleTimeout)));
        drop(sender);
    }

    #[tokio::test]
    async fn unit_receive_stops_on_shutdown() {
        let shutdown = CancellationToken::new();
        let (_sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
        let mut body = Body::from_stream(futures_util::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
        ));
        shutdown.cancel();
        let outcome = receive(&shutdown, limits(8), &mut body, 10).await;
        assert!(matches!(outcome, Err(ProxyError::ShuttingDown)));
    }
}
