use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Cursor, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

use proptest::prelude::*;
use rustix::io::Errno;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

use super::paths::{ObjectLocation, Root};
use super::{
    is_low_space, CloneStep, CopyRangeStep, FinalizeRoute, Finalized, FsOps, LocalProvider,
    StagingWriter, Step, SystemOps, UploadId, DIRECTORY_MODE, FILE_MODE,
};
use crate::storage::error::StorageError;
use crate::storage::key::{BrandingKind, KeyNamespace, ObjectKey};
use crate::storage::provider::{Capacity, CapacityReport, ObjectBody};
use crate::storage::ProviderKind;

const COPY_BUFFER: u32 = 65_536;

const BUFFER: u32 = 262_144;
const SMALL_BUFFER: u32 = 4_096;
const DISPLAY_NAME_BASELINE: &str = "Quarterly report.pdf";

struct Fault {
    step: Step,
    skip: usize,
    errno: Errno,
}

#[derive(Debug, Clone, Copy)]
struct Written {
    len: usize,
    dev: u64,
    ino: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CloneMode {
    #[default]
    System,
    ForceUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CopyRangeMode {
    #[default]
    System,
    ForceUnsupported,
    Cap(usize),
}

type Observer = Box<dyn Fn(Step) + Send>;

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<(Step, Option<i32>)>>,
    fault: Mutex<Option<Fault>>,
    exdev: AtomicBool,
    writes: Mutex<Vec<Written>>,
    observer: Mutex<Option<Observer>>,
    statvfs_fault: Mutex<Option<Errno>>,
    statvfs_targets: Mutex<Vec<u64>>,
    clone_mode: Mutex<CloneMode>,
    clone_calls: AtomicUsize,
    copy_range_mode: Mutex<CopyRangeMode>,
    copy_range_calls: AtomicUsize,
    seeks: Mutex<Vec<u64>>,
}

impl Recorder {
    fn fail(&self, step: Step, skip: usize, errno: Errno) {
        *self.fault.lock().unwrap() = Some(Fault { step, skip, errno });
    }

    fn observe(&self, observer: impl Fn(Step) + Send + 'static) {
        *self.observer.lock().unwrap() = Some(Box::new(observer));
    }

    fn force_reflink_unsupported(&self) {
        *self.clone_mode.lock().unwrap() = CloneMode::ForceUnsupported;
    }

    fn force_copy_range_unsupported(&self) {
        *self.copy_range_mode.lock().unwrap() = CopyRangeMode::ForceUnsupported;
    }

    fn cap_copy_range(&self, cap: usize) {
        *self.copy_range_mode.lock().unwrap() = CopyRangeMode::Cap(cap);
    }

    fn fail_statvfs(&self, errno: Errno) {
        *self.statvfs_fault.lock().unwrap() = Some(errno);
    }

    fn statvfs_targets(&self) -> Vec<u64> {
        self.statvfs_targets.lock().unwrap().clone()
    }

    fn clone_calls(&self) -> usize {
        self.clone_calls.load(Ordering::SeqCst)
    }

    fn copy_range_calls(&self) -> usize {
        self.copy_range_calls.load(Ordering::SeqCst)
    }

    fn seeks(&self) -> Vec<u64> {
        self.seeks.lock().unwrap().clone()
    }

    fn steps(&self) -> Vec<Step> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(step, _)| *step)
            .collect()
    }

    fn compressed(&self) -> Vec<Step> {
        let mut steps = self.steps();
        steps.dedup_by(|next, previous| *next == Step::WriteTemp && *previous == Step::WriteTemp);
        steps
    }

    fn outcome(&self, step: Step) -> Option<Option<i32>> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find(|(recorded, _)| *recorded == step)
            .map(|(_, errno)| *errno)
    }

    fn writes(&self) -> Vec<Written> {
        self.writes.lock().unwrap().clone()
    }

    fn intercept(&self, step: Step) -> Option<io::Error> {
        if let Some(observer) = self.observer.lock().unwrap().as_ref() {
            observer(step);
        }
        let mut fault = self.fault.lock().unwrap();
        if let Some(armed) = fault.as_mut().filter(|armed| armed.step == step) {
            if armed.skip == 0 {
                let errno = armed.errno;
                *fault = None;
                return Some(io::Error::from(errno));
            }
            armed.skip -= 1;
        }
        (step == Step::RenameStaging && self.exdev.load(Ordering::SeqCst))
            .then(|| io::Error::from(Errno::XDEV))
    }

    fn run(&self, step: Step, real: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        let outcome = self.intercept(step).map_or_else(real, Err);
        self.events.lock().unwrap().push((
            step,
            outcome.as_ref().err().and_then(io::Error::raw_os_error),
        ));
        outcome
    }
}

struct Recording(Arc<Recorder>);

impl FsOps for Recording {
    fn fsync(&self, fd: BorrowedFd<'_>, step: Step) -> io::Result<()> {
        self.0.run(step, || SystemOps.fsync(fd, step))
    }

    fn rename(
        &self,
        from_dir: BorrowedFd<'_>,
        from: &str,
        to_dir: BorrowedFd<'_>,
        to: &str,
        step: Step,
    ) -> io::Result<()> {
        self.0
            .run(step, || SystemOps.rename(from_dir, from, to_dir, to, step))
    }

    fn write_all(&self, file: &File, chunk: &[u8], step: Step) -> io::Result<()> {
        self.0.run(step, || {
            let metadata = file.metadata()?;
            self.0.writes.lock().unwrap().push(Written {
                len: chunk.len(),
                dev: metadata.dev(),
                ino: metadata.ino(),
            });
            SystemOps.write_all(file, chunk, step)
        })
    }

    fn unlink(&self, dir: BorrowedFd<'_>, name: &str, step: Step) -> io::Result<()> {
        self.0.run(step, || SystemOps.unlink(dir, name, step))
    }

    fn remove_dir(&self, dir: BorrowedFd<'_>, name: &str, step: Step) -> io::Result<()> {
        self.0.run(step, || SystemOps.remove_dir(dir, name, step))
    }

    fn seek(&self, file: &File, pos: u64) -> io::Result<u64> {
        self.0.seeks.lock().unwrap().push(pos);
        SystemOps.seek(file, pos)
    }

    fn filesystem_stats(&self, dir: BorrowedFd<'_>) -> io::Result<Capacity> {
        let stat = rustix::fs::fstat(dir).map_err(io::Error::from)?;
        self.0.statvfs_targets.lock().unwrap().push(stat.st_ino);
        if let Some(errno) = *self.0.statvfs_fault.lock().unwrap() {
            return Err(io::Error::from(errno));
        }
        SystemOps.filesystem_stats(dir)
    }

