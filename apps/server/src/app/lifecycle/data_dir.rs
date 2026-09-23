use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, RawMode};

pub const STARTUP_DATA_DIR_NOT_WRITABLE: &str = "STARTUP_DATA_DIR_NOT_WRITABLE";
pub const STARTUP_UPLOADS_STORAGE_CROSS_DEVICE: &str = "STARTUP_UPLOADS_STORAGE_CROSS_DEVICE";

pub const PROCESS_UMASK: RawMode = 0o027;
pub const OWNED_DIRECTORY_MODE: u32 = 0o750;
const PROBE_FILE_MODE: u32 = 0o600;

pub const OWNED_DIRECTORIES: [&str; 7] = [
    "storage",
    "storage/objects",
    "uploads",
    "thumbnails",
    "branding",
    "runtime",
    "backup",
];
const UPLOADS: &str = "uploads";
const OBJECTS: &str = "storage/objects";

const PROBE_PREFIX: &str = ".palmr-write-test-";
const PROBE_PAYLOAD: &[u8] = b"palmr write probe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirOperation {
    Metadata,
    Mkdir,
    Create,
    Write,
    Fsync,
    Read,
    Remove,
}

impl DataDirOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Mkdir => "mkdir",
            Self::Create => "create",
            Self::Write => "write",
            Self::Fsync => "fsync",
            Self::Read => "read",
            Self::Remove => "remove",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub uid: u32,
    pub gid: u32,
}

impl ProcessIdentity {
    pub fn effective() -> Self {
        Self {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedPath {
    pub parent: bool,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

#[derive(Debug)]
pub enum DataDirCause {
    Io(io::Error),
    NotADirectory,
    ReadBackMismatch,
}

#[derive(Debug)]
pub struct DataDirError {
    pub path: PathBuf,
    pub operation: DataDirOperation,
    pub cause: DataDirCause,
    pub identity: ProcessIdentity,
    pub observed: Option<ObservedPath>,
    pub(super) data_root: bool,
}

impl DataDirError {
    pub const fn code(&self) -> &'static str {
        STARTUP_DATA_DIR_NOT_WRITABLE
    }

    fn new(path: &Path, operation: DataDirOperation, cause: DataDirCause, data_root: bool) -> Self {
        let observed =
            observe(path, false).or_else(|| path.parent().and_then(|parent| observe(parent, true)));
        Self {
            path: path.to_path_buf(),
            operation,
            cause,
            identity: ProcessIdentity::effective(),
            observed,
            data_root,
        }
    }

    fn hint(&self) -> String {
        let ProcessIdentity { uid, gid } = self.identity;
        let ownership = format!(
            "fix ownership on the host once, for example `chown -R {uid}:{gid} <host directory mounted at PALMR_DATA_DIR>`, or run the container as the owning user with Docker `user:`; Palmr never changes ownership itself"
        );
        match &self.cause {
            DataDirCause::NotADirectory => format!("{} must be a directory", self.path.display()),
            DataDirCause::ReadBackMismatch => {
                "the filesystem returned different bytes than were written; check the volume and its storage stack".to_owned()
            }
            DataDirCause::Io(error) => match error.kind() {
                io::ErrorKind::NotFound if self.data_root => {
                    "PALMR_DATA_DIR does not exist; the persistent volume is probably not mounted. Palmr never creates the data directory itself".to_owned()
                }
                io::ErrorKind::ReadOnlyFilesystem => {
                    "the volume is mounted read-only; remove `:ro` from the mount".to_owned()
                }
                io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => {
                    "the filesystem is full; free space on the volume".to_owned()
                }
                io::ErrorKind::PermissionDenied => ownership,
                io::ErrorKind::NotADirectory => {
                    "a component of the path is not a directory".to_owned()
                }
                _ => format!("check the volume and its mount options; {ownership}"),
            },
        }
    }
}

impl fmt::Display for DataDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{STARTUP_DATA_DIR_NOT_WRITABLE}: Palmr cannot use {} (operation: {}; reason: ",
            self.path.display(),
            self.operation.as_str()
        )?;
        match &self.cause {
            DataDirCause::Io(error) => write!(f, "{error}")?,
            DataDirCause::NotADirectory => f.write_str("not a directory")?,
            DataDirCause::ReadBackMismatch => f.write_str("read-back mismatch")?,
        }
        write!(
            f,
            "; running as uid={} gid={}",
            self.identity.uid, self.identity.gid
        )?;
        if let Some(observed) = self.observed {
            write!(
                f,
                "; {} owner uid={} gid={} mode {:04o}",
                if observed.parent { "parent" } else { "path" },
                observed.uid,
                observed.gid,
                observed.mode
            )?;
        }
        write!(f, "). {}", self.hint())
    }
}

impl std::error::Error for DataDirError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            DataDirCause::Io(error) => Some(error),
            DataDirCause::NotADirectory | DataDirCause::ReadBackMismatch => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossDevice {
    pub uploads_device: u64,
    pub objects_device: u64,
}

#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    cross_device: Option<CrossDevice>,
}

