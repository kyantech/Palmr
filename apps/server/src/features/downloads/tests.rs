use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt;
use rstest::rstest;
use time::macros::datetime;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

use super::error::DownloadError;
use super::model::{
    effective_disposition, inline_is_safe, parse_disposition, served_content_type, EntityTag,
    CHUNK_BYTES, DOWNLOAD_CONCURRENCY, DOWNLOAD_IDLE_TIMEOUT, PRESIGN_TTL,
};
use super::stream::{ContentStream, DownloadPermits};
use crate::domain::content_disposition::DispositionType;
use crate::domain::error_code::ErrorCode;
use crate::storage::error::{Retryable, StorageError};
use crate::storage::provider::{ETag, ObjectBody, ObjectStat};

fn repeating(len: u64) -> ObjectBody {
    Box::pin(tokio::io::repeat(0xA5).take(len))
}

fn permits(capacity: usize) -> DownloadPermits {
    DownloadPermits::new(capacity, Duration::from_millis(40))
}

struct Failing {
    served: usize,
    fail_after: usize,
}

impl AsyncRead for Failing {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.served >= self.fail_after {
            return Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionReset)));
        }
        let take = buffer.remaining().min(self.fail_after - self.served);
        buffer.put_slice(&vec![1; take]);
        self.served += take;
        Poll::Ready(Ok(()))
    }
}

#[test]
fn unit_download_limits_match_the_documented_contract() {
    assert_eq!(DOWNLOAD_CONCURRENCY, 128);
    assert_eq!(CHUNK_BYTES, 256 * 1024);
    assert_eq!(PRESIGN_TTL, Duration::from_secs(300));
    assert_eq!(DOWNLOAD_IDLE_TIMEOUT, Duration::from_secs(60));
}

#[tokio::test]
async fn unit_download_stream_chunks_are_bounded_and_exact() {
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let total = 3 * CHUNK_BYTES as u64 + 12_345;
    let mut stream = ContentStream::new(repeating(total), total, permit);
    let (mut seen, mut largest, mut frames) = (0_u64, 0_usize, 0_u32);
    while let Some(frame) = stream.next().await {
        let frame = frame.unwrap();
        largest = largest.max(frame.len());
        seen += frame.len() as u64;
        frames += 1;
    }
    assert_eq!(seen, total);
    assert!(largest <= CHUNK_BYTES, "a frame of {largest} bytes");
    assert!(frames >= 4);
    assert_eq!(pool.available(), 1, "completion returns the permit");
}

#[tokio::test]
async fn unit_download_stream_never_exceeds_the_declared_length() {
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let mut stream = ContentStream::new(repeating(10_000), 4_000, permit);
    let mut seen = 0;
    while let Some(frame) = stream.next().await {
        seen += frame.unwrap().len();
    }
    assert_eq!(seen, 4_000);
    assert_eq!(pool.available(), 1);
}

#[tokio::test]
async fn unit_download_stream_short_source_is_an_error_not_a_silent_truncation() {
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let mut stream = ContentStream::new(repeating(1_000), 5_000, permit);
    let mut outcome = Ok(0_usize);
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => outcome = outcome.map(|seen| seen + frame.len()),
            Err(error) => {
                outcome = Err(error.kind());
                break;
            }
        }
    }
    assert_eq!(outcome, Err(io::ErrorKind::UnexpectedEof));
    assert_eq!(pool.available(), 1, "a failed stream returns the permit");
}

