use std::time::Duration;

pub const TUS_PATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const S3_PART_SIZE_BYTES: u64 = 64 * 1024 * 1024;

pub struct PartPlan {
    pub part_size: u64,
    pub part_count: u64,
}

pub fn plan_parts(size_bytes: u64) -> PartPlan {
    let part_count = size_bytes.div_ceil(S3_PART_SIZE_BYTES).max(1);
    PartPlan { part_size: S3_PART_SIZE_BYTES, part_count }
}

pub fn fits_quota(used_bytes: u64, quota_bytes: u64, size_bytes: u64) -> bool {
    used_bytes.saturating_add(size_bytes) <= quota_bytes
}

pub fn progress_percent(offset: u64, upload_length: u64) -> u64 {
    offset.saturating_mul(100) / upload_length.max(1)
}

pub async fn next_frame<F: std::future::Future>(frame: F) -> Option<F::Output> {
    tokio::time::timeout(TUS_PATCH_IDLE_TIMEOUT, frame).await.ok()
}
