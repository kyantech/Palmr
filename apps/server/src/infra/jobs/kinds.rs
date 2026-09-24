use std::fmt;
use std::str::FromStr;
use std::time::Duration;

pub const DEFAULT_LEASE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u16);

impl Priority {
    pub const HIGH: Self = Self(50);
    pub const NORMAL: Self = Self(100);
    pub const LOW: Self = Self(200);
    const MAX: u16 = 1_000;

    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindPolicy {
    pub priority: Priority,
    pub max_attempts: u32,
    pub lease: Duration,
}

impl KindPolicy {
    const fn new(priority: Priority, max_attempts: u32) -> Self {
        Self {
            priority,
            max_attempts,
            lease: DEFAULT_LEASE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobKind {
    EmailSend,
    ShareNotifyRecipients,
    ReverseShareNotifyOwner,
    ReceivedRetentionSweep,
    TusExpireStale,
    S3AbortAbandonedMultipart,
    S3ReconcileMultipart,
    StorageDeleteBlob,
    StorageOrphanSweep,
    AuditRetentionSweep,
    SessionsPrune,
    TokensPrune,
    QuotaReconcile,
    AvatarFetchExternal,
    ImageNormalize,
    FoldersDeleteTree,
    ReverseShareDeleteCascade,
    ReceivedCopyToMyFiles,
}

impl JobKind {
    pub const ALL: [Self; 18] = [
        Self::EmailSend,
        Self::ShareNotifyRecipients,
        Self::ReverseShareNotifyOwner,
        Self::ReceivedRetentionSweep,
        Self::TusExpireStale,
        Self::S3AbortAbandonedMultipart,
        Self::S3ReconcileMultipart,
        Self::StorageDeleteBlob,
        Self::StorageOrphanSweep,
        Self::AuditRetentionSweep,
        Self::SessionsPrune,
        Self::TokensPrune,
        Self::QuotaReconcile,
        Self::AvatarFetchExternal,
        Self::ImageNormalize,
        Self::FoldersDeleteTree,
        Self::ReverseShareDeleteCascade,
        Self::ReceivedCopyToMyFiles,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmailSend => "email.send",
            Self::ShareNotifyRecipients => "share.notify_recipients",
            Self::ReverseShareNotifyOwner => "reverse_share.notify_owner",
            Self::ReceivedRetentionSweep => "received.retention_sweep",
            Self::TusExpireStale => "tus.expire_stale",
            Self::S3AbortAbandonedMultipart => "s3.abort_abandoned_multipart",
            Self::S3ReconcileMultipart => "s3.reconcile_multipart",
            Self::StorageDeleteBlob => "storage.delete_blob",
            Self::StorageOrphanSweep => "storage.orphan_sweep",
            Self::AuditRetentionSweep => "audit.retention_sweep",
            Self::SessionsPrune => "sessions.prune",
            Self::TokensPrune => "tokens.prune",
            Self::QuotaReconcile => "quota.reconcile",
            Self::AvatarFetchExternal => "avatar.fetch_external",
            Self::ImageNormalize => "image.normalize",
            Self::FoldersDeleteTree => "folders.delete_tree",
            Self::ReverseShareDeleteCascade => "reverse_share.delete_cascade",
            Self::ReceivedCopyToMyFiles => "received.copy_to_my_files",
        }
    }

    pub const fn policy(self) -> KindPolicy {
        match self {
            Self::EmailSend | Self::ShareNotifyRecipients => KindPolicy::new(Priority::HIGH, 10),
            Self::ReverseShareNotifyOwner => KindPolicy::new(Priority::NORMAL, 10),
            Self::ReceivedRetentionSweep | Self::TusExpireStale => {
                KindPolicy::new(Priority::LOW, 5)
            }
            Self::S3AbortAbandonedMultipart | Self::S3ReconcileMultipart => {
                KindPolicy::new(Priority::NORMAL, 8)
            }
            Self::StorageDeleteBlob
            | Self::FoldersDeleteTree
            | Self::ReverseShareDeleteCascade
            | Self::ReceivedCopyToMyFiles => KindPolicy::new(Priority::NORMAL, 12),
            Self::StorageOrphanSweep
            | Self::AuditRetentionSweep
            | Self::SessionsPrune
            | Self::TokensPrune
            | Self::QuotaReconcile
            | Self::AvatarFetchExternal => KindPolicy::new(Priority::LOW, 3),
            Self::ImageNormalize => KindPolicy::new(Priority::NORMAL, 5),
        }
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownJobKind;

impl fmt::Display for UnknownJobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("job kind is not one of the v4.0 job kinds")
    }
}

impl std::error::Error for UnknownJobKind {}

impl FromStr for JobKind {
    type Err = UnknownJobKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or(UnknownJobKind)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{JobKind, Priority, UnknownJobKind, DEFAULT_LEASE};

    #[test]
    fn unit_job_kinds_closed_set() {
        let names: Vec<&str> = JobKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            names,
            [
                "email.send",
                "share.notify_recipients",
                "reverse_share.notify_owner",
                "received.retention_sweep",
                "tus.expire_stale",
                "s3.abort_abandoned_multipart",
                "s3.reconcile_multipart",
                "storage.delete_blob",
                "storage.orphan_sweep",
                "audit.retention_sweep",
                "sessions.prune",
                "tokens.prune",
                "quota.reconcile",
                "avatar.fetch_external",
                "image.normalize",
                "folders.delete_tree",
                "reverse_share.delete_cascade",
                "received.copy_to_my_files",
            ]
        );
        assert_eq!(names.iter().collect::<BTreeSet<_>>().len(), 18);

        for kind in JobKind::ALL {
            let name = kind.as_str();
            assert_eq!(name.parse::<JobKind>(), Ok(kind));
            assert!((1..=64).contains(&name.len()));
            assert!(name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'));

            let policy = kind.policy();
            assert!((3..=12).contains(&policy.max_attempts), "{name}");
            assert_eq!(policy.lease, DEFAULT_LEASE);
            assert!(Priority::new(policy.priority.get()).is_some());
        }

        for text in ["", "Email.send", "email.send ", "arbitrary.code", "run"] {
            assert_eq!(text.parse::<JobKind>(), Err(UnknownJobKind), "{text:?}");
        }
        assert_eq!(Priority::new(1_001), None);
        assert_eq!(Priority::new(0).map(Priority::get), Some(0));
    }
}
