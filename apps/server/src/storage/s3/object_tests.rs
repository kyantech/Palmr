use std::collections::BTreeSet;
use std::io;
use std::time::Duration;

use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use http_body::Body as _;
use http_body_util::BodyExt as _;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use url::Url;

use super::client::S3Clients;
use super::copy::copy_source;
use super::list::max_keys;
use super::object::{ContentRange, RequestedRange, SizedBody, SourceBodyError};
use super::{classify_failure, Failure, Operation, S3Provider};
use crate::config::{S3Config, S3Profile, S3TlsVerification, StorageConfig};
use crate::domain::secret::Secret;
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{ListCursor, ObjectBody, MAX_LIST_PAGE_SIZE};

#[path = "../../../tests/support/minio.rs"]
mod minio;

use minio::MinioServer;

const BUFFER_BYTES: u32 = 64 * 1024;
const PAYLOAD_BYTES: usize = 300 * 1024 + 17;
const LISTED_OBJECTS: usize = 5;

const PRIMITIVE_SOURCES: [(&str, &str); 4] = [
    ("mod.rs", include_str!("mod.rs")),
    ("object.rs", include_str!("object.rs")),
    ("copy.rs", include_str!("copy.rs")),
    ("list.rs", include_str!("list.rs")),
];

const OPERATION_CALLS: [&str; 6] = [
    ".head_object()",
    ".get_object()",
    ".put_object()",
    ".delete_object()",
    ".copy_object()",
    ".list_objects_v2()",
];

fn service(status: u16, code: Option<&str>) -> Failure {
    Failure::Service {
        status,
        code: code.map(str::to_owned),
    }
}

#[test]
fn unit_s3_error_classification() {
    for failure in [
        service(404, Some("NoSuchKey")),
        service(404, Some("NotFound")),
        service(404, None),
    ] {
        let error = classify_failure(Operation::GetObject, failure);
        assert!(matches!(error, StorageError::NotFound), "{error:?}");
        assert!(!error.is_retryable());
    }

    for failure in [
        service(403, Some("AccessDenied")),
        service(403, Some("SignatureDoesNotMatch")),
        service(403, Some("InvalidAccessKeyId")),
        service(403, None),
        service(401, None),
    ] {
        let error = classify_failure(Operation::HeadObject, failure);
        assert!(matches!(error, StorageError::PermissionDenied), "{error:?}");
    }

    for failure in [
        Failure::Dispatch,
        Failure::Timeout,
        Failure::Response { status: Some(200) },
        Failure::Response { status: None },
        service(500, Some("InternalError")),
        service(503, Some("SlowDown")),
        service(503, None),
        service(429, None),
        service(400, Some("RequestTimeout")),
    ] {
        let error = classify_failure(Operation::GetObject, failure);
        assert!(
            matches!(error, StorageError::ProviderUnavailable(_)),
            "{error:?}"
        );
        assert!(error.is_retryable());
    }

    for failure in [
        service(404, Some("NoSuchBucket")),
        service(400, Some("InvalidArgument")),
        service(400, None),
        Failure::Construction,
        Failure::MalformedResponse("probe"),
        Failure::InvalidRequest("probe"),
    ] {
        let error = classify_failure(Operation::ListObjectsV2, failure);
        assert!(matches!(error, StorageError::S3(_)), "{error:?}");
    }

    let not_found = classify_failure(Operation::GetObject, service(404, Some("NoSuchKey")));
    let outage = classify_failure(Operation::GetObject, Failure::Dispatch);
    assert_ne!(not_found.api_code(), outage.api_code());

    let rendered = classify_failure(Operation::CopyObject, service(400, Some("InvalidRequest")));
    let StorageError::S3(source) = &rendered else {
        panic!("{rendered:?}");
    };
    assert_eq!(
        source.to_string(),
        "S3 CopyObject failed with HTTP 400 (InvalidRequest)"
    );
    assert_eq!(super::sanitize_code("No<Such>Key\r\nX: y"), "NoSuchKeyXy");
    assert_eq!(super::sanitize_code(&"A".repeat(500)).len(), 64);
}

