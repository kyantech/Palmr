use std::os::fd::{AsFd, OwnedFd};

use async_trait::async_trait;
use rustix::fs::{Access, AtFlags, Dir, FileType};

use super::paths::{open_dir, ObjectLocation};
use super::read::stat_regular;
use super::{classify, modified_at, LocalProvider, OBJECTS_DIR};
use crate::storage::error::StorageError;
use crate::storage::health::{
    diagnose, remove_and_verify, skip_removal, sweep_stale, write_and_verify, CheckName,
    ListedProbe, ProbeDepth, ProbeKey, ProbeRun, ProbeStore, SelfTestReport, PROBE_DIRS,
};
use crate::storage::provider::{ObjectBody, ObjectStat};
use crate::storage::ProviderKind;

impl LocalProvider {
    pub(super) async fn run_self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        let mut run = ProbeRun::new(self.clock.as_ref(), ProviderKind::Local, depth);
        run.run(CheckName::Layout, async {
            self.verify_layout().map_err(|error| diagnose(&error))
        })
        .await;
        let key = ProbeKey::generate();
        if write_and_verify(&mut run, self, &key).await {
            remove_and_verify(&mut run, self, &key).await;
        } else {
            skip_removal(&mut run);
        }
        if depth == ProbeDepth::Full {
            sweep_stale(&mut run, self, &[&key]).await;
        }
        run.finish()
    }

    fn verify_layout(&self) -> Result<(), StorageError> {
        let writable = Access::WRITE_OK | Access::EXEC_OK;
        for (dir, name) in [
            (self.storage.as_fd(), OBJECTS_DIR),
            (self.uploads.as_fd(), "."),
        ] {
            rustix::fs::accessat(dir, name, writable, AtFlags::EACCESS)
                .map_err(|errno| classify(errno, name))?;
        }
        Ok(())
    }

    fn probe_dir(&self) -> Result<Option<OwnedFd>, StorageError> {
        let mut current: Option<OwnedFd> = None;
        for name in PROBE_DIRS {
            let parent = current.as_ref().map_or(self.storage.as_fd(), AsFd::as_fd);
            match open_dir(parent, name) {
                Ok(dir) => current = Some(dir),
                Err(StorageError::NotFound) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Ok(current)
    }

    fn listed_probes(&self, limit: u32) -> Result<Vec<ListedProbe>, StorageError> {
        let Some(dir) = self.probe_dir()? else {
            return Ok(Vec::new());
        };
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut listed = Vec::new();
        let mut reader = Dir::read_from(dir.as_fd()).map_err(|errno| classify(errno, "probe"))?;
        while let Some(entry) = reader.read() {
            if listed.len() >= limit {
                break;
            }
            let entry = entry.map_err(|errno| classify(errno, "probe"))?;
            let Ok(name) = std::str::from_utf8(entry.file_name().to_bytes()) else {
                continue;
            };
            let Some(key) = ProbeKey::from_listed_name(name) else {
                continue;
            };
            let Ok(stat) = rustix::fs::statat(dir.as_fd(), name, AtFlags::SYMLINK_NOFOLLOW) else {
                continue;
            };
            if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
                listed.push(ListedProbe {
                    key,
                    modified_at: modified_at(stat.st_mtime, stat.st_mtime_nsec),
                });
            }
        }
        Ok(listed)
    }
}

#[async_trait]
impl ProbeStore for LocalProvider {
    async fn put_probe(
        &self,
        key: &ProbeKey,
        body: ObjectBody,
        len: u64,
    ) -> Result<ObjectStat, StorageError> {
        self.put_at(&ObjectLocation::probe(key), body, Some(len))
            .await
    }

    async fn stat_probe(&self, key: &ProbeKey) -> Result<ObjectStat, StorageError> {
        let location = ObjectLocation::probe(key);
        let dir = self.leaf_dir(&location)?;
        stat_regular(dir.as_fd(), location.leaf)
    }

    async fn read_probe(&self, key: &ProbeKey) -> Result<ObjectBody, StorageError> {
        self.open_read_at(&ObjectLocation::probe(key))
            .map(|(_, body)| body)
    }

    async fn read_probe_range(
        &self,
        key: &ProbeKey,
        start: u64,
        len: u64,
    ) -> Result<ObjectBody, StorageError> {
        self.open_range_at(&ObjectLocation::probe(key), start, len)
            .map(|(_, body)| body)
    }

    async fn delete_probe(&self, key: &ProbeKey) -> Result<(), StorageError> {
        self.delete_at(&ObjectLocation::probe(key)).map(|_| ())
    }

    async fn probe_exists(&self, key: &ProbeKey) -> Result<bool, StorageError> {
        self.exists_at(&ObjectLocation::probe(key))
    }

    async fn list_probes(&self, limit: u32) -> Result<Vec<ListedProbe>, StorageError> {
        self.listed_probes(limit)
    }
}
