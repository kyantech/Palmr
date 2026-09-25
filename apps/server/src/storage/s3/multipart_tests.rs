use std::fmt::{self, Debug};
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
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
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt as _;
use url::Url;

use super::client::S3Clients;
use super::multipart::{completion_manifest, copy_source_range, decode_cursor, encode_cursor};
use super::presign_tests::{payload, sha256};
use super::profile::{ProfileLimits, GIB, MIB};
use super::tests::test_base_url;
use super::{Failure, Operation, S3Failure, S3Provider};
use crate::config::{S3Config, S3Profile, S3TlsVerification, StorageConfig};
use crate::domain::secret::Secret;
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{
    ETag, ListCursor, MultipartHandle, MultipartStorage, ObjectBody, PutHint, UploadedPart,
};

use super::object_tests::minio::MinioServer;

const BUFFER_BYTES: u32 = 64 * 1024;
const INTERNAL: &str = "https://s3.internal.palmr.test:9000";
const PUBLIC: &str = "https://files.palmr.test";
const BUCKET: &str = "palmr";
const UPLOAD_ID: &str = "palmr-upload-id-sentinel";
const LAST_MODIFIED: &str = "Thu, 24 Sep 2026 12:00:00 GMT";
const FINAL_SIZE: u64 = 123_456_789;
const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
const KEEPALIVE: &str = "      \n      \n      \n";

const MULTIPART_CALLS: [&str; 7] = [
    ".create_multipart_upload()",
    ".list_parts()",
    ".upload_part()",
    ".upload_part_copy()",
    ".complete_multipart_upload()",
    ".abort_multipart_upload()",
    ".list_multipart_uploads()",
];

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    url: Url,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    largest_frame: usize,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn query(&self, name: &str) -> Option<String> {
        self.url
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    fn body_text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
}

struct Canned {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: impl Into<String>) -> Canned {
    Canned {
        status,
        headers: Vec::new(),
        body: body.into(),
    }
}

fn error_body(code: &str) -> String {
    format!(
        "{XML_DECLARATION}<Error><Code>{code}</Code><Message>provider said no</Message><RequestId>r</RequestId></Error>"
    )
}

fn head_reply() -> Canned {
    Canned {
        status: 200,
        headers: vec![
            ("content-length", FINAL_SIZE.to_string()),
            ("last-modified", LAST_MODIFIED.to_owned()),
            ("etag", "\"final-etag-2\"".to_owned()),
        ],
        body: String::new(),
    }
}

type Responder = dyn Fn(&Recorded) -> Canned + Send + Sync;

#[derive(Clone)]
struct FakeS3 {
    requests: Arc<Mutex<Vec<Recorded>>>,
    responder: Arc<Responder>,
}

impl Debug for FakeS3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FakeS3")
    }
}

impl FakeS3 {
    fn new(responder: impl Fn(&Recorded) -> Canned + Send + Sync + 'static) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responder: Arc::new(responder),
        }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn provider(&self) -> S3Provider {
        let config = S3Config {
            profile: S3Profile::Minio,
            endpoint: Url::parse(INTERNAL).unwrap(),
            public_endpoint: Some(Url::parse(PUBLIC).unwrap()),
            region: "us-east-1".to_owned(),
            bucket: BUCKET.to_owned(),
            access_key: Secret::new("AKIA-palmr-fake".to_owned()),
            secret_key: Secret::new("palmr-fake-secret".to_owned()),
            force_path_style: true,
            ca_file: None,
            tls_verification: S3TlsVerification::Enabled,
            multipart_ttl: Duration::from_secs(24 * 60 * 60),
        };
        let clients = S3Clients::build(&StorageConfig::S3(Box::new(config)))
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
        S3Provider::new(clients, BUFFER_BYTES, &test_base_url()).unwrap()
    }
}