impl DataDir {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn cross_device(&self) -> Option<CrossDevice> {
        self.cross_device
    }

    pub fn prepare(root: &Path) -> Result<Self, DataDirError> {
        Self::prepare_with(root, device_id)
    }

    pub fn prepare_with(
        root: &Path,
        device_of: impl Fn(&Path) -> io::Result<u64>,
    ) -> Result<Self, DataDirError> {
        let probe_id = probe_id().map_err(|error| {
            DataDirError::new(
                root,
                DataDirOperation::Create,
                DataDirCause::Io(error),
                true,
            )
        })?;

        require_directory(root, true)?;
        probe(root, &probe_id, true)?;
        for relative in OWNED_DIRECTORIES {
            let path = root.join(relative);
            ensure_directory(&path)?;
            probe(&path, &probe_id, false)?;
        }

        let uploads = root.join(UPLOADS);
        let objects = root.join(OBJECTS);
        let uploads_device =
            device_of(&uploads).map_err(|error| metadata_failed(&uploads, error))?;
        let objects_device =
            device_of(&objects).map_err(|error| metadata_failed(&objects, error))?;

        Ok(Self {
            root: root.to_path_buf(),
            cross_device: (uploads_device != objects_device).then_some(CrossDevice {
                uploads_device,
                objects_device,
            }),
        })
    }
}

pub fn apply_process_umask() -> u32 {
    u32::from(rustix::process::umask(Mode::from_raw_mode(PROCESS_UMASK)).as_raw_mode())
}

fn metadata_failed(path: &Path, error: io::Error) -> DataDirError {
    DataDirError::new(
        path,
        DataDirOperation::Metadata,
        DataDirCause::Io(error),
        false,
    )
}

fn device_id(path: &Path) -> io::Result<u64> {
    fs::metadata(path).map(|metadata| metadata.dev())
}

fn observe(path: &Path, parent: bool) -> Option<ObservedPath> {
    fs::metadata(path).ok().map(|metadata| ObservedPath {
        parent,
        uid: metadata.uid(),
        gid: metadata.gid(),
        mode: metadata.mode() & 0o7777,
    })
}

fn require_directory(path: &Path, root: bool) -> Result<(), DataDirError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(DataDirError::new(
            path,
            DataDirOperation::Metadata,
            DataDirCause::NotADirectory,
            root,
        )),
        Err(error) => Err(DataDirError::new(
            path,
            DataDirOperation::Metadata,
            DataDirCause::Io(error),
            root,
        )),
    }
}

fn ensure_directory(path: &Path) -> Result<(), DataDirError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_directory(path, false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match DirBuilder::new().mode(OWNED_DIRECTORY_MODE).create(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    require_directory(path, false)
                }
                Err(error) => Err(DataDirError::new(
                    path,
                    DataDirOperation::Mkdir,
                    DataDirCause::Io(error),
                    false,
                )),
            }
        }
        Err(error) => Err(DataDirError::new(
            path,
            DataDirOperation::Metadata,
            DataDirCause::Io(error),
            false,
        )),
    }
}

fn probe(dir: &Path, probe_id: &str, root: bool) -> Result<(), DataDirError> {
    let path = dir.join(format!("{PROBE_PREFIX}{probe_id}"));
    let failed = |operation, cause| DataDirError::new(dir, operation, cause, root);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(PROBE_FILE_MODE)
        .open(&path)
        .map_err(|error| failed(DataDirOperation::Create, DataDirCause::Io(error)))?;

    let exercised = exercise(file);
    let removed = fs::remove_file(&path);
    exercised.map_err(|(operation, cause)| failed(operation, cause))?;
    removed.map_err(|error| failed(DataDirOperation::Remove, DataDirCause::Io(error)))
}

fn exercise(mut file: File) -> Result<(), (DataDirOperation, DataDirCause)> {
    let io = |operation| move |error| (operation, DataDirCause::Io(error));
    file.write_all(PROBE_PAYLOAD)
        .map_err(io(DataDirOperation::Write))?;
    file.sync_all().map_err(io(DataDirOperation::Fsync))?;
    file.seek(SeekFrom::Start(0))
        .map_err(io(DataDirOperation::Read))?;
    let mut read_back = [0_u8; PROBE_PAYLOAD.len()];
    file.read_exact(&mut read_back)
        .map_err(io(DataDirOperation::Read))?;
    if read_back != PROBE_PAYLOAD {
        return Err((DataDirOperation::Read, DataDirCause::ReadBackMismatch));
    }
    Ok(())
}