#[tokio::test]
async fn unit_download_stream_provider_failure_releases_the_permit() {
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let body: ObjectBody = Box::pin(Failing {
        served: 0,
        fail_after: 1_000,
    });
    let mut stream = ContentStream::new(body, 9_000, permit);
    let mut failed = false;
    while let Some(frame) = stream.next().await {
        if frame.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert_eq!(pool.available(), 1);
}

#[tokio::test]
async fn unit_download_stream_drop_releases_the_permit_without_reading_the_rest() {
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let total = 64 * CHUNK_BYTES as u64;
    let mut stream = ContentStream::new(repeating(total), total, permit);
    let first = stream.next().await.unwrap().unwrap();
    assert!(first.len() <= CHUNK_BYTES);
    assert_eq!(pool.available(), 0, "the permit is held while streaming");
    drop(stream);
    assert_eq!(pool.available(), 1, "a disconnect returns the permit");
}

#[tokio::test]
async fn unit_download_permits_are_bounded_and_returned() {
    let pool = permits(2);
    let first = pool.acquire().await.unwrap();
    let second = pool.acquire().await.unwrap();
    match pool.acquire().await {
        Err(DownloadError::Backpressure { retry_after }) => assert!(retry_after >= 1),
        other => panic!("expected backpressure, got {other:?}"),
    }
    assert_eq!(pool.available(), 0, "a rejected wait takes nothing");
    drop(first);
    let third = pool.acquire().await.unwrap();
    drop((second, third));
    assert_eq!(pool.available(), 2);
}

#[tokio::test]
async fn unit_download_empty_stream_holds_no_permit_and_no_body() {
    let mut stream = ContentStream::empty();
    assert!(stream.next().await.is_none());
}

#[rstest]
#[case::png("image/png", true)]
#[case::jpeg("image/jpeg", true)]
#[case::webp("image/webp", true)]
#[case::mp4("video/mp4", true)]
#[case::webm("video/webm", true)]
#[case::mp3("audio/mpeg", true)]
#[case::ogg("audio/ogg", true)]
#[case::pdf("application/pdf", true)]
#[case::plain("text/plain", true)]
#[case::plain_charset("text/plain; charset=utf-8", true)]
#[case::svg("image/svg+xml", false)]
#[case::html("text/html", false)]
#[case::xhtml("application/xhtml+xml", false)]
#[case::xml("text/xml", false)]
#[case::app_xml("application/xml", false)]
#[case::javascript("application/javascript", false)]
#[case::text_javascript("text/javascript", false)]
#[case::json("application/json", false)]
#[case::css("text/css", false)]
#[case::shell("application/x-sh", false)]
#[case::octet("application/octet-stream", false)]
#[case::garbage("not a mime", false)]
#[case::empty("", false)]
fn unit_download_inline_is_limited_to_safe_types(#[case] mime: &str, #[case] safe: bool) {
    assert_eq!(inline_is_safe(mime), safe, "{mime}");
    let served = effective_disposition(DispositionType::Inline, mime);
    assert_eq!(served == DispositionType::Inline, safe, "{mime}");
    assert_eq!(
        effective_disposition(DispositionType::Attachment, mime),
        DispositionType::Attachment,
        "{mime}"
    );
}

#[rstest]
#[case(None, Some(DispositionType::Attachment))]
#[case(Some(""), Some(DispositionType::Attachment))]
#[case(Some("other=1"), Some(DispositionType::Attachment))]
#[case(Some("disposition=attachment"), Some(DispositionType::Attachment))]
#[case(Some("disposition=inline"), Some(DispositionType::Inline))]
#[case(Some("disposition=Inline"), None)]
#[case(Some("disposition="), None)]
#[case(Some("disposition=../../etc/passwd"), None)]
#[case(Some("disposition=inline&disposition=attachment"), None)]
#[case(Some("disposition=https%3A%2F%2Fevil.test%2F"), None)]
fn unit_download_disposition_query_is_strict(
    #[case] query: Option<&str>,
    #[case] expected: Option<DispositionType>,
) {
    match (parse_disposition(query), expected) {
        (Ok(parsed), Some(expected)) => assert_eq!(parsed, expected),
        (Err(error), None) => {
            assert_eq!(error.code(), ErrorCode::ValidationError);
            assert_eq!(error.status(), 422);
        }
        (outcome, expected) => panic!("{query:?}: {outcome:?} vs {expected:?}"),
    }
}

#[test]
fn unit_download_content_type_is_the_stored_type_or_the_octet_stream_fallback() {
    assert_eq!(served_content_type("video/mp4"), "video/mp4");
    assert_eq!(served_content_type("application/pdf"), "application/pdf");
    assert_eq!(
        served_content_type("garbage\r\nSet-Cookie: x"),
        "application/octet-stream"
    );
}

#[test]
fn unit_download_entity_tag_is_strong_and_prefers_the_verified_checksum() {
    let stat = ObjectStat {
        size: 4_096,
        modified_at: datetime!(2026-09-25 10:15:30.250 UTC),
        etag: Some(ETag::new("provider-etag-must-not-leak")),
    };
    let digest = "ab".repeat(32);
    let by_checksum = EntityTag::of(Some(&digest), &stat);
    assert_eq!(by_checksum.as_str(), format!("\"{digest}\""));

    let fallback = EntityTag::of(None, &stat);
    assert!(fallback.as_str().starts_with('"') && fallback.as_str().ends_with('"'));
    assert!(!fallback.as_str().starts_with("W/"));
    assert!(!fallback.as_str().contains("provider-etag"));
    assert_ne!(fallback, by_checksum);
    for bad in ["", "xyz", &"AB".repeat(32), &"a".repeat(63)] {
        assert_eq!(EntityTag::of(Some(bad), &stat), fallback, "{bad:?}");
    }

    let rewritten = ObjectStat {
        modified_at: datetime!(2026-09-25 10:15:31 UTC),
        ..stat.clone()
    };
    assert_ne!(EntityTag::of(None, &rewritten), fallback);
    let resized = ObjectStat {
        size: 4_097,
        ..stat
    };
    assert_ne!(EntityTag::of(None, &resized), fallback);
}

fn io_failure() -> StorageError {
    StorageError::Io(io::Error::from(io::ErrorKind::BrokenPipe))
}

fn transient() -> StorageError {
    StorageError::ProviderUnavailable(Retryable::new("503 from the provider"))
}

#[rstest]
#[case::missing_file(DownloadError::NotFound, ErrorCode::FileNotFound, 404)]
#[case::unsatisfiable(
    DownloadError::RangeNotSatisfiable { size: 9 },
    ErrorCode::RangeNotSatisfiable,
    416
)]
#[case::missing_object(DownloadError::ObjectMissing, ErrorCode::StorageObjectMissing, 502)]
#[case::size_mismatch(
    DownloadError::SizeMismatch { recorded: 1, actual: 2 },
    ErrorCode::StorageSizeMismatch,
    500
)]
#[case::provider_mismatch(
    DownloadError::ProviderMismatch {
        recorded: "s3".to_owned(),
        configured: "local",
    },
    ErrorCode::StorageProviderMismatch,
    500
)]
#[case::corrupt(
    DownloadError::CorruptRecord { column: "object_key" },
    ErrorCode::InternalError,
    500
)]
#[case::busy(
    DownloadError::Backpressure { retry_after: 5 },
    ErrorCode::TransferBackpressure,
    503
)]
#[case::permission(
    DownloadError::from_storage(StorageError::PermissionDenied),
    ErrorCode::StoragePermissionDenied,
    500
)]
#[case::local_io(
    DownloadError::from_storage(io_failure()),
    ErrorCode::StorageUnavailable,
    503
)]
#[case::provider_5xx(
    DownloadError::from_storage(StorageError::S3("upstream said 500 at secret-bucket".into())),
    ErrorCode::StorageUpstreamError,
    502
)]
#[case::provider_unreachable(
    DownloadError::from_storage(transient()),
    ErrorCode::StorageUnavailable,
    503
)]
#[case::storage_not_found(
    DownloadError::from_storage(StorageError::NotFound),
    ErrorCode::StorageObjectMissing,
    502
)]
#[case::storage_range(
    DownloadError::from_storage(StorageError::RangeNotSatisfiable { size: 3 }),
    ErrorCode::RangeNotSatisfiable,
    416
)]
fn it_download_storage_errors_structured(
    #[case] error: DownloadError,
    #[case] code: ErrorCode,
    #[case] status: u16,
) {
    let api = error.api_error();
    assert_eq!(api.code(), code, "{error}");
    assert_eq!(api.status().as_u16(), status, "{error}");
    assert!(api.details().is_empty(), "{error}");
}