impl HttpConnector for FakeS3 {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let fake = self.clone();
        HttpConnectorFuture::new(async move {
            let method = request.method().to_owned();
            let url = Url::parse(request.uri()).unwrap();
            let headers = request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
                .collect();
            let mut body = request.into_body();
            let mut bytes = Vec::new();
            let mut largest_frame = 0;
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|error| ConnectorError::other(error, None))?;
                if let Ok(data) = frame.into_data() {
                    largest_frame = largest_frame.max(data.len());
                    bytes.extend_from_slice(&data);
                }
            }
            let recorded = Recorded {
                method,
                url,
                headers,
                body: bytes,
                largest_frame,
            };
            let canned = (fake.responder)(&recorded);
            fake.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(recorded);
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

impl HttpClient for FakeS3 {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

fn handle() -> MultipartHandle {
    MultipartHandle::new(ObjectKey::allocate(KeyNamespace::Objects), UPLOAD_ID)
}

fn uploaded(part_number: u32, etag: &str) -> UploadedPart {
    UploadedPart {
        part_number,
        etag: ETag::new(etag),
        size: 8 * MIB,
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

fn rejected<T: Debug>(outcome: Result<T, StorageError>, operation: Operation) {
    let error = outcome.unwrap_err();
    let failure = s3_failure(&error).unwrap_or_else(|| panic!("{error:?}"));
    assert_eq!(failure.operation, operation, "{error:?}");
    assert!(
        matches!(failure.failure, Failure::InvalidRequest(_)),
        "{error:?}"
    );
}

fn service_code(error: &StorageError) -> Option<(u16, Option<String>)> {
    match &s3_failure(error)?.failure {
        Failure::Service { status, code } => Some((*status, code.clone())),
        _ => None,
    }
}

pub(super) async fn drain(mut body: ObjectBody) -> (u64, [u8; 32]) {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = body.read(&mut buffer).await.unwrap();
        if read == 0 {
            return (total, hasher.finalize().into());
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
}

fn completion_fake(completion: Canned) -> FakeS3 {
    let completion = Arc::new(Mutex::new(Some(completion)));
    FakeS3::new(move |request| match request.method.as_str() {
        "HEAD" => head_reply(),
        "POST" => completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| reply(500, error_body("InternalError"))),
        other => panic!("unexpected {other}"),
    })
}

#[tokio::test]
async fn it_complete_multipart_parses_200_error_body() {
    let parts = [uploaded(1, "\"etag-1\""), uploaded(2, "\"etag-2\"")];

    let failures: [(&str, String); 7] = [
        (
            "declaration then keep-alive then error",
            format!(
                "{XML_DECLARATION}{KEEPALIVE}<Error><Code>InternalError</Code><Message>We encountered an internal error. Please try again.</Message></Error>"
            ),
        ),
        (
            "keep-alive before the declaration",
            format!(
                "{KEEPALIVE}{XML_DECLARATION}<Error><Code>InvalidPart</Code><Message>x</Message></Error>"
            ),
        ),
        ("invalid part order", error_body("InvalidPartOrder")),
        ("entity too small", error_body("EntityTooSmall")),
        ("no such upload", error_body("NoSuchUpload")),
        ("empty 200 body", String::new()),
        (
            "result without an ETag",
            format!(
                "{XML_DECLARATION}<CompleteMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>k</Key></CompleteMultipartUploadResult>"
            ),
        ),
    ];

    for (label, body) in failures {
        let fake = completion_fake(reply(200, body));
        let s3 = fake.provider();
        let outcome = s3.complete_multipart(&handle(), &parts).await;
        let error = match outcome {
            Err(error) => error,
            Ok(stat) => panic!("{label}: an HTTP 200 error body became success: {stat:?}"),
        };
        let requests = fake.requests();
        assert_eq!(requests.len(), 1, "{label}: no HeadObject after a failure");
        assert_eq!(requests[0].method, "POST");
        match label {
            "declaration then keep-alive then error" => {
                assert!(
                    matches!(error, StorageError::ProviderUnavailable(_)),
                    "{label}: {error:?}"
                );
                assert!(error.is_retryable());
                assert_eq!(
                    service_code(&error),
                    Some((200, Some("InternalError".to_owned())))
                );
            }
            "invalid part order" | "entity too small" => {
                let code = label
                    .split(' ')
                    .map(|word| {
                        let mut chars = word.chars();
                        chars.next().map_or_else(String::new, |first| {
                            first.to_ascii_uppercase().to_string() + chars.as_str()
                        })
                    })
                    .collect::<String>();
                assert!(matches!(error, StorageError::S3(_)), "{label}: {error:?}");
                assert_eq!(service_code(&error), Some((200, Some(code))), "{label}");
            }
            "no such upload" => {
                assert!(
                    matches!(error, StorageError::NotFound),
                    "{label}: {error:?}"
                );
            }
            _ => {
                assert!(
                    s3_failure(&error).is_some(),
                    "{label}: the failure is typed: {error:?}"
                );
            }
        }
        assert!(!format!("{error:?}").contains(UPLOAD_ID), "{label}");
    }

    for body in [
        format!(
            "{XML_DECLARATION}{KEEPALIVE}<CompleteMultipartUploadResult><Location>{INTERNAL}/{BUCKET}/k</Location><Bucket>{BUCKET}</Bucket><Key>k</Key><ETag>&quot;final-etag-2&quot;</ETag></CompleteMultipartUploadResult>"
        ),
        format!(
            "{XML_DECLARATION}<CompleteMultipartUploadResult><ETag>\"final-etag-2\"</ETag></CompleteMultipartUploadResult>"
        ),
    ] {
        let fake = completion_fake(reply(200, body));
        let s3 = fake.provider();
        let handle = handle();
        let stat = s3.complete_multipart(&handle, &parts).await.unwrap();
        assert_eq!(stat.size, FINAL_SIZE, "the HeadObject size is authoritative");
        assert_eq!(stat.etag, Some(ETag::new("\"final-etag-2\"")));

        let requests = fake.requests();
        assert_eq!(
            requests
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            ["POST", "HEAD"]
        );
        let complete = &requests[0];
        assert_eq!(complete.query("uploadId").as_deref(), Some(UPLOAD_ID));
        assert_eq!(
            complete.url.path(),
            format!("/{BUCKET}/{}", handle.key().as_str())
        );
        let manifest = complete.body_text();
        let first = manifest.find("<PartNumber>1</PartNumber>").unwrap();
        let second = manifest.find("<PartNumber>2</PartNumber>").unwrap();
        assert!(first < second, "{manifest}");
        assert!(manifest.contains("<ETag>&quot;etag-1&quot;</ETag>"), "{manifest}");
        assert!(!manifest.to_ascii_lowercase().contains("checksum"));
        assert_eq!(
            requests[1].url.path(),
            format!("/{BUCKET}/{}", handle.key().as_str())
        );
    }

    let fake = completion_fake(reply(503, error_body("SlowDown")));
    let error = fake
        .provider()
        .complete_multipart(&handle(), &parts)
        .await
        .unwrap_err();
    assert!(matches!(error, StorageError::ProviderUnavailable(_)));
}

#[tokio::test]
async fn unit_completion_manifest_is_validated_before_sending() {
    let fake = FakeS3::new(|request| panic!("no request expected: {}", request.method));
    let s3 = fake.provider();
    let invalid: [Vec<UploadedPart>; 7] = [
        Vec::new(),
        vec![uploaded(1, "\"a\""), uploaded(1, "\"b\"")],
        vec![uploaded(2, "\"a\""), uploaded(1, "\"b\"")],
        vec![
            uploaded(1, "\"a\""),
            uploaded(3, "\"c\""),
            uploaded(2, "\"b\""),
        ],
        vec![uploaded(0, "\"a\"")],
        vec![uploaded(10_001, "\"a\"")],
        vec![uploaded(1, "")],
    ];
    for parts in invalid {
        rejected(
            s3.complete_multipart(&handle(), &parts).await,
            Operation::CompleteMultipartUpload,
        );
    }
    rejected(
        s3.complete_multipart(&handle(), &[uploaded(1, "\"a\n\"")])
            .await,
        Operation::CompleteMultipartUpload,
    );
    assert!(fake.requests().is_empty());

    let limits = ProfileLimits::BASELINE;
    let sparse = [
        uploaded(1, "\"a\""),
        uploaded(7, "W/\"b\""),
        uploaded(10_000, "c"),
    ];
    let manifest = completion_manifest(&sparse, &limits).unwrap();
    let built: Vec<(Option<i32>, Option<&str>)> = manifest
        .parts()
        .iter()
        .map(|part| (part.part_number(), part.e_tag()))
        .collect();
    assert_eq!(
        built,
        [
            (Some(1), Some("\"a\"")),
            (Some(7), Some("W/\"b\"")),
            (Some(10_000), Some("c"))
        ]
    );
    for part in manifest.parts() {
        assert!(part.checksum_crc32().is_none());
        assert!(part.checksum_crc32_c().is_none());
        assert!(part.checksum_sha1().is_none());
        assert!(part.checksum_sha256().is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_abort_nosuchupload_is_success() {
    let cases: [(u16, String, Option<&str>); 7] = [
        (204, String::new(), None),
        (200, String::new(), None),
        (404, error_body("NoSuchUpload"), None),
        (404, String::new(), None),
        (503, error_body("SlowDown"), Some("unavailable")),
        (500, error_body("InternalError"), Some("unavailable")),
        (403, error_body("AccessDenied"), Some("denied")),
    ];
    for (status, body, failure) in cases {
        let fake = FakeS3::new(move |_| reply(status, body.clone()));
        let outcome = fake.provider().abort_multipart(&handle()).await;
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "DELETE");
        assert_eq!(requests[0].query("uploadId").as_deref(), Some(UPLOAD_ID));
        match (failure, outcome) {
            (None, Ok(())) => {}
            (Some("unavailable"), Err(error @ StorageError::ProviderUnavailable(_))) => {
                assert!(error.is_retryable(), "{status}");
            }
            (Some("denied"), Err(StorageError::PermissionDenied)) => {}
            (expected, outcome) => panic!("{status}: expected {expected:?}, got {outcome:?}"),
        }
    }

    let fake = FakeS3::new(|_| reply(404, error_body("NoSuchBucket")));
    assert!(matches!(
        fake.provider().abort_multipart(&handle()).await,
        Err(StorageError::S3(_))
    ));

    let unreachable = S3Provider::new(
        S3Clients::build(&StorageConfig::S3(Box::new(S3Config {
            profile: S3Profile::Minio,
            endpoint: Url::parse("http://127.0.0.1:1").unwrap(),
            public_endpoint: None,
            region: "us-east-1".to_owned(),
            bucket: BUCKET.to_owned(),
            access_key: Secret::new("AKIA-palmr-fake".to_owned()),
            secret_key: Secret::new("palmr-fake-secret".to_owned()),
            force_path_style: true,
            ca_file: None,
            tls_verification: S3TlsVerification::Enabled,
            multipart_ttl: Duration::from_secs(3_600),
        })))
        .unwrap()
        .unwrap(),
        BUFFER_BYTES,
        &test_base_url(),
    )
    .unwrap();
    match unreachable.abort_multipart(&handle()).await {
        Err(error @ StorageError::ProviderUnavailable(_)) => assert!(error.is_retryable()),
        other => panic!("an unreachable provider is an outage, not success: {other:?}"),
    }

    let server = MinioServer::start().await.unwrap();
    let bucket = server.create_bucket("abort").await.unwrap();
    let s3 = minio_provider(&server, &bucket, true);
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let live = s3.create_multipart(&key, PutHint::default()).await.unwrap();
    s3.abort_multipart(&live).await.unwrap();
    s3.abort_multipart(&live).await.unwrap();
    assert!(matches!(
        s3.list_parts(&live).await,
        Err(StorageError::NotFound)
    ));
    let never = MultipartHandle::new(
        ObjectKey::allocate(KeyNamespace::Objects),
        "bm9uZXhpc3RlbnQ",
    );
    s3.abort_multipart(&never).await.unwrap();
}

fn parts_page(numbers: impl IntoIterator<Item = u32>, next: Option<u32>) -> String {
    let parts: String = numbers
        .into_iter()
        .map(|number| {
            format!(
                "<Part><PartNumber>{number}</PartNumber><LastModified>2026-09-24T12:00:00.000Z</LastModified><ETag>&quot;etag-{number}&quot;</ETag><Size>{}</Size></Part>",
                5 * MIB + u64::from(number)
            )
        })
        .collect();
    let truncation = next.map_or_else(
        || "<IsTruncated>false</IsTruncated>".to_owned(),
        |marker| {
            format!("<IsTruncated>true</IsTruncated><NextPartNumberMarker>{marker}</NextPartNumberMarker>")
        },
    );
    format!(
        "{XML_DECLARATION}<ListPartsResult><Bucket>{BUCKET}</Bucket><Key>k</Key><UploadId>{UPLOAD_ID}</UploadId><MaxParts>1000</MaxParts>{truncation}{parts}</ListPartsResult>"
    )
}

#[tokio::test]
async fn unit_list_parts_pages_past_the_provider_page() {
    let fake = FakeS3::new(
        |request| match request.query("part-number-marker").as_deref() {
            None => reply(200, parts_page(1..=1_000, Some(1_000))),
            Some("1000") => reply(200, parts_page(1_001..=1_005, None)),
            other => panic!("unexpected marker {other:?}"),
        },
    );
    let s3 = fake.provider();
    let parts = s3.list_parts(&handle()).await.unwrap();
    assert_eq!(parts.len(), 1_005);
    for (part, number) in parts.iter().zip(1_u32..) {
        assert_eq!(part.part_number, number);
        assert_eq!(part.etag.as_str(), format!("\"etag-{number}\""));
        assert_eq!(part.size, 5 * MIB + u64::from(number));
    }
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.query("max-parts").as_deref(), Some("1000"));
        assert_eq!(request.query("uploadId").as_deref(), Some(UPLOAD_ID));
        assert_eq!(request.url.host_str(), Some("s3.internal.palmr.test"));
    }
    assert_eq!(requests[0].query("part-number-marker"), None);
    assert_eq!(
        requests[1].query("part-number-marker").as_deref(),
        Some("1000")
    );

    let broken: [String; 4] = [
        parts_page(1..=3, None).replace(
            "<IsTruncated>false</IsTruncated>",
            "<IsTruncated>true</IsTruncated>",
        ),
        parts_page([], Some(3)),
        parts_page(1..=3, None).replace("<PartNumber>2</PartNumber>", "<PartNumber>1</PartNumber>"),
        parts_page(1..=3, None).replace("<Size>5242881</Size>", ""),
    ];
    for body in broken {
        let fake = FakeS3::new(move |_| reply(200, body.clone()));
        let error = fake.provider().list_parts(&handle()).await.unwrap_err();
        assert!(
            matches!(
                s3_failure(&error).map(|failure| &failure.failure),
                Some(Failure::MalformedResponse(_))
            ),
            "{error:?}"
        );
        assert_eq!(fake.requests().len(), 1, "no unbounded re-listing");
    }

    let fake = FakeS3::new(|_| reply(404, error_body("NoSuchUpload")));
    assert!(matches!(
        fake.provider().list_parts(&handle()).await,
        Err(StorageError::NotFound)
    ));
}

fn uploads_page(entries: &[(&str, &str)], next: Option<(&str, &str)>) -> String {
    let uploads: String = entries
        .iter()
        .map(|(key, upload_id)| {
            format!(
                "<Upload><Key>{key}</Key><UploadId>{upload_id}</UploadId><Initiated>2026-09-24T12:00:00.000Z</Initiated></Upload>"
            )
        })
        .collect();
    let truncation = next.map_or_else(
        || "<IsTruncated>false</IsTruncated>".to_owned(),
        |(key, upload_id)| {
            format!(
                "<IsTruncated>true</IsTruncated><NextKeyMarker>{key}</NextKeyMarker><NextUploadIdMarker>{upload_id}</NextUploadIdMarker>"
            )
        },
    );
    format!(
        "{XML_DECLARATION}<ListMultipartUploadsResult><Bucket>{BUCKET}</Bucket><MaxUploads>1000</MaxUploads>{truncation}{uploads}</ListMultipartUploadsResult>"
    )
}

#[tokio::test]
async fn unit_list_multipart_uploads_is_paged() {
    let first = ObjectKey::allocate(KeyNamespace::Objects);
    let second = ObjectKey::allocate(KeyNamespace::Objects);
    let third = ObjectKey::allocate(KeyNamespace::Objects);
    let (first_key, second_key, third_key) = (
        first.as_str().to_owned(),
        second.as_str().to_owned(),
        third.as_str().to_owned(),
    );
    let fake = FakeS3::new(
        move |request| match request.query("key-marker").as_deref() {
            None => reply(
                200,
                uploads_page(
                    &[
                        (&first_key, "upload-a"),
                        ("objects/../../foreign", "upload-foreign"),
                        ("not-a-palmr-key", "upload-foreign-2"),
                        (&second_key, "upload-b"),
                    ],
                    Some((&second_key, "upload-b")),
                ),
            ),
            Some(marker) if marker == second_key => {
                assert_eq!(
                    request.query("upload-id-marker").as_deref(),
                    Some("upload-b")
                );
                reply(200, uploads_page(&[(&third_key, "upload-c")], None))
            }
            other => panic!("unexpected marker {other:?}"),
        },
    );
    let s3 = fake.provider();

    let page = s3.list_multipart_uploads("objects/", None).await.unwrap();
    let listed: Vec<(&ObjectKey, &str)> = page
        .uploads
        .iter()
        .map(|pending| (pending.handle.key(), pending.handle.upload_id()))
        .collect();
    assert_eq!(listed, [(&first, "upload-a"), (&second, "upload-b")]);
    assert_eq!(
        page.uploads[0].initiated_at,
        time::macros::datetime!(2026-09-24 12:00:00 UTC)
    );
    let cursor = page.next.clone().unwrap();
    assert!(!format!("{cursor:?}").contains("upload-b"));
    assert!(!format!("{page:?}").contains("upload-a"));

    let page = s3
        .list_multipart_uploads("objects/", Some(cursor))
        .await
        .unwrap();
    assert_eq!(page.uploads.len(), 1);
    assert_eq!(page.uploads[0].handle.key(), &third);
    assert!(page.next.is_none());

    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert!(request.url.query_pairs().any(|(name, _)| name == "uploads"));
        assert_eq!(request.query("max-uploads").as_deref(), Some("1000"));
        assert_eq!(request.query("prefix").as_deref(), Some("objects/"));
    }

    rejected(
        s3.list_multipart_uploads("objects/", Some(ListCursor::new("list-page-token")))
            .await,
        Operation::ListMultipartUploads,
    );
    assert_eq!(fake.requests().len(), 2);

    let round_trip = encode_cursor("objects/aa/bb/key", Some("id\nwith-newline"));
    assert_eq!(
        decode_cursor(&round_trip),
        Some((
            Some("objects/aa/bb/key".to_owned()),
            Some("id\nwith-newline".to_owned())
        ))
    );
    assert_eq!(
        decode_cursor(&encode_cursor("objects/aa/bb/key", None)),
        Some((Some("objects/aa/bb/key".to_owned()), None))
    );
    assert_eq!(decode_cursor(&ListCursor::new("\nupload")), None);

    let fake = FakeS3::new(|_| {
        reply(
            200,
            uploads_page(&[], None).replace(
                "<IsTruncated>false</IsTruncated>",
                "<IsTruncated>true</IsTruncated>",
            ),
        )
    });
    let error = fake
        .provider()
        .list_multipart_uploads("objects/", None)
        .await
        .unwrap_err();
    assert!(matches!(
        s3_failure(&error).map(|failure| &failure.failure),
        Some(Failure::MalformedResponse(_))
    ));
}

#[tokio::test]
async fn unit_upload_part_copy_sends_source_range() {
    let fake = FakeS3::new(|request| {
        if request.query("partNumber").as_deref() == Some("4") {
            return reply(200, error_body("InternalError"));
        }
        reply(
            200,
            format!(
                "{XML_DECLARATION}<CopyPartResult><LastModified>2026-09-24T12:00:00.000Z</LastModified><ETag>&quot;copied-3&quot;</ETag></CopyPartResult>"
            ),
        )
    });
    let s3 = fake.provider();
    let handle = handle();
    let source = ObjectKey::allocate(KeyNamespace::Objects);
    let range = 5 * GIB..=(10 * GIB - 1);
    let part = s3
        .upload_part_copy(&handle, 3, &source, range)
        .await
        .unwrap();
    assert_eq!(
        part,
        UploadedPart {
            part_number: 3,
            etag: ETag::new("\"copied-3\""),
            size: 5 * GIB,
        }
    );

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "PUT");
    assert_eq!(request.url.host_str(), Some("s3.internal.palmr.test"));
    assert_eq!(
        request.url.path(),
        format!("/{BUCKET}/{}", handle.key().as_str())
    );
    assert_eq!(request.query("partNumber").as_deref(), Some("3"));
    assert_eq!(request.query("uploadId").as_deref(), Some(UPLOAD_ID));
    assert_eq!(
        request.header("x-amz-copy-source"),
        Some(format!("{BUCKET}/{}", source.as_str()).as_str())
    );
    assert_eq!(
        request.header("x-amz-copy-source-range"),
        Some("bytes=5368709120-10737418239")
    );
    assert!(
        request.body.is_empty(),
        "no source byte passes through Palmr"
    );
    assert!(request
        .headers
        .iter()
        .all(|(name, _)| !name.starts_with("x-amz-checksum")));

    let error = s3
        .upload_part_copy(&handle, 4, &source, 0..=(5 * MIB - 1))
        .await
        .unwrap_err();
    assert!(
        matches!(error, StorageError::ProviderUnavailable(_)),
        "{error:?}"
    );
    assert_eq!(
        service_code(&error),
        Some((200, Some("InternalError".to_owned())))
    );

    let before = fake.requests().len();
    let empty = std::ops::RangeInclusive::new(10, 9);
    for (part_number, range) in [(1, empty), (1, 0..=(5 * GIB)), (0, 0..=9), (10_001, 0..=9)] {
        rejected(
            s3.upload_part_copy(&handle, part_number, &source, range)
                .await,
            Operation::UploadPartCopy,
        );
    }
    assert_eq!(fake.requests().len(), before);

    assert_eq!(
        copy_source_range(&(0..=0)),
        Some(("bytes=0-0".to_owned(), 1))
    );
    assert_eq!(copy_source_range(&(0..=u64::MAX)), None);
    let copy = include_str!("multipart.rs")
        .split("async fn upload_part_copy(")
        .nth(1)
        .unwrap()
        .split("async fn complete_multipart(")
        .next()
        .unwrap();
    for byte_path in [
        ".get_object()",
        ".upload_part()",
        "ByteStream",
        "ObjectBody",
        "plan_parts",
    ] {
        assert!(
            !copy.contains(byte_path),
            "upload_part_copy moves bytes via {byte_path}"
        );
    }
}

struct Failing;

impl tokio::io::AsyncRead for Failing {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionReset)))
    }
}

