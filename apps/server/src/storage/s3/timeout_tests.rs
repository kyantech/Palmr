use std::io;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use super::client::{http_client, S3Clients, CONNECT_TIMEOUT, STALL_GRACE};
use super::tests::test_base_url;
use super::{Failure, Operation, S3Failure, S3Provider};
use crate::config::{S3Config, S3Profile, S3TlsVerification, StorageConfig};
use crate::domain::secret::Secret;
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{ETag, MultipartHandle, MultipartStorage, ObjectBody, PartChecksum};

const PART_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Provider {
    Silent,
    SlowReader,
    StopsReading,
}

struct Peer {
    endpoint: Url,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_head(stream: &mut TcpStream) -> io::Result<(usize, Vec<u8>)> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let length = text
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Ok((length, head))
}

async fn serve(behavior: Provider) -> Peer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let Ok((length, _)) = read_head(&mut stream).await else {
                    return;
                };
                match behavior {
                    Provider::StopsReading => std::future::pending::<()>().await,
                    Provider::SlowReader => {
                        let mut left = length;
                        let mut chunk = vec![0_u8; 8 * 1024];
                        while left > 0 {
                            let want = left.min(chunk.len());
                            if stream.read_exact(&mut chunk[..want]).await.is_err() {
                                return;
                            }
                            left -= want;
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nETag: \"slow-part\"\r\nContent-Length: 0\r\n\r\n",
                            )
                            .await;
                        let _ = stream.flush().await;
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    Provider::Silent => {
                        let mut sink = vec![0_u8; 64 * 1024];
                        let mut left = length;
                        while left > 0 {
                            match stream.read(&mut sink).await {
                                Ok(0) | Err(_) => return,
                                Ok(read) => left = left.saturating_sub(read),
                            }
                        }
                        std::future::pending::<()>().await;
                    }
                }
            });
        }
    });
    Peer { endpoint, task }
}

fn config(endpoint: &Url) -> S3Config {
    S3Config {
        profile: S3Profile::Minio,
        endpoint: endpoint.clone(),
        public_endpoint: None,
        region: "us-east-1".to_owned(),
        bucket: "palmr".to_owned(),
        access_key: Secret::new("AKIA-timeout-test".to_owned()),
        secret_key: Secret::new("timeout-test-secret".to_owned()),
        force_path_style: true,
        ca_file: None,
        tls_verification: S3TlsVerification::Enabled,
        multipart_ttl: Duration::from_secs(24 * 60 * 60),
        require_part_checksums: None,
    }
}

fn clients(endpoint: &Url) -> S3Clients {
    S3Clients::build(&StorageConfig::S3(Box::new(config(endpoint))))
        .unwrap()
        .unwrap()
}

fn handle() -> MultipartHandle {
    MultipartHandle::new(ObjectKey::allocate(KeyNamespace::Objects), "upload-id")
}

fn body(len: usize) -> (ObjectBody, PartChecksum) {
    let bytes = vec![0x5a_u8; len];
    let checksum = PartChecksum::sha256(sha2::Sha256::digest(&bytes).into());
    (Box::pin(std::io::Cursor::new(bytes)), checksum)
}

use sha2::Digest as _;

fn timed_out(error: &StorageError) -> bool {
    let failure = match error {
        StorageError::ProviderUnavailable(retryable) => std::error::Error::source(retryable)
            .and_then(|source| source.downcast_ref::<S3Failure>()),
        _ => None,
    };
    failure.is_some_and(|failure| {
        failure.operation == Operation::UploadPart
            && matches!(failure.failure, Failure::Timeout | Failure::Dispatch)
    })
}

#[test]
fn unit_s3_clients_pin_connect_and_stall_protection_explicitly() {
    let peer = Url::parse("http://127.0.0.1:9").unwrap();
    let clients = clients(&peer);
    let stalled = clients
        .base_config()
        .stalled_stream_protection()
        .expect("stalled stream protection is configured, not inherited");
    assert!(stalled.upload_enabled() && stalled.download_enabled());
    assert_eq!(stalled.grace_period(), STALL_GRACE);
    assert_eq!(STALL_GRACE, Duration::from_secs(60));
    assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(10));
}

#[tokio::test]
async fn it_provider_that_never_answers_after_the_part_is_sent_times_out() {
    let peer = serve(Provider::Silent).await;
    let provider = S3Provider::new(clients(&peer.endpoint), 64 * 1024, &test_base_url())
        .unwrap()
        .with_response_wait(Duration::from_millis(400));
    let (stream, checksum) = body(1024 * 1024);
    let started = Instant::now();
    let error = provider
        .upload_part_checksummed(&handle(), 1, 1024 * 1024, stream, checksum)
        .await
        .unwrap_err();
    assert!(timed_out(&error), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn it_slow_but_progressing_upload_outlives_the_response_wait() {
    let peer = serve(Provider::SlowReader).await;
    let provider = S3Provider::new(clients(&peer.endpoint), 64 * 1024, &test_base_url())
        .unwrap()
        .with_response_wait(Duration::from_millis(300));
    let len = PART_BYTES;
    let (stream, checksum) = body(len);
    let started = Instant::now();
    let part = provider
        .upload_part_checksummed(&handle(), 1, len as u64, stream, checksum)
        .await
        .expect("the provider keeps reading, so the part is never cut off");
    assert_eq!(part.etag, ETag::new("\"slow-part\""));
    assert!(
        started.elapsed() > Duration::from_millis(1_200),
        "the upload took far longer than the response wait: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn it_stalled_upload_is_cut_by_the_inactivity_guard_not_a_total_deadline() {
    let peer = serve(Provider::StopsReading).await;
    let clients = clients(&peer.endpoint);
    let sdk = aws_sdk_s3::Client::from_conf(
        clients
            .base_config()
            .to_builder()
            .endpoint_url(peer.endpoint.as_str())
            .http_client(http_client(clients.internal_tls()))
            .stalled_stream_protection(
                aws_sdk_s3::config::StalledStreamProtectionConfig::enabled()
                    .grace_period(Duration::from_secs(1))
                    .build(),
            )
            .build(),
    );
    let clients = clients.with_internal_client(sdk);
    let provider = S3Provider::new(clients, 64 * 1024, &test_base_url()).unwrap();
    let (stream, checksum) = body(PART_BYTES * 2);
    let started = Instant::now();
    let error = provider
        .upload_part_checksummed(&handle(), 1, (PART_BYTES * 2) as u64, stream, checksum)
        .await
        .unwrap_err();
    assert!(timed_out(&error), "{error:?}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}
