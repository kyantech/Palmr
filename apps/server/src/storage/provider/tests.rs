use std::fs::File;
use std::io::{Cursor, Read};
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::{HeaderMap, HeaderValue, Method};
use time::macros::datetime;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt;
use url::Url;

use super::{
    CapacityReport, ETag, GrantContext, ListCursor, ListEntry, ListPage, LocalStorageDescriptor,
    MultipartHandle, MultipartStorage, MultipartUploadPage, ObjectBody, ObjectStat, PartPlanEntry,
    PendingMultipart, PresignStorage, PresignedRequest, PutHint, StorageDescriptor,
    StorageProvider, UploadedPart, MAX_LIST_PAGE_SIZE,
};
use crate::config::{EnvironmentSource, OperatorConfig, StorageConfig};
use crate::storage::caps::{
    CopyStrategy, DownloadDataPlane, PartUpload, StorageCapabilities, UploadDataPlane,
};
use crate::storage::error::StorageError;
use crate::storage::health::SelfTestReport;
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::ProviderKind;

const CONTENT: &[u8] = b"palmr-storage-contract-body";
const UPLOAD_ID: &str = "upload-id-sentinel-7c1f";
const SIGNATURE: &str = "X-Amz-Signature=signature-sentinel";

const BRANCH_FIELDS: [&str; 4] = [
    "supports_multipart",
    "requires_checksum_headers",
    "supports_presigned_get",
    "supports_server_side_copy",
];

const PROVIDER_BRANCH_TOKENS: [&str; 5] = [
    "ProviderKind",
    "StorageConfig::",
    "PALMR_STORAGE_PROVIDER",
    "supports_presigned_put",
    "S3_DEFAULT",
];

const PROVIDER_CONFIG_READERS: [(&str, &str); 1] = [(
    "src/infra/http/headers.rs",
    "CSP connect-src names the configured S3 public origin",
)];

const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;

fn at() -> OffsetDateTime {
    datetime!(2026-01-01 00:00 UTC)
}

fn stat(size: u64, etag: Option<ETag>) -> ObjectStat {
    ObjectStat {
        size,
        modified_at: at(),
        etag,
    }
}

fn body(bytes: &'static [u8]) -> ObjectBody {
    Box::pin(Cursor::new(bytes))
}

async fn measure(mut body: ObjectBody) -> Result<u64, StorageError> {
    let mut buffer = [0_u8; 8];
    let mut total = 0_u64;
    loop {
        let read = body.read(&mut buffer).await?;
        if read == 0 {
            return Ok(total);
        }
        total += read as u64;
    }
}

fn presigned(method: Method, key: &ObjectKey, ttl: Duration) -> PresignedRequest {
    let mut url = Url::parse("https://s3.example.test/palmr/").unwrap();
    url.set_path(&format!("/palmr/{}", key.as_str()));
    url.set_query(Some(SIGNATURE));
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::HOST,
        HeaderValue::from_static("s3.example.test"),
    );
    PresignedRequest::new(
        method,
        url,
        headers,
        at() + time::Duration::try_from(ttl).unwrap(),
    )
}

struct S3Shaped {
    caps: StorageCapabilities,
}

#[async_trait]
impl StorageProvider for S3Shaped {
    fn caps(&self) -> &StorageCapabilities {
        &self.caps
    }

    fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: ProviderKind::S3,
            local: None,
        }
    }

    async fn put_stream(
        &self,
        _key: &ObjectKey,
        body: ObjectBody,
        _hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        Ok(stat(measure(body).await?, Some(ETag::new("\"object\""))))
    }

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        Ok((self.stat(key).await?, body(CONTENT)))
    }

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        let stat = self.stat(key).await?;
        if start >= stat.size {
            return Err(StorageError::RangeNotSatisfiable { size: stat.size });
        }
        let start = usize::try_from(start).unwrap();
        let end = usize::try_from(len)
            .unwrap()
            .saturating_add(start)
            .min(CONTENT.len());
        Ok((stat, body(&CONTENT[start..end])))
    }

    async fn stat(&self, _key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        Ok(stat(CONTENT.len() as u64, None))
    }

    async fn delete(&self, _key: &ObjectKey) -> Result<bool, StorageError> {
        Ok(true)
    }

    async fn exists(&self, _key: &ObjectKey) -> Result<bool, StorageError> {
        Ok(true)
    }

    async fn copy(&self, _src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.stat(dst).await
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        let entries = vec![ListEntry {
            key: key.as_str().to_owned(),
            size: CONTENT.len() as u64,
            modified_at: at(),
        }];
        let next = cursor
            .is_none()
            .then(|| ListCursor::new(format!("{prefix}{}", page_size.min(MAX_LIST_PAGE_SIZE))));
        Ok(ListPage { entries, next })
    }

    async fn self_test(&self) -> Result<SelfTestReport, StorageError> {
        Ok(SelfTestReport { passed: true })
    }

    fn as_multipart(&self) -> Option<&dyn MultipartStorage> {
        Some(self)
    }

    fn as_presign(&self) -> Option<&dyn PresignStorage> {
        Some(self)
    }
}