#[tokio::test]
async fn unit_upload_part_stream_is_bounded() {
    let fake = FakeS3::new(|_| Canned {
        status: 200,
        headers: vec![("etag", "\"part-2\"".to_owned())],
        body: String::new(),
    });
    let s3 = fake.provider();
    let handle = handle();
    let len = 3 * MIB + 17;
    let body: ObjectBody = Box::pin(tokio::io::repeat(0x5a).take(len));
    let part = s3.upload_part_stream(&handle, 2, len, body).await.unwrap();
    assert_eq!(
        part,
        UploadedPart {
            part_number: 2,
            etag: ETag::new("\"part-2\""),
            size: len,
        }
    );
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "PUT");
    assert_eq!(request.url.host_str(), Some("s3.internal.palmr.test"));
    assert_eq!(request.query("partNumber").as_deref(), Some("2"));
    assert_eq!(
        request.header("content-length"),
        Some(len.to_string().as_str())
    );
    assert_eq!(request.body.len() as u64, len);
    assert!(request.largest_frame <= BUFFER_BYTES as usize);
    assert!(request
        .headers
        .iter()
        .all(|(name, _)| !name.starts_with("x-amz-checksum")
            && name != "x-amz-sdk-checksum-algorithm"));

    let short: ObjectBody = Box::pin(tokio::io::repeat(0).take(10));
    let error = s3
        .upload_part_stream(&handle, 1, 11, short)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, StorageError::Io(io) if io.kind() == io::ErrorKind::UnexpectedEof),
        "{error:?}"
    );
    let long: ObjectBody = Box::pin(tokio::io::repeat(0).take(12));
    assert!(matches!(
        s3.upload_part_stream(&handle, 1, 11, long).await,
        Err(StorageError::Io(io)) if io.kind() == io::ErrorKind::InvalidData
    ));
    assert!(matches!(
        s3.upload_part_stream(&handle, 1, 11, Box::pin(Failing)).await,
        Err(StorageError::Io(io)) if io.kind() == io::ErrorKind::ConnectionReset
    ));

    let before = fake.requests().len();
    for (part_number, len) in [(0, 1), (10_001, 1), (1, 0), (1, 5 * GIB + 1)] {
        rejected(
            s3.upload_part_stream(&handle, part_number, len, Box::pin(tokio::io::empty()))
                .await,
            Operation::UploadPart,
        );
    }
    assert_eq!(fake.requests().len(), before);

    let missing_etag = FakeS3::new(|_| reply(200, ""));
    let error = missing_etag
        .provider()
        .upload_part_stream(&handle, 1, 3, Box::pin(std::io::Cursor::new(vec![1, 2, 3])))
        .await
        .unwrap_err();
    assert!(matches!(
        s3_failure(&error).map(|failure| &failure.failure),
        Some(Failure::MalformedResponse(_))
    ));
}

