use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use super::repo::TusUploadId;

type Entries = HashMap<TusUploadId, Arc<AsyncMutex<()>>>;

#[derive(Clone, Default)]
pub struct UploadLocks {
    entries: Arc<Mutex<Entries>>,
}

pub struct UploadGuard {
    locks: UploadLocks,
    id: TusUploadId,
    held: Option<OwnedMutexGuard<()>>,
}

struct Sweep<'a> {
    locks: &'a UploadLocks,
    id: TusUploadId,
}

impl Drop for Sweep<'_> {
    fn drop(&mut self) {
        self.locks.sweep(self.id);
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        self.held.take();
        self.locks.sweep(self.id);
    }
}

impl UploadLocks {
    pub fn try_acquire(&self, id: TusUploadId) -> Option<UploadGuard> {
        let _sweep = Sweep { locks: self, id };
        let held = self.entry(id).try_lock_owned().ok()?;
        Some(UploadGuard {
            locks: self.clone(),
            id,
            held: Some(held),
        })
    }

    pub async fn acquire(&self, id: TusUploadId) -> UploadGuard {
        let _sweep = Sweep { locks: self, id };
        let held = self.entry(id).lock_owned().await;
        UploadGuard {
            locks: self.clone(),
            id,
            held: Some(held),
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries().len()
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn entry(&self, id: TusUploadId) -> Arc<AsyncMutex<()>> {
        Arc::clone(self.entries().entry(id).or_default())
    }

    fn sweep(&self, id: TusUploadId) {
        let mut entries = self.entries();
        if entries
            .get(&id)
            .is_some_and(|entry| Arc::strong_count(entry) == 1)
        {
            entries.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{TusUploadId, UploadLocks};
    use crate::domain::clock::TestClock;
    use time::macros::datetime;

    fn ids(count: usize) -> Vec<TusUploadId> {
        let clock = TestClock::new(datetime!(2026-10-01 00:00 UTC));
        (0..count).map(|_| TusUploadId::generate(&clock)).collect()
    }

    #[tokio::test]
    async fn unit_upload_lock_is_exclusive_and_idle_entries_are_forgotten() {
        let locks = UploadLocks::default();
        let ids = ids(2);
        let first = locks.try_acquire(ids[0]).unwrap();
        assert!(locks.try_acquire(ids[0]).is_none());
        assert_eq!(locks.len(), 1, "a held entry survives a failed attempt");
        let second = locks.try_acquire(ids[1]).unwrap();
        assert_eq!(locks.len(), 2);
        drop(first);
        assert_eq!(locks.len(), 1, "only the entry still held remains");
        assert!(locks.try_acquire(ids[0]).is_some());
        drop(second);
        assert_eq!(locks.len(), 0);
    }

    #[tokio::test]
    async fn unit_upload_lock_map_does_not_retain_every_id_ever_seen() {
        let locks = UploadLocks::default();
        for id in ids(2_000) {
            let guard = locks.try_acquire(id).unwrap();
            assert_eq!(locks.len(), 1);
            drop(guard);
        }
        assert_eq!(locks.len(), 0);
    }

    #[tokio::test]
    async fn unit_upload_lock_waiter_gets_the_lock_and_a_cancelled_waiter_leaks_nothing() {
        let locks = UploadLocks::default();
        let id = ids(1)[0];
        let held = locks.try_acquire(id).unwrap();

        let cancelled = tokio::time::timeout(Duration::from_millis(20), locks.acquire(id)).await;
        assert!(cancelled.is_err());
        assert_eq!(locks.len(), 1);

        let waiter = {
            let locks = locks.clone();
            tokio::spawn(async move { locks.acquire(id).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(held);
        let guard = waiter.await.unwrap();
        assert_eq!(locks.len(), 1);
        assert!(locks.try_acquire(id).is_none());
        drop(guard);
        assert_eq!(locks.len(), 0);
    }
}