#[async_trait]
impl MultipartStorage for S3Shaped {
    async fn create_multipart(
        &self,
        key: &ObjectKey,
        _hint: PutHint,
    ) -> Result<MultipartHandle, StorageError> {
        Ok(MultipartHandle::new(key.clone(), UPLOAD_ID))
    }

    async fn sign_part_urls(
        &self,
        handle: &MultipartHandle,
        parts: &[PartPlanEntry],
        ttl: Duration,
    ) -> Result<Vec<PresignedRequest>, StorageError> {
        Ok(parts
            .iter()
            .map(|_| presigned(Method::PUT, handle.key(), ttl))
            .collect())
    }

    async fn list_parts(
        &self,
        _handle: &MultipartHandle,
    ) -> Result<Vec<UploadedPart>, StorageError> {
        Ok(Vec::new())
    }

    async fn upload_part_stream(
        &self,
        _handle: &MultipartHandle,
        part_number: u32,
        _len: u64,
        body: ObjectBody,
    ) -> Result<UploadedPart, StorageError> {
        Ok(UploadedPart {
            part_number,
            etag: ETag::new(format!("\"part-{part_number}\"")),
            size: measure(body).await?,
        })
    }

    async fn upload_part_copy(
        &self,
        _handle: &MultipartHandle,
        part_number: u32,
        _src: &ObjectKey,
        range: RangeInclusive<u64>,
    ) -> Result<UploadedPart, StorageError> {
        Ok(UploadedPart {
            part_number,
            etag: ETag::new("\"copied\""),
            size: range.end() - range.start() + 1,
        })
    }

    async fn complete_multipart(
        &self,
        _handle: &MultipartHandle,
        parts: &[UploadedPart],
    ) -> Result<ObjectStat, StorageError> {
        Ok(stat(
            parts.iter().map(|part| part.size).sum(),
            Some(ETag::new("\"multipart-2\"")),
        ))
    }

    async fn abort_multipart(&self, _handle: &MultipartHandle) -> Result<(), StorageError> {
        Ok(())
    }

    async fn list_multipart_uploads(
        &self,
        _prefix: &str,
        cursor: Option<ListCursor>,
    ) -> Result<MultipartUploadPage, StorageError> {
        let handle = MultipartHandle::new(ObjectKey::allocate(KeyNamespace::Objects), UPLOAD_ID);
        Ok(MultipartUploadPage {
            uploads: vec![PendingMultipart {
                handle,
                initiated_at: at(),
            }],
            next: cursor,
        })
    }
}

#[async_trait]
impl PresignStorage for S3Shaped {
    async fn presign_get(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        _disposition: &HeaderValue,
        _content_type: &str,
        _authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError> {
        Ok(presigned(Method::GET, key, ttl))
    }

    async fn presign_put(
        &self,
        key: &ObjectKey,
        ttl: Duration,
        _authorized: &GrantContext,
    ) -> Result<PresignedRequest, StorageError> {
        Ok(presigned(Method::PUT, key, ttl))
    }
}

struct LocalShaped(S3Shaped);

#[async_trait]
impl StorageProvider for LocalShaped {
    fn caps(&self) -> &StorageCapabilities {
        self.0.caps()
    }

    fn describe(&self) -> StorageDescriptor {
        StorageDescriptor {
            provider: ProviderKind::Local,
            local: Some(LocalStorageDescriptor {
                capacity: CapacityReport::Unavailable,
            }),
        }
    }

    async fn put_stream(
        &self,
        key: &ObjectKey,
        body: ObjectBody,
        hint: PutHint,
    ) -> Result<ObjectStat, StorageError> {
        let stat = self.0.put_stream(key, body, hint).await?;
        Ok(ObjectStat { etag: None, ..stat })
    }

    async fn open_read(&self, key: &ObjectKey) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.0.open_read(key).await
    }

    async fn open_range(
        &self,
        key: &ObjectKey,
        start: u64,
        len: u64,
    ) -> Result<(ObjectStat, ObjectBody), StorageError> {
        self.0.open_range(key, start, len).await
    }

