use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};

use crate::domain::secret::REDACTED;

pub const STARTUP_INSTANCE_KEY_INVALID: &str = "STARTUP_INSTANCE_KEY_INVALID";
pub const STARTUP_INSTANCE_KEY_UNWRITABLE: &str = "STARTUP_INSTANCE_KEY_UNWRITABLE";

pub const INSTANCE_KEY_FILE: &str = "instance.key";
pub const INSTANCE_KEY_LEN: usize = 32;
pub const INSTANCE_KEY_MODE: u32 = 0o600;

const TEMP_PREFIX: &str = ".instance.key.tmp-";

pub struct InstanceKey([u8; INSTANCE_KEY_LEN]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    Loaded,
    Created,
}

impl InstanceKey {
    pub fn load_or_create(data_dir: &Path) -> Result<(Self, KeyOrigin), InstanceKeyError> {
        let path = data_dir.join(INSTANCE_KEY_FILE);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                Err(InstanceKeyError::invalid(&path, InvalidKey::NotRegularFile))
            }
            Ok(_) => Self::load(&path).map(|key| (key, KeyOrigin::Loaded)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match Self::create(data_dir, &path)? {
                    Some(key) => Ok((key, KeyOrigin::Created)),
                    None => Self::load(&path).map(|key| (key, KeyOrigin::Loaded)),
                }
            }
            Err(error) => Err(InstanceKeyError::invalid(
                &path,
                InvalidKey::Unreadable(error),
            )),
        }
    }

    pub const fn expose_secret(&self) -> &[u8; INSTANCE_KEY_LEN] {
        &self.0
    }

    fn load(path: &Path) -> Result<Self, InstanceKeyError> {
        let unreadable =
            |error: io::Error| InstanceKeyError::invalid(path, InvalidKey::Unreadable(error));
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|errno| unreadable(errno.into()))?;
        let mut file = File::from(fd);
        let metadata = file.metadata().map_err(unreadable)?;

        if !metadata.file_type().is_file() {
            return Err(InstanceKeyError::invalid(path, InvalidKey::NotRegularFile));
        }
        let mode = metadata.mode() & 0o7777;
        if mode != INSTANCE_KEY_MODE {
            return Err(InstanceKeyError::invalid(
                path,
                InvalidKey::InsecureMode { mode },
            ));
        }
        if metadata.len() != INSTANCE_KEY_LEN as u64 {
            return Err(InstanceKeyError::invalid(
                path,
                InvalidKey::WrongLength {
                    actual: metadata.len(),
                },
            ));
        }

        let mut bytes = [0_u8; INSTANCE_KEY_LEN];
        file.read_exact(&mut bytes).map_err(unreadable)?;
        Ok(Self(bytes))
    }

    fn create(dir: &Path, path: &Path) -> Result<Option<Self>, InstanceKeyError> {
        let failed = |operation, error| InstanceKeyError::Unwritable {
            path: path.to_path_buf(),
            operation,
            source: error,
        };

        let mut bytes = [0_u8; INSTANCE_KEY_LEN];
        getrandom::fill(&mut bytes)
            .map_err(|error| failed(KeyOperation::Generate, io::Error::other(error)))?;
        let temp_name = temp_file_name().map_err(|error| failed(KeyOperation::Generate, error))?;

        let mut temp = TempKeyFile::create(dir.join(temp_name))
            .map_err(|error| failed(KeyOperation::Create, error))?;
        temp.file
            .write_all(&bytes)
            .map_err(|error| failed(KeyOperation::Write, error))?;
        temp.file
            .sync_all()
            .map_err(|error| failed(KeyOperation::Fsync, error))?;

        match publish(&temp.path, path).map_err(|error| failed(KeyOperation::Rename, error))? {
            Publish::Placed => {}
            Publish::AlreadyExists => return Ok(None),
        }
        temp.published();
        sync_directory(dir).map_err(|error| failed(KeyOperation::FsyncDirectory, error))?;
        Ok(Some(Self(bytes)))
    }
}

impl fmt::Debug for InstanceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceKey({REDACTED})")
    }
}

struct TempKeyFile {
    path: PathBuf,
    file: File,
    owned: bool,
}

impl TempKeyFile {
    fn create(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(INSTANCE_KEY_MODE)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            owned: true,
        })
    }

    fn published(&mut self) {
        self.owned = false;
    }
}

