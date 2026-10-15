#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub scanned: u64,
    pub matched: u64,
    pub expired: u64,
    pub abandoned: u64,
    pub aborted: u64,
    pub provider_missing: u64,
    pub provider_orphans: u64,
    pub provider_orphans_too_young: u64,
    pub queued: u64,
    pub retried: u32,
    pub skipped_live: u64,
    pub skipped_completing: u64,
    pub listing_unsupported: bool,
    pub listing_incomplete: bool,
    pub failed: u64,
    pub pruned: u64,
    pub reconciled: u64,
}

impl Summary {
    pub(super) fn log(&self, job: &'static str, continuation: u32) {
        tracing::info!(
            job,
            continuation,
            scanned = self.scanned,
            matched = self.matched,
            expired = self.expired,
            abandoned = self.abandoned,
            aborted = self.aborted,
            provider_missing = self.provider_missing,
            provider_orphans = self.provider_orphans,
            provider_orphans_too_young = self.provider_orphans_too_young,
            queued = self.queued,
            retried = self.retried,
            skipped_live = self.skipped_live,
            skipped_completing = self.skipped_completing,
            listing_unsupported = self.listing_unsupported,
            listing_incomplete = self.listing_incomplete,
            failed = self.failed,
            pruned = self.pruned,
            reconciled = self.reconciled,
            "s3 multipart reaper pass finished"
        );
    }
}