#[tokio::test]
async fn unit_create_multipart_uses_caller_key() {
    let fake = FakeS3::new(|_| {
        reply(
            200,
            format!(
                "{XML_DECLARATION}<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>k</Key><UploadId>{UPLOAD_ID}</UploadId></InitiateMultipartUploadResult>"
            ),
        )
    });
    let s3 = fake.provider();
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = s3
        .create_multipart(
            &key,
            PutHint {
                declared_len: Some(40 * GIB),
                content_type: Some("video/mp4".to_owned()),
            },
        )
        .await
        .unwrap();
    assert_eq!(handle.key(), &key);
    assert_eq!(handle.upload_id(), UPLOAD_ID);
    assert!(!format!("{handle:?}").contains(UPLOAD_ID));
    let requests = fake.requests();
    assert_eq!(requests[0].method, "POST");
    assert!(requests[0]
        .url
        .query_pairs()
        .any(|(name, _)| name == "uploads"));
    assert_eq!(
        requests[0].url.path(),
        format!("/{BUCKET}/{}", key.as_str())
    );
    assert_eq!(requests[0].header("content-type"), Some("video/mp4"));

    rejected(
        s3.create_multipart(
            &key,
            PutHint {
                declared_len: Some(5 * 1024 * GIB + 1),
                content_type: None,
            },
        )
        .await,
        Operation::CreateMultipartUpload,
    );
    let empty_id = FakeS3::new(|_| {
        reply(
            200,
            format!("{XML_DECLARATION}<InitiateMultipartUploadResult><UploadId></UploadId></InitiateMultipartUploadResult>"),
        )
    });
    assert!(empty_id
        .provider()
        .create_multipart(&key, PutHint::default())
        .await
        .is_err());
}