#[test]
fn unit_download_sources_never_materialise_a_whole_object() {
    for (name, source) in [
        ("service.rs", include_str!("service.rs")),
        ("stream.rs", include_str!("stream.rs")),
        ("routes.rs", include_str!("routes.rs")),
        ("model.rs", include_str!("model.rs")),
    ] {
        for forbidden in [
            "tokio::fs::read",
            "std::fs::read",
            "read_to_end",
            "read_to_string",
            "to_bytes",
            "ByteStream",
            ".collect().await",
            "Vec::with_capacity(size",
            "BytesMut::with_capacity(size",
            "Compress",
            "gzip",
        ] {
            assert!(!source.contains(forbidden), "{name} uses {forbidden}");
        }
    }
}

#[test]
fn unit_download_grant_context_is_minted_only_where_ownership_was_proven() {
    fn walk(directory: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::io::read_to_string(std::io::Read::take(
                    std::fs::File::open(&path).unwrap(),
                    4 * 1024 * 1024,
                ))
                .unwrap();
                if text.contains("GrantContext::after_authorization") {
                    found.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    walk(&source, &mut found);
    found.sort();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found[0].ends_with("features/downloads/service.rs"),
        "{found:?}"
    );
    assert!(
        found[1].ends_with("features/downloads/tests.rs"),
        "{found:?}"
    );
}

#[test]
fn unit_download_route_declares_its_byte_path_policy() {
    use crate::app::auth_class::AuthClass;
    use crate::app::router::{
        application_routes, Deadline, RateLimitClass, ResponseEncoding, Transport,
    };

    let inventory = application_routes().build().unwrap().inventory;
    let entry = inventory
        .get(&http::Method::GET, "/api/v1/files/{id}/content")
        .expect("the content route is registered");
    let policy = entry.policy();
    assert_eq!(policy.auth(), AuthClass::Authenticated);
    assert_eq!(policy.rate_limit(), RateLimitClass::TransferData);
    let Transport::BytePath(path) = policy.transport() else {
        panic!("a download is a byte path");
    };
    assert!(path.declares_opt_out());
    assert_eq!(
        policy.transport().response_encoding(),
        ResponseEncoding::Identity
    );
    assert_eq!(policy.transport().deadline(), Deadline::IdleOnly);
    let layers = entry.layers();
    assert!(!layers.compresses_response());
    assert_eq!(layers.deadline(), None);
}

#[tokio::test]
async fn unit_download_stream_reads_only_what_the_client_consumes() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    struct Counting {
        produced: Arc<AtomicU64>,
    }
    impl AsyncRead for Counting {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let take = buffer.remaining();
            buffer.put_slice(&vec![9; take]);
            self.produced.fetch_add(take as u64, Ordering::SeqCst);
            Poll::Ready(Ok(()))
        }
    }

    let produced = Arc::new(AtomicU64::new(0));
    let pool = permits(1);
    let permit = pool.acquire().await.unwrap();
    let body: ObjectBody = Box::pin(Counting {
        produced: Arc::clone(&produced),
    });
    let mut stream = ContentStream::new(body, 8 * 1024 * 1024 * 1024, permit);
    for _ in 0..3 {
        let frame = stream.next().await.unwrap().unwrap();
        assert!(frame.len() <= CHUNK_BYTES);
    }
    drop(stream);
    assert!(
        produced.load(Ordering::SeqCst) <= 8 * CHUNK_BYTES as u64,
        "an 8 GiB response read {} bytes ahead of three consumed frames",
        produced.load(Ordering::SeqCst)
    );
    assert_eq!(pool.available(), 1);
}

struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

struct Endless {
    _flag: DropFlag,
}

impl AsyncRead for Endless {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let take = buffer.remaining();
        buffer.put_slice(&vec![7; take]);
        Poll::Ready(Ok(()))
    }
}

#[tokio::test(start_paused = true)]
async fn unit_download_idle_client_is_released_after_the_idle_timeout() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dropped = Arc::new(AtomicBool::new(false));
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let body: ObjectBody = Box::pin(Endless {
        _flag: DropFlag(Arc::clone(&dropped)),
    });
    let mut stream = ContentStream::new(body, 1 << 40, permit);

    stream.next().await.unwrap().unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        pool.available(),
        0,
        "a client inside the window keeps its slot"
    );
    assert!(!dropped.load(Ordering::SeqCst));

    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        pool.available(),
        1,
        "a client that stopped consuming loses the slot"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "and the provider reader is closed"
    );

    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(pool.available(), 1);
}

#[tokio::test(start_paused = true)]
async fn unit_download_progress_resets_the_idle_clock() {
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let mut stream = ContentStream::new(repeating(1 << 30), 1 << 30, permit);
    for _ in 0..6 {
        stream.next().await.unwrap().unwrap();
        tokio::time::advance(Duration::from_secs(45)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            pool.available(),
            0,
            "a steadily consuming client is never timed out"
        );
    }
    drop(stream);
    assert_eq!(pool.available(), 1);
}

