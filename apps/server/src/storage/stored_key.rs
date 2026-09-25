use super::health::ProbeKey;
use super::key::ObjectKey;

pub(crate) trait StoredKey: Send + Sync {
    fn stored_key(&self) -> &str;
}

impl StoredKey for ObjectKey {
    fn stored_key(&self) -> &str {
        self.as_str()
    }
}

impl StoredKey for ProbeKey {
    fn stored_key(&self) -> &str {
        self.as_str()
    }
}
