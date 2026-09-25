use std::collections::BTreeSet;
use std::fmt::{self, Debug};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use aws_sdk_s3::config::retry::RetryConfig;
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
    SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::body::SdkBody;
use http_body_util::BodyExt as _;
use tokio::io::{AsyncRead, ReadBuf};
use url::Url;

use super::assembly::{planned_part, WriteRoute, COPY_CONCURRENCY};
use super::client::S3Clients;
use super::copy::{CopyRoute, SINGLE_COPY_MAX};
use super::multipart_tests::drain;
use super::object_tests::minio::MinioServer;
use super::plan::{plan_parts, FileTooLargeReason, PartPlan, PartPlanError};
use super::profile::{ProfileLimits, ProviderProfile, GIB, MIB, TIB};
use super::provider::capabilities;
use super::{Failure, Operation, S3Failure, S3Provider};
use crate::config::{S3Config, S3Profile, S3TlsVerification, StorageConfig};
use crate::domain::secret::Secret;
use crate::storage::caps::StorageCapabilities;
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{
    ObjectBody, PutHint, StorageDescriptor, StorageProvider, MAX_LIST_PAGE_SIZE,
};
use crate::storage::ProviderKind;

const BUFFER_BYTES: u32 = 64 * 1024;
const INTERNAL: &str = "https://s3.internal.palmr.test:9000";
const BUCKET: &str = "palmr";
const UPLOAD_ID: &str = "palmr-t06b-upload-id";
const LAST_MODIFIED: &str = "Thu, 24 Sep 2026 12:00:00 GMT";
const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
const HEAD_ETAG: &str = "\"provider-measured\"";
const PART_DELAY: Duration = Duration::from_millis(2);

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    url: Url,
    copy_source: Option<String>,
    copy_range: Option<String>,
    content_length: Option<u64>,
    body_len: u64,
    largest_frame: usize,
}

impl Seen {
    fn from_request(request: &HttpRequest) -> Self {
        let header = |name: &str| request.headers().get(name).map(str::to_owned);
        Self {
            method: request.method().to_owned(),
            url: Url::parse(request.uri()).unwrap(),
            copy_source: header("x-amz-copy-source"),
            copy_range: header("x-amz-copy-source-range"),
            content_length: header("content-length").and_then(|len| len.parse().ok()),
            body_len: 0,
            largest_frame: 0,
        }
    }

    fn query(&self, name: &str) -> Option<String> {
        self.url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    fn targets(&self, key: &ObjectKey) -> bool {
        self.url.path().ends_with(key.as_str())
    }

    fn call(&self) -> Call {
        let part = self
            .query("partNumber")
            .and_then(|number| number.parse::<u32>().ok());
        let upload = self.query("uploadId").is_some();
        match (self.method.as_str(), part, upload) {
            ("HEAD", _, _) => Call::HeadObject,
            ("GET", _, _) => Call::Get,
            ("POST", _, false) if self.query("uploads").is_some() => Call::CreateMultipart,
            ("POST", _, true) => Call::Complete,
            ("DELETE", _, true) => Call::Abort,
            ("DELETE", _, false) => Call::DeleteObject,
            ("PUT", Some(number), true) => match &self.copy_range {
                Some(range) => Call::UploadPartCopy(number, range.clone()),
                None => Call::UploadPart(number, self.body_len),
            },
            ("PUT", None, false) if self.copy_source.is_some() => Call::CopyObject,
            ("PUT", None, false) => Call::PutObject(self.body_len),
            _ => Call::Other,
        }
    }

    fn is_part_copy(&self) -> bool {
        self.method == "PUT" && self.copy_range.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    HeadObject,
    Get,
    CopyObject,
    PutObject(u64),
    DeleteObject,
    CreateMultipart,
    UploadPart(u32, u64),
    UploadPartCopy(u32, String),
    Complete,
    Abort,
    Other,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: impl Into<String>) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.into(),
    }
}

fn error_reply(status: u16, code: &str) -> Reply {
    reply(
        status,
        format!("{XML}<Error><Code>{code}</Code><Message>injected</Message><RequestId>r</RequestId></Error>"),
    )
}

fn head(size: u64) -> Reply {
    Reply {
        status: 200,
        headers: vec![
            ("content-length", size.to_string()),
            ("last-modified", LAST_MODIFIED.to_owned()),
            ("etag", HEAD_ETAG.to_owned()),
        ],
        body: String::new(),
    }
}

fn etag_reply(etag: String) -> Reply {
    Reply {
        status: 200,
        headers: vec![("etag", etag)],
        body: String::new(),
    }
}

fn initiated() -> Reply {
    reply(
        200,
        format!("{XML}<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>k</Key><UploadId>{UPLOAD_ID}</UploadId></InitiateMultipartUploadResult>"),
    )
}

fn part_copied(number: u32) -> Reply {
    reply(
        200,
        format!("{XML}<CopyPartResult><ETag>\"copied-{number}\"</ETag></CopyPartResult>"),
    )
}

fn object_copied() -> Reply {
    reply(
        200,
        format!("{XML}<CopyObjectResult><ETag>\"copied\"</ETag></CopyObjectResult>"),
    )
}

fn completed() -> Reply {
    reply(
        200,
        format!("{XML}<CompleteMultipartUploadResult><ETag>\"final-3\"</ETag></CompleteMultipartUploadResult>"),
    )
}

type Script = dyn Fn(&Seen) -> Reply + Send + Sync;

#[derive(Default)]
struct Gauge {
    current: AtomicUsize,
    peak: AtomicUsize,
}

impl Gauge {
    fn enter(&self) {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    fn leave(&self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct Scripted {
    seen: Arc<Mutex<Vec<Seen>>>,
    script: Arc<Script>,
    gauge: Arc<Gauge>,
    part_delay: Option<Duration>,
}

impl Debug for Scripted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Scripted")
    }
}

impl Scripted {
    fn new(script: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            script: Arc::new(script),
            gauge: Arc::new(Gauge::default()),
            part_delay: None,
        }
    }

