use std::fs::{self, File};
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use time::macros::datetime;
use tokio::io::AsyncReadExt as _;

use super::LocalProvider;
use crate::domain::clock::TestClock;
use crate::storage::error::StorageError;
use crate::storage::health::{
    CheckName, CheckStatus, Diagnosis, FailureClass, ProbeDepth, SelfTestResult,
};
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::provider::{ObjectBody, PutHint, StorageProvider};
use crate::storage::ProviderKind;

fn root(provider: &LocalProvider) -> PathBuf {
    provider._owned_root.as_ref().unwrap().path().to_path_buf()
}

fn probe_dir(root: &Path) -> PathBuf {
    root.join("storage").join("_palmr").join("probe")
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(read) => read
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return found;
    };
    for entry in read {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(tree(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

fn body(bytes: &'static [u8]) -> ObjectBody {
    Box::pin(Cursor::new(bytes))
}

#[tokio::test]
async fn it_local_full_self_test_passes() {
    let provider = LocalProvider::temporary()
        .with_clock(Arc::new(TestClock::new(datetime!(2026-09-25 12:00 UTC))));
    let root = root(&provider);

    let report = provider.self_test(ProbeDepth::Full).await;

    assert_eq!(report.result(), SelfTestResult::Passed, "{report:#?}");
    assert_eq!(report.provider, ProviderKind::Local);
    assert_eq!(report.ran_at, datetime!(2026-09-25 12:00 UTC));
    let names: Vec<(CheckName, CheckStatus)> = report
        .checks
        .iter()
        .map(|check| (check.name, check.status))
        .collect();
    assert_eq!(
        names,
        [
            (CheckName::Layout, CheckStatus::Passed),
            (CheckName::Write, CheckStatus::Passed),
            (CheckName::Stat, CheckStatus::Passed),
            (CheckName::Read, CheckStatus::Passed),
            (CheckName::Range, CheckStatus::Passed),
            (CheckName::Delete, CheckStatus::Passed),
            (CheckName::Absent, CheckStatus::Passed),
            (CheckName::Cleanup, CheckStatus::Passed),
        ]
    );
    assert!(report.facts.is_empty());
    assert!(entries(&probe_dir(&root)).is_empty());
    assert!(tree(&root.join("storage").join("objects")).is_empty());
    assert!(tree(&root.join("uploads")).is_empty());
    assert!(entries(&root.join("uploads")).is_empty());
    let mode = fs::metadata(probe_dir(&root)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o750);
}

#[tokio::test]
async fn it_local_light_self_test_is_core_only() {
    let provider = LocalProvider::temporary();
    let report = provider.self_test(ProbeDepth::Light).await;
    assert_eq!(report.result(), SelfTestResult::Passed);
    assert!(report.check(CheckName::Cleanup).is_none());
    assert!(report
        .checks
        .iter()
        .all(|check| check.name.scope() == crate::storage::health::CheckScope::Core));
}

#[tokio::test]
async fn it_local_stale_probe_cleanup_touches_only_probe_namespace() {
    let provider = LocalProvider::temporary();
    let root = root(&provider);
    let probes = probe_dir(&root);
    fs::create_dir_all(&probes).unwrap();
    let hour_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_758_000_000);
    let stale = "0192f3c8d7e97a1b8f0c2d5e6a7b8c9d";
    let unknown = [
        "notes.txt",
        "0192F3C8D7E97A1B8F0C2D5E6A7B8C9E",
        "0192f3c8d7e94a1b8f0c2d5e6a7b8c9f",
        ".tmp-0192f3c8d7e97a1b8f0c2d5e6a7b8c9a",
    ];
    for name in std::iter::once(stale).chain(unknown) {
        let file = File::create(probes.join(name)).unwrap();
        file.set_modified(hour_ago).unwrap();
    }
    fs::create_dir(probes.join("0192f3c8d7e97a1b8f0c2d5e6a7b8c9b")).unwrap();
    let user = ObjectKey::allocate(KeyNamespace::Objects);
    let user_path = root.join("storage").join(user.as_str());
    fs::create_dir_all(user_path.parent().unwrap()).unwrap();
    File::create(&user_path)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();

    let report = provider.self_test(ProbeDepth::Full).await;

    assert_eq!(
        report.check(CheckName::Cleanup).unwrap().status,
        CheckStatus::Passed
    );
    let mut expected: Vec<String> = unknown.iter().map(|name| (*name).to_owned()).collect();
    expected.push("0192f3c8d7e97a1b8f0c2d5e6a7b8c9b".to_owned());
    expected.sort();
    assert_eq!(entries(&probes), expected);
    assert!(user_path.exists());
}

#[tokio::test]
async fn it_local_unwritable_storage_is_a_non_transient_failure() {
    if is_root() {
        return;
    }
    let provider = LocalProvider::temporary();
    let root = root(&provider);
    let uploads = root.join("uploads");
    fs::set_permissions(&uploads, fs::Permissions::from_mode(0o550)).unwrap();

    let report = provider.self_test(ProbeDepth::Light).await;

    fs::set_permissions(&uploads, fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(report.result(), SelfTestResult::Failed);
    let diagnosis = report.core_failure().unwrap();
    assert_eq!(diagnosis, Diagnosis::PermissionDenied);
    assert_eq!(diagnosis.class(), FailureClass::NonTransient);
    for skipped in [
        CheckName::Write,
        CheckName::Stat,
        CheckName::Read,
        CheckName::Range,
        CheckName::Delete,
        CheckName::Absent,
    ] {
        assert_eq!(
            report.check(skipped).unwrap().status,
            CheckStatus::Skipped,
            "{skipped:?}"
        );
    }
}

#[tokio::test]
async fn it_local_put_stream_is_staged_and_finalized() {
    let provider: Arc<dyn StorageProvider> = Arc::new(LocalProvider::temporary());
    assert_eq!(provider.describe().provider, ProviderKind::Local);
    assert!(provider.as_multipart().is_none() && provider.as_presign().is_none());

    let key = ObjectKey::allocate(KeyNamespace::Objects);
    let stat = provider
        .put_stream(
            &key,
            body(b"palmr-local-put"),
            PutHint {
                declared_len: Some(15),
                content_type: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(stat.size, 15);
    let (_, read) = provider.open_read(&key).await.unwrap();
    let mut text = String::new();
    read.take(64).read_to_string(&mut text).await.unwrap();
    assert_eq!(text, "palmr-local-put");
    assert!(provider.exists(&key).await.unwrap());

    let short = ObjectKey::allocate(KeyNamespace::Objects);
    let error = provider
        .put_stream(
            &short,
            body(b"four"),
            PutHint {
                declared_len: Some(5),
                content_type: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        StorageError::SizeMismatch {
            expected: 5,
            actual: 4
        }
    ));
    assert!(!provider.exists(&short).await.unwrap());

    let long = ObjectKey::allocate(KeyNamespace::Objects);
    assert!(matches!(
        provider
            .put_stream(
                &long,
                body(b"sixsix"),
                PutHint {
                    declared_len: Some(3),
                    content_type: None,
                },
            )
            .await
            .unwrap_err(),
        StorageError::SizeMismatch { .. }
    ));
    assert!(!provider.exists(&long).await.unwrap());
    assert!(provider.delete(&key).await.unwrap());
}

#[tokio::test]
async fn it_local_put_stream_leaves_no_staging() {
    let provider = LocalProvider::temporary();
    let root = root(&provider);
    let provider: Arc<dyn StorageProvider> = Arc::new(provider);
    let key = ObjectKey::allocate(KeyNamespace::Branding(
        crate::storage::key::BrandingKind::Logo,
    ));
    provider
        .put_stream(&key, body(b"logo"), PutHint::default())
        .await
        .unwrap();
    assert!(entries(&root.join("uploads")).is_empty());
    assert!(root
        .join("branding")
        .join(key.as_str().trim_start_matches("branding/"))
        .exists());
}
