use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Method};
use sha2::{Digest, Sha256};
use time::macros::datetime;
use time::OffsetDateTime;
use url::Url;

use super::client::S3Clients;
use super::presign::{MAX_GET_URL_TTL, MAX_PART_URLS_PER_CALL, MAX_PART_URL_TTL};
use super::profile::{ProviderProfile, GIB, MIB};
use super::{Failure, Operation, S3Failure, S3Provider};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::TestClock;
use crate::storage::caps::{PartUpload, StorageCapabilities};
use crate::storage::error::StorageError;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{
    GrantContext, MultipartHandle, MultipartStorage, PartPlanEntry, PresignStorage,
    PresignedRequest, PutHint, UploadedPart,
};

use super::object_tests::minio::MinioServer;

const BUFFER_BYTES: u32 = 64 * 1024;
const NOW: OffsetDateTime = datetime!(2026-09-24 12:00:00.750 UTC);
const SIGNED_AT: OffsetDateTime = datetime!(2026-09-24 12:00:00 UTC);
const UPLOAD_ID: &str = "palmr-upload-id-sentinel";
const INTERNAL: &str = "https://s3.internal.palmr.test:9000";
const PUBLIC: &str = "https://files.palmr.test";
const BUCKET: &str = "palmr";
const DISPOSITION: &str =
    "attachment; filename=\"resume\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%E2%9C%93";
const CONTENT_TYPE: &str = "text/plain; charset=utf-8";

const PRESIGN_SOURCES: [(&str, &str); 2] = [
    ("presign.rs", include_str!("presign.rs")),
    ("multipart.rs", include_str!("multipart.rs")),
];