    fn try_reflink(&self, dst: &File, src: &File) -> io::Result<CloneStep> {
        self.0.clone_calls.fetch_add(1, Ordering::SeqCst);
        match *self.0.clone_mode.lock().unwrap() {
            CloneMode::ForceUnsupported => Ok(CloneStep::Unsupported),
            CloneMode::System => SystemOps.try_reflink(dst, src),
        }
    }

    fn copy_range_step(
        &self,
        src: &File,
        src_offset: u64,
        dst: &File,
        dst_offset: u64,
        len: usize,
    ) -> io::Result<CopyRangeStep> {
        self.0.copy_range_calls.fetch_add(1, Ordering::SeqCst);
        match *self.0.copy_range_mode.lock().unwrap() {
            CopyRangeMode::ForceUnsupported => Ok(CopyRangeStep::Unsupported),
            CopyRangeMode::System => {
                SystemOps.copy_range_step(src, src_offset, dst, dst_offset, len)
            }
            CopyRangeMode::Cap(cap) => {
                let request = len.min(cap);
                if request == 0 {
                    return Ok(CopyRangeStep::Copied(0));
                }
                let mut buffer = vec![0_u8; request];
                let read = rustix::io::pread(src.as_fd(), &mut buffer[..], src_offset)
                    .map_err(io::Error::from)?;
                if read == 0 {
                    return Ok(CopyRangeStep::Copied(0));
                }
                let mut written = 0;
                while written < read {
                    let wrote = rustix::io::pwrite(
                        dst.as_fd(),
                        &buffer[written..read],
                        dst_offset + written as u64,
                    )
                    .map_err(io::Error::from)?;
                    if wrote == 0 {
                        return Err(io::Error::other("the copy range test seam stalled"));
                    }
                    written += wrote;
                }
                Ok(CopyRangeStep::Copied(read))
            }
        }
    }
}

struct Fixture {
    _dirs: Vec<TempDir>,
    data: PathBuf,
    provider: LocalProvider,
    recorder: Arc<Recorder>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_buffer(BUFFER)
    }

    fn with_buffer(buffer: u32) -> Self {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        make_layout(&data, None);
        Self::open(vec![dir], data, buffer)
    }

    fn open(dirs: Vec<TempDir>, data: PathBuf, buffer: u32) -> Self {
        let recorder = Arc::new(Recorder::default());
        let provider =
            LocalProvider::with_ops(&data, buffer, Box::new(Recording(Arc::clone(&recorder))))
                .unwrap();
        Self {
            _dirs: dirs,
            data,
            provider,
            recorder,
        }
    }

    fn final_path(&self, key: &ObjectKey) -> PathBuf {
        match key.namespace() {
            KeyNamespace::Objects => self.data.join("storage").join(key.as_str()),
            KeyNamespace::Branding(_) => self.data.join(key.as_str()),
        }
    }

    fn staging_dir(&self, id: &UploadId) -> PathBuf {
        self.data.join("uploads").join(id.as_str())
    }

    fn staging_blob(&self, id: &UploadId) -> PathBuf {
        self.staging_dir(id).join("blob")
    }

    fn temp_path(&self, key: &ObjectKey, id: &UploadId) -> PathBuf {
        self.final_path(key)
            .with_file_name(format!(".tmp-{}", id.as_str()))
    }

    fn copy_temp_path(&self, key: &ObjectKey) -> PathBuf {
        let oid = key.as_str().rsplit('/').next().unwrap();
        self.final_path(key).with_file_name(format!(".tmp-{oid}"))
    }

    fn stage(&self, bytes: &[u8]) -> UploadId {
        let id = upload_id();
        let mut writer = self.provider.create_staging(&id).unwrap();
        let copied = writer
            .append_from(&mut Cursor::new(bytes), u64::MAX)
            .unwrap();
        assert_eq!(copied, bytes.len() as u64);
        writer.sync().unwrap();
        id
    }
}

fn make_layout(data: &Path, uploads_target: Option<&Path>) {
    for relative in ["", "storage", "storage/objects", "branding"] {
        fs::create_dir_all(data.join(relative)).unwrap();
    }
    match uploads_target {
        Some(target) => symlink(target, data.join("uploads")).unwrap(),
        None => fs::create_dir(data.join("uploads")).unwrap(),
    }
}

fn upload_id() -> UploadId {
    UploadId::parse(&Uuid::now_v7().simple().to_string()).unwrap()
}

fn objects_key() -> ObjectKey {
    ObjectKey::allocate(KeyNamespace::Objects)
}

fn place(fixture: &Fixture, bytes: &[u8]) -> ObjectKey {
    let id = fixture.stage(bytes);
    let key = objects_key();
    fixture.provider.finalize_staged(&id, &key).unwrap();
    key
}

async fn read_all(body: ObjectBody) -> Vec<u8> {
    let mut reader = body;
    let mut collected = Vec::new();
    let mut buffer = vec![0_u8; 8_192];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut reader, &mut buffer)
            .await
            .unwrap();
        if read == 0 {
            return collected;
        }
        collected.extend_from_slice(&buffer[..read]);
    }
}

fn pattern(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn file_digest(path: &Path) -> Vec<u8> {
    let mut file = File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 65_536];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            return hasher.finalize().to_vec();
        }
        hasher.update(&buffer[..read]);
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path);
                entries.insert(relative, None);
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                entries.insert(relative, Some(target.to_string_lossy().as_bytes().to_vec()));
            } else {
                entries.insert(relative, Some(file_digest(&path)));
            }
        }
    }
    entries
}

fn assert_staging_intact(fixture: &Fixture, id: &UploadId, expected: &[u8]) {
    assert_eq!(file_digest(&fixture.staging_blob(id)), digest(expected));
}

