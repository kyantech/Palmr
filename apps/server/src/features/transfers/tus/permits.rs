use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

use super::error::TusError;

#[derive(Clone)]
pub struct PatchPermits {
    semaphore: Arc<Semaphore>,
    wait: Duration,
}

impl PatchPermits {
    pub fn new(capacity: usize, wait: Duration) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            wait,
        }
    }

    pub async fn acquire(&self) -> Result<OwnedSemaphorePermit, TusError> {
        match timeout(self.wait, Arc::clone(&self.semaphore).acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_closed)) => Err(TusError::ShuttingDown),
            Err(_elapsed) => Err(TusError::Backpressure {
                retry_after: self.wait.as_secs().max(1),
            }),
        }
    }

    #[cfg(test)]
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::PatchPermits;
    use crate::features::transfers::tus::error::TusError;

    #[tokio::test]
    async fn unit_patch_permits_are_bounded_and_returned() {
        let permits = PatchPermits::new(2, Duration::from_millis(30));
        let first = permits.acquire().await.unwrap();
        let second = permits.acquire().await.unwrap();
        assert_eq!(permits.available(), 0);
        match permits.acquire().await {
            Err(TusError::Backpressure { retry_after }) => assert!(retry_after >= 1),
            other => panic!("expected backpressure, got {other:?}"),
        }
        assert_eq!(permits.available(), 0, "a rejected wait takes nothing");
        drop(first);
        assert_eq!(permits.available(), 1);
        let third = permits.acquire().await.unwrap();
        drop((second, third));
        assert_eq!(permits.available(), 2);
    }

    #[tokio::test]
    async fn unit_patch_permit_wait_is_bounded_not_indefinite() {
        let permits = PatchPermits::new(1, Duration::from_millis(50));
        let _held = permits.acquire().await.unwrap();
        let started = tokio::time::Instant::now();
        assert!(permits.acquire().await.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