impl Drop for TempKeyFile {
    fn drop(&mut self) {
        if self.owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

enum Publish {
    Placed,
    AlreadyExists,
}

// The final name must appear atomically with complete contents and must never
// replace a key another process published first, so a plain `rename` (which
// overwrites) is not acceptable here.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn publish(temp: &Path, path: &Path) -> io::Result<Publish> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    use rustix::io::Errno;

    match renameat_with(CWD, temp, CWD, path, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(Publish::Placed),
        Err(Errno::EXIST) => Ok(Publish::AlreadyExists),
        Err(errno)
            if errno == Errno::INVAL
                || errno == Errno::NOSYS
                || errno == Errno::NOTSUP
                || errno == Errno::OPNOTSUPP =>
        {
            link_into_place(temp, path)
        }
        Err(errno) => Err(errno.into()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn publish(temp: &Path, path: &Path) -> io::Result<Publish> {
    link_into_place(temp, path)
}

fn link_into_place(temp: &Path, path: &Path) -> io::Result<Publish> {
    match fs::hard_link(temp, path) {
        Ok(()) => {
            fs::remove_file(temp)?;
            Ok(Publish::Placed)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(Publish::AlreadyExists),
        Err(error) => Err(error),
    }
}

fn sync_directory(dir: &Path) -> io::Result<()> {
    match File::open(dir).and_then(|handle| handle.sync_all()) {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
            ) =>
        {
            tracing::warn!(
                "the data directory filesystem does not support directory fsync; the new instance key is durable only after the filesystem next syncs"
            );
            Ok(())
        }
        other => other,
    }
}

fn temp_file_name() -> io::Result<String> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let hex: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{TEMP_PREFIX}{}-{hex}", std::process::id()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOperation {
    Generate,
    Create,
    Write,
    Fsync,
    Rename,
    FsyncDirectory,
}

impl KeyOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Create => "create",
            Self::Write => "write",
            Self::Fsync => "fsync",
            Self::Rename => "rename",
            Self::FsyncDirectory => "fsync_directory",
        }
    }
}

#[derive(Debug)]
pub enum InvalidKey {
    NotRegularFile,
    InsecureMode { mode: u32 },
    WrongLength { actual: u64 },
    Unreadable(io::Error),
}

#[derive(Debug)]
pub enum InstanceKeyError {
    Invalid {
        path: PathBuf,
        problem: InvalidKey,
    },
    Unwritable {
        path: PathBuf,
        operation: KeyOperation,
        source: io::Error,
    },
}

impl InstanceKeyError {
    fn invalid(path: &Path, problem: InvalidKey) -> Self {
        Self::Invalid {
            path: path.to_path_buf(),
            problem,
        }
    }

    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => STARTUP_INSTANCE_KEY_INVALID,
            Self::Unwritable { .. } => STARTUP_INSTANCE_KEY_UNWRITABLE,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Invalid { path, .. } | Self::Unwritable { path, .. } => path,
        }
    }
}

impl fmt::Display for InstanceKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::Invalid { path, problem } => {
                let path = path.display();
                match problem {
                    InvalidKey::NotRegularFile => write!(
                        f,
                        "{path} is not a regular file; restore the original key file from backup"
                    ),
                    InvalidKey::InsecureMode { mode } => write!(
                        f,
                        "{path} has mode {mode:04o} but must be exactly 0600; run `chmod 600` on it from the host. Palmr never changes the mode of an existing key"
                    ),
                    InvalidKey::WrongLength { actual } => write!(
                        f,
                        "{path} is {actual} bytes but must be exactly {INSTANCE_KEY_LEN}; restore the original key from backup. Palmr never replaces an existing key"
                    ),
                    InvalidKey::Unreadable(source) => {
                        write!(f, "{path} cannot be read: {source}")
                    }
                }
            }
            Self::Unwritable {
                path,
                operation,
                source,
            } => write!(
                f,
                "{} could not be created ({}: {source}); the data directory must be writable by the Palmr process",
                path.display(),
                operation.as_str()
            ),
        }
    }
}