    async fn stat(&self, key: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.0.stat(key).await
    }

    async fn delete(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.0.delete(key).await
    }

    async fn exists(&self, key: &ObjectKey) -> Result<bool, StorageError> {
        self.0.exists(key).await
    }

    async fn copy(&self, src: &ObjectKey, dst: &ObjectKey) -> Result<ObjectStat, StorageError> {
        self.0.copy(src, dst).await
    }

    async fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        self.0.list_page(prefix, cursor, page_size).await
    }

    async fn self_test(&self) -> Result<SelfTestReport, StorageError> {
        self.0.self_test().await
    }
}

fn local_shaped() -> Arc<dyn StorageProvider> {
    Arc::new(LocalShaped(S3Shaped {
        caps: StorageCapabilities::LOCAL,
    }))
}

fn s3_shaped(caps: StorageCapabilities) -> Arc<dyn StorageProvider> {
    Arc::new(S3Shaped { caps })
}

type BranchRow = (UploadDataPlane, PartUpload, DownloadDataPlane, CopyStrategy);

fn branches(provider: &dyn StorageProvider) -> BranchRow {
    let caps = provider.caps();
    (
        caps.upload_data_plane(),
        caps.part_upload(),
        caps.download_data_plane(),
        caps.copy_strategy(),
    )
}

fn assert_capability_paths_match_caps(provider: &dyn StorageProvider) {
    let caps = provider.caps();
    assert_eq!(
        provider.as_multipart().is_some(),
        caps.upload_data_plane() == UploadDataPlane::Multipart
    );
    assert_eq!(
        provider.as_presign().is_some(),
        caps.download_data_plane() == DownloadDataPlane::PresignedRedirect
    );
}

fn production_part(source: &str) -> &str {
    source
        .split("#[cfg(test)]\nmod ")
        .next()
        .unwrap_or_default()
}

fn read_bounded(path: &Path) -> String {
    std::io::read_to_string(File::open(path).unwrap().take(MAX_SOURCE_BYTES)).unwrap()
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("tests.rs"))
        || path.components().any(|part| part.as_os_str() == "tests")
}

fn collect_sources(dir: &Path, sources: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_sources(&path, sources);
        } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_file(&path) {
            let relative = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap()
                .display()
                .to_string();
            sources.push((relative, read_bounded(&path)));
        }
    }
}

fn provider_branch_violations(sources: &[(String, String)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, source) in sources {
        if path.starts_with("src/storage/") || path.starts_with("src/config/") {
            continue;
        }
        let reads_config = PROVIDER_CONFIG_READERS
            .iter()
            .any(|(reader, _)| reader == path);
        for (number, line) in production_part(source).lines().enumerate() {
            for token in BRANCH_FIELDS.iter().chain(&PROVIDER_BRANCH_TOKENS) {
                if line.contains(token) && !(reads_config && *token == "StorageConfig::") {
                    violations.push(format!("{path}:{} names {token}", number + 1));
                }
            }
        }
    }
    violations
}

fn storage_config(vars: &[(&str, &str)]) -> StorageConfig {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
        .unwrap()
        .config
        .storage
}