    fn with_part_delay(mut self, delay: Duration) -> Self {
        self.part_delay = Some(delay);
        self
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn calls(&self) -> Vec<Call> {
        self.seen().iter().map(Seen::call).collect()
    }

    fn provider_for(&self, profile: S3Profile) -> S3Provider {
        let clients = S3Clients::build(&StorageConfig::S3(Box::new(s3_config(
            profile,
            INTERNAL,
            BUCKET,
            "AKIA-palmr-fake",
            "palmr-fake-secret",
            true,
        ))))
        .unwrap()
        .unwrap();
        let transport = clients
            .base_config()
            .to_builder()
            .endpoint_url(INTERNAL)
            .http_client(SharedHttpClient::new(self.clone()))
            .retry_config(RetryConfig::disabled())
            .build();
        let clients = clients.with_internal_client(aws_sdk_s3::Client::from_conf(transport));
        S3Provider::new(clients, BUFFER_BYTES).unwrap()
    }

    fn provider(&self) -> S3Provider {
        self.provider_for(S3Profile::Minio)
    }
}

impl HttpConnector for Scripted {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let fake = self.clone();
        HttpConnectorFuture::new(async move {
            let mut seen = Seen::from_request(&request);
            let gauged = seen.is_part_copy();
            if gauged {
                fake.gauge.enter();
            }
            let mut body = request.into_body();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|error| ConnectorError::other(error, None))?;
                if let Ok(data) = frame.into_data() {
                    seen.largest_frame = seen.largest_frame.max(data.len());
                    seen.body_len += data.len() as u64;
                }
            }
            if let (true, Some(delay)) = (gauged, fake.part_delay) {
                tokio::time::sleep(delay).await;
            }
            let canned = (fake.script)(&seen);
            fake.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(seen);
            if gauged {
                fake.gauge.leave();
            }
            let mut response = HttpResponse::new(
                StatusCode::try_from(canned.status).unwrap(),
                SdkBody::from(canned.body),
            );
            for (name, value) in canned.headers {
                response.headers_mut().insert(name, value);
            }
            Ok(response)
        })
    }
}

impl HttpClient for Scripted {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

fn s3_config(
    profile: S3Profile,
    endpoint: &str,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
    force_path_style: bool,
) -> S3Config {
    S3Config {
        profile,
        endpoint: Url::parse(endpoint).unwrap(),
        public_endpoint: None,
        region: "us-east-1".to_owned(),
        bucket: bucket.to_owned(),
        access_key: Secret::new(access_key.to_owned()),
        secret_key: Secret::new(secret_key.to_owned()),
        force_path_style,
        ca_file: None,
        tls_verification: S3TlsVerification::Enabled,
        multipart_ttl: Duration::from_secs(24 * 60 * 60),
    }
}

struct Pattern {
    position: u64,
    end: u64,
    seed: u8,
}

impl AsyncRead for Pattern {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut chunk = [0_u8; 4096];
        let remaining = this.end - this.position;
        let len = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.remaining())
            .min(chunk.len());
        for (offset, byte) in chunk[..len].iter_mut().enumerate() {
            let at = this.position + offset as u64;
            *byte = (at.wrapping_mul(2_654_435_761) >> 13) as u8 ^ this.seed;
        }
        buf.put_slice(&chunk[..len]);
        this.position += len as u64;
        Poll::Ready(Ok(()))
    }
}

fn pattern_range(start: u64, len: u64, seed: u8) -> ObjectBody {
    Box::pin(Pattern {
        position: start,
        end: start + len,
        seed,
    })
}

fn pattern(len: u64, seed: u8) -> ObjectBody {
    pattern_range(0, len, seed)
}

fn declared(len: u64) -> PutHint {
    PutHint {
        declared_len: Some(len),
        content_type: Some("application/octet-stream".to_owned()),
    }
}

fn s3_failure(error: &StorageError) -> Option<&S3Failure> {
    match error {
        StorageError::S3(source) => source.downcast_ref::<S3Failure>(),
        StorageError::ProviderUnavailable(retryable) => std::error::Error::source(retryable)
            .and_then(|source| source.downcast_ref::<S3Failure>()),
        _ => None,
    }
}

fn assert_invalid(error: &StorageError, operation: Operation) {
    let failure = s3_failure(error).unwrap_or_else(|| panic!("{error:?}"));
    assert_eq!(failure.operation, operation, "{error:?}");
    assert!(
        matches!(failure.failure, Failure::InvalidRequest(_)),
        "{error:?}"
    );
    assert!(!error.to_string().contains(UPLOAD_ID));
}

fn by_part_number(calls: &[Call]) -> Vec<Call> {
    let mut ordered = calls.to_vec();
    ordered.sort_by_key(|call| match call {
        Call::UploadPartCopy(number, _) | Call::UploadPart(number, _) => *number,
        _ => 0,
    });
    ordered
}

fn expected_copy_ranges(size: u64) -> Vec<Call> {
    let plan = plan_parts(size, &ProviderProfile::Minio).unwrap();
    (1..=plan.part_count())
        .map(|number| {
            let (start, end) = plan.part_range(number, size).unwrap();
            Call::UploadPartCopy(
                u32::try_from(number).unwrap(),
                format!("bytes={start}-{}", end - 1),
            )
        })
        .collect()
}

fn expected_upload_parts(len: u64) -> Vec<Call> {
    let plan = plan_parts(len, &ProviderProfile::Minio).unwrap();
    (1..=plan.part_count())
        .map(|number| {
            let (start, end) = plan.part_range(number, len).unwrap();
            Call::UploadPart(u32::try_from(number).unwrap(), end - start)
        })
        .collect()
}