#[test]
fn unit_s3_range_header_is_exact() {
    assert_eq!(RequestedRange::new(0, 1).header(), "bytes=0-0");
    assert_eq!(
        RequestedRange::new(1_000, 5_000).header(),
        "bytes=1000-5999"
    );
    assert_eq!(RequestedRange::new(10, u64::MAX).header(), "bytes=10-");
    let largest = i64::MAX.unsigned_abs();
    assert_eq!(
        RequestedRange::new(0, largest + 1).header(),
        format!("bytes=0-{largest}")
    );
    assert_eq!(RequestedRange::new(0, largest + 2).header(), "bytes=0-");

    let parsed = ContentRange::parse("bytes 1000-5999/307217").unwrap();
    assert_eq!(
        parsed,
        ContentRange::parse("bytes 1000-5999/307217").unwrap()
    );
    for invalid in [
        "bytes */307217",
        "bytes 10-5/100",
        "bytes 0-100/100",
        "bytes 0-9/*",
        "0-9/10",
        "items 0-9/10",
        "bytes 0-9",
    ] {
        assert_eq!(ContentRange::parse(invalid), None, "{invalid}");
    }
}

#[test]
fn unit_s3_list_page_size_clamped() {
    assert_eq!(max_keys(0), None);
    assert_eq!(max_keys(1), Some(1));
    assert_eq!(max_keys(999), Some(999));
    assert_eq!(max_keys(MAX_LIST_PAGE_SIZE), Some(1_000));
    assert_eq!(max_keys(MAX_LIST_PAGE_SIZE + 1), Some(1_000));
    assert_eq!(max_keys(u32::MAX), Some(1_000));
}

#[test]
fn unit_s3_copy_source_names_configured_bucket() {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    assert_eq!(
        copy_source("palmr-bucket", &key),
        format!("palmr-bucket/{}", key.as_str())
    );
}

async fn frames(mut body: SizedBody) -> (Vec<usize>, Option<SourceBodyError>) {
    let mut sizes = Vec::new();
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(frame) => sizes.push(frame.into_data().unwrap().len()),
            Err(error) => return (sizes, Some(error)),
        }
    }
    (sizes, None)
}

fn repeat(len: u64) -> ObjectBody {
    Box::pin(tokio::io::repeat(0x5a).take(len))
}

#[tokio::test]
async fn unit_s3_put_body_is_bounded_and_length_checked() {
    let buffer = 64 * 1024;
    let declared = 8 * 1024 * 1024 + 3;

    let body = SizedBody::new(repeat(declared), declared, buffer);
    assert_eq!(body.size_hint().exact(), Some(declared));
    let (sizes, error) = frames(body).await;
    assert_eq!(error, None);
    assert_eq!(sizes.iter().sum::<usize>() as u64, declared);
    assert!(sizes.len() > 1);
    assert!(sizes.iter().all(|size| *size <= buffer), "{sizes:?}");

    let (sizes, error) = frames(SizedBody::new(repeat(0), 0, buffer)).await;
    assert_eq!((sizes.len(), error), (0, None));

    let (_, error) = frames(SizedBody::new(repeat(10), 11, buffer)).await;
    assert_eq!(error, Some(SourceBodyError::Short));

    let (sizes, error) = frames(SizedBody::new(repeat(12), 11, buffer)).await;
    assert_eq!(error, Some(SourceBodyError::Long));
    assert!(sizes.is_empty());

    let (_, error) = frames(SizedBody::new(repeat(1), 0, buffer)).await;
    assert_eq!(error, Some(SourceBodyError::Long));

    let failing: ObjectBody = Box::pin(FailingReader);
    let (_, error) = frames(SizedBody::new(failing, 5, buffer)).await;
    assert_eq!(
        error,
        Some(SourceBodyError::Read(io::ErrorKind::ConnectionReset))
    );
    assert_eq!(
        SourceBodyError::Short.to_io_error().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

struct FailingReader;

impl AsyncRead for FailingReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionReset)))
    }
}

#[test]
fn unit_s3_primitives_use_internal_client_only() {
    let module = include_str!("mod.rs");
    assert!(module.contains(
        "fn internal(&self) -> &aws_sdk_s3::Client {\n        self.clients.internal_client().client()\n    }"
    ));

    for (name, source) in PRIMITIVE_SOURCES {
        for forbidden in [
            "public_signer",
            "PublicSigner",
            "raw_client",
            "presign",
            "S3Clients::build",
        ] {
            assert!(!source.contains(forbidden), "{name} mentions {forbidden}");
        }
        let calls: usize = OPERATION_CALLS
            .iter()
            .map(|call| source.matches(call).count())
            .sum();
        let internal = source.matches(".internal()").count();
        assert_eq!(
            calls, internal,
            "{name}: every S3 call starts at self.internal()"
        );
    }

    let copy = include_str!("copy.rs");
    assert!(copy.contains(".copy_object()"));
    for byte_path in [".get_object()", ".put_object()", "ByteStream", "ObjectBody"] {
        assert!(
            !copy.contains(byte_path),
            "copy.rs moves bytes via {byte_path}"
        );
    }

    let object = include_str!("object.rs");
    assert!(object.contains(".range(requested.header())"));
    let list = include_str!("list.rs");
    assert!(list.contains(".max_keys(max_keys)"));
    assert!(list.contains("let Some(max_keys) = max_keys(page_size)"));
}