fn cross_device_pair() -> (TempDir, TempDir) {
    let candidates = [
        PathBuf::from("/dev/shm"),
        std::env::temp_dir(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
    ];
    let mut dirs: Vec<TempDir> = candidates
        .iter()
        .filter_map(|candidate| {
            tempfile::Builder::new()
                .prefix("palmr-exdev-")
                .tempdir_in(candidate)
                .ok()
        })
        .collect();
    let devices: Vec<u64> = dirs
        .iter()
        .map(|dir| fs::metadata(dir.path()).unwrap().dev())
        .collect();
    for first in 0..devices.len() {
        for second in first + 1..devices.len() {
            if devices[first] != devices[second] {
                let uploads = dirs.remove(second);
                let data = dirs.remove(first);
                return (data, uploads);
            }
        }
    }
    panic!("regression_272_exdev_finalization needs two distinct filesystems among {candidates:?}");
}

fn cross_device_fixture(buffer: u32) -> Fixture {
    let (data_dir, uploads_dir) = cross_device_pair();
    let data = data_dir.path().join("data");
    make_layout(&data, Some(uploads_dir.path()));
    let fixture = Fixture::open(vec![data_dir, uploads_dir], data, buffer);
    let objects_dev = fs::metadata(fixture.data.join("storage/objects"))
        .unwrap()
        .dev();
    let uploads_dev = fs::metadata(fixture.data.join("uploads")).unwrap().dev();
    assert_ne!(objects_dev, uploads_dev);
    fixture
}

#[test]
fn regression_272_exdev_finalization() {
    let fixture = cross_device_fixture(BUFFER);
    let bytes = pattern(5 * 1024 * 1024 + 12_345, 272);
    let id = fixture.stage(&bytes);
    let key = objects_key();
    let final_path = fixture.final_path(&key);
    let staging_blob = fixture.staging_blob(&id);

    let observed_final = Arc::new(AtomicBool::new(false));
    let observed_missing_staging = Arc::new(AtomicBool::new(false));
    {
        let final_path = final_path.clone();
        let staging_blob = staging_blob.clone();
        let observed_final = Arc::clone(&observed_final);
        let observed_missing_staging = Arc::clone(&observed_missing_staging);
        fixture.recorder.observe(move |step| {
            if matches!(
                step,
                Step::WriteTemp | Step::SyncTemp | Step::RenameTemp | Step::SyncLeaf
            ) {
                if step != Step::SyncLeaf && exists(&final_path) {
                    observed_final.store(true, Ordering::SeqCst);
                }
                if !exists(&staging_blob) {
                    observed_missing_staging.store(true, Ordering::SeqCst);
                }
            }
        });
    }

    let finalized: Finalized = fixture.provider.finalize_staged(&id, &key).unwrap();

    assert_eq!(finalized.route, FinalizeRoute::CrossDeviceCopy);
    assert_eq!(
        fixture.recorder.outcome(Step::RenameStaging),
        Some(Some(Errno::XDEV.raw_os_error()))
    );
    assert_eq!(finalized.stat.size, bytes.len() as u64);
    assert_eq!(fs::metadata(&final_path).unwrap().len(), bytes.len() as u64);
    assert_eq!(file_digest(&final_path), digest(&bytes));
    assert!(!observed_final.load(Ordering::SeqCst));
    assert!(!observed_missing_staging.load(Ordering::SeqCst));

    let writes = fixture.recorder.writes();
    let buffer = usize::try_from(BUFFER).unwrap();
    assert!(writes.iter().all(|written| written.len <= buffer));
    assert_eq!(writes.len(), bytes.len().div_ceil(buffer));
    let final_metadata = fs::metadata(&final_path).unwrap();
    let objects_dev = fs::metadata(fixture.data.join("storage/objects"))
        .unwrap()
        .dev();
    assert!(writes
        .iter()
        .all(|written| written.dev == objects_dev && written.ino == final_metadata.ino()));

    assert!(!exists(&fixture.temp_path(&key, &id)));
    assert!(!exists(&staging_blob));
    assert!(!exists(&fixture.staging_dir(&id)));
    let steps = fixture.recorder.steps();
    let position = |step| steps.iter().position(|recorded| *recorded == step).unwrap();
    assert!(position(Step::SyncTemp) < position(Step::RenameTemp));
    assert!(position(Step::RenameTemp) < position(Step::SyncLeaf));
    assert!(position(Step::SyncLeaf) < position(Step::UnlinkStaging));

    for errno in [Errno::NOSPC, Errno::DQUOT] {
        let bytes = pattern(3 * 1024 * 1024 + 7, 88);
        let id = fixture.stage(&bytes);
        let key = objects_key();
        fixture.recorder.fail(Step::WriteTemp, 3, errno);

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(matches!(error, StorageError::QuotaOnDevice), "{error:?}");
        assert!(!exists(&fixture.final_path(&key)));
        assert!(!exists(&fixture.temp_path(&key, &id)));
        assert_staging_intact(&fixture, &id, &bytes);

        let retried = fixture.provider.finalize_staged(&id, &key).unwrap();
        assert_eq!(retried.route, FinalizeRoute::CrossDeviceCopy);
        assert_eq!(file_digest(&fixture.final_path(&key)), digest(&bytes));
    }

    let same = Fixture::new();
    let bytes = pattern(1024 * 1024 + 3, 7);
    let id = same.stage(&bytes);
    let key = objects_key();
    let finalized = same.provider.finalize_staged(&id, &key).unwrap();
    assert_eq!(finalized.route, FinalizeRoute::Rename);
    assert!(same.recorder.writes().is_empty());
    assert_eq!(same.recorder.outcome(Step::RenameStaging), Some(None));
    assert_eq!(file_digest(&same.final_path(&key)), digest(&bytes));
}

#[test]
fn it_local_finalize_fsync_rename_order() {
    let bytes = pattern(700_000, 11);

    let fixture = Fixture::new();
    let id = fixture.stage(&bytes);
    let staged_ino = fs::metadata(fixture.staging_blob(&id)).unwrap().ino();
    let key = objects_key();
    fixture.recorder.events.lock().unwrap().clear();
    let finalized = fixture.provider.finalize_staged(&id, &key).unwrap();
    assert_eq!(finalized.route, FinalizeRoute::Rename);
    assert_eq!(
        fixture.recorder.steps(),
        [
            Step::SyncStaging,
            Step::SyncCreatedDir,
            Step::SyncCreatedDir,
            Step::RenameStaging,
            Step::SyncLeaf,
        ]
    );
    let final_path = fixture.final_path(&key);
    assert_eq!(fs::metadata(&final_path).unwrap().ino(), staged_ino);
    assert_eq!(finalized.stat.size, bytes.len() as u64);
    assert!(!exists(&fixture.staging_blob(&id)));

    for step in [
        Step::SyncStaging,
        Step::SyncCreatedDir,
        Step::RenameStaging,
        Step::SyncLeaf,
    ] {
        let fixture = Fixture::new();
        let id = fixture.stage(&bytes);
        let key = objects_key();
        fixture.recorder.fail(step, 0, Errno::IO);

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(matches!(error, StorageError::Io(_)), "{step:?}: {error:?}");
        let final_path = fixture.final_path(&key);
        if step == Step::SyncLeaf {
            assert_eq!(file_digest(&final_path), digest(&bytes));
            assert!(!exists(&fixture.staging_blob(&id)));
        } else {
            assert!(!exists(&final_path), "{step:?}");
            assert_staging_intact(&fixture, &id, &bytes);
        }
    }

    let fixture = Fixture::with_buffer(SMALL_BUFFER);
    fixture.recorder.exdev.store(true, Ordering::SeqCst);
    let id = fixture.stage(&bytes);
    let key = objects_key();
    fixture.recorder.events.lock().unwrap().clear();
    let finalized = fixture.provider.finalize_staged(&id, &key).unwrap();
    assert_eq!(finalized.route, FinalizeRoute::CrossDeviceCopy);
    assert_eq!(
        fixture.recorder.compressed(),
        [
            Step::SyncStaging,
            Step::SyncCreatedDir,
            Step::SyncCreatedDir,
            Step::RenameStaging,
            Step::WriteTemp,
            Step::SyncTemp,
            Step::RenameTemp,
            Step::SyncLeaf,
            Step::UnlinkStaging,
            Step::RemoveStagingDir,
        ]
    );
    assert_eq!(
        fixture.recorder.writes().len(),
        bytes.len().div_ceil(usize::try_from(SMALL_BUFFER).unwrap())
    );
    assert_eq!(file_digest(&fixture.final_path(&key)), digest(&bytes));
    assert!(!exists(&fixture.staging_dir(&id)));

    for step in [
        Step::WriteTemp,
        Step::SyncTemp,
        Step::RenameTemp,
        Step::SyncLeaf,
        Step::UnlinkStaging,
        Step::RemoveStagingDir,
    ] {
        let fixture = Fixture::with_buffer(SMALL_BUFFER);
        fixture.recorder.exdev.store(true, Ordering::SeqCst);
        let id = fixture.stage(&bytes);
        let key = objects_key();
        fixture
            .recorder
            .fail(step, usize::from(step == Step::WriteTemp) * 5, Errno::IO);

        let outcome = fixture.provider.finalize_staged(&id, &key);

        let final_path = fixture.final_path(&key);
        assert!(!exists(&fixture.temp_path(&key, &id)), "{step:?}");
        match step {
            Step::WriteTemp | Step::SyncTemp | Step::RenameTemp => {
                assert!(matches!(outcome, Err(StorageError::Io(_))), "{step:?}");
                assert!(!exists(&final_path), "{step:?}");
                assert_staging_intact(&fixture, &id, &bytes);
                assert!(fixture.recorder.steps().contains(&Step::UnlinkTemp));
            }
            Step::SyncLeaf => {
                assert!(matches!(outcome, Err(StorageError::Io(_))), "{step:?}");
                assert_eq!(file_digest(&final_path), digest(&bytes));
                assert_staging_intact(&fixture, &id, &bytes);
            }
            Step::UnlinkStaging => {
                assert_eq!(outcome.unwrap().route, FinalizeRoute::CrossDeviceCopy);
                assert_eq!(file_digest(&final_path), digest(&bytes));
                assert!(exists(&fixture.staging_blob(&id)));
            }
            _ => {
                assert_eq!(outcome.unwrap().route, FinalizeRoute::CrossDeviceCopy);
                assert_eq!(file_digest(&final_path), digest(&bytes));
                assert!(!exists(&fixture.staging_blob(&id)));
                assert!(exists(&fixture.staging_dir(&id)));
            }
        }
    }
}

fn outside_fixture() -> (TempDir, BTreeMap<String, Option<Vec<u8>>>) {
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("victim"), b"outside-the-root").unwrap();
    fs::create_dir(outside.path().join("nested")).unwrap();
    fs::write(outside.path().join("blob"), b"outside-blob").unwrap();
    let snapshot = tree(outside.path());
    (outside, snapshot)
}