fn copy_script(
    src: ObjectKey,
    src_size: u64,
    dst_size: u64,
    fail_part: Option<u32>,
    abort_status: u16,
) -> impl Fn(&Seen) -> Reply + Send + Sync + 'static {
    move |seen| match seen.call() {
        Call::HeadObject if seen.targets(&src) => head(src_size),
        Call::HeadObject => head(dst_size),
        Call::CopyObject => object_copied(),
        Call::CreateMultipart => initiated(),
        Call::UploadPartCopy(number, _) if Some(number) == fail_part => {
            error_reply(403, "AccessDenied")
        }
        Call::UploadPartCopy(number, _) => part_copied(number),
        Call::Complete => completed(),
        Call::Abort => reply(abort_status, ""),
        other => panic!("unexpected {other:?}"),
    }
}

async fn routed_copy(size: u64, delay: Option<Duration>) -> (Scripted, ObjectKey, ObjectKey) {
    let src = ObjectKey::allocate(KeyNamespace::Objects);
    let dst = ObjectKey::allocate(KeyNamespace::Objects);
    let mut fake = Scripted::new(copy_script(src.clone(), size, size, None, 204));
    if let Some(delay) = delay {
        fake = fake.with_part_delay(delay);
    }
    let stat = fake.provider().copy(&src, &dst).await.unwrap();
    assert_eq!(stat.size, size);
    assert_eq!(
        stat.etag.as_ref().map(|etag| etag.as_str()),
        Some(HEAD_ETAG)
    );
    (fake, src, dst)
}

#[tokio::test]
async fn unit_copy_threshold_5gib_boundary() {
    assert_eq!(SINGLE_COPY_MAX, 5_368_709_120);
    assert_eq!(SINGLE_COPY_MAX, 5 * GIB);
    assert_eq!(COPY_CONCURRENCY, 4);
    let max_object = ProviderProfile::Minio.limits().max_object;
    for (size, route) in [
        (0, CopyRoute::CopyObject),
        (SINGLE_COPY_MAX - 1, CopyRoute::CopyObject),
        (SINGLE_COPY_MAX, CopyRoute::CopyObject),
        (SINGLE_COPY_MAX + 1, CopyRoute::UploadPartCopy),
        (max_object, CopyRoute::UploadPartCopy),
    ] {
        assert_eq!(CopyRoute::for_size(size, SINGLE_COPY_MAX), route, "{size}");
    }
    assert_eq!(
        S3Provider::new(
            S3Clients::build(&StorageConfig::S3(Box::new(s3_config(
                S3Profile::Generic,
                INTERNAL,
                BUCKET,
                "a",
                "b",
                true
            ))))
            .unwrap()
            .unwrap(),
            BUFFER_BYTES
        )
        .unwrap()
        .single_copy_max,
        SINGLE_COPY_MAX
    );

    for size in [0, SINGLE_COPY_MAX - 1, SINGLE_COPY_MAX] {
        let (fake, src, dst) = routed_copy(size, None).await;
        let seen = fake.seen();
        assert_eq!(
            fake.calls(),
            [Call::HeadObject, Call::CopyObject, Call::HeadObject],
            "{size}"
        );
        assert!(seen[0].targets(&src) && seen[1].targets(&dst) && seen[2].targets(&dst));
        assert_eq!(
            seen[1].copy_source.as_deref(),
            Some(format!("{BUCKET}/{}", src.as_str()).as_str())
        );
        assert!(seen.iter().all(|request| request.body_len == 0));
    }

    let size = SINGLE_COPY_MAX + 1;
    let (fake, src, dst) = routed_copy(size, Some(PART_DELAY)).await;
    let calls = fake.calls();
    let parts = expected_copy_ranges(size);
    assert_eq!(parts.len(), 641);
    assert_eq!(
        parts.last(),
        Some(&Call::UploadPartCopy(
            641,
            format!("bytes={SINGLE_COPY_MAX}-{SINGLE_COPY_MAX}")
        ))
    );
    assert_eq!(calls[..2], [Call::HeadObject, Call::CreateMultipart]);
    assert_eq!(by_part_number(&calls[2..calls.len() - 2]), parts);
    assert_eq!(calls[calls.len() - 2..], [Call::Complete, Call::HeadObject]);
    assert_eq!(fake.gauge.peak(), COPY_CONCURRENCY);
    let seen = fake.seen();
    assert!(seen[0].targets(&src));
    assert!(seen[1..].iter().all(|request| request.targets(&dst)));
    assert!(seen
        .iter()
        .filter(|request| request.is_part_copy())
        .all(|request| request.copy_source.as_deref()
            == Some(format!("{BUCKET}/{}", src.as_str()).as_str())
            && request.query("uploadId").as_deref() == Some(UPLOAD_ID)));
    assert!(seen.iter().all(|request| request.body_len == 0
        || matches!(request.call(), Call::CreateMultipart | Call::Complete)));

    let (fake, _, _) = routed_copy(max_object, None).await;
    let calls = fake.calls();
    let parts = expected_copy_ranges(max_object);
    assert_eq!(parts.len(), 5_120);
    assert_eq!(by_part_number(&calls[2..calls.len() - 2]), parts);
    assert!(!calls.contains(&Call::Get));

    let src = ObjectKey::allocate(KeyNamespace::Objects);
    let dst = ObjectKey::allocate(KeyNamespace::Objects);
    let fake = Scripted::new(copy_script(src.clone(), max_object + 1, 0, None, 204));
    let error = fake.provider().copy(&src, &dst).await.unwrap_err();
    assert_invalid(&error, Operation::UploadPartCopy);
    assert_eq!(fake.calls(), [Call::HeadObject]);
}