#[test]
fn unit_s3_primitives_never_buffer_whole_objects() {
    for (name, source) in PRIMITIVE_SOURCES {
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
            "aggregate",
            "from_static",
            "SdkBody::from(",
        ] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}

#[test]
fn unit_s3_primitives_stay_in_scope() {
    for (name, source) in PRIMITIVE_SOURCES {
        for forbidden in [
            "create_multipart_upload",
            "upload_part",
            "complete_multipart_upload",
            "abort_multipart_upload",
            "list_parts",
            "list_multipart_uploads",
            "impl StorageProvider for S3Provider",
            "MultipartStorage for",
            "PresignStorage for",
            "plan_parts",
            "todo!",
            "unimplemented!",
            "panic!",
            "unwrap()",
            "expect(",
            "tracing::",
        ] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}

fn provider_for(
    endpoint: &str,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
    force_path_style: bool,
) -> S3Provider {
    let config = S3Config {
        profile: S3Profile::Minio,
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
    };
    let clients = S3Clients::build(&StorageConfig::S3(Box::new(config)))
        .unwrap()
        .unwrap();
    S3Provider::new(clients, BUFFER_BYTES).unwrap()
}

fn provider(server: &MinioServer, bucket: &str, force_path_style: bool) -> S3Provider {
    provider_for(
        server.endpoint(),
        bucket,
        server.access_key(),
        server.secret_key(),
        force_path_style,
    )
}

fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + 7) % 251).unwrap())
        .collect()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

