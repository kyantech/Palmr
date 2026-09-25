use std::fmt;
use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use uuid::Uuid;

use super::{classify, sync_directory, FsOps, Step, DIRECTORY_MODE};
use crate::storage::error::StorageError;
use crate::storage::health::{ProbeKey, PROBE_DIRS};
use crate::storage::key::{InvalidKey, KeyNamespace, ObjectKey};

const BRANDING_PREFIX: &str = "branding/";

pub(super) const STAGING_BLOB: &str = "blob";
pub(super) const STAGING_META: &str = "meta.json";
pub(super) const TEMP_PREFIX: &str = ".tmp-";

const UPLOAD_ID_LEN: usize = 32;
const MAX_KEY_DEPTH: usize = 3;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct UploadId(String);

impl UploadId {
    pub fn parse(text: &str) -> Result<Self, InvalidKey> {
        let bytes = text.as_bytes();
        let hex = bytes.len() == UPLOAD_ID_LEN
            && bytes
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if hex && bytes[12] == b'7' && matches!(bytes[16], b'8' | b'9' | b'a' | b'b') {
            Ok(Self(text.to_owned()))
        } else {
            Err(InvalidKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn generate() -> Self {
        Self(Uuid::now_v7().simple().to_string())
    }

    pub(super) fn temp_name(&self) -> String {
        format!("{TEMP_PREFIX}{}", self.0)
    }
}

impl fmt::Debug for UploadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UploadId").field(&self.0).finish()
    }
}

impl fmt::Display for UploadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Root {
    Storage,
    Branding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObjectLocation<'k> {
    pub root: Root,
    dirs: [&'k str; MAX_KEY_DEPTH],
    depth: usize,
    pub leaf: &'k str,
}

impl<'k> ObjectLocation<'k> {
    pub fn of(key: &'k ObjectKey) -> Self {
        let (root, relative) = match key.namespace() {
            KeyNamespace::Objects => (Root::Storage, key.as_str()),
            KeyNamespace::Branding(_) => (
                Root::Branding,
                key.as_str()
                    .strip_prefix(BRANDING_PREFIX)
                    .unwrap_or_default(),
            ),
        };
        let mut segments = relative.split('/');
        let leaf = segments.next_back().unwrap_or_default();
        let mut dirs = [""; MAX_KEY_DEPTH];
        let mut depth = 0;
        for (slot, segment) in dirs.iter_mut().zip(segments) {
            *slot = segment;
            depth += 1;
        }
        Self {
            root,
            dirs,
            depth,
            leaf,
        }
    }

    pub fn probe(key: &'k ProbeKey) -> Self {
        let mut dirs = [""; MAX_KEY_DEPTH];
        for (slot, segment) in dirs.iter_mut().zip(PROBE_DIRS) {
            *slot = segment;
        }
        Self {
            root: Root::Storage,
            dirs,
            depth: PROBE_DIRS.len(),
            leaf: key.oid(),
        }
    }

    pub fn dirs(&self) -> &[&'k str] {
        &self.dirs[..self.depth]
    }
}

pub(super) fn open_dir(parent: BorrowedFd<'_>, name: &str) -> Result<OwnedFd, StorageError> {
    rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| classify(errno, name))
}

pub(super) fn open_leaf_dir(
    root: BorrowedFd<'_>,
    location: &ObjectLocation<'_>,
) -> Result<OwnedFd, StorageError> {
    let mut current: Option<OwnedFd> = None;
    for name in location.dirs() {
        let parent = current.as_ref().map_or(root, AsFd::as_fd);
        current = Some(open_dir(parent, name)?);
    }
    current.ok_or(StorageError::InvalidKey)
}

pub(super) fn temp_name(leaf: &str) -> String {
    format!("{TEMP_PREFIX}{leaf}")
}

pub(super) fn ensure_dir(
    ops: &dyn FsOps,
    parent: BorrowedFd<'_>,
    name: &str,
) -> Result<OwnedFd, StorageError> {
    let created = match rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE)) {
        Ok(()) => true,
        Err(Errno::EXIST) => false,
        Err(errno) => return Err(classify(errno, name)),
    };
    let dir = open_dir(parent, name)?;
    if created {
        sync_directory(ops, parent, Step::SyncCreatedDir)?;
    }
    Ok(dir)
}

pub(super) fn ensure_leaf_dir(
    ops: &dyn FsOps,
    root: BorrowedFd<'_>,
    location: &ObjectLocation<'_>,
) -> Result<OwnedFd, StorageError> {
    let mut current: Option<OwnedFd> = None;
    for name in location.dirs() {
        let parent = current.as_ref().map_or(root, AsFd::as_fd);
        current = Some(ensure_dir(ops, parent, name)?);
    }
    current.ok_or(StorageError::InvalidKey)
}

pub(super) fn open_regular(
    dir: BorrowedFd<'_>,
    name: &str,
    flags: OFlags,
    mode: Mode,
) -> Result<File, StorageError> {
    let fd = rustix::fs::openat(
        dir,
        name,
        flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        mode,
    )
    .map_err(|errno| classify(errno, name))?;
    let file = File::from(fd);
    let is_file = file.metadata().map_err(StorageError::from)?.is_file();
    if is_file {
        Ok(file)
    } else {
        tracing::error!(
            storage_error = "not_regular_file",
            entry = name,
            "a storage entry is not a regular file"
        );
        Err(StorageError::PermissionDenied)
    }
}
