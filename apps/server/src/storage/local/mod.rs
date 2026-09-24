mod finalize;
mod paths;
mod write;

use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::fs::{AtFlags, Mode, OFlags, RawMode};
use rustix::io::Errno;

pub use self::finalize::{FinalizeRoute, Finalized};
use self::paths::Root;
pub use self::paths::UploadId;
pub use self::write::StagingWriter;
use super::error::StorageError;

const STORAGE_DIR: &str = "storage";
const UPLOADS_DIR: &str = "uploads";
const BRANDING_DIR: &str = "branding";

const DIRECTORY_MODE: RawMode = 0o750;
const FILE_MODE: RawMode = 0o640;

static DIRECTORY_FSYNC_UNSUPPORTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    SyncStaging,
    SyncCreatedDir,
    RenameStaging,
    WriteTemp,
    SyncTemp,
    RenameTemp,
    SyncLeaf,
    UnlinkTemp,
    UnlinkStaging,
    RemoveStagingDir,
}

trait FsOps: Send + Sync {
    fn fsync(&self, fd: BorrowedFd<'_>, step: Step) -> io::Result<()>;

    fn rename(
        &self,
        from_dir: BorrowedFd<'_>,
        from: &str,
        to_dir: BorrowedFd<'_>,
        to: &str,
        step: Step,
    ) -> io::Result<()>;

    fn write_all(&self, file: &File, chunk: &[u8], step: Step) -> io::Result<()>;

    fn unlink(&self, dir: BorrowedFd<'_>, name: &str, step: Step) -> io::Result<()>;

    fn remove_dir(&self, dir: BorrowedFd<'_>, name: &str, step: Step) -> io::Result<()>;
}

struct SystemOps;

impl FsOps for SystemOps {
    fn fsync(&self, fd: BorrowedFd<'_>, _: Step) -> io::Result<()> {
        rustix::fs::fsync(fd).map_err(io::Error::from)
    }

    fn rename(
        &self,
        from_dir: BorrowedFd<'_>,
        from: &str,
        to_dir: BorrowedFd<'_>,
        to: &str,
        _: Step,
    ) -> io::Result<()> {
        rename_no_replace(from_dir, from, to_dir, to).map_err(io::Error::from)
    }

    fn write_all(&self, mut file: &File, chunk: &[u8], _: Step) -> io::Result<()> {
        file.write_all(chunk)
    }

    fn unlink(&self, dir: BorrowedFd<'_>, name: &str, _: Step) -> io::Result<()> {
        rustix::fs::unlinkat(dir, name, AtFlags::empty()).map_err(io::Error::from)
    }

    fn remove_dir(&self, dir: BorrowedFd<'_>, name: &str, _: Step) -> io::Result<()> {
        rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR).map_err(io::Error::from)
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_no_replace(
    from_dir: BorrowedFd<'_>,
    from: &str,
    to_dir: BorrowedFd<'_>,
    to: &str,
) -> Result<(), Errno> {
    let unsupported = [Errno::INVAL, Errno::NOSYS, Errno::NOTSUP, Errno::OPNOTSUPP];
    match rustix::fs::renameat_with(
        from_dir,
        from,
        to_dir,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Err(errno) if unsupported.contains(&errno) => rename_if_absent(from_dir, from, to_dir, to),
        other => other,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn rename_no_replace(
    from_dir: BorrowedFd<'_>,
    from: &str,
    to_dir: BorrowedFd<'_>,
    to: &str,
) -> Result<(), Errno> {
    rename_if_absent(from_dir, from, to_dir, to)
}

fn rename_if_absent(
    from_dir: BorrowedFd<'_>,
    from: &str,
    to_dir: BorrowedFd<'_>,
    to: &str,
) -> Result<(), Errno> {
    match rustix::fs::statat(to_dir, to, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Err(Errno::EXIST),
        Err(Errno::NOENT) => rustix::fs::renameat(from_dir, from, to_dir, to),
        Err(errno) => Err(errno),
    }
}

pub struct LocalProvider {
    storage: OwnedFd,
    uploads: OwnedFd,
    branding: OwnedFd,
    buffer_bytes: usize,
    ops: Box<dyn FsOps>,
}

impl LocalProvider {
    pub fn open(data_dir: &Path, buffer_bytes: u32) -> Result<Self, StorageError> {
        Self::with_ops(data_dir, buffer_bytes, Box::new(SystemOps))
    }

    fn with_ops(
        data_dir: &Path,
        buffer_bytes: u32,
        ops: Box<dyn FsOps>,
    ) -> Result<Self, StorageError> {
        let buffer_bytes = usize::try_from(buffer_bytes)
            .ok()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| {
                StorageError::Config("the upload buffer must hold at least one byte".to_owned())
            })?;
        Ok(Self {
            storage: open_root(data_dir, STORAGE_DIR)?,
            uploads: open_root(data_dir, UPLOADS_DIR)?,
            branding: open_root(data_dir, BRANDING_DIR)?,
            buffer_bytes,
            ops,
        })
    }

    fn root(&self, root: Root) -> BorrowedFd<'_> {
        match root {
            Root::Storage => self.storage.as_fd(),
            Root::Branding => self.branding.as_fd(),
        }
    }
}

impl std::fmt::Debug for LocalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProvider")
            .field("buffer_bytes", &self.buffer_bytes)
            .finish_non_exhaustive()
    }
}

fn open_root(data_dir: &Path, name: &str) -> Result<OwnedFd, StorageError> {
    let canonical = std::fs::canonicalize(data_dir.join(name))?;
    rustix::fs::open(
        &canonical,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| classify(errno, name))
}

fn classify(errno: Errno, entry: &str) -> StorageError {
    if errno == Errno::LOOP || errno == Errno::NOTDIR {
        tracing::error!(
            storage_error = "unexpected_entry_type",
            entry,
            "a storage path component is a symlink or not a directory; something other than Palmr is writing into the storage tree"
        );
        return StorageError::PermissionDenied;
    }
    StorageError::from(io::Error::from(errno))
}

fn sync_directory(ops: &dyn FsOps, dir: BorrowedFd<'_>, step: Step) -> Result<(), StorageError> {
    match ops.fsync(dir, step) {
        Ok(()) => Ok(()),
        Err(error) if directory_fsync_unsupported(&error) => {
            if !DIRECTORY_FSYNC_UNSUPPORTED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    errno = error.raw_os_error(),
                    "the storage filesystem does not support fsync on directories; rename durability depends on the filesystem"
                );
            }
            Ok(())
        }
        Err(error) => Err(StorageError::from(error)),
    }
}

fn directory_fsync_unsupported(error: &io::Error) -> bool {
    let tolerated = [Errno::INVAL, Errno::NOTSUP, Errno::OPNOTSUPP, Errno::BADF];
    error
        .raw_os_error()
        .is_some_and(|raw| tolerated.contains(&Errno::from_raw_os_error(raw)))
}

fn best_effort(outcome: io::Result<()>, step: Step, tolerated: &[Errno]) {
    if let Err(error) = outcome {
        let expected = error
            .raw_os_error()
            .is_some_and(|raw| tolerated.contains(&Errno::from_raw_os_error(raw)));
        if !expected {
            tracing::warn!(
                step = ?step,
                error_kind = %error.kind(),
                errno = error.raw_os_error(),
                "storage cleanup failed; the leftover entry is reaped later"
            );
        }
    }
}

#[cfg(test)]
mod tests;
