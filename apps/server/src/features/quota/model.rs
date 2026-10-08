use crate::domain::bytes::ByteSize;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;

pub enum Reservation {}

pub type ReservationId = Id<Reservation>;

pub enum TransferSession {}

pub type TransferSessionId = Id<TransferSession>;

pub const QUOTA_ADMISSION_MARGIN: ByteSize = ByteSize::from_u32(1_048_576);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationState {
    Held,
    Committed,
    Released,
}

impl ReservationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Committed => "committed",
            Self::Released => "released",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "held" => Some(Self::Held),
            "committed" => Some(Self::Committed),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    Canceled,
    Failed,
    Expired,
    Superseded,
    QuotaExceeded,
    Reaped,
}

impl ReleaseReason {
    pub const ALL: [Self; 6] = [
        Self::Canceled,
        Self::Failed,
        Self::Expired,
        Self::Superseded,
        Self::QuotaExceeded,
        Self::Reaped,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Superseded => "superseded",
            Self::QuotaExceeded => "quota_exceeded",
            Self::Reaped => "reaped",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationContext {
    MyFiles,
    ReverseShare,
}

impl ReservationContext {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MyFiles => "my_files",
            Self::ReverseShare => "reverse_share",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "my_files" => Some(Self::MyFiles),
            "reverse_share" => Some(Self::ReverseShare),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRow {
    pub id: ReservationId,
    pub user_id: UserId,
    pub session_id: TransferSessionId,
    pub context: ReservationContext,
    pub reserved: ByteSize,
    pub committed: Option<ByteSize>,
    pub state: ReservationState,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub settled_at: Option<Timestamp>,
    pub release_reason: Option<ReleaseReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: ByteSize,
    pub held: ByteSize,
    pub quota: Option<ByteSize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admission {
    pub usage: Usage,
    pub requested: ByteSize,
    pub projected: ByteSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldRequest {
    pub owner: UserId,
    pub session: TransferSessionId,
    pub context: ReservationContext,
    pub reserved: ByteSize,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    Created(ReservationRow),
    Reused(ReservationRow),
}

impl Hold {
    pub const fn reservation(&self) -> &ReservationRow {
        match self {
            Self::Created(row) | Self::Reused(row) => row,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settlement {
    Applied(ReservationRow),
    AlreadySettled(ReservationRow),
}

impl Settlement {
    pub const fn reservation(&self) -> &ReservationRow {
        match self {
            Self::Applied(row) | Self::AlreadySettled(row) => row,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adjustment {
    pub previous: ByteSize,
    pub current: ByteSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeCoverage {
    Reserved,
    RunningCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSettlement {
    pub session: TransferSessionId,
    pub share: ByteSize,
    pub authoritative: ByteSize,
    pub coverage: SizeCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledItem {
    pub released_share: ByteSize,
    pub used: ByteSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapedReservation {
    pub id: String,
    pub session_id: String,
    pub owner_id: String,
    pub expires_at: String,
    pub reserved: ByteSize,
}
