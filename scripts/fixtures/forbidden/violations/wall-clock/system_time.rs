use std::time::SystemTime;

pub fn is_expired(expires_at: SystemTime) -> bool {
    SystemTime::now() > expires_at
}