#[tokio::test]
async fn unit_caps_branch_points() {
    let local = local_shaped();
    let s3 = s3_shaped(StorageCapabilities::S3_DEFAULT);
    let degraded = s3_shaped(StorageCapabilities {
        requires_checksum_headers: true,
        ..StorageCapabilities::S3_DEFAULT
    });
    let no_server_copy = StorageCapabilities {
        supports_server_side_copy: false,
        ..StorageCapabilities::LOCAL
    };

    assert!(local.as_multipart().is_none());
    assert!(local.as_presign().is_none());
    assert!(s3.as_multipart().is_some());
    assert!(s3.as_presign().is_some());
    for provider in [&local, &s3, &degraded] {
        assert_capability_paths_match_caps(provider.as_ref());
    }

    assert_eq!(
        branches(local.as_ref()),
        (
            UploadDataPlane::Tus,
            PartUpload::BrowserDirect,
            DownloadDataPlane::Streamed,
            CopyStrategy::ServerSide
        )
    );
    assert_eq!(
        branches(s3.as_ref()),
        (
            UploadDataPlane::Multipart,
            PartUpload::BrowserDirect,
            DownloadDataPlane::PresignedRedirect,
            CopyStrategy::ServerSide
        )
    );
    assert_eq!(
        branches(degraded.as_ref()),
        (
            UploadDataPlane::Multipart,
            PartUpload::ServerProxied,
            DownloadDataPlane::PresignedRedirect,
            CopyStrategy::ServerSide
        )
    );
    assert_eq!(no_server_copy.copy_strategy(), CopyStrategy::Unsupported);

    let presign_put_only = StorageCapabilities {
        supports_presigned_put: false,
        max_object_size: 1,
        max_part_size: 1,
        min_part_size: 1,
        max_parts: 1,
        ..StorageCapabilities::S3_DEFAULT
    };
    assert_eq!(
        (
            presign_put_only.upload_data_plane(),
            presign_put_only.part_upload(),
            presign_put_only.download_data_plane(),
            presign_put_only.copy_strategy()
        ),
        branches(s3.as_ref())
    );

    let caps_source = production_part(include_str!("../caps.rs"));
    let branch_fns: Vec<&str> = caps_source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub const fn "))
        .map(|rest| rest.split('(').next().unwrap_or_default())
        .collect();
    assert_eq!(
        branch_fns,
        [
            "upload_data_plane",
            "part_upload",
            "download_data_plane",
            "copy_strategy"
        ]
    );
    let branch_enums: Vec<&str> = caps_source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub enum "))
        .map(|rest| rest.trim_end_matches(" {"))
        .collect();
    assert_eq!(
        branch_enums,
        [
            "UploadDataPlane",
            "PartUpload",
            "DownloadDataPlane",
            "CopyStrategy"
        ]
    );
    for field in BRANCH_FIELDS {
        assert_eq!(
            caps_source.matches(&format!("if self.{field} {{")).count(),
            1,
            "{field}"
        );
    }
    assert!(!caps_source.contains("if self.supports_presigned_put"));

    let mut sources = Vec::new();
    collect_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(sources.iter().any(|(path, _)| path == "src/app/router.rs"));
    assert!(sources
        .iter()
        .any(|(path, _)| path.starts_with("src/features/")));
    assert_eq!(provider_branch_violations(&sources), Vec::<String>::new());
    let planted = [(
        "src/features/files/mod.rs".to_owned(),
        "fn plane(p: &dyn StorageProvider) -> bool { p.caps().supports_multipart }\nmatch kind { ProviderKind::S3 => {} }\n#[cfg(test)]\nmod tests { const X: &str = \"PALMR_STORAGE_PROVIDER\"; }\n".to_owned(),
    )];
    assert_eq!(
        provider_branch_violations(&planted),
        [
            "src/features/files/mod.rs:1 names supports_multipart",
            "src/features/files/mod.rs:2 names ProviderKind",
        ]
    );

    assert_eq!(
        crate::storage::configured_provider(&storage_config(&[])),
        ProviderKind::Local
    );
    assert_eq!(
        crate::storage::configured_provider(&storage_config(&[
            ("PALMR_STORAGE_PROVIDER", "s3"),
            ("PALMR_S3_ENDPOINT", "http://minio:9000"),
            ("PALMR_S3_REGION", "us-east-1"),
            ("PALMR_S3_BUCKET", "palmr"),
            ("PALMR_S3_ACCESS_KEY", "access"),
            ("PALMR_S3_SECRET_KEY", "secret"),
        ])),
        ProviderKind::S3
    );
    for kind in [ProviderKind::Local, ProviderKind::S3] {
        let name = match kind {
            ProviderKind::Local | ProviderKind::S3 => kind.as_str(),
        };
        assert_eq!(name, kind.to_string());
    }
    assert_eq!(local.describe().provider, ProviderKind::Local);
    assert_eq!(s3.describe().provider, ProviderKind::S3);

    let provider_source = production_part(include_str!("../provider.rs"));
    for whole_object in [
        "Vec<u8>",
        "&[u8]",
        "Bytes",
        "read_to_end",
        "get_bytes",
        "put_bytes",
    ] {
        assert!(!provider_source.contains(whole_object), "{whole_object}");
    }

    exercise_core_contract(local.as_ref()).await;
    exercise_core_contract(s3.as_ref()).await;
    exercise_capability_contract(s3.as_ref()).await;
}

