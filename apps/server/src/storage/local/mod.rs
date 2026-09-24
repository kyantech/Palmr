mod capacity;
mod copy;
mod delete;
mod finalize;
mod list;
mod paths;
mod read;
mod write;

use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode};
use rustix::io::Errno;
use time::OffsetDateTime;

pub use self::capacity::is_low_space;
pub use self::finalize::{FinalizeRoute, Finalized};
pub use self::paths::UploadId;
use self::paths::{open_dir, open_leaf_dir, ObjectLocation, Root};
pub use self::write::StagingWriter;
use super::error::StorageError;
use super::provider::{Capacity, ObjectStat};

const STORAGE_DIR: &str = "storage";
const OBJECTS_DIR: &str = "objects";
const UPLOADS_DIR: &str = "uploads";
const BRANDING_DIR: &str = "branding";

const DIRECTORY_MODE: RawMode = 0o750;
const FILE_MODE: RawMode = 0o640;

const REFLINK_UNKNOWN: u8 = 0;
const REFLINK_SUPPORTED: u8 = 1;
const REFLINK_UNSUPPORTED: u8 = 2;

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
    UnlinkObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(super) enum CloneStep {
    Cloned,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CopyRangeStep {
    Copied(usize),
    Unsupported,
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

    fn seek(&self, file: &File, pos: u64) -> io::Result<u64>;

    fn filesystem_stats(&self, dir: BorrowedFd<'_>) -> io::Result<Capacity>;

    fn try_reflink(&self, dst: &File, src: &File) -> io::Result<CloneStep>;

    fn copy_range_step(
        &self,
        src: &File,
        src_offset: u64,
        dst: &File,
        dst_offset: u64,
        len: usize,
    ) -> io::Result<CopyRangeStep>;
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

    fn seek(&self, file: &File, pos: u64) -> io::Result<u64> {
        rustix::fs::seek(file.as_fd(), rustix::fs::SeekFrom::Start(pos)).map_err(io::Error::from)
    }

    fn filesystem_stats(&self, dir: BorrowedFd<'_>) -> io::Result<Capacity> {
        let stats = rustix::fs::fstatvfs(dir).map_err(io::Error::from)?;
        let unit = if stats.f_frsize == 0 {
            stats.f_bsize
        } else {
            stats.f_frsize
        };
        let total_bytes = stats.f_blocks.checked_mul(unit).ok_or_else(|| {
            io::Error::other("the filesystem block count does not fit in 64 bits")
        })?;
        let available_bytes = stats.f_bavail.checked_mul(unit).ok_or_else(|| {
            io::Error::other("the filesystem available block count does not fit in 64 bits")
        })?;
        Ok(Capacity {
            total_bytes,
            available_bytes,
        })
    }

    fn try_reflink(&self, dst: &File, src: &File) -> io::Result<CloneStep> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            match rustix::fs::ioctl_ficlone(dst.as_fd(), src.as_fd()) {
                Ok(()) => Ok(CloneStep::Cloned),
                Err(errno) if optimization_unsupported(errno) => Ok(CloneStep::Unsupported),
                Err(errno) => Err(io::Error::from(errno)),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let _ = (dst, src);
            Ok(CloneStep::Unsupported)
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
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let mut src_offset = src_offset;
            let mut dst_offset = dst_offset;
            match rustix::fs::copy_file_range(
                src.as_fd(),
                Some(&mut src_offset),
                dst.as_fd(),
                Some(&mut dst_offset),
                len,
            ) {
                Ok(copied) => Ok(CopyRangeStep::Copied(copied)),
                Err(errno) if optimization_unsupported(errno) => Ok(CopyRangeStep::Unsupported),
                Err(errno) => Err(io::Error::from(errno)),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let _ = (src, src_offset, dst, dst_offset, len);
            Ok(CopyRangeStep::Unsupported)
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn optimization_unsupported(errno: Errno) -> bool {
    [
        Errno::NOTTY,
        Errno::NOSYS,
        Errno::NOTSUP,
        Errno::OPNOTSUPP,
        Errno::INVAL,
        Errno::XDEV,
    ]
    .contains(&errno)
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

struct Destination<'a> {
    leaf_dir: BorrowedFd<'a>,
    name: &'a str,
}

pub struct LocalProvider {
    storage: OwnedFd,
    objects: OwnedFd,
    uploads: OwnedFd,
    branding: OwnedFd,
    buffer_bytes: usize,
    reflink: AtomicU8,
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
        let storage = open_root(data_dir, STORAGE_DIR)?;
        let objects = open_dir(storage.as_fd(), OBJECTS_DIR)?;
        Ok(Self {
            storage,
            objects,
            uploads: open_root(data_dir, UPLOADS_DIR)?,
            branding: open_root(data_dir, BRANDING_DIR)?,
            buffer_bytes,
            reflink: AtomicU8::new(REFLINK_UNKNOWN),
            ops,
        })
    }

    fn root(&self, root: Root) -> BorrowedFd<'_> {
        match root {
            Root::Storage => self.storage.as_fd(),
            Root::Branding => self.branding.as_fd(),
        }
    }

    fn reflink_state(&self) -> u8 {
        self.reflink.load(Ordering::Relaxed)
    }

    fn set_reflink_state(&self, state: u8) {
        self.reflink.store(state, Ordering::Relaxed);
    }

    fn leaf_dir(&self, location: &ObjectLocation<'_>) -> Result<OwnedFd, StorageError> {
        open_leaf_dir(self.root(location.root), location)
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

fn refuse_existing(destination: &Destination<'_>) -> Result<(), StorageError> {
    match rustix::fs::statat(
        destination.leaf_dir,
        destination.name,
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Err(Errno::NOENT) => Ok(()),
        Err(errno) => Err(classify(errno, destination.name)),
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink => {
            Err(classify(Errno::LOOP, destination.name))
        }
        Ok(_) => {
            tracing::error!(
                storage_error = "already_exists",
                "a storage target already holds an object; objects are never overwritten"
            );
            Err(StorageError::AlreadyExists)
        }
    }
}

fn confirm_placed(placed: &File, destination: &Destination<'_>) -> Result<(), StorageError> {
    let expected = rustix::fs::fstat(placed).map_err(|errno| classify(errno, destination.name))?;
    let found = rustix::fs::statat(
        destination.leaf_dir,
        destination.name,
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|errno| classify(errno, destination.name))?;
    if found.st_dev == expected.st_dev && found.st_ino == expected.st_ino {
        Ok(())
    } else {
        tracing::error!(
            storage_error = "placement_mismatch",
            "the entry at the storage target is not the file that was placed"
        );
        Err(StorageError::PermissionDenied)
    }
}

fn measure(file: &File) -> Result<ObjectStat, StorageError> {
    let metadata = file.metadata()?;
    Ok(ObjectStat {
        size: metadata.len(),
        modified_at: modified_at(metadata.mtime(), metadata.mtime_nsec()),
        etag: None,
    })
}

fn modified_at(seconds: i64, nanos: i64) -> OffsetDateTime {
    let nanos = i128::from(seconds) * 1_000_000_000 + i128::from(nanos);
    OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests;