fn env_provider(pairs: &[(&str, &str)]) -> S3Provider {
    let mut vars: Vec<(String, String)> = [
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_ACCESS_KEY", "AKIA-palmr-presign-test"),
        ("PALMR_S3_SECRET_KEY", "palmr-presign-secret-sentinel"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
    .collect();
    vars.extend(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    let storage = OperatorConfig::load(&EnvironmentSource::from_vars(vars))
        .unwrap()
        .config
        .storage;
    let clients = S3Clients::build(&storage).unwrap().unwrap();
    S3Provider::new(clients, BUFFER_BYTES).unwrap()
}

fn split_provider(profile: &str, force_path_style: &str) -> S3Provider {
    env_provider(&[
        ("PALMR_S3_ENDPOINT", INTERNAL),
        ("PALMR_S3_PUBLIC_ENDPOINT", PUBLIC),
        ("PALMR_S3_BUCKET", BUCKET),
        ("PALMR_S3_PROFILE", profile),
        ("PALMR_S3_FORCE_PATH_STYLE", force_path_style),
    ])
}

fn handle() -> MultipartHandle {
    MultipartHandle::new(ObjectKey::allocate(KeyNamespace::Objects), UPLOAD_ID)
}

fn part(part_number: u32) -> PartPlanEntry {
    PartPlanEntry {
        part_number,
        len: 8 * MIB,
    }
}

fn disposition() -> HeaderValue {
    HeaderValue::from_static(DISPOSITION)
}

fn query(request: &PresignedRequest) -> BTreeMap<String, String> {
    request
        .url()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn rejected<T: Debug>(outcome: Result<T, StorageError>, operation: Operation) {
    let error = outcome.unwrap_err();
    let StorageError::S3(source) = &error else {
        panic!("expected a typed request rejection, got {error:?}");
    };
    let failure = source.downcast_ref::<S3Failure>().unwrap();
    assert_eq!(failure.operation, operation, "{error:?}");
    assert!(
        matches!(failure.failure, Failure::InvalidRequest(_)),
        "{error:?}"
    );
}

pub(super) fn assert_browser_contract(request: &PresignedRequest) {
    let query = query(request);
    assert_eq!(
        query.get("X-Amz-SignedHeaders").map(String::as_str),
        Some("host")
    );
    assert_eq!(
        query.get("X-Amz-Algorithm").map(String::as_str),
        Some("AWS4-HMAC-SHA256")
    );
    for name in query.keys() {
        let lower = name.to_ascii_lowercase();
        assert!(!lower.starts_with("x-amz-checksum"), "{name}");
        assert_ne!(lower, "x-amz-sdk-checksum-algorithm");
    }
    assert!(request.headers().is_empty(), "{:?}", request.headers());
}

async fn sign_one(s3: &S3Provider, ttl: Duration) -> Result<PresignedRequest, StorageError> {
    s3.sign_upload_parts(&handle(), &[part(1)], ttl, NOW)
        .await
        .map(|mut signed| signed.remove(0))
}

async fn sign_get(s3: &S3Provider, ttl: Duration) -> Result<PresignedRequest, StorageError> {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    s3.sign_get(&key, ttl, &disposition(), CONTENT_TYPE, NOW)
        .await
}

#[tokio::test]
async fn unit_presign_ttl_ceiling() {
    let s3 = split_provider("minio", "true");
    assert_eq!(MAX_PART_URL_TTL, Duration::from_secs(900));
    assert_eq!(MAX_GET_URL_TTL, Duration::from_secs(300));

    let accepted = sign_one(&s3, Duration::from_secs(900)).await.unwrap();
    assert_eq!(accepted.expires_at(), datetime!(2026-09-24 12:15:00 UTC));
    let fields = query(&accepted);
    assert_eq!(fields["X-Amz-Expires"], "900");
    assert_eq!(fields["X-Amz-Date"], "20260924T120000Z");
    for ttl in [
        Duration::from_secs(901),
        Duration::from_millis(900_001),
        Duration::ZERO,
        Duration::from_millis(999),
        Duration::from_secs(7 * 24 * 3_600),
    ] {
        rejected(sign_one(&s3, ttl).await, Operation::UploadPart);
    }

    let accepted = sign_get(&s3, Duration::from_secs(300)).await.unwrap();
    assert_eq!(accepted.expires_at(), datetime!(2026-09-24 12:05:00 UTC));
    assert_eq!(query(&accepted)["X-Amz-Expires"], "300");
    for ttl in [
        Duration::from_secs(301),
        Duration::from_millis(300_001),
        Duration::ZERO,
        Duration::from_millis(500),
    ] {
        rejected(sign_get(&s3, ttl).await, Operation::GetObject);
    }

    let clock = TestClock::new(NOW);
    let s3 = split_provider("minio", "true").with_clock(Arc::new(clock.clone()));
    let signed = s3
        .sign_part_urls(&handle(), &[part(1)], Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(signed[0].expires_at(), SIGNED_AT + Duration::from_secs(60));
    clock.advance(Duration::from_secs(3_600));
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let grant = GrantContext::authorized_for_test();
    let get = s3
        .presign_get(
            &key,
            Duration::from_secs(300),
            &disposition(),
            CONTENT_TYPE,
            &grant,
        )
        .await
        .unwrap();
    assert_eq!(
        get.expires_at(),
        datetime!(2026-09-24 13:05:00 UTC),
        "the trait path signs with the injected clock"
    );
    rejected(
        s3.presign_get(
            &key,
            Duration::from_secs(301),
            &disposition(),
            CONTENT_TYPE,
            &grant,
        )
        .await,
        Operation::GetObject,
    );
    rejected(
        s3.sign_part_urls(&handle(), &[part(1)], Duration::from_secs(901))
            .await,
        Operation::UploadPart,
    );
}

#[tokio::test]
async fn unit_part_url_batch_ceiling_and_resigning() {
    let s3 = split_provider("minio", "true");
    let handle = handle();
    let ttl = Duration::from_secs(900);
    assert_eq!(MAX_PART_URLS_PER_CALL, 16);

    let batch: Vec<PartPlanEntry> = (1..=16).map(part).collect();
    let signed = s3
        .sign_upload_parts(&handle, &batch, ttl, NOW)
        .await
        .unwrap();
    assert_eq!(signed.len(), 16);
    for (index, request) in signed.iter().enumerate() {
        assert_eq!(request.method(), &Method::PUT);
        let fields = query(request);
        assert_eq!(fields["partNumber"], (index + 1).to_string());
        assert_eq!(fields["uploadId"], UPLOAD_ID);
        assert_browser_contract(request);
    }

    let over: Vec<PartPlanEntry> = (1..=17).map(part).collect();
    rejected(
        s3.sign_upload_parts(&handle, &over, ttl, NOW).await,
        Operation::UploadPart,
    );
    assert!(s3
        .sign_upload_parts(&handle, &[], ttl, NOW)
        .await
        .unwrap()
        .is_empty());

    let again = s3
        .sign_upload_parts(&handle, &batch[..1], ttl, NOW)
        .await
        .unwrap();
    assert_eq!(again[0].url(), signed[0].url());
    let later = s3
        .sign_upload_parts(&handle, &batch[..1], ttl, NOW + Duration::from_secs(120))
        .await
        .unwrap();
    assert_ne!(later[0].url(), signed[0].url());
    assert_eq!(query(&later[0])["partNumber"], "1");

    for invalid in [
        PartPlanEntry {
            part_number: 0,
            len: MIB,
        },
        PartPlanEntry {
            part_number: 10_001,
            len: MIB,
        },
        PartPlanEntry {
            part_number: 1,
            len: 0,
        },
        PartPlanEntry {
            part_number: 1,
            len: 5 * GIB + 1,
        },
    ] {
        rejected(
            s3.sign_upload_parts(&handle, &[invalid], ttl, NOW).await,
            Operation::UploadPart,
        );
    }
    assert_eq!(
        s3.sign_upload_parts(
            &handle,
            &[PartPlanEntry {
                part_number: 10_000,
                len: 5 * GIB,
            }],
            ttl,
            NOW,
        )
        .await
        .unwrap()
        .len(),
        1
    );
}

#[tokio::test]
async fn unit_presign_uses_public_signer_only() {
    for style in ["true", "false"] {
        let s3 = split_provider("minio", style);
        let part = sign_one(&s3, MAX_PART_URL_TTL).await.unwrap();
        let get = sign_get(&s3, MAX_GET_URL_TTL).await.unwrap();
        for request in [&part, &get] {
            let host = request.url().host_str().unwrap();
            assert!(host.ends_with("files.palmr.test"), "{style}: {host}");
            assert!(!host.contains("internal"), "{style}: {host}");
            assert_eq!(request.url().scheme(), "https");
            assert_eq!(request.url().port_or_known_default(), Some(443));
            assert!(query(request)["X-Amz-Credential"].ends_with("/us-east-1/s3/aws4_request"));
        }
    }

    let presign = include_str!("presign.rs");
    assert!(!presign.contains(".send()"));
    assert!(!presign.contains(".internal()"));
    assert!(!presign.contains("internal_client"));
    assert_eq!(
        presign.matches(".presigned(").count(),
        presign.matches(".signing_client()").count()
    );

    let multipart = include_str!("multipart.rs");
    for forbidden in [
        "public_signer",
        "signing_client",
        "PublicSigner",
        ".presigned(",
    ] {
        assert!(!multipart.contains(forbidden), "multipart.rs: {forbidden}");
    }

    for (name, source) in [
        ("mod.rs", include_str!("mod.rs")),
        ("object.rs", include_str!("object.rs")),
        ("copy.rs", include_str!("copy.rs")),
        ("list.rs", include_str!("list.rs")),
        ("plan.rs", include_str!("plan.rs")),
        ("config.rs", include_str!("config.rs")),
        ("profile.rs", include_str!("profile.rs")),
        ("tls.rs", include_str!("tls.rs")),
        ("provider.rs", include_str!("provider.rs")),
        ("assembly.rs", include_str!("assembly.rs")),
    ] {
        assert!(!source.contains("signing_client"), "{name}");
    }
    let client = include_str!("client.rs");
    assert!(client.contains("pub(super) fn signing_client(&self) -> &aws_sdk_s3::Client {"));
    assert_eq!(client.matches("signing_client").count(), 1);
}

#[tokio::test]
async fn unit_presign_get_pins_response_headers() {
    let s3 = split_provider("minio", "true");
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let signed = s3
        .sign_get(&key, MAX_GET_URL_TTL, &disposition(), CONTENT_TYPE, NOW)
        .await
        .unwrap();
    assert_eq!(signed.method(), &Method::GET);
    assert_browser_contract(&signed);
    let fields = query(&signed);
    assert_eq!(fields["response-content-disposition"], DISPOSITION);
    assert_eq!(fields["response-content-type"], CONTENT_TYPE);
    assert_eq!(signed.url().path(), format!("/{BUCKET}/{}", key.as_str()));

    let extensionless = HeaderValue::from_static("attachment; filename=\"README\"");
    let signed = s3
        .sign_get(
            &key,
            MAX_GET_URL_TTL,
            &extensionless,
            "application/octet-stream",
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        query(&signed)["response-content-disposition"],
        "attachment; filename=\"README\""
    );

    let raw_utf8 = HeaderValue::from_bytes("attachment; filename=\"résumé\"".as_bytes()).unwrap();
    for (value, content_type) in [
        (raw_utf8, CONTENT_TYPE),
        (HeaderValue::from_static(""), CONTENT_TYPE),
        (disposition(), ""),
        (disposition(), "text/plain\r\nx-injected: 1"),
        (disposition(), "text/plain; name=\"é\""),
    ] {
        rejected(
            s3.sign_get(&key, MAX_GET_URL_TTL, &value, content_type, NOW)
                .await,
            Operation::GetObject,
        );
    }
}

#[tokio::test]
async fn unit_presign_put_issues_no_capability() {
    let s3 = split_provider("minio", "true");
    let grant = GrantContext::authorized_for_test();
    for ttl in [Duration::from_secs(60), MAX_PART_URL_TTL] {
        rejected(
            s3.presign_put(&ObjectKey::allocate(KeyNamespace::Objects), ttl, &grant)
                .await,
            Operation::PutObject,
        );
    }
    let presign = include_str!("presign.rs");
    assert!(!presign.contains(".put_object()"));
    assert!(!presign.contains("Method::PUT"));
}

#[tokio::test]
async fn unit_presigned_urls_and_upload_ids_are_redacted() {
    let s3 = split_provider("minio", "true");
    let handle = handle();
    let signed = s3
        .sign_upload_parts(&handle, &[part(1)], MAX_PART_URL_TTL, NOW)
        .await
        .unwrap();
    let get = sign_get(&s3, MAX_GET_URL_TTL).await.unwrap();
    for rendered in [
        format!("{:?}", signed[0]),
        format!("{signed:?}"),
        format!("{get:?}"),
        format!("{handle:?}"),
        format!(
            "{:?}",
            super::multipart::encode_cursor("objects/aa/bb/x", Some(UPLOAD_ID))
        ),
        format!("{s3:?}"),
    ] {
        for secret in [
            UPLOAD_ID,
            "X-Amz-Signature",
            "X-Amz-Credential",
            "palmr-presign-secret-sentinel",
            "AKIA-palmr-presign-test",
        ] {
            assert!(!rendered.contains(secret), "{rendered} leaks {secret}");
        }
    }

    for (name, source) in PRESIGN_SOURCES {
        for forbidden in ["tracing::", "println!", "eprintln!", "dbg!", "log::"] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}

#[test]
fn unit_presign_and_multipart_never_buffer_whole_objects() {
    for (name, source) in PRESIGN_SOURCES {
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
            "SdkBody::from(",
            "todo!",
            "unimplemented!",
            "panic!",
            "unwrap()",
            "expect(",
            concat!("SystemTime::", "now"),
            concat!("OffsetDateTime::", "now_utc"),
            "plan_parts",
            "impl StorageProvider for S3Provider",
        ] {
            assert!(!source.contains(forbidden), "{name} contains {forbidden}");
        }
    }
}

const HOSTILE_ENDPOINTS: [&str; 7] = [
    "https://s3.amazonaws.com",
    "https://s3.us-east-1.amazonaws.com",
    "https://evil-amazonaws.com.attacker.net",
    "https://storage.googleapis.com",
    "https://account.r2.cloudflarestorage.com",
    "https://palmr.minio.lan:9443",
    "http://nas.local:9000",
];

async fn assert_hostile_hostnames_follow_configuration() {
    for endpoint in HOSTILE_ENDPOINTS {
        let base = Url::parse(endpoint).unwrap();
        let host = base.host_str().unwrap();
        for force_path_style in [true, false] {
            let s3 = env_provider(&[
                ("PALMR_S3_ENDPOINT", endpoint),
                ("PALMR_S3_BUCKET", "palmr-media"),
                ("PALMR_S3_PROFILE", "generic"),
                (
                    "PALMR_S3_FORCE_PATH_STYLE",
                    if force_path_style { "true" } else { "false" },
                ),
            ]);
            let handle = handle();
            let key = handle.key().as_str().to_owned();
            let part = s3
                .sign_upload_parts(&handle, &[part(1)], MAX_PART_URL_TTL, NOW)
                .await
                .unwrap()
                .remove(0);
            let get = s3
                .sign_get(
                    handle.key(),
                    MAX_GET_URL_TTL,
                    &disposition(),
                    CONTENT_TYPE,
                    NOW,
                )
                .await
                .unwrap();
            for request in [&part, &get] {
                let url = request.url();
                assert_eq!(url.scheme(), base.scheme(), "{endpoint}");
                assert_eq!(url.port_or_known_default(), base.port_or_known_default());
                if force_path_style {
                    assert_eq!(url.host_str(), Some(host), "{endpoint}");
                    assert_eq!(url.path(), format!("/palmr-media/{key}"), "{endpoint}");
                } else {
                    assert_eq!(
                        url.host_str(),
                        Some(format!("palmr-media.{host}").as_str()),
                        "{endpoint}"
                    );
                    assert_eq!(url.path(), format!("/{key}"), "{endpoint}");
                }
                assert_browser_contract(request);
            }
        }
    }
}

fn minio_provider(server: &MinioServer, bucket: &str, profile: &str, style: &str) -> S3Provider {
    env_provider(&[
        ("PALMR_S3_ENDPOINT", server.endpoint()),
        ("PALMR_S3_BUCKET", bucket),
        ("PALMR_S3_ACCESS_KEY", server.access_key()),
        ("PALMR_S3_SECRET_KEY", server.secret_key()),
        ("PALMR_S3_PROFILE", profile),
        ("PALMR_S3_FORCE_PATH_STYLE", style),
    ])
}

pub(super) fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + 7) % 251).unwrap())
        .collect()
}

pub(super) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

async fn browser_put(request: &PresignedRequest, body: Vec<u8>) -> reqwest::Response {
    assert_eq!(request.method(), &Method::PUT);
    reqwest::Client::new()
        .put(request.url().as_str())
        .body(body)
        .send()
        .await
        .unwrap()
}

async fn upload_through_browser(
    s3: &S3Provider,
    parts: &[Vec<u8>],
) -> (MultipartHandle, Vec<UploadedPart>) {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = s3
        .create_multipart(
            &key,
            PutHint {
                declared_len: Some(parts.iter().map(|part| part.len() as u64).sum()),
                content_type: Some("application/octet-stream".to_owned()),
            },
        )
        .await
        .unwrap();
    let plan: Vec<PartPlanEntry> = parts
        .iter()
        .zip(1_u32..)
        .map(|(bytes, part_number)| PartPlanEntry {
            part_number,
            len: bytes.len() as u64,
        })
        .collect();
    let signed = s3
        .sign_part_urls(&handle, &plan, MAX_PART_URL_TTL)
        .await
        .unwrap();
    let mut uploaded = Vec::new();
    for ((request, bytes), part_number) in signed.iter().zip(parts).zip(1_u32..) {
        assert_browser_contract(request);
        let response = browser_put(request, bytes.clone()).await;
        assert!(response.status().is_success(), "{}", response.status());
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        uploaded.push(UploadedPart {
            part_number,
            etag: crate::storage::provider::ETag::new(etag),
            size: bytes.len() as u64,
        });
    }
    (handle, uploaded)
}

#[tokio::test(flavor = "multi_thread")]
async fn regression_356_presign_checksum_header_rejection() {
    let s3 = split_provider("minio", "true");
    let signed = s3
        .sign_upload_parts(&handle(), &[part(1), part(2)], MAX_PART_URL_TTL, NOW)
        .await
        .unwrap();
    for request in &signed {
        assert_browser_contract(request);
    }

    let server = MinioServer::start().await.unwrap();
    let bucket = server.create_bucket("checksum").await.unwrap();
    let s3 = minio_provider(&server, &bucket, "minio", "true");
    let first = payload(5 * 1024 * 1024);
    let second = payload(4_099);
    let (handle, uploaded) = upload_through_browser(&s3, &[first.clone(), second.clone()]).await;

    let listed = s3.list_parts(&handle).await.unwrap();
    assert_eq!(listed, uploaded);
    let stat = s3.complete_multipart(&handle, &uploaded).await.unwrap();
    assert_eq!(stat.size, (first.len() + second.len()) as u64);

    let mut whole = first;
    whole.extend_from_slice(&second);
    let (_, body) = s3.get_object(handle.key()).await.unwrap();
    assert_eq!(
        super::multipart_tests::drain(body).await,
        (stat.size, sha256(&whole))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn regression_382_s3_addressing_style_presign() {
    assert_hostile_hostnames_follow_configuration().await;

    let server = MinioServer::start().await.unwrap();
    let endpoint = Url::parse(server.endpoint()).unwrap();
    let domain = endpoint.host_str().unwrap().to_owned();
    for (style, force_path_style) in [("true", true), ("false", false)] {
        let bucket = server
            .create_bucket(if force_path_style { "path" } else { "vhost" })
            .await
            .unwrap();
        let s3 = minio_provider(&server, &bucket, "minio", style);
        let bytes = payload(6 * 1024 * 1024 + 11);
        let (first, second) = bytes.split_at(5 * 1024 * 1024);
        let (handle, uploaded) =
            upload_through_browser(&s3, &[first.to_vec(), second.to_vec()]).await;
        let signed_part = s3
            .sign_part_urls(&handle, &[part(1)], MAX_PART_URL_TTL)
            .await
            .unwrap()
            .remove(0);
        let stat = s3.complete_multipart(&handle, &uploaded).await.unwrap();
        assert_eq!(stat.size, bytes.len() as u64);
        assert_eq!(s3.head_object(handle.key()).await.unwrap(), stat);

        let grant = GrantContext::authorized_for_test();
        let get = s3
            .presign_get(
                handle.key(),
                MAX_GET_URL_TTL,
                &disposition(),
                CONTENT_TYPE,
                &grant,
            )
            .await
            .unwrap();
        for request in [&signed_part, &get] {
            let url = request.url();
            assert_eq!(url.port(), endpoint.port(), "{style}");
            if force_path_style {
                assert_eq!(url.host_str(), Some(domain.as_str()), "{style}");
                assert_eq!(url.path(), format!("/{bucket}/{}", handle.key().as_str()));
            } else {
                assert_eq!(
                    url.host_str(),
                    Some(format!("{bucket}.{domain}").as_str()),
                    "{style}"
                );
                assert_eq!(url.path(), format!("/{}", handle.key().as_str()));
            }
        }

        let response = reqwest::get(get.url().as_str()).await.unwrap();
        assert_eq!(response.status(), 200, "{style}");
        assert_eq!(response.headers()["content-disposition"], DISPOSITION);
        assert_eq!(response.headers()["content-type"], CONTENT_TYPE);
        let body = response.bytes().await.unwrap();
        assert_eq!(sha256(&body), sha256(&bytes), "{style}");

        let mut tampered = get.url().clone();
        let pairs: Vec<(String, String)> = tampered
            .query_pairs()
            .map(|(name, value)| {
                let value = if name == "response-content-type" {
                    "text/html".to_owned()
                } else {
                    value.into_owned()
                };
                (name.into_owned(), value)
            })
            .collect();
        tampered.query_pairs_mut().clear().extend_pairs(pairs);
        let response = reqwest::get(tampered.as_str()).await.unwrap();
        assert_eq!(response.status(), 403, "{style}: pinned headers are signed");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn regression_381_provider_checksum_rejection() {
    for profile in ProviderProfile::ALL {
        let limits = profile.limits();
        let s3 = split_provider(profile.as_str(), "true");
        let outcome = s3
            .sign_upload_parts(&handle(), &[part(1)], MAX_PART_URL_TTL, NOW)
            .await;
        let caps = StorageCapabilities {
            requires_checksum_headers: limits.requires_part_checksums,
            ..StorageCapabilities::S3_DEFAULT
        };
        if limits.requires_part_checksums {
            rejected(outcome, Operation::UploadPart);
            assert_eq!(caps.part_upload(), PartUpload::ServerProxied, "{profile:?}");
        } else {
            assert_browser_contract(&outcome.unwrap()[0]);
            assert_eq!(caps.part_upload(), PartUpload::BrowserDirect, "{profile:?}");
        }
    }
    assert!(ProviderProfile::R2.limits().requires_part_checksums);

    let presign = include_str!("presign.rs");
    assert!(presign.contains("if limits.requires_part_checksums {"));
    for inference in [
        "XAmzContentChecksumMismatch",
        "BadDigest",
        "InvalidArgument",
        "code()",
    ] {
        assert!(!presign.contains(inference), "{inference}");
        assert!(
            !include_str!("multipart.rs").contains(inference),
            "{inference}"
        );
    }

    let server = MinioServer::start().await.unwrap();
    let bucket = server.create_bucket("proxied").await.unwrap();
    let s3 = minio_provider(&server, &bucket, "r2", "true");
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let handle = s3.create_multipart(&key, PutHint::default()).await.unwrap();
    rejected(
        s3.sign_part_urls(&handle, &[part(1)], MAX_PART_URL_TTL)
            .await,
        Operation::UploadPart,
    );
    let bytes = payload(70_001);
    let uploaded = s3
        .upload_part_stream(
            &handle,
            1,
            bytes.len() as u64,
            Box::pin(std::io::Cursor::new(bytes.clone())),
        )
        .await
        .unwrap();
    let stat = s3.complete_multipart(&handle, &[uploaded]).await.unwrap();
    assert_eq!(stat.size, bytes.len() as u64);
}