#[test]
fn unit_multipart_ranges_follow_the_canonical_plan() {
    for size in [
        SINGLE_COPY_MAX + 1,
        6 * GIB + 7,
        48 * GIB + 800 * MIB,
        TIB,
        5 * TIB,
    ] {
        let plan = plan_parts(size, &ProviderProfile::Minio).unwrap();
        let mut next = 0_u64;
        for number in 1..=plan.part_count() {
            let part = planned_part(Operation::UploadPartCopy, plan, number, size).unwrap();
            assert_eq!(u64::from(part.number), number);
            assert_eq!(*part.range.start(), next);
            assert!(part.byte_len() >= 1);
            assert!(part.byte_len() <= plan.part_size().unwrap());
            next = part.range.end() + 1;
        }
        assert_eq!(next, size, "{size}");
        assert!(planned_part(Operation::UploadPartCopy, plan, 0, size).is_err());
        assert!(
            planned_part(Operation::UploadPartCopy, plan, plan.part_count() + 1, size).is_err()
        );
    }
}

#[tokio::test]
async fn unit_multipart_copy_aborts_and_keeps_the_primary_error() {
    let size = SINGLE_COPY_MAX + 1;
    for abort_status in [204, 500] {
        let src = ObjectKey::allocate(KeyNamespace::Objects);
        let dst = ObjectKey::allocate(KeyNamespace::Objects);
        let fake = Scripted::new(copy_script(src.clone(), size, size, Some(7), abort_status))
            .with_part_delay(PART_DELAY);
        let error = fake.provider().copy(&src, &dst).await.unwrap_err();
        assert!(
            matches!(error, StorageError::PermissionDenied),
            "{abort_status}: {error:?}"
        );
        let calls = fake.calls();
        assert_eq!(calls.iter().filter(|call| **call == Call::Abort).count(), 1);
        assert_eq!(calls.last(), Some(&Call::Abort));
        assert!(!calls.contains(&Call::Complete));
        assert!(
            calls
                .iter()
                .filter(|call| **call == Call::HeadObject)
                .count()
                == 1
        );
        assert!(fake.gauge.peak() <= COPY_CONCURRENCY);
        let abort = fake
            .seen()
            .into_iter()
            .find(|request| request.call() == Call::Abort)
            .unwrap();
        assert!(abort.targets(&dst));
        assert_eq!(abort.query("uploadId").as_deref(), Some(UPLOAD_ID));
    }

    let src = ObjectKey::allocate(KeyNamespace::Objects);
    let dst = ObjectKey::allocate(KeyNamespace::Objects);
    let completion_error = src.clone();
    let fake = Scripted::new(move |seen| match seen.call() {
        Call::Complete => error_reply(200, "InternalError"),
        _ => copy_script(completion_error.clone(), size, size, None, 204)(seen),
    });
    let error = fake.provider().copy(&src, &dst).await.unwrap_err();
    assert!(
        !matches!(error, StorageError::SizeMismatch { .. }),
        "{error:?}"
    );
    let calls = fake.calls();
    assert_eq!(calls[calls.len() - 2..], [Call::Complete, Call::Abort]);

    let src = ObjectKey::allocate(KeyNamespace::Objects);
    let dst = ObjectKey::allocate(KeyNamespace::Objects);
    let fake = Scripted::new(copy_script(src.clone(), size, size - 1, None, 204));
    let error = fake.provider().copy(&src, &dst).await.unwrap_err();
    assert!(
        matches!(error, StorageError::SizeMismatch { expected, actual } if expected == size && actual == size - 1),
        "{error:?}"
    );
    assert!(!fake.calls().contains(&Call::Abort));

    let fake = Scripted::new(copy_script(src.clone(), 42, 41, None, 204));
    let error = fake.provider().copy(&src, &dst).await.unwrap_err();
    assert!(matches!(
        error,
        StorageError::SizeMismatch {
            expected: 42,
            actual: 41
        }
    ));
}

fn write_script(key: ObjectKey, measured: Option<u64>, fail_part: Option<u32>) -> Scripted {
    Scripted::new(move |seen| {
        assert!(seen.targets(&key));
        match seen.call() {
            Call::PutObject(_) => etag_reply("\"single\"".to_owned()),
            Call::CreateMultipart => initiated(),
            Call::UploadPart(number, _) if Some(number) == fail_part => {
                error_reply(503, "SlowDown")
            }
            Call::UploadPart(number, _) => etag_reply(format!("\"part-{number}\"")),
            Call::Complete => completed(),
            Call::Abort => reply(204, ""),
            Call::HeadObject => head(measured.unwrap_or(0)),
            other => panic!("unexpected {other:?}"),
        }
    })
}

#[test]
fn unit_put_stream_route_uses_the_planner() {
    let limits = ProviderProfile::Minio.limits();
    assert_eq!(
        WriteRoute::for_len(0, &limits).unwrap(),
        WriteRoute::PutObject
    );
    assert_eq!(
        WriteRoute::for_len(1, &limits).unwrap(),
        WriteRoute::PutObject
    );
    assert_eq!(
        WriteRoute::for_len(limits.min_part, &limits).unwrap(),
        WriteRoute::PutObject
    );
    for len in [
        limits.min_part + 1,
        64 * MIB + 5,
        SINGLE_COPY_MAX + 1,
        TIB,
        limits.max_object,
    ] {
        assert_eq!(
            WriteRoute::for_len(len, &limits).unwrap(),
            WriteRoute::Multipart(plan_parts(len, &ProviderProfile::Minio).unwrap()),
            "{len}"
        );
    }
    assert_eq!(
        WriteRoute::for_len(limits.max_object + 1, &limits).unwrap_err(),
        PartPlanError::FileTooLarge {
            declared_bytes: limits.max_object + 1,
            provider_max_bytes: limits.max_object,
            reason: FileTooLargeReason::ObjectSize,
        }
    );
    assert!(matches!(
        plan_parts(0, &ProviderProfile::Minio).unwrap(),
        PartPlan::ZeroByte
    ));
}