#[test]
fn it_local_symlink_escape_rejected() {
    let bytes = pattern(50_000, 3);

    for depth in 0..3 {
        let fixture = Fixture::new();
        let (outside, snapshot) = outside_fixture();
        let id = fixture.stage(&bytes);
        let key = objects_key();
        let mut planted = fixture.final_path(&key);
        for _ in depth..3 {
            planted.pop();
        }
        fs::create_dir_all(planted.parent().unwrap()).unwrap();
        if exists(&planted) {
            fs::remove_dir(&planted).unwrap();
        }
        symlink(outside.path(), &planted).unwrap();

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(
            matches!(error, StorageError::PermissionDenied),
            "{depth}: {error:?}"
        );
        assert_eq!(tree(outside.path()), snapshot, "{depth}");
        assert_staging_intact(&fixture, &id, &bytes);
    }

    for exdev in [false, true] {
        let fixture = Fixture::new();
        fixture.recorder.exdev.store(exdev, Ordering::SeqCst);
        let (outside, snapshot) = outside_fixture();
        let id = fixture.stage(&bytes);
        let key = objects_key();
        let final_path = fixture.final_path(&key);
        fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        symlink(outside.path().join("victim"), &final_path).unwrap();

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(matches!(error, StorageError::PermissionDenied), "{error:?}");
        assert!(fs::symlink_metadata(&final_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(tree(outside.path()), snapshot);
        assert_staging_intact(&fixture, &id, &bytes);
    }

    {
        let fixture = Fixture::new();
        let (outside, snapshot) = outside_fixture();
        let id = upload_id();
        symlink(outside.path(), fixture.staging_dir(&id)).unwrap();
        let key = objects_key();

        assert!(matches!(
            fixture.provider.finalize_staged(&id, &key),
            Err(StorageError::PermissionDenied)
        ));
        assert!(matches!(
            fixture.provider.open_staging(&id),
            Err(StorageError::PermissionDenied)
        ));
        assert!(matches!(
            fixture.provider.create_staging(&id),
            Err(StorageError::AlreadyExists)
        ));
        assert!(matches!(
            fixture.provider.remove_staging(&id),
            Err(StorageError::PermissionDenied)
        ));
        assert!(!exists(&fixture.final_path(&key)));
        assert_eq!(tree(outside.path()), snapshot);
    }

    {
        let fixture = Fixture::new();
        let (outside, snapshot) = outside_fixture();
        let id = upload_id();
        fs::create_dir(fixture.staging_dir(&id)).unwrap();
        symlink(outside.path().join("victim"), fixture.staging_blob(&id)).unwrap();
        let key = objects_key();

        assert!(matches!(
            fixture.provider.finalize_staged(&id, &key),
            Err(StorageError::PermissionDenied)
        ));
        assert!(matches!(
            fixture.provider.open_staging(&id),
            Err(StorageError::PermissionDenied)
        ));
        assert!(!exists(&fixture.final_path(&key)));
        assert_eq!(tree(outside.path()), snapshot);
    }

    {
        let fixture = Fixture::new();
        fixture.recorder.exdev.store(true, Ordering::SeqCst);
        let (outside, snapshot) = outside_fixture();
        let id = fixture.stage(&bytes);
        let key = objects_key();
        let temp = fixture.temp_path(&key, &id);
        fs::create_dir_all(temp.parent().unwrap()).unwrap();
        symlink(outside.path().join("victim"), &temp).unwrap();

        let finalized = fixture.provider.finalize_staged(&id, &key).unwrap();

        assert_eq!(finalized.route, FinalizeRoute::CrossDeviceCopy);
        assert!(fs::symlink_metadata(fixture.final_path(&key))
            .unwrap()
            .is_file());
        assert_eq!(file_digest(&fixture.final_path(&key)), digest(&bytes));
        assert!(!exists(&temp));
        assert_eq!(tree(outside.path()), snapshot);
    }

    {
        let fixture = Fixture::new();
        let (outside, snapshot) = outside_fixture();
        let id = fixture.stage(&bytes);
        let key = ObjectKey::allocate(KeyNamespace::Branding(BrandingKind::Logo));
        symlink(outside.path(), fixture.data.join("branding/logo")).unwrap();

        assert!(matches!(
            fixture.provider.finalize_staged(&id, &key),
            Err(StorageError::PermissionDenied)
        ));
        assert_eq!(tree(outside.path()), snapshot);
        assert_staging_intact(&fixture, &id, &bytes);
    }
}

const HOSTILE_NAMES: [&str; 22] = [
    "../../etc/passwd",
    "../",
    "..",
    ".",
    "/etc/shadow",
    "/",
    "a/b/c.txt",
    "..\\..\\windows\\system32\\config\\SAM",
    "C:\\Windows\\win.ini",
    "C:",
    "\\\\server\\share\\file",
    "evil\0name.txt",
    "line\r\nbreak\t\u{7}\u{1b}[31m",
    ".bashrc",
    ".tmp-0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
    "archive.tar.gz",
    "résumé-naïve-ÅÄÖ-日本語-עברית\u{202e}fdp.exe",
    "e\u{301}",
    "😀🎉🔥👩\u{200d}💻",
    "objects/01/92/0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
    "0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
    "",
];

fn layout_for(name: &str, id: &UploadId, key: &ObjectKey, exdev: bool) -> Vec<String> {
    let fixture = Fixture::new();
    fixture.recorder.exdev.store(exdev, Ordering::SeqCst);
    let display_name = name.to_owned();
    let mut writer: StagingWriter = fixture.provider.create_staging(id).unwrap();
    writer.append(display_name.as_bytes()).unwrap();
    writer.append(b"-payload").unwrap();
    writer.sync().unwrap();
    let finalized = fixture.provider.finalize_staged(id, key).unwrap();
    assert_eq!(
        finalized.stat.size,
        (display_name.len() + b"-payload".len()) as u64
    );
    assert_eq!(id.temp_name(), format!(".tmp-{}", id.as_str()));
    fixture.provider.remove_staging(id).unwrap();
    tree(&fixture.data).into_keys().collect()
}

fn check_display_name(name: &str) {
    let id = upload_id();
    let key = objects_key();
    let oid = key.as_str().rsplit('/').next().unwrap();
    let expected: Vec<String> = [
        "branding".to_owned(),
        "storage".to_owned(),
        "storage/objects".to_owned(),
        format!("storage/objects/{}", &oid[..2]),
        format!("storage/objects/{}/{}", &oid[..2], &oid[2..4]),
        format!("storage/objects/{}/{}/{oid}", &oid[..2], &oid[2..4]),
        "uploads".to_owned(),
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>()
    .into_iter()
    .collect();

    for exdev in [false, true] {
        assert_eq!(
            layout_for(DISPLAY_NAME_BASELINE, &id, &key, exdev),
            expected
        );
        assert_eq!(layout_for(name, &id, &key, exdev), expected, "{name:?}");
    }
    let location = ObjectLocation::of(&key);
    assert_eq!(location.root, Root::Storage);
    assert_eq!(location.dirs(), ["objects", &oid[..2], &oid[2..4]]);
    assert_eq!(location.leaf, oid);

    if let Ok(parsed) = UploadId::parse(name) {
        assert_eq!(parsed.as_str().len(), 32);
        assert!(parsed.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
    }
    if ObjectKey::parse(name).is_ok() {
        assert!(name.starts_with("objects/") || name.starts_with("branding/"));
    }
}

static HOSTILE_FIXED: Once = Once::new();

fn display_names() -> impl Strategy<Value = String> {
    prop_oneof![
        proptest::sample::select(HOSTILE_NAMES.to_vec()).prop_map(str::to_owned),
        "\\PC{0,64}",
        "[./\\\\:a-zA-Z0-9\u{0}-\u{1f}]{1,48}",
        "[\u{1F300}-\u{1F6FF}\u{4E00}-\u{4E80}\u{0590}-\u{05FF}]{200,600}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn prop_display_name_never_influences_path(name in display_names()) {
        HOSTILE_FIXED.call_once(|| HOSTILE_NAMES.iter().for_each(|name| check_display_name(name)));
        check_display_name(&name);
    }
}

#[test]
fn unit_local_file_and_directory_modes() {
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o027));
    assert_eq!(DIRECTORY_MODE, 0o750);
    assert_eq!(FILE_MODE, 0o640);

    for exdev in [false, true] {
        let fixture = Fixture::new();
        fixture.recorder.exdev.store(exdev, Ordering::SeqCst);
        let id = fixture.stage(b"modes");
        assert_eq!(mode(&fixture.staging_dir(&id)), 0o750);
        assert_eq!(mode(&fixture.staging_blob(&id)), 0o640);

        let key = objects_key();
        fixture.provider.finalize_staged(&id, &key).unwrap();
        let final_path = fixture.final_path(&key);
        assert_eq!(mode(&final_path), 0o640);
        assert_eq!(mode(final_path.parent().unwrap()), 0o750);
        assert_eq!(mode(final_path.parent().unwrap().parent().unwrap()), 0o750);
    }
}

#[test]
fn unit_local_existing_final_never_overwritten() {
    for exdev in [false, true] {
        let fixture = Fixture::new();
        fixture.recorder.exdev.store(exdev, Ordering::SeqCst);
        let id = fixture.stage(b"new-bytes");
        let key = objects_key();
        let final_path = fixture.final_path(&key);
        fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        fs::write(&final_path, b"durable-object").unwrap();

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(matches!(error, StorageError::AlreadyExists), "{error:?}");
        assert_eq!(file_digest(&final_path), digest(b"durable-object"));
        assert_staging_intact(&fixture, &id, b"new-bytes");
        assert!(!exists(&fixture.temp_path(&key, &id)));
    }

    for (exdev, racing_step) in [(false, Step::RenameStaging), (true, Step::RenameTemp)] {
        let fixture = Fixture::new();
        fixture.recorder.exdev.store(exdev, Ordering::SeqCst);
        let id = fixture.stage(b"new-bytes");
        let key = objects_key();
        let final_path = fixture.final_path(&key);
        {
            let final_path = final_path.clone();
            fixture.recorder.observe(move |step| {
                if step == racing_step {
                    fs::write(&final_path, b"raced-object").unwrap();
                }
            });
        }

        let error = fixture.provider.finalize_staged(&id, &key).unwrap_err();

        assert!(matches!(error, StorageError::AlreadyExists), "{error:?}");
        assert_eq!(file_digest(&final_path), digest(b"raced-object"));
        assert_staging_intact(&fixture, &id, b"new-bytes");
        assert!(!exists(&fixture.temp_path(&key, &id)));
    }
}

#[test]
fn unit_local_temp_name_is_not_object_key() {
    let id = upload_id();
    let key = objects_key();
    let oid = key.as_str().rsplit('/').next().unwrap();
    let leaf = key.as_str().rsplit_once('/').unwrap().0;
    for temp in [id.temp_name(), format!(".tmp-{oid}")] {
        assert!(
            ObjectKey::parse(&format!("{leaf}/{temp}")).is_err(),
            "{temp}"
        );
        assert!(UploadId::parse(&temp).is_err(), "{temp}");
    }
}

#[test]
fn unit_upload_id_grammar() {
    let generated = Uuid::now_v7();
    let simple = generated.simple().to_string();
    assert_eq!(UploadId::parse(&simple).unwrap().as_str(), simple);
    for rejected in [
        generated.hyphenated().to_string(),
        simple.to_uppercase(),
        Uuid::from_u128(0x6f1c_2a3b_4c5d_4e6f_8a7b_9c0d_1e2f_3a4b)
            .simple()
            .to_string(),
        simple[..31].to_owned(),
        format!("{simple}0"),
        format!("../{}", &simple[3..]),
        String::new(),
    ] {
        assert!(UploadId::parse(&rejected).is_err(), "{rejected}");
    }
}

#[test]
fn unit_object_location_components() {
    let key = ObjectKey::parse("objects/01/92/0192f3c8d7e94a1b8f0c2d5e6a7b8c9d").unwrap();
    let location = ObjectLocation::of(&key);
    assert_eq!(location.root, Root::Storage);
    assert_eq!(location.dirs(), ["objects", "01", "92"]);
    assert_eq!(location.leaf, "0192f3c8d7e94a1b8f0c2d5e6a7b8c9d");

    for kind in BrandingKind::ALL {
        let key = ObjectKey::allocate(KeyNamespace::Branding(kind));
        let location = ObjectLocation::of(&key);
        assert_eq!(location.root, Root::Branding);
        assert_eq!(location.dirs(), [kind.as_str()]);
        assert_eq!(location.leaf.len(), 32);
    }
}

#[test]
fn unit_staging_append_is_bounded_and_appends() {
    struct Probe<'a> {
        inner: Cursor<&'a [u8]>,
        largest: usize,
    }

    impl Read for Probe<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.largest = self.largest.max(buf.len());
            self.inner.read(buf)
        }
    }

    let fixture = Fixture::with_buffer(SMALL_BUFFER);
    let id = upload_id();
    let first = pattern(40_000, 5);
    let mut writer = fixture.provider.create_staging(&id).unwrap();
    let mut probe = Probe {
        inner: Cursor::new(&first),
        largest: 0,
    };
    assert_eq!(writer.append_from(&mut probe, 30_000).unwrap(), 30_000);
    assert!(probe.largest <= usize::try_from(SMALL_BUFFER).unwrap());
    writer.sync().unwrap();
    drop(writer);

    let mut reopened = fixture.provider.open_staging(&id).unwrap();
    assert_eq!(reopened.staged_len().unwrap(), 30_000);
    reopened.append(&first[30_000..]).unwrap();
    assert_eq!(reopened.staged_len().unwrap(), 40_000);
    assert_eq!(file_digest(&fixture.staging_blob(&id)), digest(&first));

    assert!(matches!(
        fixture.provider.create_staging(&id),
        Err(StorageError::AlreadyExists)
    ));
    assert!(fixture.provider.remove_staging(&id).unwrap());
    assert!(!exists(&fixture.staging_dir(&id)));
    assert!(!fixture.provider.remove_staging(&id).unwrap());
    assert!(matches!(
        fixture.provider.open_staging(&id),
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        LocalProvider::open(&fixture.data, 0),
        Err(StorageError::Config(_))
    ));
}

