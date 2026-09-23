use std::time::Duration;

pub fn transfer_deadline(size_bytes: u64) -> Duration {
    Duration::from_secs(60 + size_bytes / 1_048_576)
}

pub fn patch_timeout(upload_length: u64) -> Duration {
    let timeout_secs = upload_length / 100_000;
    Duration::from_secs(timeout_secs)
}