#[tokio::test]
async fn unit_put_stream_single_shot_and_zero_byte() {
    for len in [0, 1, 300 * 1024 + 17, 5 * MIB] {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let fake = write_script(key.clone(), Some(len), None);
        let stat = fake
            .provider()
            .put_stream(&key, pattern(len, 3), declared(len))
            .await
            .unwrap();
        assert_eq!(stat.size, len);
        assert_eq!(
            stat.etag.as_ref().map(|etag| etag.as_str()),
            Some(HEAD_ETAG)
        );
        assert_eq!(
            fake.calls(),
            [Call::PutObject(len), Call::HeadObject],
            "{len}"
        );
        let put = &fake.seen()[0];
        assert_eq!(put.content_length, Some(len));
        assert!(put.largest_frame <= BUFFER_BYTES as usize, "{len}");
    }
}

#[tokio::test]
async fn unit_put_stream_multipart_is_bounded_and_planned() {
    for len in [5 * MIB + 1, 16 * MIB + 3, 64 * MIB + 5] {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let fake = write_script(key.clone(), Some(len), None);
        let stat = fake
            .provider()
            .put_stream(&key, pattern(len, 4), declared(len))
            .await
            .unwrap();
        assert_eq!(stat.size, len);
        assert_eq!(
            stat.etag.as_ref().map(|etag| etag.as_str()),
            Some(HEAD_ETAG)
        );

        let calls = fake.calls();
        let parts = expected_upload_parts(len);
        assert_eq!(calls[0], Call::CreateMultipart);
        assert_eq!(calls[1..calls.len() - 2], parts[..], "{len}");
        assert_eq!(calls[calls.len() - 2..], [Call::Complete, Call::HeadObject]);
        for request in fake.seen() {
            assert!(
                request.largest_frame <= BUFFER_BYTES as usize,
                "{len}: {}",
                request.largest_frame
            );
            if let Call::UploadPart(_, body) = request.call() {
                assert_eq!(request.content_length, Some(body));
                assert_eq!(request.query("uploadId").as_deref(), Some(UPLOAD_ID));
            }
        }
    }
}

#[tokio::test]
async fn unit_put_stream_rejects_before_any_byte_moves() {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let fake = write_script(key.clone(), None, None);
    let s3 = fake.provider();
    let unknown = PutHint {
        declared_len: None,
        content_type: None,
    };
    let error = s3
        .put_stream(&key, pattern(1, 1), unknown)
        .await
        .unwrap_err();
    assert_invalid(&error, Operation::PutObject);
    let max_object = s3.limits().max_object;
    let error = s3
        .put_stream(&key, pattern(0, 1), declared(max_object + 1))
        .await
        .unwrap_err();
    assert_invalid(&error, Operation::PutObject);
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn unit_put_stream_aborts_incomplete_multipart() {
    let declared_len = 20 * MIB + 5;
    let cases: [(u64, Option<u32>, io::ErrorKind); 3] = [
        (10 * MIB, None, io::ErrorKind::UnexpectedEof),
        (declared_len + 1, None, io::ErrorKind::InvalidData),
        (declared_len, Some(2), io::ErrorKind::Other),
    ];
    for (actual, fail_part, kind) in cases {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let fake = write_script(key.clone(), Some(declared_len), fail_part);
        let error = fake
            .provider()
            .put_stream(&key, pattern(actual, 5), declared(declared_len))
            .await
            .unwrap_err();
        match (&error, fail_part) {
            (StorageError::ProviderUnavailable(_), Some(_)) => {}
            (StorageError::Io(source), None) => assert_eq!(source.kind(), kind),
            other => panic!("{other:?}"),
        }
        let calls = fake.calls();
        assert_eq!(calls.last(), Some(&Call::Abort), "{actual}");
        assert!(!calls.contains(&Call::Complete), "{actual}");
        assert!(!calls.contains(&Call::HeadObject), "{actual}");
    }

    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let fake = write_script(key.clone(), Some(99), None);
    let error = fake
        .provider()
        .put_stream(&key, pattern(100, 6), declared(100))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        StorageError::SizeMismatch {
            expected: 100,
            actual: 99
        }
    ));
}

#[tokio::test]
async fn unit_s3_capabilities_follow_profile() {
    for profile in ProviderProfile::ALL {
        let fake = Scripted::new(|seen| panic!("unexpected {:?}", seen.call()));
        let provider: Arc<dyn StorageProvider> =
            Arc::new(fake.provider_for(profile.config_profile()));
        let limits = profile.limits();
        assert_eq!(*provider.caps(), capabilities(&limits), "{profile:?}");
        assert_eq!(provider.caps().max_object_size, limits.max_object);
        assert_eq!(provider.caps().min_part_size, limits.min_part);
        assert_eq!(provider.caps().max_part_size, limits.max_part);
        assert_eq!(provider.caps().max_parts, limits.max_parts);
        assert_eq!(
            provider.caps().requires_checksum_headers,
            limits.requires_part_checksums
        );
        if profile == ProviderProfile::R2 {
            assert!(provider.caps().requires_checksum_headers);
            assert!(!provider.caps().supports_presigned_put);
        } else {
            assert_eq!(
                *provider.caps(),
                StorageCapabilities::S3_DEFAULT,
                "{profile:?}"
            );
        }
        assert_eq!(
            provider.as_multipart().is_some(),
            provider.caps().supports_multipart
        );
        assert_eq!(
            provider.as_presign().is_some(),
            provider.caps().supports_presigned_get
        );
        assert!(provider.as_multipart().is_some() && provider.as_presign().is_some());
        assert_eq!(
            provider.describe(),
            StorageDescriptor {
                provider: ProviderKind::S3,
                local: None,
            }
        );
        let report = provider.self_test().await.unwrap();
        assert!(!report.passed);
        assert!(fake.calls().is_empty());
    }

    let reduced = ProfileLimits {
        supports_presigned_get: false,
        ..ProfileLimits::BASELINE
    };
    let fake = Scripted::new(|seen| panic!("unexpected {:?}", seen.call()));
    let provider = fake.provider().with_limits(reduced);
    assert!(!provider.caps().supports_presigned_get);
    assert!(provider.as_presign().is_none());
}