impl std::error::Error for InstanceKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid {
                problem: InvalidKey::Unreadable(source),
                ..
            }
            | Self::Unwritable { source, .. } => Some(source),
            Self::Invalid { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions, Permissions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::{
        InstanceKey, InstanceKeyError, InvalidKey, KeyOperation, KeyOrigin, INSTANCE_KEY_FILE,
        INSTANCE_KEY_LEN, STARTUP_INSTANCE_KEY_INVALID, STARTUP_INSTANCE_KEY_UNWRITABLE,
    };

    const SENTINEL: [u8; INSTANCE_KEY_LEN] = *b"instance-key-sentinel-bytes-0042";

    fn write_key(dir: &Path, bytes: &[u8], mode: u32) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(INSTANCE_KEY_FILE))
            .unwrap();
        file.write_all(bytes).unwrap();
        fs::set_permissions(dir.join(INSTANCE_KEY_FILE), Permissions::from_mode(mode)).unwrap();
    }

    fn key_file(dir: &Path) -> (Vec<u8>, u32) {
        let path = dir.join(INSTANCE_KEY_FILE);
        let metadata = fs::metadata(&path).unwrap();
        let mut bytes = vec![0_u8; usize::try_from(metadata.len()).unwrap()];
        File::open(&path).unwrap().read_exact(&mut bytes).unwrap();
        (bytes, metadata.mode() & 0o7777)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn is_root() -> bool {
        rustix::process::geteuid().is_root()
    }

    #[test]
    fn it_startup_instance_key_created_0600() {
        let dir = TempDir::new().unwrap();

        let (key, origin) = InstanceKey::load_or_create(dir.path()).unwrap();

        assert_eq!(origin, KeyOrigin::Created);
        let (bytes, mode) = key_file(dir.path());
        assert_eq!(mode, 0o600);
        assert_eq!(bytes.len(), INSTANCE_KEY_LEN);
        assert_eq!(bytes, key.expose_secret());
        assert_ne!(key.expose_secret(), &[0_u8; INSTANCE_KEY_LEN]);
        assert!(fs::symlink_metadata(dir.path().join(INSTANCE_KEY_FILE))
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(entries(dir.path()), [INSTANCE_KEY_FILE]);
    }

    #[test]
    fn unit_instance_key_created_keys_differ() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();

        let (a, _) = InstanceKey::load_or_create(first.path()).unwrap();
        let (b, _) = InstanceKey::load_or_create(second.path()).unwrap();

        assert_ne!(a.expose_secret(), b.expose_secret());
    }

    #[test]
    fn unit_instance_key_existing_key_loads_unchanged() {
        let dir = TempDir::new().unwrap();
        write_key(dir.path(), &SENTINEL, 0o600);

        let (key, origin) = InstanceKey::load_or_create(dir.path()).unwrap();

        assert_eq!(origin, KeyOrigin::Loaded);
        assert_eq!(key.expose_secret(), &SENTINEL);
        assert_eq!(key_file(dir.path()), (SENTINEL.to_vec(), 0o600));
    }

    #[test]
    fn unit_instance_key_second_start_reuses_created_key() {
        let dir = TempDir::new().unwrap();

        let (created, _) = InstanceKey::load_or_create(dir.path()).unwrap();
        let (loaded, origin) = InstanceKey::load_or_create(dir.path()).unwrap();

        assert_eq!(origin, KeyOrigin::Loaded);
        assert_eq!(created.expose_secret(), loaded.expose_secret());
    }

    #[rstest]
    #[case::group_readable(0o640)]
    #[case::world_readable(0o644)]
    #[case::world_only(0o604)]
    #[case::group_writable(0o660)]
    #[case::owner_executable(0o700)]
    #[case::read_only(0o400)]
    fn it_startup_instance_key_mode_enforced(#[case] mode: u32) {
        let dir = TempDir::new().unwrap();
        write_key(dir.path(), &SENTINEL, mode);

        let error = InstanceKey::load_or_create(dir.path()).unwrap_err();

        assert_eq!(error.code(), STARTUP_INSTANCE_KEY_INVALID);
        assert!(matches!(
            error,
            InstanceKeyError::Invalid { problem: InvalidKey::InsecureMode { mode: seen }, .. } if seen == mode
        ));
        assert!(error.to_string().contains(&format!("{mode:04o}")));
        assert_eq!(key_file(dir.path()), (SENTINEL.to_vec(), mode));
    }

    #[rstest]
    #[case::empty(0)]
    #[case::truncated(31)]
    #[case::oversized(33)]
    #[case::hex_encoded(64)]
    fn unit_instance_key_wrong_length_is_invalid(#[case] len: usize) {
        let dir = TempDir::new().unwrap();
        let content: Vec<u8> = SENTINEL.iter().copied().cycle().take(len).collect();
        write_key(dir.path(), &content, 0o600);

        let error = InstanceKey::load_or_create(dir.path()).unwrap_err();

        assert_eq!(error.code(), STARTUP_INSTANCE_KEY_INVALID);
        assert!(matches!(
            error,
            InstanceKeyError::Invalid { problem: InvalidKey::WrongLength { actual }, .. } if actual == len as u64
        ));
        assert_eq!(key_file(dir.path()), (content, 0o600));
        assert_eq!(entries(dir.path()), [INSTANCE_KEY_FILE]);
    }

    #[test]
    fn unit_instance_key_symlink_is_not_followed() {
        let dir = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        write_key(target.path(), &SENTINEL, 0o600);
        std::os::unix::fs::symlink(
            target.path().join(INSTANCE_KEY_FILE),
            dir.path().join(INSTANCE_KEY_FILE),
        )
        .unwrap();

        let error = InstanceKey::load_or_create(dir.path()).unwrap_err();

        assert_eq!(error.code(), STARTUP_INSTANCE_KEY_INVALID);
        assert!(matches!(
            error,
            InstanceKeyError::Invalid {
                problem: InvalidKey::NotRegularFile,
                ..
            }
        ));
    }

    #[test]
    fn unit_instance_key_directory_is_invalid() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(INSTANCE_KEY_FILE)).unwrap();

        let error = InstanceKey::load_or_create(dir.path()).unwrap_err();

        assert!(matches!(
            error,
            InstanceKeyError::Invalid {
                problem: InvalidKey::NotRegularFile,
                ..
            }
        ));
        assert!(dir.path().join(INSTANCE_KEY_FILE).is_dir());
    }

    #[test]
    fn unit_instance_key_unwritable_directory_fails() {
        if is_root() {
            eprintln!("skipped: permission bits do not restrict the root user");
            return;
        }
        let dir = TempDir::new().unwrap();
        fs::set_permissions(dir.path(), Permissions::from_mode(0o500)).unwrap();

        let error = InstanceKey::load_or_create(dir.path()).unwrap_err();

        fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
        assert_eq!(error.code(), STARTUP_INSTANCE_KEY_UNWRITABLE);
        assert!(matches!(
            error,
            InstanceKeyError::Unwritable {
                operation: KeyOperation::Create,
                ..
            }
        ));
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn unit_instance_key_concurrent_creation_keeps_one_key() {
        for _ in 0..16 {
            let dir = Arc::new(TempDir::new().unwrap());
            let barrier = Arc::new(Barrier::new(2));
            let attempts: Vec<_> = (0..2)
                .map(|_| {
                    let dir = Arc::clone(&dir);
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        let (key, origin) = InstanceKey::load_or_create(dir.path()).unwrap();
                        (*key.expose_secret(), origin)
                    })
                })
                .collect();
            let results: Vec<_> = attempts
                .into_iter()
                .map(|attempt| attempt.join().unwrap())
                .collect();

            assert_eq!(results[0].0, results[1].0);
            assert_eq!(
                results
                    .iter()
                    .filter(|(_, origin)| *origin == KeyOrigin::Created)
                    .count(),
                1
            );
            assert_eq!(key_file(dir.path()), (results[0].0.to_vec(), 0o600));
            assert_eq!(entries(dir.path()), [INSTANCE_KEY_FILE]);
        }
    }

    #[test]
    fn unit_instance_key_never_formats_key_material() {
        let dir = TempDir::new().unwrap();
        write_key(dir.path(), &SENTINEL, 0o600);
        let (key, _) = InstanceKey::load_or_create(dir.path()).unwrap();

        let sentinel = String::from_utf8(SENTINEL.to_vec()).unwrap();
        let byte_list = format!("{:?}", SENTINEL);
        for text in [format!("{key:?}"), format!("{key:#?}")] {
            assert_eq!(text, "InstanceKey(<redacted>)");
            assert!(!text.contains(&sentinel));
            assert!(!text.contains(&byte_list[1..20]));
        }

        let invalid = TempDir::new().unwrap();
        write_key(invalid.path(), &SENTINEL[..31], 0o644);
        let error = InstanceKey::load_or_create(invalid.path()).unwrap_err();
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(!text.contains(&sentinel[..31]), "{text}");
        }
    }
}