#[test]
fn unit_local_primitives_stay_in_scope() {
    let sources = [
        ("mod.rs", include_str!("mod.rs")),
        ("paths.rs", include_str!("paths.rs")),
        ("write.rs", include_str!("write.rs")),
        ("finalize.rs", include_str!("finalize.rs")),
    ];
    let forbidden = [
        "allocate(",
        "sqlx",
        "infra::db",
        "Database",
        "chown",
        "read_to_end",
        "fs::copy(",
        "with_capacity(",
        "set_permissions",
        "fchmod",
        "impl StorageProvider",
        "unimplemented!",
        "todo!",
    ];
    for (file, source) in sources {
        for token in forbidden {
            assert!(!source.contains(token), "{file} contains {token}");
        }
        if file != "mod.rs" {
            assert!(!source.contains("canonicalize"), "{file}");
            assert!(!source.contains(".join("), "{file}");
        }
    }
    let root = include_str!("mod.rs");
    assert_eq!(root.matches("canonicalize").count(), 1);
    assert_eq!(root.matches("data_dir.join(name)").count(), 1);
    assert_eq!(root.matches(".join(").count(), 1);
}

#[tokio::test]
async fn it_storage_contract_local() {
    let fixture = Fixture::new();

    let descriptor = fixture.provider.describe();
    assert_eq!(descriptor.provider, ProviderKind::Local);
    assert!(matches!(
        descriptor.local,
        Some(local) if matches!(local.capacity, CapacityReport::Available(_))
    ));

    let zero = place(&fixture, b"");
    let zero_stat = fixture.provider.stat(&zero).unwrap();
    assert_eq!(zero_stat.size, 0);
    assert!(zero_stat.etag.is_none());

    let bytes = pattern(300_000, 1234);
    let key = place(&fixture, &bytes);
    let size = bytes.len() as u64;
    let stat = fixture.provider.stat(&key).unwrap();
    assert_eq!(stat.size, size);
    assert!(stat.etag.is_none());
    assert!(stat.modified_at.year() >= 2020);

    assert!(fixture.provider.exists(&zero).unwrap());
    assert!(fixture.provider.exists(&key).unwrap());
    let missing = objects_key();
    assert!(!fixture.provider.exists(&missing).unwrap());
    assert!(matches!(
        fixture.provider.stat(&missing).err().unwrap(),
        StorageError::NotFound
    ));

    let (read_stat, read_body) = fixture.provider.open_read(&key).unwrap();
    assert_eq!(read_stat.size, size);
    assert_eq!(read_all(read_body).await, bytes);

    let (zero_read_stat, zero_body) = fixture.provider.open_read(&zero).unwrap();
    assert_eq!(zero_read_stat.size, 0);
    assert!(read_all(zero_body).await.is_empty());

    assert!(matches!(
        fixture.provider.open_range(&zero, 0, 1).err().unwrap(),
        StorageError::RangeNotSatisfiable { size: 0 }
    ));
    assert!(matches!(
        fixture.provider.open_range(&key, size, 1).err().unwrap(),
        StorageError::RangeNotSatisfiable { size: reported } if reported == size
    ));

    let start = 1_234u64;
    let (range_stat, range_body) = fixture.provider.open_range(&key, start, 4_096).unwrap();
    assert_eq!(range_stat.size, size);
    assert!(range_stat.etag.is_none());
    assert_eq!(
        read_all(range_body).await,
        bytes[start as usize..(start + 4_096) as usize]
    );

    let clamp_start = size - 100;
    let (_, clamp_body) = fixture
        .provider
        .open_range(&key, clamp_start, 1_000_000)
        .unwrap();
    assert_eq!(read_all(clamp_body).await, bytes[(size - 100) as usize..]);

    let (_, zero_len_body) = fixture.provider.open_range(&key, start, 0).unwrap();
    assert!(read_all(zero_len_body).await.is_empty());

    let dst = objects_key();
    let copy_stat = fixture.provider.copy(&key, &dst).unwrap();
    assert_eq!(copy_stat.size, size);
    assert!(copy_stat.etag.is_none());
    let (_, copy_body) = fixture.provider.open_read(&dst).unwrap();
    assert_eq!(read_all(copy_body).await, bytes);

    assert!(fixture.provider.delete(&key).unwrap());
    assert!(!fixture.provider.exists(&key).unwrap());
    let (_, survivor_body) = fixture.provider.open_read(&dst).unwrap();
    assert_eq!(read_all(survivor_body).await, bytes);

    assert!(fixture.provider.delete(&dst).unwrap());
    assert!(!fixture.provider.delete(&dst).unwrap());

    let mut keys: Vec<String> = Vec::new();
    for _ in 0..11 {
        keys.push(place(&fixture, b"listed").as_str().to_owned());
    }
    keys.push(zero.as_str().to_owned());
    keys.sort();

    let mut collected = Vec::new();
    let mut cursor = None;
    loop {
        let page = fixture.provider.list_page("objects/", cursor, 5).unwrap();
        assert!(page.entries.len() <= 5);
        for entry in &page.entries {
            assert_eq!(ObjectKey::parse(&entry.key).unwrap().as_str(), entry.key);
            collected.push(entry.key.clone());
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(collected, keys);
}

#[tokio::test]
async fn it_local_range_does_not_discard_prefix() {
    let fixture = Fixture::with_buffer(COPY_BUFFER);
    let bytes = pattern(180_000, 21);
    let key = place(&fixture, &bytes);
    let start = 120_000u64;

    let (stat, body) = fixture.provider.open_range(&key, start, 10_000).unwrap();

    assert_eq!(stat.size, bytes.len() as u64);
    assert_eq!(
        read_all(body).await,
        bytes[start as usize..(start as usize + 10_000)]
    );
    assert_eq!(fixture.recorder.seeks(), vec![start]);
}

#[test]
fn it_local_copy_bounded_fallback() {
    let fixture = Fixture::with_buffer(COPY_BUFFER);
    fixture.recorder.force_reflink_unsupported();
    fixture.recorder.force_copy_range_unsupported();
    let bytes = pattern(2 * 1024 * 1024 + 321, 65);
    let key = place(&fixture, &bytes);
    let dst = objects_key();
    let final_path = fixture.final_path(&dst);
    let source_path = fixture.final_path(&key);

    let observed_final = Arc::new(AtomicBool::new(false));
    {
        let final_path = final_path.clone();
        let observed_final = Arc::clone(&observed_final);
        fixture.recorder.observe(move |step| {
            if matches!(step, Step::WriteTemp | Step::SyncTemp | Step::RenameTemp)
                && exists(&final_path)
            {
                observed_final.store(true, Ordering::SeqCst);
            }
        });
    }

    let stat = fixture.provider.copy(&key, &dst).unwrap();

    assert_eq!(stat.size, bytes.len() as u64);
    assert!(stat.etag.is_none());
    assert_eq!(file_digest(&final_path), digest(&bytes));
    assert!(!observed_final.load(Ordering::SeqCst));
    assert!(fixture.recorder.clone_calls() >= 1);
    assert!(fixture.recorder.copy_range_calls() >= 1);

    let buffer = usize::try_from(COPY_BUFFER).unwrap();
    let writes = fixture.recorder.writes();
    assert!(!writes.is_empty());
    assert!(writes.iter().all(|written| written.len <= buffer));
    assert_eq!(writes.len(), bytes.len().div_ceil(buffer));
    assert!(!exists(&fixture.copy_temp_path(&dst)));
    assert_eq!(file_digest(&source_path), digest(&bytes));

    let failure = Fixture::with_buffer(COPY_BUFFER);
    failure.recorder.force_reflink_unsupported();
    failure.recorder.force_copy_range_unsupported();
    let bytes = pattern(1_500_000, 12);
    let key = place(&failure, &bytes);
    let dst = objects_key();
    failure.recorder.fail(Step::WriteTemp, 3, Errno::NOSPC);

    let error = failure.provider.copy(&key, &dst).unwrap_err();

    assert!(matches!(error, StorageError::QuotaOnDevice), "{error:?}");
    assert!(!exists(&failure.final_path(&dst)));
    assert!(!exists(&failure.copy_temp_path(&dst)));
    assert_eq!(file_digest(&failure.final_path(&key)), digest(&bytes));
}

#[test]
fn it_local_copy_file_range_short_copy_loop() {
    let fixture = Fixture::with_buffer(COPY_BUFFER);
    fixture.recorder.force_reflink_unsupported();
    fixture.recorder.cap_copy_range(4_096);
    let bytes = pattern(1_000_007, 9);
    let key = place(&fixture, &bytes);
    let dst = objects_key();

    let stat = fixture.provider.copy(&key, &dst).unwrap();

    assert_eq!(stat.size, bytes.len() as u64);
    assert_eq!(file_digest(&fixture.final_path(&dst)), digest(&bytes));
    assert!(fixture.recorder.writes().is_empty());
    assert_eq!(fixture.recorder.clone_calls(), 1);
    assert_eq!(
        fixture.recorder.copy_range_calls(),
        bytes.len().div_ceil(4_096)
    );
}

#[test]
fn it_local_delete_notfound_is_success() {
    let fixture = Fixture::new();
    let bytes = pattern(2_048, 4);
    let key = place(&fixture, &bytes);
    let final_path = fixture.final_path(&key);
    let leaf_dir = final_path.parent().unwrap().to_path_buf();

    assert!(fixture.provider.delete(&key).unwrap());
    assert!(!exists(&final_path));
    assert!(leaf_dir.is_dir());
    assert!(leaf_dir.parent().unwrap().is_dir());
    assert!(!fixture.provider.delete(&key).unwrap());

    let never_placed = objects_key();
    assert!(!fixture.provider.delete(&never_placed).unwrap());
}

#[test]
fn unit_local_list_page_clamped_to_max() {
    let fixture = Fixture::new();
    for _ in 0..1_005 {
        let _ = place(&fixture, b"x");
    }

    let first = fixture
        .provider
        .list_page("objects/", None, u32::MAX)
        .unwrap();
    assert_eq!(first.entries.len(), 1_000);
    let cursor = first.next.expect("a full page must carry a cursor");

    let second = fixture
        .provider
        .list_page("objects/", Some(cursor), u32::MAX)
        .unwrap();
    assert_eq!(second.entries.len(), 5);
    assert!(second.next.is_none());
}

#[test]
fn unit_local_list_skips_malformed_entries() {
    let fixture = Fixture::new();
    let key = place(&fixture, b"valid-object");
    let final_path = fixture.final_path(&key);
    let leaf = final_path.parent().unwrap().to_path_buf();
    let oid = key.as_str().rsplit('/').next().unwrap().to_owned();
    let shard = oid[..4].to_owned();
    let ab = &shard[..2];
    let cd = &shard[2..4];

    let planted = format!("{shard}{}", "f".repeat(28));
    fs::write(leaf.join(&planted), b"planted").unwrap();
    fs::write(leaf.join("notanoid"), b"x").unwrap();
    fs::write(leaf.join(oid.to_ascii_uppercase()), b"x").unwrap();
    fs::write(leaf.join(".tmp-deadbeef"), b"x").unwrap();
    fs::write(leaf.join(format!("{shard}{}", "0".repeat(30))), b"x").unwrap();
    fs::write(leaf.join(format!("{}{}", "0".repeat(4), &oid[4..])), b"x").unwrap();
    fs::create_dir(leaf.join(format!("{shard}{}", "e".repeat(28)))).unwrap();
    let symlinked = format!("{shard}{}", "b".repeat(28));
    symlink(final_path.file_name().unwrap(), leaf.join(&symlinked)).unwrap();
    fs::write(fixture.data.join("storage/objects/stray"), b"x").unwrap();

    let page = fixture
        .provider
        .list_page("objects/", None, u32::MAX)
        .unwrap();
    let emitted: Vec<String> = page.entries.iter().map(|entry| entry.key.clone()).collect();

    assert_eq!(emitted.len(), 2, "{emitted:?}");
    assert!(emitted.contains(&key.as_str().to_owned()));
    assert!(emitted.contains(&format!("objects/{ab}/{cd}/{planted}")));
    for entry in &emitted {
        assert!(ObjectKey::parse(entry).is_ok());
    }
}

#[test]
fn unit_local_copy_destination_collision_is_not_overwritten() {
    let fixture = Fixture::new();
    let key = place(&fixture, b"source-bytes");
    let dst = objects_key();
    let final_path = fixture.final_path(&dst);
    fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    fs::write(&final_path, b"existing-object").unwrap();

    let error = fixture.provider.copy(&key, &dst).unwrap_err();

    assert!(matches!(error, StorageError::AlreadyExists), "{error:?}");
    assert_eq!(file_digest(&final_path), digest(b"existing-object"));
    assert!(!exists(&fixture.copy_temp_path(&dst)));
    assert_eq!(
        file_digest(&fixture.final_path(&key)),
        digest(b"source-bytes")
    );
}

#[test]
fn unit_local_reflink_capability_is_cached_once_per_root() {
    let fixture = Fixture::with_buffer(COPY_BUFFER);
    fixture.recorder.force_reflink_unsupported();
    fixture.recorder.force_copy_range_unsupported();
    let key = place(&fixture, b"cache-me");
    let first = objects_key();
    let second = objects_key();

    fixture.provider.copy(&key, &first).unwrap();
    fixture.provider.copy(&key, &second).unwrap();

    assert_eq!(fixture.recorder.clone_calls(), 1);
    assert_eq!(fixture.recorder.copy_range_calls(), 2);
}

#[test]
fn regression_65_disk_space_detection() {
    let fixture = Fixture::new();

    let report = fixture.provider.capacity();
    let CapacityReport::Available(capacity) = report else {
        panic!("a local root must report capacity when statvfs succeeds");
    };
    assert!(capacity.total_bytes > 0);
    assert!(capacity.available_bytes <= capacity.total_bytes);

    let objects_ino = fs::metadata(fixture.data.join("storage/objects"))
        .unwrap()
        .ino();
    let data_ino = fs::metadata(&fixture.data).unwrap().ino();
    assert_ne!(objects_ino, data_ino);
    let targets = fixture.recorder.statvfs_targets();
    assert!(!targets.is_empty());
    assert!(targets.iter().all(|ino| *ino == objects_ino));
    assert!(targets.iter().all(|ino| *ino != data_ino));

    fixture.recorder.fail_statvfs(Errno::INVAL);
    assert_eq!(fixture.provider.capacity(), CapacityReport::Unavailable);
    assert!(!matches!(
        fixture.provider.capacity(),
        CapacityReport::Available(_)
    ));

    let floor = 1024 * 1024 * 1024;
    assert!(is_low_space(&Capacity {
        total_bytes: 2 * floor,
        available_bytes: floor - 1,
    }));
    assert!(!is_low_space(&Capacity {
        total_bytes: 2 * floor,
        available_bytes: floor,
    }));

    let huge = 1_000 * floor;
    let threshold = huge / 50;
    assert!(threshold > floor);
    assert!(is_low_space(&Capacity {
        total_bytes: huge,
        available_bytes: threshold - 1,
    }));
    assert!(!is_low_space(&Capacity {
        total_bytes: huge,
        available_bytes: threshold,
    }));

    let bytes = pattern(100_000, 5);
    let key = place(&fixture, &bytes);
    let dst = objects_key();
    assert_eq!(
        fixture.provider.copy(&key, &dst).unwrap().size,
        bytes.len() as u64
    );

    let source = include_str!("capacity.rs");
    for token in [
        "QuotaOnDevice",
        "StorageFull",
        "used_bytes",
        "quota",
        "users.",
        "Command::new",
    ] {
        assert!(!source.contains(token), "capacity.rs contains {token}");
    }
}

#[test]
fn unit_local_no_whole_object_read_paths() {
    let sources = [
        ("read.rs", include_str!("read.rs")),
        ("copy.rs", include_str!("copy.rs")),
        ("delete.rs", include_str!("delete.rs")),
        ("list.rs", include_str!("list.rs")),
        ("capacity.rs", include_str!("capacity.rs")),
    ];
    let forbidden = [
        "read_to_end",
        "read_to_string",
        "fs::read",
        "fs::read_dir",
        "Command::new",
        "list_all",
        "Vec::from_iter",
    ];
    for (file, source) in sources {
        for token in forbidden {
            assert!(!source.contains(token), "{file} contains {token}");
        }
    }
}