async fn drain(mut body: ObjectBody) -> (u64, [u8; 32]) {
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

fn source(bytes: &[u8]) -> ObjectBody {
    Box::pin(std::io::Cursor::new(bytes.to_owned()))
}

async fn assert_addressing(
    server: &MinioServer,
    s3: &S3Provider,
    bucket: &str,
    force_path_style: bool,
) {
    let endpoint = Url::parse(server.endpoint()).unwrap();
    let domain = endpoint.host_str().unwrap();
    let resolved = s3
        .internal()
        .head_object()
        .bucket(bucket)
        .key("objects/addressing-probe")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    let url = Url::parse(resolved.uri()).unwrap();
    assert_eq!(url.port(), endpoint.port());
    if force_path_style {
        assert_eq!(url.host_str(), Some(domain));
        assert_eq!(url.path(), format!("/{bucket}/objects/addressing-probe"));
    } else {
        assert_eq!(url.host_str(), Some(format!("{bucket}.{domain}").as_str()));
        assert_eq!(url.path(), "/objects/addressing-probe");
    }
}

async fn object_primitives(server: &MinioServer, force_path_style: bool) {
    let style = if force_path_style { "path" } else { "vhost" };
    let bucket = server.create_bucket(style).await.unwrap();
    let s3 = provider(server, &bucket, force_path_style);
    assert_addressing(server, &s3, &bucket, force_path_style).await;
    let bytes = payload(PAYLOAD_BYTES);
    let size = bytes.len() as u64;

    let empty = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = s3
        .put_object_single(&empty, source(&[]), 0, None)
        .await
        .unwrap();
    assert_eq!(stat.size, 0, "{style}");
    assert_eq!(s3.head_object(&empty).await.unwrap().size, 0);
    assert!(s3.object_exists(&empty).await.unwrap());
    let (stat, body) = s3.get_object(&empty).await.unwrap();
    assert_eq!(stat.size, 0);
    assert_eq!(drain(body).await, (0, sha256(&[])));

    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let put = s3
        .put_object_single(&key, source(&bytes), size, Some("application/octet-stream"))
        .await
        .unwrap();
    assert_eq!(put.size, size, "{style}");
    let etag = put.etag.clone().expect("the provider returns an ETag");
    assert!(!etag.as_str().is_empty());

    let head = s3.head_object(&key).await.unwrap();
    assert_eq!(head.size, size);
    assert_eq!(head.etag.as_ref(), Some(&etag));
    assert_eq!(head.modified_at, put.modified_at);
    assert!(s3.object_exists(&key).await.unwrap());

    let missing = ObjectKey::allocate(KeyNamespace::Objects);
    assert!(!s3.object_exists(&missing).await.unwrap());
    assert!(matches!(
        s3.head_object(&missing).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        s3.get_object(&missing).await.map(|(stat, _)| stat),
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        s3.get_object_range(&missing, 0, 10)
            .await
            .map(|(stat, _)| stat),
        Err(StorageError::NotFound)
    ));

    let (stat, body) = s3.get_object(&key).await.unwrap();
    assert_eq!(stat.size, size);
    assert_eq!(stat.etag.as_ref(), Some(&etag));
    assert_eq!(drain(body).await, (size, sha256(&bytes)));

    for (start, len) in [
        (0_u64, 1_u64),
        (1_000, 5_000),
        (size - 10, 100),
        (0, u64::MAX),
    ] {
        let (stat, body) = s3.get_object_range(&key, start, len).await.unwrap();
        assert_eq!(stat.size, size, "{style} {start}+{len}");
        let end = usize::try_from(start.saturating_add(len).min(size)).unwrap();
        let expected = &bytes[usize::try_from(start).unwrap()..end];
        assert_eq!(
            drain(body).await,
            (expected.len() as u64, sha256(expected)),
            "{style} {start}+{len}"
        );
    }
    let (stat, body) = s3.get_object_range(&key, 5, 0).await.unwrap();
    assert_eq!((stat.size, drain(body).await.0), (size, 0));
    for (target, start) in [
        (&key, size),
        (&key, size + 1),
        (&key, u64::MAX),
        (&empty, 0),
    ] {
        let expected = if *target == key { size } else { 0 };
        match s3
            .get_object_range(target, start, 1)
            .await
            .map(|(stat, _)| stat)
        {
            Err(StorageError::RangeNotSatisfiable { size }) => assert_eq!(size, expected),
            other => panic!("{style} start {start}: {other:?}"),
        }
    }
    assert!(matches!(
        s3.get_object_range(&key, size, 0)
            .await
            .map(|(stat, _)| stat),
        Err(StorageError::RangeNotSatisfiable { .. })
    ));

    let short = ObjectKey::allocate(KeyNamespace::Objects);
    match s3
        .put_object_single(&short, source(&bytes[..5]), 10, None)
        .await
    {
        Err(StorageError::Io(error)) => assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("{style}: {other:?}"),
    }
    match s3
        .put_object_single(&short, source(&bytes[..12]), 10, None)
        .await
    {
        Err(StorageError::Io(error)) => assert_eq!(error.kind(), io::ErrorKind::InvalidData),
        other => panic!("{style}: {other:?}"),
    }
    assert!(!s3.object_exists(&short).await.unwrap());

    let copy = ObjectKey::allocate(KeyNamespace::Objects);
    let copied = s3.copy_object(&key, &copy).await.unwrap();
    assert_eq!(copied.size, size, "{style}");
    assert!(copied.etag.is_some());
    assert_eq!(s3.head_object(&copy).await.unwrap(), copied);
    let empty_copy = ObjectKey::allocate(KeyNamespace::Objects);
    assert_eq!(s3.copy_object(&empty, &empty_copy).await.unwrap().size, 0);
    assert!(matches!(
        s3.copy_object(&missing, &ObjectKey::allocate(KeyNamespace::Objects))
            .await,
        Err(StorageError::NotFound)
    ));

    s3.delete_object(&key).await.unwrap();
    assert!(!s3.object_exists(&key).await.unwrap());
    s3.delete_object(&key).await.unwrap();
    s3.delete_object(&missing).await.unwrap();
    let (stat, body) = s3.get_object(&copy).await.unwrap();
    assert_eq!(stat.size, size);
    assert_eq!(drain(body).await, (size, sha256(&bytes)));

    list_pages(server, force_path_style).await;
}

async fn list_pages(server: &MinioServer, force_path_style: bool) {
    let style = if force_path_style { "path" } else { "vhost" };
    let bucket = server
        .create_bucket(&format!("{style}-list"))
        .await
        .unwrap();
    let s3 = provider(server, &bucket, force_path_style);

    let mut expected = BTreeSet::new();
    for index in 0..LISTED_OBJECTS {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let body = payload(index + 1);
        s3.put_object_single(&key, source(&body), body.len() as u64, None)
            .await
            .unwrap();
        expected.insert((key.as_str().to_owned(), body.len() as u64));
    }
    let stray = "objects/README.txt";
    s3.internal()
        .put_object()
        .bucket(&bucket)
        .key(stray)
        .body(ByteStream::from_static(b"left by an operator"))
        .send()
        .await
        .unwrap();
    let branding = ObjectKey::allocate(KeyNamespace::Branding(
        crate::storage::key::BrandingKind::Logo,
    ));
    s3.put_object_single(&branding, source(b"logo"), 4, None)
        .await
        .unwrap();

    let empty = s3.list_objects_page("objects/", None, 0).await.unwrap();
    assert!(empty.entries.is_empty() && empty.next.is_none());

    let mut listed = Vec::new();
    let mut cursor: Option<ListCursor> = None;
    let mut pages = 0;
    loop {
        let page = s3.list_objects_page("objects/", cursor, 2).await.unwrap();
        pages += 1;
        assert!(page.entries.len() <= 2, "{style}");
        listed.extend(page.entries);
        match page.next {
            Some(next) => {
                assert!(!next.as_str().is_empty());
                cursor = Some(next);
            }
            None => break,
        }
        assert!(
            pages <= LISTED_OBJECTS,
            "{style}: pagination does not terminate"
        );
    }
    let full_pages = (LISTED_OBJECTS + 1).div_ceil(2);
    assert!(
        (full_pages..=full_pages + 1).contains(&pages),
        "{style}: {pages} pages"
    );

    let keys: Vec<&str> = listed.iter().map(|entry| entry.key.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "{style}: provider order is lexicographic");
    assert!(keys.contains(&stray));
    assert!(ObjectKey::parse(stray).is_err());
    assert!(!keys.contains(&branding.as_str()));
    let parsed: BTreeSet<(String, u64)> = listed
        .iter()
        .filter(|entry| ObjectKey::parse(&entry.key).is_ok())
        .map(|entry| (entry.key.clone(), entry.size))
        .collect();
    assert_eq!(parsed, expected, "{style}");

    let clamped = s3
        .list_objects_page("objects/", None, u32::MAX)
        .await
        .unwrap();
    assert_eq!(clamped.entries.len(), LISTED_OBJECTS + 1);
    assert!(clamped.next.is_none());

    let foreign = s3
        .list_objects_page("objects/", Some(ListCursor::new("not-a-provider-token")), 2)
        .await
        .map(|page| page.entries.len());
    assert!(
        !matches!(foreign, Err(StorageError::ProviderUnavailable(_))),
        "{style}: {foreign:?}"
    );
}

async fn failure_classes(server: &MinioServer) {
    let bucket = server.create_bucket("errors").await.unwrap();
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let s3 = provider(server, &bucket, true);
    s3.put_object_single(&key, source(b"probe"), 5, None)
        .await
        .unwrap();

    let forged = provider_for(
        server.endpoint(),
        &bucket,
        server.access_key(),
        "not-the-secret-key",
        true,
    );
    assert!(matches!(
        forged.object_exists(&key).await,
        Err(StorageError::PermissionDenied)
    ));
    assert!(matches!(
        forged.get_object(&key).await.map(|(stat, _)| stat),
        Err(StorageError::PermissionDenied)
    ));
    assert!(matches!(
        forged.delete_object(&key).await,
        Err(StorageError::PermissionDenied)
    ));
    assert!(s3.object_exists(&key).await.unwrap());

    let absent_bucket = provider(server, "palmr-no-such-bucket", true);
    assert!(matches!(
        absent_bucket.get_object(&key).await.map(|(stat, _)| stat),
        Err(StorageError::S3(_))
    ));

    let unreachable = provider_for(
        "http://127.0.0.1:1",
        &bucket,
        server.access_key(),
        server.secret_key(),
        true,
    );
    for outcome in [
        unreachable.object_exists(&key).await,
        unreachable.get_object(&key).await.map(|_| true),
        unreachable.delete_object(&key).await.map(|()| true),
    ] {
        match outcome {
            Err(error @ StorageError::ProviderUnavailable(_)) => {
                assert!(error.is_retryable());
                assert!(!format!("{error:?}").contains(server.secret_key()));
            }
            other => panic!("an unreachable provider must be an outage: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_s3_object_primitives_minio() {
    let server = MinioServer::start().await.unwrap();
    object_primitives(&server, true).await;
    object_primitives(&server, false).await;
    failure_classes(&server).await;
}