#[test]
fn unit_s3_provider_composes_primitives() {
    let provider = include_str!("provider.rs");
    let assembly = include_str!("assembly.rs");
    for (name, source) in [("provider.rs", provider), ("assembly.rs", assembly)] {
        for forbidden in [
            concat!("ByteStream::", "collect"),
            ".collect().await",
            "body.collect",
            "read_to_end",
            "read_to_string",
            "into_bytes",
            "to_vec()",
            "Vec<u8>",
            "BytesMut",
            "with_capacity(",
            "aggregate",
            "todo!",
            "unimplemented!",
            "panic!",
            "unwrap()",
            "expect(",
            ".internal()",
            ".get_object()",
            ".put_object()",
            ".copy_object()",
            ".upload_part()",
            ".upload_part_copy()",
            ".create_multipart_upload()",
            ".complete_multipart_upload()",
            ".abort_multipart_upload()",
            "fn plan_parts",
            "ceil_div",
            "div_ceil",
            "next_pow2",
            "next_power_of_two",
            "MIN_PART_SIZE",
            "PROXY_PART_SIZE",
            "* MIB",
            "* GIB",
            "unknown_part_size",
            "presign_get(",
            "presign_put(",
            "sign_get(",
            "sign_upload_parts",
            "sign_part_urls",
            "signing_client",
            "public_signer",
            "tokio::spawn",
            "JoinSet",
            "join_all",
            "buffer_unordered",
            "upload_id",
            "f32",
            "f64",
            concat!("SystemTime::", "now"),
            concat!("OffsetDateTime::", "now_utc"),
        ] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }

    assert!(assembly.contains("pub const COPY_CONCURRENCY: usize = 4;"));
    assert_eq!(assembly.matches(".buffered(COPY_CONCURRENCY)").count(), 1);
    assert_eq!(assembly.matches("plan_parts_with_limits(").count(), 2);
    assert!(assembly.contains("self.upload_part_copy(handle, part.number, src, part.range)"));
    assert!(assembly
        .contains(".upload_part_stream(&handle, part.number, part_len, source.part(part_len))"));
    assert_eq!(
        assembly
            .matches("self.complete_multipart(&handle, &parts)")
            .count(),
        2
    );
    assert_eq!(
        assembly
            .matches("self.abort_unless_complete(&handle, assembled)")
            .count(),
        2
    );
    assert_eq!(assembly.matches("self.abort_multipart(handle)").count(), 1);
    assert_eq!(assembly.matches("tracing::").count(), 1);

    assert!(provider.contains("CopyRoute::CopyObject => self.copy_object(src, dst).await?"));
    assert!(provider.contains("self.copy_multipart(src, dst, source.size)"));
    assert!(provider.contains("let source = self.head_object(src).await?;"));
    assert!(
        provider.contains("self.put_object_single(key, body, len, hint.content_type.as_deref())")
    );
    assert!(!provider.contains("tracing::"));
    assert!(!provider.contains("AppState"));
    assert!(!assembly.contains("AppState"));

    let copy = include_str!("copy.rs");
    assert!(copy.contains("pub const SINGLE_COPY_MAX: u64 = 5 * GIB;"));
    assert!(copy.contains("if size <= single_copy_max {"));
}

async fn equal_bodies(left: ObjectBody, right: ObjectBody) {
    assert_eq!(drain(left).await, drain(right).await);
}

fn minio_clients(server: &MinioServer, bucket: &str, force_path_style: bool) -> S3Clients {
    S3Clients::build(&StorageConfig::S3(Box::new(s3_config(
        S3Profile::Minio,
        server.endpoint(),
        bucket,
        server.access_key(),
        server.secret_key(),
        force_path_style,
    ))))
    .unwrap()
    .unwrap()
}

fn style(force_path_style: bool) -> &'static str {
    if force_path_style {
        "path"
    } else {
        "vhost"
    }
}

async fn list_all(provider: &dyn StorageProvider) -> Vec<String> {
    let mut collected = Vec::new();
    let mut cursor = None;
    loop {
        let page = provider.list_page("objects/", cursor, 5).await.unwrap();
        assert!(page.entries.len() <= 5);
        for entry in &page.entries {
            assert_eq!(ObjectKey::parse(&entry.key).unwrap().as_str(), entry.key);
            collected.push(entry.key.clone());
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => return collected,
        }
    }
}