// The process id alone is not unique across restarts: in a container Palmr is
// always PID 1, so a probe left behind by a crash would collide on every later
// start and the exclusive create would fail forever.
fn probe_id() -> io::Result<String> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let hex: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{hex}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use std::fs::{self, Permissions};
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;
    use std::sync::Mutex;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::{
        apply_process_umask, DataDir, DataDirCause, DataDirOperation, ProcessIdentity,
        OWNED_DIRECTORIES, PROBE_PREFIX, PROCESS_UMASK, STARTUP_DATA_DIR_NOT_WRITABLE,
    };

    fn is_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    fn tree(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                found.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
                if fs::symlink_metadata(&path).unwrap().is_dir() {
                    pending.push(path);
                }
            }
        }
        found.sort();
        found
    }

    fn owned_tree() -> Vec<String> {
        let mut expected: Vec<String> = OWNED_DIRECTORIES.iter().map(|d| (*d).to_owned()).collect();
        expected.sort();
        expected
    }

    #[test]
    fn unit_data_dir_creates_owned_tree_and_leaves_no_probe() {
        let root = TempDir::new().unwrap();

        let data_dir = DataDir::prepare(root.path()).unwrap();

        assert_eq!(data_dir.root(), root.path());
        assert_eq!(tree(root.path()), owned_tree());
        assert_eq!(data_dir.cross_device(), None);
        for relative in OWNED_DIRECTORIES {
            let metadata = fs::metadata(root.path().join(relative)).unwrap();
            assert!(metadata.is_dir());
            assert_eq!(metadata.mode() & 0o7027, 0, "{relative}");
        }
    }

    #[test]
    fn unit_data_dir_keeps_existing_content_untouched() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("uploads/0192")).unwrap();
        fs::write(root.path().join("uploads/0192/blob"), b"staged").unwrap();
        fs::write(root.path().join("palmr.db"), b"db").unwrap();
        fs::write(root.path().join(".palmr-write-test-1"), b"stale").unwrap();
        fs::create_dir(root.path().join("minio-data")).unwrap();

        DataDir::prepare(root.path()).unwrap();
        DataDir::prepare(root.path()).unwrap();

        let mut expected = owned_tree();
        expected.extend(
            [
                ".palmr-write-test-1",
                "minio-data",
                "palmr.db",
                "uploads/0192",
                "uploads/0192/blob",
            ]
            .map(str::to_owned),
        );
        expected.sort();
        assert_eq!(tree(root.path()), expected);
        assert_eq!(
            fs::metadata(root.path().join("uploads/0192/blob"))
                .unwrap()
                .len(),
            6
        );
    }

    #[test]
    fn unit_data_dir_missing_root_is_not_created() {
        let parent = TempDir::new().unwrap();
        let root = parent.path().join("data");

        let error = DataDir::prepare(&root).unwrap_err();

        assert_eq!(error.code(), STARTUP_DATA_DIR_NOT_WRITABLE);
        assert_eq!(error.operation, DataDirOperation::Metadata);
        assert_eq!(error.path, root);
        assert!(matches!(&error.cause, DataDirCause::Io(e) if e.kind() == io::ErrorKind::NotFound));
        assert!(error.to_string().contains("not mounted"));
        assert!(error.to_string().contains("; parent owner uid="));
        assert!(!root.exists());
        assert!(tree(parent.path()).is_empty());
    }

    #[test]
    fn unit_data_dir_root_file_is_rejected() {
        let parent = TempDir::new().unwrap();
        let root = parent.path().join("data");
        fs::write(&root, b"").unwrap();

        let error = DataDir::prepare(&root).unwrap_err();

        assert_eq!(error.operation, DataDirOperation::Metadata);
        assert!(matches!(error.cause, DataDirCause::NotADirectory));
        assert_eq!(tree(parent.path()), ["data"]);
    }

    #[test]
    fn unit_data_dir_owned_path_that_is_a_file_is_rejected() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("thumbnails"), b"not a directory").unwrap();

        let error = DataDir::prepare(root.path()).unwrap_err();

        assert_eq!(error.path, root.path().join("thumbnails"));
        assert!(matches!(error.cause, DataDirCause::NotADirectory));
        assert_eq!(
            tree(root.path()),
            ["storage", "storage/objects", "thumbnails", "uploads"]
        );
    }

    #[test]
    fn unit_data_dir_operator_symlinked_directory_is_accepted() {
        let root = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join("branding")).unwrap();

        DataDir::prepare(root.path()).unwrap();

        assert!(fs::symlink_metadata(root.path().join("branding"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(tree(elsewhere.path()).is_empty());
    }

    #[test]
    fn it_startup_readonly_data_dir_fails_with_code() {
        if is_root() {
            eprintln!("skipped: permission bits do not restrict the root user");
            return;
        }
        let root = TempDir::new().unwrap();
        fs::set_permissions(root.path(), Permissions::from_mode(0o500)).unwrap();

        let error = DataDir::prepare(root.path()).unwrap_err();
        fs::set_permissions(root.path(), Permissions::from_mode(0o700)).unwrap();

        assert_eq!(error.code(), STARTUP_DATA_DIR_NOT_WRITABLE);
        assert_eq!(error.path, root.path());
        assert_eq!(error.operation, DataDirOperation::Create);
        assert!(
            matches!(&error.cause, DataDirCause::Io(e) if e.kind() == io::ErrorKind::PermissionDenied)
        );
        assert_eq!(error.identity, ProcessIdentity::effective());
        assert_eq!(error.observed.map(|observed| observed.mode), Some(0o500));
        let text = error.to_string();
        assert!(text.starts_with("STARTUP_DATA_DIR_NOT_WRITABLE: "));
        assert!(text.contains("operation: create"));
        assert!(text.contains(&format!(
            "running as uid={} gid={}",
            error.identity.uid, error.identity.gid
        )));
        assert!(text.contains("; path owner uid="));
        assert!(text.contains("mode 0500"));
        assert!(text.contains("never changes ownership"));
        assert!(tree(root.path()).is_empty());
    }

    #[rstest]
    #[case::objects("storage/objects")]
    #[case::uploads("uploads")]
    #[case::thumbnails("thumbnails")]
    #[case::branding("branding")]
    #[case::runtime("runtime")]
    #[case::backup("backup")]
    fn unit_data_dir_readonly_owned_directory_is_named(#[case] relative: &str) {
        if is_root() {
            eprintln!("skipped: permission bits do not restrict the root user");
            return;
        }
        let root = TempDir::new().unwrap();
        for dir in OWNED_DIRECTORIES {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        let blocked = root.path().join(relative);
        fs::set_permissions(&blocked, Permissions::from_mode(0o555)).unwrap();

        let error = DataDir::prepare(root.path()).unwrap_err();
        fs::set_permissions(&blocked, Permissions::from_mode(0o755)).unwrap();

        assert_eq!(error.code(), STARTUP_DATA_DIR_NOT_WRITABLE);
        assert_eq!(error.path, blocked);
        assert_eq!(error.operation, DataDirOperation::Create);
        assert!(!tree(root.path())
            .iter()
            .any(|entry| entry.contains(PROBE_PREFIX)));
    }

    #[test]
    fn it_startup_exdev_warning() {
        let root = TempDir::new().unwrap();
        let queried = Mutex::new(Vec::new());

        let data_dir = DataDir::prepare_with(root.path(), |path| {
            queried.lock().unwrap().push(path.to_path_buf());
            Ok(if path.ends_with("uploads") {
                0x0801
            } else {
                0x0900
            })
        })
        .unwrap();

        let cross = data_dir.cross_device().unwrap();
        assert_eq!(cross.uploads_device, 0x0801);
        assert_eq!(cross.objects_device, 0x0900);
        assert_eq!(
            *queried.lock().unwrap(),
            [
                root.path().join("uploads"),
                root.path().join("storage/objects")
            ]
        );
        assert_eq!(tree(root.path()), owned_tree());

        let same = DataDir::prepare_with(root.path(), |_| Ok(7)).unwrap();
        assert_eq!(same.cross_device(), None);
    }

    #[test]
    fn unit_data_dir_device_lookup_failure_names_metadata() {
        let root = TempDir::new().unwrap();

        let error = DataDir::prepare_with(root.path(), |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();

        assert_eq!(error.operation, DataDirOperation::Metadata);
        assert_eq!(error.path, root.path().join("uploads"));
    }

    #[test]
    fn unit_process_umask_is_restrictive() {
        let previous = apply_process_umask();
        let current = rustix::process::umask(rustix::fs::Mode::from_bits_truncate(
            rustix::fs::RawMode::try_from(previous).unwrap(),
        ));

        assert_eq!(current.as_raw_mode(), PROCESS_UMASK);
        assert_eq!(PROCESS_UMASK & 0o007, 0o007);
    }
}