#[tokio::test]
async fn unit_server_side_calls_use_internal_client() {
    let fake = FakeS3::new(|request| {
        match (request.method.as_str(), request.query("uploads")) {
        ("POST", Some(_)) => reply(
            200,
            format!("{XML_DECLARATION}<InitiateMultipartUploadResult><UploadId>{UPLOAD_ID}</UploadId></InitiateMultipartUploadResult>"),
        ),
        ("GET", Some(_)) => reply(200, uploads_page(&[], None)),
        ("GET", None) => reply(200, parts_page(1..=1, None)),
        ("PUT", _) if request.header("x-amz-copy-source").is_some() => reply(
            200,
            format!("{XML_DECLARATION}<CopyPartResult><ETag>\"c\"</ETag></CopyPartResult>"),
        ),
        ("PUT", _) => Canned {
            status: 200,
            headers: vec![("etag", "\"p\"".to_owned())],
            body: String::new(),
        },
        ("POST", None) => reply(
            200,
            format!("{XML_DECLARATION}<CompleteMultipartUploadResult><ETag>\"f\"</ETag></CompleteMultipartUploadResult>"),
        ),
        ("HEAD", _) => head_reply(),
        ("DELETE", _) => reply(204, ""),
        other => panic!("unexpected {other:?}"),
    }
    });
    let s3 = fake.provider();
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = s3.create_multipart(&key, PutHint::default()).await.unwrap();
    s3.list_parts(&handle).await.unwrap();
    let streamed = s3
        .upload_part_stream(
            &handle,
            1,
            5 * MIB,
            Box::pin(tokio::io::repeat(1).take(5 * MIB)),
        )
        .await
        .unwrap();
    let copied = s3.upload_part_copy(&handle, 2, &key, 0..=99).await.unwrap();
    s3.complete_multipart(&handle, &[streamed, copied])
        .await
        .unwrap();
    s3.abort_multipart(&handle).await.unwrap();
    s3.list_multipart_uploads("objects/", None).await.unwrap();

    let requests = fake.requests();
    assert_eq!(requests.len(), 8);
    for request in &requests {
        assert_eq!(request.url.host_str(), Some("s3.internal.palmr.test"));
        assert_eq!(request.url.port(), Some(9000));
    }

    let multipart = include_str!("multipart.rs");
    let calls: usize = MULTIPART_CALLS
        .iter()
        .map(|call| multipart.matches(call).count())
        .sum();
    assert_eq!(calls, 7);
    assert_eq!(
        calls,
        multipart.matches(".internal()").count(),
        "every multipart call starts at self.internal()"
    );
    assert!(multipart.contains("self.head_object(target.key()).await"));
    assert!(multipart.contains(".max_parts(provider_page())"));
    assert!(multipart.contains(".max_uploads(provider_page())"));
}