async fn storage_contract(server: &MinioServer, force_path_style: bool) {
    let style = style(force_path_style);
    let bucket = server
        .create_bucket(&format!("contract-{style}"))
        .await
        .unwrap();
    let concrete = S3Provider::new(
        minio_clients(server, &bucket, force_path_style),
        BUFFER_BYTES,
    )
    .unwrap();
    let provider: Arc<dyn StorageProvider> = Arc::new(concrete);
    let mut live = BTreeSet::new();

    assert_eq!(
        provider.describe(),
        StorageDescriptor {
            provider: ProviderKind::S3,
            local: None,
        }
    );
    assert_eq!(
        *provider.caps(),
        capabilities(&ProviderProfile::Minio.limits())
    );
    assert!(provider.caps().supports_multipart && provider.as_multipart().is_some());
    assert!(provider.caps().supports_presigned_get && provider.as_presign().is_some());
    let multipart = provider.as_multipart().unwrap();

    let zero = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = provider
        .put_stream(&zero, pattern(0, 1), declared(0))
        .await
        .unwrap();
    assert_eq!(stat.size, 0, "{style}");
    assert!(stat.etag.is_some());
    live.insert(zero.as_str().to_owned());

    let small_len = 300 * 1024 + 17;
    let small = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = provider
        .put_stream(&small, pattern(small_len, 2), declared(small_len))
        .await
        .unwrap();
    assert_eq!(stat.size, small_len, "{style}");
    assert_eq!(provider.stat(&small).await.unwrap(), stat);
    live.insert(small.as_str().to_owned());

    let big_len = 16 * MIB + 3;
    let big = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = provider
        .put_stream(&big, pattern(big_len, 7), declared(big_len))
        .await
        .unwrap();
    assert_eq!(stat.size, big_len, "{style}");
    assert_eq!(provider.stat(&big).await.unwrap(), stat);
    assert!(
        stat.etag.as_ref().unwrap().as_str().contains('-'),
        "{style}: multipart ETag"
    );
    live.insert(big.as_str().to_owned());
    let pending = multipart
        .list_multipart_uploads(big.as_str(), None)
        .await
        .unwrap();
    assert!(pending.uploads.is_empty(), "{style}");

    let unknown = ObjectKey::allocate(KeyNamespace::Objects);
    let error = provider
        .put_stream(
            &unknown,
            pattern(10, 1),
            PutHint {
                declared_len: None,
                content_type: None,
            },
        )
        .await
        .unwrap_err();
    assert_invalid(&error, Operation::PutObject);
    assert!(!provider.exists(&unknown).await.unwrap());

    let short = ObjectKey::allocate(KeyNamespace::Objects);
    let error = provider
        .put_stream(&short, pattern(9 * MIB, 8), declared(big_len))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, StorageError::Io(source) if source.kind() == io::ErrorKind::UnexpectedEof),
        "{style}: {error:?}"
    );
    assert!(!provider.exists(&short).await.unwrap());
    let pending = multipart
        .list_multipart_uploads(short.as_str(), None)
        .await
        .unwrap();
    assert!(pending.uploads.is_empty(), "{style}: aborted");

    assert!(provider.exists(&zero).await.unwrap());
    assert!(provider.exists(&big).await.unwrap());
    let missing = ObjectKey::allocate(KeyNamespace::Objects);
    assert!(!provider.exists(&missing).await.unwrap());
    assert!(matches!(
        provider.stat(&missing).await.unwrap_err(),
        StorageError::NotFound
    ));
    assert!(matches!(
        provider.open_read(&missing).await.err().unwrap(),
        StorageError::NotFound
    ));

    let (stat, body) = provider.open_read(&big).await.unwrap();
    assert_eq!(stat.size, big_len);
    equal_bodies(body, pattern(big_len, 7)).await;
    let (stat, body) = provider.open_read(&zero).await.unwrap();
    assert_eq!(stat.size, 0);
    assert_eq!(drain(body).await.0, 0);

    assert!(matches!(
        provider.open_range(&zero, 0, 1).await.err().unwrap(),
        StorageError::RangeNotSatisfiable { size: 0 }
    ));
    assert!(matches!(
        provider.open_range(&big, big_len, 1).await.err().unwrap(),
        StorageError::RangeNotSatisfiable { size } if size == big_len
    ));
    let start = 8 * MIB - 1_234;
    let (stat, body) = provider.open_range(&big, start, 4_096).await.unwrap();
    assert_eq!(stat.size, big_len);
    equal_bodies(body, pattern_range(start, 4_096, 7)).await;
    let (_, body) = provider
        .open_range(&big, big_len - 100, 1_000_000)
        .await
        .unwrap();
    equal_bodies(body, pattern_range(big_len - 100, 100, 7)).await;
    let (_, body) = provider.open_range(&big, start, 0).await.unwrap();
    assert_eq!(drain(body).await.0, 0);

    let small_copy = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = provider.copy(&small, &small_copy).await.unwrap();
    assert_eq!(stat.size, small_len, "{style}");
    let (_, body) = provider.open_read(&small_copy).await.unwrap();
    equal_bodies(body, pattern(small_len, 2)).await;
    live.insert(small_copy.as_str().to_owned());

    let zero_copy = ObjectKey::allocate(KeyNamespace::Objects);
    assert_eq!(provider.copy(&zero, &zero_copy).await.unwrap().size, 0);
    live.insert(zero_copy.as_str().to_owned());
    assert!(matches!(
        provider
            .copy(&missing, &ObjectKey::allocate(KeyNamespace::Objects))
            .await
            .unwrap_err(),
        StorageError::NotFound
    ));

    let splitting: Arc<dyn StorageProvider> = Arc::new(
        S3Provider::new(
            minio_clients(server, &bucket, force_path_style),
            BUFFER_BYTES,
        )
        .unwrap()
        .with_single_copy_max(MIB),
    );
    let big_copy = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = splitting.copy(&big, &big_copy).await.unwrap();
    assert_eq!(stat.size, big_len, "{style}");
    assert!(
        stat.etag.as_ref().unwrap().as_str().contains('-'),
        "{style}: part copy"
    );
    live.insert(big_copy.as_str().to_owned());

    assert!(provider.delete(&big).await.unwrap(), "{style}");
    assert!(!provider.delete(&big).await.unwrap(), "{style}");
    assert!(!provider.exists(&big).await.unwrap());
    live.remove(big.as_str());
    let (stat, body) = provider.open_read(&big_copy).await.unwrap();
    assert_eq!(stat.size, big_len);
    equal_bodies(body, pattern(big_len, 7)).await;
    assert!(provider.delete(&small).await.unwrap());
    live.remove(small.as_str());
    let (_, body) = provider.open_read(&small_copy).await.unwrap();
    equal_bodies(body, pattern(small_len, 2)).await;
    assert!(!provider.delete(&missing).await.unwrap());

    for seed in 0..9 {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        provider
            .put_stream(&key, pattern(17, seed), declared(17))
            .await
            .unwrap();
        live.insert(key.as_str().to_owned());
    }
    let listed = list_all(provider.as_ref()).await;
    assert_eq!(listed, live.iter().cloned().collect::<Vec<_>>(), "{style}");
    let whole = provider
        .list_page("objects/", None, MAX_LIST_PAGE_SIZE + 1)
        .await
        .unwrap();
    assert_eq!(whole.entries.len(), live.len());
    assert!(whole.next.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn it_storage_contract_s3() {
    let server = MinioServer::start().await.unwrap();
    storage_contract(&server, true).await;
    storage_contract(&server, false).await;
}

#[derive(Clone)]
struct Tap {
    inner: SharedHttpClient,
    seen: Arc<Mutex<Vec<Seen>>>,
    gauge: Arc<Gauge>,
    fail_part: Arc<Mutex<Option<u32>>>,
}

impl Debug for Tap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Tap")
    }
}

