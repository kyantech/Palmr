use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use futures_util::Stream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout, Instant};
use tokio_util::io::ReaderStream;

use super::error::DownloadError;
use super::model::{CHUNK_BYTES, DOWNLOAD_CONCURRENCY, DOWNLOAD_IDLE_TIMEOUT, PERMIT_WAIT};
use crate::storage::provider::ObjectBody;

const NOT_HANDED_OVER: u64 = u64::MAX;

#[derive(Clone)]
pub struct DownloadPermits {
    semaphore: Arc<Semaphore>,
    wait: Duration,
}

impl DownloadPermits {
    pub fn new(capacity: usize, wait: Duration) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            wait,
        }
    }

    pub fn production() -> Self {
        Self::new(DOWNLOAD_CONCURRENCY, PERMIT_WAIT)
    }

    pub async fn acquire(&self) -> Result<OwnedSemaphorePermit, DownloadError> {
        let retry_after = self.wait.as_secs().max(1);
        match timeout(self.wait, Arc::clone(&self.semaphore).acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) | Err(_) => Err(DownloadError::Backpressure { retry_after }),
        }
    }

    #[cfg(test)]
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

struct Live {
    source: ReaderStream<ObjectBody>,
    _permit: OwnedSemaphorePermit,
}

struct Shared {
    live: Mutex<Option<Live>>,
    handed_over_ms: AtomicU64,
    idle_expired: AtomicBool,
    started: Instant,
}

impl Shared {
    fn release(&self) {
        let released = self
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(released);
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis())
            .unwrap_or(NOT_HANDED_OVER - 1)
            .min(NOT_HANDED_OVER - 1)
    }
}

pub struct ContentStream {
    shared: Arc<Shared>,
    watchdog: Option<JoinHandle<()>>,
    idle: Duration,
    remaining: u64,
    sent: u64,
    request_id: Option<String>,
    empty: bool,
}

impl ContentStream {
    pub fn new(body: ObjectBody, length: u64, permit: OwnedSemaphorePermit) -> Self {
        Self {
            shared: Arc::new(Shared {
                live: Mutex::new(Some(Live {
                    source: ReaderStream::with_capacity(body, CHUNK_BYTES),
                    _permit: permit,
                })),
                handed_over_ms: AtomicU64::new(NOT_HANDED_OVER),
                idle_expired: AtomicBool::new(false),
                started: Instant::now(),
            }),
            watchdog: None,
            idle: DOWNLOAD_IDLE_TIMEOUT,
            remaining: length,
            sent: 0,
            request_id: None,
            empty: false,
        }
    }

    pub fn empty() -> Self {
        Self {
            shared: Arc::new(Shared {
                live: Mutex::new(None),
                handed_over_ms: AtomicU64::new(NOT_HANDED_OVER),
                idle_expired: AtomicBool::new(false),
                started: Instant::now(),
            }),
            watchdog: None,
            idle: DOWNLOAD_IDLE_TIMEOUT,
            remaining: 0,
            sent: 0,
            request_id: None,
            empty: true,
        }
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<&str>) -> Self {
        self.request_id = request_id.map(str::to_owned);
        self
    }

    #[cfg(test)]
    pub fn alive_probe(&self) -> impl Fn() -> bool {
        let weak = Arc::downgrade(&self.shared);
        move || weak.strong_count() > 0
    }

    #[cfg(test)]
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.shared)
    }

    pub fn into_body(self) -> Body {
        if self.empty {
            Body::empty()
        } else {
            Body::from_stream(self)
        }
    }

    fn finish(&mut self) {
        self.shared.release();
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.abort();
        }
    }

    fn fail(&mut self, error: &io::Error) {
        tracing::warn!(
            kind = "download_stream_failed",
            request_id = self.request_id.as_deref(),
            io_error = ?error.kind(),
            sent_bytes = self.sent,
            missing_bytes = self.remaining,
            "a download stream failed after the response started"
        );
        self.finish();
    }

    fn watch(&self) -> JoinHandle<()> {
        tokio::spawn(watch_for_idle(
            Arc::clone(&self.shared),
            self.idle,
            self.request_id.clone(),
        ))
    }
}

async fn watch_for_idle(shared: Arc<Shared>, idle: Duration, request_id: Option<String>) {
    loop {
        let handed_over = shared.handed_over_ms.load(Ordering::Acquire);
        let wait = if handed_over == NOT_HANDED_OVER {
            idle
        } else {
            let due = shared.started + Duration::from_millis(handed_over) + idle;
            match due.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => left,
                _ => {
                    shared.idle_expired.store(true, Ordering::Release);
                    shared.release();
                    tracing::warn!(
                        kind = "download_idle_timeout",
                        request_id = request_id.as_deref(),
                        idle_seconds = idle.as_secs(),
                        "a download was released because the client stopped consuming it"
                    );
                    return;
                }
            }
        };
        sleep(wait).await;
    }
}

impl Stream for ContentStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.remaining == 0 {
            this.finish();
            return Poll::Ready(None);
        }
        if this.watchdog.is_none() {
            this.watchdog = Some(this.watch());
        }
        this.shared
            .handed_over_ms
            .store(NOT_HANDED_OVER, Ordering::Release);
        let polled = {
            let mut live = this
                .shared
                .live
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            live.as_mut()
                .map(|live| Pin::new(&mut live.source).poll_next(context))
        };
        match polled {
            None => {
                let kind = if this.shared.idle_expired.load(Ordering::Acquire) {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::BrokenPipe
                };
                let error = io::Error::from(kind);
                this.fail(&error);
                Poll::Ready(Some(Err(error)))
            }
            Some(Poll::Pending) => Poll::Pending,
            Some(Poll::Ready(Some(Ok(mut chunk)))) => {
                let remaining = usize::try_from(this.remaining).unwrap_or(usize::MAX);
                chunk.truncate(remaining);
                let taken = chunk.len() as u64;
                this.remaining -= taken;
                this.sent += taken;
                this.shared
                    .handed_over_ms
                    .store(this.shared.elapsed_ms(), Ordering::Release);
                Poll::Ready(Some(Ok(chunk)))
            }
            Some(Poll::Ready(Some(Err(error)))) => {
                this.fail(&error);
                Poll::Ready(Some(Err(error)))
            }
            Some(Poll::Ready(None)) => {
                let error = io::Error::from(io::ErrorKind::UnexpectedEof);
                this.fail(&error);
                Poll::Ready(Some(Err(error)))
            }
        }
    }
}

impl Drop for ContentStream {
    fn drop(&mut self) {
        if self.remaining > 0 && !self.empty {
            let abandoned = self
                .shared
                .live
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some();
            if abandoned {
                tracing::debug!(
                    kind = "download_stream_abandoned",
                    request_id = self.request_id.as_deref(),
                    sent_bytes = self.sent,
                    unsent_bytes = self.remaining,
                    "the client stopped reading a download"
                );
            }
        }
        self.finish();
    }
}