#[tokio::test(start_paused = true)]
async fn unit_download_slow_provider_is_not_an_idle_client() {
    let (reader, mut writer) = tokio::io::duplex(1024);
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let mut stream = ContentStream::new(Box::pin(reader), 10, permit);
    let pending = tokio::spawn(async move {
        let first = stream.next().await.unwrap().unwrap();
        (first, stream)
    });
    tokio::time::advance(Duration::from_secs(600)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        pool.available(),
        0,
        "waiting for the provider does not forfeit the slot"
    );
    tokio::io::AsyncWriteExt::write_all(&mut writer, b"0123456789")
        .await
        .unwrap();
    let (first, stream) = pending.await.unwrap();
    assert_eq!(first.len(), 10);
    drop(stream);
    assert_eq!(pool.available(), 1);
}

struct Blocked {
    _flag: DropFlag,
}

impl AsyncRead for Blocked {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

#[tokio::test(start_paused = true)]
async fn unit_download_idle_release_frees_the_slot_the_reader_and_the_watchdog() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dropped = Arc::new(AtomicBool::new(false));
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let body: ObjectBody = Box::pin(Endless {
        _flag: DropFlag(Arc::clone(&dropped)),
    });
    let mut stream = ContentStream::new(body, 1 << 40, permit);
    let alive = stream.alive_probe();

    stream.next().await.unwrap().unwrap();
    assert_eq!(stream.holders(), 2, "the stream and its watchdog");
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::task::yield_now().await;

    assert!(
        dropped.load(Ordering::SeqCst),
        "the provider reader is dropped"
    );
    assert_eq!(pool.available(), 1);
    assert_eq!(stream.holders(), 1, "the watchdog task has finished");
    let next = pool.acquire().await;
    assert!(next.is_ok(), "the next request gets the slot at once");
    drop(next);
    assert_eq!(
        stream.next().await.unwrap().unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    drop(stream);
    tokio::task::yield_now().await;
    assert!(!alive(), "nothing keeps the shared state alive");
}

#[tokio::test(start_paused = true)]
async fn unit_download_dropped_stream_leaves_no_background_task() {
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let mut stream = ContentStream::new(repeating(1 << 20), 1 << 20, permit);
    let alive = stream.alive_probe();
    stream.next().await.unwrap().unwrap();
    assert_eq!(stream.holders(), 2);
    drop(stream);
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    assert!(!alive(), "dropping the body aborts the watchdog");
    assert_eq!(pool.available(), 1);
}

#[tokio::test(start_paused = true)]
async fn unit_download_blocked_producer_is_not_retained_after_the_client_leaves() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dropped = Arc::new(AtomicBool::new(false));
    let pool = DownloadPermits::new(1, Duration::from_millis(40));
    let permit = pool.acquire().await.unwrap();
    let body: ObjectBody = Box::pin(Blocked {
        _flag: DropFlag(Arc::clone(&dropped)),
    });
    let mut stream = ContentStream::new(body, 100, permit);
    let alive = stream.alive_probe();
    let reading = tokio::spawn(async move { stream.next().await });
    tokio::time::advance(Duration::from_secs(3_600)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        pool.available(),
        0,
        "the provider is merely slow; the slot is kept"
    );
    assert!(!dropped.load(Ordering::SeqCst));

    reading.abort();
    let _ = reading.await;
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "a disconnect drops the blocked reader"
    );
    assert_eq!(pool.available(), 1);
    assert!(!alive());
}