impl Tap {
    fn new(inner: SharedHttpClient) -> Self {
        Self {
            inner,
            seen: Arc::new(Mutex::new(Vec::new())),
            gauge: Arc::new(Gauge::default()),
            fail_part: Arc::new(Mutex::new(None)),
        }
    }

    fn reset(&self, fail_part: Option<u32>) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        *self
            .fail_part
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = fail_part;
        self.gauge.peak.store(0, Ordering::SeqCst);
    }

    fn calls(&self) -> Vec<Call> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(Seen::call)
            .collect()
    }
}

impl HttpClient for Tap {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(TapConnector {
            inner: self.inner.http_connector(settings, components),
            tap: self.clone(),
        })
    }
}

#[derive(Debug)]
struct TapConnector {
    inner: SharedHttpConnector,
    tap: Tap,
}

impl HttpConnector for TapConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let seen = Seen::from_request(&request);
        let part_copy = seen.is_part_copy();
        let injected = match seen.call() {
            Call::UploadPartCopy(number, _) => {
                *self
                    .tap
                    .fail_part
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    == Some(number)
            }
            _ => false,
        };
        self.tap
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(seen);
        let gauge = Arc::clone(&self.tap.gauge);
        let forwarded = (!injected).then(|| self.inner.call(request));
        HttpConnectorFuture::new(async move {
            if part_copy {
                gauge.enter();
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let outcome = match forwarded {
                Some(future) => future.await,
                None => {
                    let canned = error_reply(500, "InternalError");
                    Ok(HttpResponse::new(
                        StatusCode::try_from(canned.status).unwrap(),
                        SdkBody::from(canned.body),
                    ))
                }
            };
            if part_copy {
                gauge.leave();
            }
            outcome
        })
    }
}

async fn copy_orchestration(server: &MinioServer, force_path_style: bool) {
    let style = style(force_path_style);
    let bucket = server
        .create_bucket(&format!("copy-{style}"))
        .await
        .unwrap();
    let clients = minio_clients(server, &bucket, force_path_style);
    let inner = clients
        .internal_client()
        .client()
        .config()
        .http_client()
        .unwrap();
    let tap = Tap::new(inner);
    let config = clients
        .internal_client()
        .client()
        .config()
        .to_builder()
        .http_client(SharedHttpClient::new(tap.clone()))
        .build();
    let clients = clients.with_internal_client(aws_sdk_s3::Client::from_conf(config));
    let s3 = S3Provider::new(clients, BUFFER_BYTES)
        .unwrap()
        .with_single_copy_max(MIB);

    let size = 32 * MIB + 3;
    let src = ObjectKey::allocate(KeyNamespace::Objects);
    s3.put_stream(&src, pattern(size, 11), declared(size))
        .await
        .unwrap();

    tap.reset(None);
    let dst = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = s3.copy(&src, &dst).await.unwrap();
    let calls = tap.calls();
    assert_eq!(stat.size, size, "{style}");
    assert_eq!(s3.stat(&src).await.unwrap().size, stat.size);
    let parts = expected_copy_ranges(size);
    assert_eq!(parts.len(), 5);
    assert_eq!(
        calls[..2],
        [Call::HeadObject, Call::CreateMultipart],
        "{style}"
    );
    assert_eq!(calls[2..calls.len() - 2], parts[..], "{style}");
    assert_eq!(calls[calls.len() - 2..], [Call::Complete, Call::HeadObject]);
    assert!(
        !calls.contains(&Call::Get),
        "{style}: no source bytes through Palmr"
    );
    assert_eq!(tap.gauge.peak(), COPY_CONCURRENCY, "{style}");

    let (_, body) = s3.open_read(&dst).await.unwrap();
    equal_bodies(body, pattern(size, 11)).await;
    let (_, body) = s3.open_read(&src).await.unwrap();
    equal_bodies(body, pattern(size, 11)).await;

    tap.reset(Some(3));
    let failed = ObjectKey::allocate(KeyNamespace::Objects);
    let error = s3.copy(&src, &failed).await.unwrap_err();
    assert!(
        matches!(error, StorageError::ProviderUnavailable(_)),
        "{style}: {error:?}"
    );
    let calls = tap.calls();
    assert_eq!(calls.last(), Some(&Call::Abort), "{style}");
    assert!(!calls.contains(&Call::Complete), "{style}");
    tap.reset(None);
    assert!(!s3.exists(&failed).await.unwrap(), "{style}");
    let pending = s3
        .as_multipart()
        .unwrap()
        .list_multipart_uploads(failed.as_str(), None)
        .await
        .unwrap();
    assert!(pending.uploads.is_empty(), "{style}");
    let (_, body) = s3.open_read(&src).await.unwrap();
    equal_bodies(body, pattern(size, 11)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_s3_multipart_copy_orchestration_minio() {
    let server = MinioServer::start().await.unwrap();
    copy_orchestration(&server, true).await;
    copy_orchestration(&server, false).await;
}
