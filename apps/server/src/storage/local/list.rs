use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{AtFlags, Dir, FileType};
use time::OffsetDateTime;

use super::paths::open_dir;
use super::{modified_at, LocalProvider};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{ListCursor, ListEntry, ListPage, MAX_LIST_PAGE_SIZE};

const OBJECTS_PREFIX: &str = "objects/";
const SHARD_LEN: usize = 2;
const OID_LEN: usize = 32;

impl LocalProvider {
    pub fn list_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        let limit = usize::try_from(page_size.min(MAX_LIST_PAGE_SIZE)).unwrap_or(0);
        if limit == 0 {
            return Ok(ListPage {
                entries: Vec::new(),
                next: None,
            });
        }
        let start = cursor
            .as_ref()
            .map(|cursor| Cursor::parse(cursor.as_str()))
            .transpose()?;

        let root = self.objects.as_fd();
        let mut entries = Vec::with_capacity(limit.saturating_add(1));
        'outer: for ab in sorted_shards(root, SHARD_LEN)? {
            if start
                .as_ref()
                .is_some_and(|start| ab.as_str() < start.ab.as_str())
            {
                continue;
            }
            let Some(ab_fd) = open_child(root, &ab) else {
                continue;
            };
            for cd in sorted_shards(ab_fd.as_fd(), SHARD_LEN)? {
                if start
                    .as_ref()
                    .is_some_and(|start| ab == start.ab && cd.as_str() < start.cd.as_str())
                {
                    continue;
                }
                let after = match &start {
                    Some(start) if ab == start.ab && cd == start.cd => Some(start.oid.as_str()),
                    _ => None,
                };
                let Some(cd_fd) = open_child(ab_fd.as_fd(), &cd) else {
                    continue;
                };
                let remaining = limit.saturating_add(1).saturating_sub(entries.len());
                for (oid, size, modified_at) in leaf_entries(cd_fd.as_fd(), after, remaining)? {
                    let text = format!("{OBJECTS_PREFIX}{ab}/{cd}/{oid}");
                    let Ok(key) = ObjectKey::parse(&text) else {
                        continue;
                    };
                    if !key.as_str().starts_with(prefix) {
                        continue;
                    }
                    entries.push(ListEntry {
                        key: key.as_str().to_owned(),
                        size,
                        modified_at,
                    });
                }
                if entries.len() > limit {
                    break 'outer;
                }
            }
        }

        let next = if entries.len() > limit {
            entries.truncate(limit);
            entries
                .last()
                .map(|entry| ListCursor::new(cursor_token(&entry.key)))
        } else {
            None
        };
        Ok(ListPage { entries, next })
    }
}

#[derive(Debug)]
struct Cursor {
    ab: String,
    cd: String,
    oid: String,
}

impl Cursor {
    fn parse(text: &str) -> Result<Self, StorageError> {
        let mut parts = text.split('/');
        let (Some(ab), Some(cd), Some(oid), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(StorageError::InvalidKey);
        };
        if is_hex(ab, SHARD_LEN) && is_hex(cd, SHARD_LEN) && is_hex(oid, OID_LEN) {
            Ok(Self {
                ab: ab.to_owned(),
                cd: cd.to_owned(),
                oid: oid.to_owned(),
            })
        } else {
            Err(StorageError::InvalidKey)
        }
    }
}

fn cursor_token(key: &str) -> String {
    key.strip_prefix(OBJECTS_PREFIX).unwrap_or(key).to_owned()
}

fn storage_error(errno: rustix::io::Errno) -> StorageError {
    StorageError::from(std::io::Error::from(errno))
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(is_hex_digit)
}

fn is_hex_digit(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'f')
}

fn sorted_shards(dir: BorrowedFd<'_>, len: usize) -> Result<Vec<String>, StorageError> {
    let mut names = Vec::new();
    let mut reader = Dir::read_from(dir).map_err(storage_error)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(storage_error)?;
        let bytes = entry.file_name().to_bytes();
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        if !is_hex(name, len) {
            continue;
        }
        let Ok(stat) = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) else {
            continue;
        };
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn open_child(parent: BorrowedFd<'_>, name: &str) -> Option<OwnedFd> {
    open_dir(parent, name).ok()
}

fn leaf_entries(
    dir: BorrowedFd<'_>,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, u64, OffsetDateTime)>, StorageError> {
    let mut selected: BTreeMap<String, (u64, OffsetDateTime)> = BTreeMap::new();
    let mut reader = Dir::read_from(dir).map_err(storage_error)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(storage_error)?;
        let bytes = entry.file_name().to_bytes();
        if !is_hex_bytes(bytes, OID_LEN) {
            continue;
        }
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        if after.is_some_and(|after| name <= after) {
            continue;
        }
        let Ok(stat) = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) else {
            continue;
        };
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            continue;
        }
        if selected.len() >= limit
            && selected
                .keys()
                .next_back()
                .is_some_and(|greatest| name >= greatest.as_str())
        {
            continue;
        }
        selected.insert(
            name.to_owned(),
            (
                u64::try_from(stat.st_size).unwrap_or(0),
                modified_at(stat.st_mtime, stat.st_mtime_nsec),
            ),
        );
        if selected.len() > limit {
            selected.pop_last();
        }
    }
    Ok(selected
        .into_iter()
        .map(|(name, (size, modified_at))| (name, size, modified_at))
        .collect())
}

fn is_hex_bytes(bytes: &[u8], len: usize) -> bool {
    bytes.len() == len && bytes.iter().copied().all(is_hex_digit)
}