fn minio_provider(server: &MinioServer, bucket: &str, force_path_style: bool) -> S3Provider {
    let config = S3Config {
        profile: S3Profile::Minio,
        endpoint: Url::parse(server.endpoint()).unwrap(),
        public_endpoint: None,
        region: "us-east-1".to_owned(),
        bucket: bucket.to_owned(),
        access_key: Secret::new(server.access_key().to_owned()),
        secret_key: Secret::new(server.secret_key().to_owned()),
        force_path_style,
        ca_file: None,
        tls_verification: S3TlsVerification::Enabled,
        multipart_ttl: Duration::from_secs(24 * 60 * 60),
    };
    let clients = S3Clients::build(&StorageConfig::S3(Box::new(config)))
        .unwrap()
        .unwrap();
    S3Provider::new(clients, BUFFER_BYTES, &test_base_url()).unwrap()
}

fn reader(bytes: &[u8]) -> ObjectBody {
    Box::pin(std::io::Cursor::new(bytes.to_owned()))
}

async fn multipart_primitives(server: &MinioServer, force_path_style: bool) {
    let style = if force_path_style { "path" } else { "vhost" };
    let bucket = server.create_bucket(&format!("mp-{style}")).await.unwrap();
    let s3 = minio_provider(server, &bucket, force_path_style);

    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = s3
        .create_multipart(
            &key,
            PutHint {
                declared_len: Some(5 * MIB + 3),
                content_type: Some("application/octet-stream".to_owned()),
            },
        )
        .await
        .unwrap();
    assert!(
        !s3.object_exists(&key).await.unwrap(),
        "{style}: invisible before completion"
    );

    let bytes = payload(usize::try_from(5 * MIB).unwrap() + 3);
    let (head, tail) = bytes.split_at(usize::try_from(5 * MIB).unwrap());
    let first = s3
        .upload_part_stream(&handle, 1, head.len() as u64, reader(head))
        .await
        .unwrap();
    let second = s3
        .upload_part_stream(&handle, 2, tail.len() as u64, reader(tail))
        .await
        .unwrap();
    assert!(first.etag.as_str().starts_with('"') && first.etag.as_str().ends_with('"'));

    let listed = s3.list_parts(&handle).await.unwrap();
    assert_eq!(
        listed,
        [first.clone(), second.clone()],
        "{style}: ETags are verbatim"
    );

    for prefix in ["", key.as_str()] {
        let page = s3.list_multipart_uploads(prefix, None).await.unwrap();
        assert!(
            page.uploads.iter().any(|pending| pending.handle == handle),
            "{style}: {prefix}"
        );
        assert!(page.next.is_none());
    }

    let wrong = UploadedPart {
        etag: ETag::new("\"00000000000000000000000000000000\""),
        ..second.clone()
    };
    let error = s3
        .complete_multipart(&handle, &[first.clone(), wrong])
        .await
        .unwrap_err();
    assert_eq!(
        service_code(&error).and_then(|(_, code)| code).as_deref(),
        Some("InvalidPart"),
        "{style}: {error:?}"
    );

    let stat = s3
        .complete_multipart(&handle, &[first, second])
        .await
        .unwrap();
    assert_eq!(stat.size, bytes.len() as u64, "{style}");
    let (_, body) = s3.get_object(&key).await.unwrap();
    assert_eq!(drain(body).await, (stat.size, sha256(&bytes)));
    let page = s3.list_multipart_uploads("", None).await.unwrap();
    assert!(!page.uploads.iter().any(|pending| pending.handle == handle));

    let copy_key = ObjectKey::allocate(KeyNamespace::Objects);
    let copy = s3
        .create_multipart(&copy_key, PutHint::default())
        .await
        .unwrap();
    let split = 5 * MIB;
    let size = bytes.len() as u64;
    let copied_first = s3
        .upload_part_copy(&copy, 1, &key, 0..=(split - 1))
        .await
        .unwrap();
    let copied_second = s3
        .upload_part_copy(&copy, 2, &key, split..=(size - 1))
        .await
        .unwrap();
    assert_eq!(
        (copied_first.size, copied_second.size),
        (split, size - split)
    );
    let copied = s3
        .complete_multipart(&copy, &[copied_first, copied_second])
        .await
        .unwrap();
    assert_eq!(copied.size, size);
    let (_, body) = s3.get_object(&copy_key).await.unwrap();
    assert_eq!(drain(body).await, (size, sha256(&bytes)));

    let small_key = ObjectKey::allocate(KeyNamespace::Objects);
    let small = s3
        .create_multipart(&small_key, PutHint::default())
        .await
        .unwrap();
    let tiny_one = s3
        .upload_part_stream(&small, 1, 4, reader(b"tiny"))
        .await
        .unwrap();
    let tiny_two = s3
        .upload_part_stream(&small, 2, 4, reader(b"tail"))
        .await
        .unwrap();
    let error = s3
        .complete_multipart(&small, &[tiny_one, tiny_two])
        .await
        .unwrap_err();
    assert_eq!(
        service_code(&error).and_then(|(_, code)| code).as_deref(),
        Some("EntityTooSmall"),
        "{style}: {error:?}"
    );
    s3.abort_multipart(&small).await.unwrap();
    s3.abort_multipart(&small).await.unwrap();
    assert!(!s3.object_exists(&small_key).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn it_s3_multipart_primitives_minio() {
    let server = MinioServer::start().await.unwrap();
    multipart_primitives(&server, true).await;
    multipart_primitives(&server, false).await;
}
