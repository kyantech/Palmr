use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::features::auth::lockout::LockState;
use crate::infra::http::pagination::WireBytes;

use super::model::{QuotaOverride, UserId};
use super::service::effective_quota;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Active,
    Inactive,
}

impl UserStatus {
    pub const ALL: [Self; 2] = [Self::Active, Self::Inactive];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == raw)
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceCounts {
    pub files: u64,
    pub shares: u64,
    pub reverse_shares: u64,
    pub received_files: u64,
    pub identity_links: u64,
}

#[derive(Debug, Clone)]
pub struct AdminUserRecord {
    pub id: UserId,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub username_normalized: String,
    pub email: String,
    pub email_normalized: String,
    pub pending_email: Option<String>,
    pub role: Role,
    pub is_active: bool,
    pub must_change_password: bool,
    pub totp_enabled: bool,
    pub has_local_password: bool,
    pub quota: QuotaOverride,
    pub used_bytes: ByteSize,
    pub lock: Option<LockState>,
    pub last_login_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLinkState {
    Active,
    Suspended,
}

impl IdentityLinkState {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLinkMethod {
    AutoVerifiedEmail,
    Manual,
    AutoProvision,
}

impl IdentityLinkMethod {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "auto_verified_email" => Some(Self::AutoVerifiedEmail),
            "manual" => Some(Self::Manual),
            "auto_provision" => Some(Self::AutoProvision),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdentityLinkRecord {
    pub id: String,
    pub provider_key: String,
    pub provider_name: String,
    pub state: IdentityLinkState,
    pub method: IdentityLinkMethod,
    pub created_at: Timestamp,
    pub last_login_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserCounts {
    #[schema(minimum = 0)]
    pub files: u64,
    #[schema(minimum = 0)]
    pub shares: u64,
    #[schema(minimum = 0)]
    pub reverse_shares: u64,
    #[schema(minimum = 0)]
    pub received_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserItem {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub email: String,
    #[schema(required = true)]
    pub pending_email: Option<String>,
    #[schema(example = "admin")]
    pub role: &'static str,
    pub is_active: bool,
    pub must_change_password: bool,
    pub two_factor_enabled: bool,
    pub is_locked_out: bool,
    pub has_local_password: bool,
    #[schema(minimum = 0)]
    pub identity_link_count: u64,
    pub used_bytes: WireBytes,
    #[schema(required = true)]
    pub quota_bytes: Option<WireBytes>,
    #[schema(required = true)]
    pub effective_quota_bytes: Option<WireBytes>,
    pub counts: AdminUserCounts,
    #[schema(required = true)]
    pub last_login_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminLockout {
    #[schema(required = true)]
    pub locked_until: Option<String>,
    #[schema(minimum = 0)]
    pub failed_count: u32,
    #[schema(minimum = 0)]
    pub lock_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminIdentityLink {
    pub id: String,
    pub provider_key: String,
    pub provider_name: String,
    pub state: IdentityLinkState,
    pub link_method: IdentityLinkMethod,
    pub created_at: String,
    #[schema(required = true)]
    pub last_login_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminUserDetail {
    #[serde(flatten)]
    pub user: AdminUserItem,
    pub over_quota: bool,
    #[schema(minimum = 0)]
    pub session_count: u64,
    #[schema(minimum = 0)]
    pub trusted_device_count: u64,
    pub lockout: AdminLockout,
    pub identity_links: Vec<AdminIdentityLink>,
}

pub struct DetailParts {
    pub record: AdminUserRecord,
    pub counts: ResourceCounts,
    pub session_count: u64,
    pub trusted_device_count: u64,
    pub identity_links: Vec<IdentityLinkRecord>,
}

fn wire(size: ByteSize) -> WireBytes {
    WireBytes::clamped(size).0
}

pub fn is_over_quota(used: ByteSize, effective: Option<ByteSize>) -> bool {
    effective.is_some_and(|limit| used > limit)
}

impl AdminUserItem {
    pub fn new(
        record: &AdminUserRecord,
        counts: ResourceCounts,
        instance_default: Option<ByteSize>,
        now: Timestamp,
    ) -> Self {
        Self {
            id: record.id.to_string(),
            first_name: record.first_name.clone(),
            last_name: record.last_name.clone(),
            username: record.username.clone(),
            email: record.email.clone(),
            pending_email: record.pending_email.clone(),
            role: record.role.as_str(),
            is_active: record.is_active,
            must_change_password: record.must_change_password,
            two_factor_enabled: record.totp_enabled,
            is_locked_out: record
                .lock
                .is_some_and(|lock| lock.active_until(now).is_some()),
            has_local_password: record.has_local_password,
            identity_link_count: counts.identity_links,
            used_bytes: wire(record.used_bytes),
            quota_bytes: record.quota.quota_bytes().map(wire),
            effective_quota_bytes: effective_quota(record.quota, instance_default).map(wire),
            counts: AdminUserCounts {
                files: counts.files,
                shares: counts.shares,
                reverse_shares: counts.reverse_shares,
                received_files: counts.received_files,
            },
            last_login_at: record.last_login_at.map(|at| at.to_string()),
            created_at: record.created_at.to_string(),
        }
    }
}

impl AdminUserDetail {
    pub fn new(parts: DetailParts, instance_default: Option<ByteSize>, now: Timestamp) -> Self {
        let DetailParts {
            record,
            counts,
            session_count,
            trusted_device_count,
            identity_links,
        } = parts;
        let effective = effective_quota(record.quota, instance_default);
        let lockout = AdminLockout {
            locked_until: record
                .lock
                .and_then(|lock| lock.active_until(now))
                .map(|until| until.to_string()),
            failed_count: record.lock.map_or(0, |lock| lock.failed_count),
            lock_count: record.lock.map_or(0, |lock| lock.lock_count),
        };
        Self {
            over_quota: is_over_quota(record.used_bytes, effective),
            user: AdminUserItem::new(&record, counts, instance_default, now),
            session_count,
            trusted_device_count,
            lockout,
            identity_links: identity_links
                .into_iter()
                .map(|link| AdminIdentityLink {
                    id: link.id,
                    provider_key: link.provider_key,
                    provider_name: link.provider_name,
                    state: link.state,
                    link_method: link.method,
                    created_at: link.created_at.to_string(),
                    last_login_at: link.last_login_at.map(|at| at.to_string()),
                })
                .collect(),
        }
    }
}