async fn exercise_core_contract(provider: &dyn StorageProvider) {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let hint = PutHint {
        declared_len: Some(1),
        content_type: Some("application/octet-stream".to_owned()),
    };
    assert_ne!(hint, PutHint::default());
    let written = provider
        .put_stream(&key, body(CONTENT), hint)
        .await
        .unwrap();
    assert_eq!(written.size, CONTENT.len() as u64);
    assert_eq!(written.modified_at, at());
    match provider.describe().provider {
        ProviderKind::Local => assert!(written.etag.is_none()),
        ProviderKind::S3 => assert_eq!(written.etag.unwrap().as_str(), "\"object\""),
    }

    let (whole, reader) = provider.open_read(&key).await.unwrap();
    assert_eq!(measure(reader).await.unwrap(), whole.size);
    let (_, mut ranged) = provider.open_range(&key, 6, 7).await.unwrap();
    let mut slice = [0_u8; 7];
    ranged.read_exact(&mut slice).await.unwrap();
    assert_eq!(&slice, b"storage");
    assert!(matches!(
        provider
            .open_range(&key, CONTENT.len() as u64, 1)
            .await
            .map(|_| ()),
        Err(StorageError::RangeNotSatisfiable { size }) if size == CONTENT.len() as u64
    ));

    let copy = ObjectKey::allocate(KeyNamespace::Objects);
    assert_eq!(provider.copy(&key, &copy).await.unwrap().size, whole.size);
    assert!(provider.exists(&copy).await.unwrap());
    assert!(provider.delete(&copy).await.unwrap());

    let page = provider
        .list_page("objects/", None, u32::MAX)
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    let entry = &page.entries[0];
    assert!(ObjectKey::parse(&entry.key).is_ok());
    assert_eq!((entry.size, entry.modified_at), (whole.size, at()));
    let cursor = page.next.unwrap();
    assert_eq!(cursor.as_str(), format!("objects/{MAX_LIST_PAGE_SIZE}"));
    let last = provider
        .list_page("objects/", Some(cursor), 10)
        .await
        .unwrap();
    assert!(last.next.is_none());

    assert!(provider.self_test().await.unwrap().passed);
}

async fn exercise_capability_contract(provider: &dyn StorageProvider) {
    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let multipart = provider.as_multipart().unwrap();
    let handle = multipart
        .create_multipart(&key, PutHint::default())
        .await
        .unwrap();
    assert_eq!(handle.key(), &key);
    assert_eq!(handle.upload_id(), UPLOAD_ID);
    assert!(!format!("{handle:?}").contains(UPLOAD_ID));

    let plan = [
        PartPlanEntry {
            part_number: 1,
            len: 8,
        },
        PartPlanEntry {
            part_number: 2,
            len: 4,
        },
    ];
    let signed = multipart
        .sign_part_urls(&handle, &plan, Duration::from_secs(900))
        .await
        .unwrap();
    assert_eq!(signed.len(), plan.len());
    for request in &signed {
        assert_eq!(request.method(), Method::PUT);
        assert!(request.url().as_str().contains(SIGNATURE));
        assert!(request.headers().contains_key(http::header::HOST));
        assert_eq!(request.expires_at(), at() + time::Duration::minutes(15));
        let debug = format!("{request:?}");
        assert!(!debug.contains(SIGNATURE), "{debug}");
        assert!(!debug.contains(key.as_str()), "{debug}");
    }

    assert!(multipart.list_parts(&handle).await.unwrap().is_empty());
    let streamed = multipart
        .upload_part_stream(&handle, 1, 8, body(&CONTENT[..8]))
        .await
        .unwrap();
    let copied = multipart
        .upload_part_copy(&handle, 2, &key, 8..=11)
        .await
        .unwrap();
    assert_eq!((streamed.part_number, streamed.size), (1, 8));
    assert_eq!(streamed.etag, ETag::new("\"part-1\""));
    assert_eq!((copied.part_number, copied.size), (2, 4));
    let completed = multipart
        .complete_multipart(&handle, &[streamed, copied])
        .await
        .unwrap();
    assert_eq!(completed.size, 12);
    multipart.abort_multipart(&handle).await.unwrap();
    let pending = multipart
        .list_multipart_uploads("objects/", None)
        .await
        .unwrap();
    assert_eq!(pending.uploads[0].initiated_at, at());
    assert_eq!(pending.uploads[0].handle.upload_id(), UPLOAD_ID);
    assert!(pending.next.is_none());

    let presign = provider.as_presign().unwrap();
    let grant = GrantContext::authorized_for_test();
    assert_eq!(format!("{grant:?}"), "GrantContext");
    let disposition = HeaderValue::from_static("attachment; filename=\"report.pdf\"");
    let get = presign
        .presign_get(
            &key,
            Duration::from_secs(300),
            &disposition,
            "application/pdf",
            &grant,
        )
        .await
        .unwrap();
    assert_eq!(get.method(), Method::GET);
    let put = presign
        .presign_put(&key, Duration::from_secs(300), &grant)
        .await
        .unwrap();
    assert_eq!(put.method(), Method::PUT);
}
