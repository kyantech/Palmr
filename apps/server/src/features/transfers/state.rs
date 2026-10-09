use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransferSessionState {
    Created,
    Uploading,
    Finalizing,
    Completed,
    Failed,
    Canceled,
    Expired,
}

impl TransferSessionState {
    pub const ALL: [Self; 7] = [
        Self::Created,
        Self::Uploading,
        Self::Finalizing,
        Self::Completed,
        Self::Failed,
        Self::Canceled,
        Self::Expired,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Uploading => "uploading",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Canceled | Self::Expired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransferItemState {
    Created,
    Uploading,
    Finalizing,
    Completed,
    Failed,
    Canceled,
    Expired,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemState {
    Pending,
    Uploading,
    Finalizing,
    Completed,
    Failed,
    Canceled,
    Expired,
    Skipped,
}

impl ItemState {
    pub const ALL: [Self; 8] = [
        Self::Pending,
        Self::Uploading,
        Self::Finalizing,
        Self::Completed,
        Self::Failed,
        Self::Canceled,
        Self::Expired,
        Self::Skipped,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploading => "uploading",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
            Self::Skipped => "skipped",
        }
    }

    pub const fn wire(self) -> TransferItemState {
        match self {
            Self::Pending => TransferItemState::Created,
            Self::Uploading => TransferItemState::Uploading,
            Self::Finalizing => TransferItemState::Finalizing,
            Self::Completed => TransferItemState::Completed,
            Self::Failed => TransferItemState::Failed,
            Self::Canceled => TransferItemState::Canceled,
            Self::Expired => TransferItemState::Expired,
            Self::Skipped => TransferItemState::Skipped,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }

    pub const fn holds_reservation(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Uploading | Self::Finalizing | Self::Failed
        )
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the upload adapters and the reaper raise the remaining triggers as TUS, S3 and finalization are wired"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    ProtocolStarted,
    FinalizeIntent,
    Committed,
    Failed,
    Retry,
    Cancel,
    Expire,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<S> {
    Moved(S),
    Unchanged(S),
}

impl<S: Copy> Outcome<S> {
    pub const fn moved(self) -> bool {
        matches!(self, Self::Moved(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition;

pub const fn session_transition(
    from: TransferSessionState,
    trigger: Trigger,
) -> Result<Outcome<TransferSessionState>, InvalidTransition> {
    use TransferSessionState::{
        Canceled, Completed, Created, Expired, Failed, Finalizing, Uploading,
    };
    use Trigger::{Cancel, Close, Committed, Expire, FinalizeIntent, Retry};

    let trigger = match trigger {
        Committed => Close,
        other => other,
    };
    match (from, trigger) {
        (Created, Trigger::ProtocolStarted) => Ok(Outcome::Moved(Uploading)),
        (Uploading, Trigger::ProtocolStarted)
        | (Finalizing, FinalizeIntent)
        | (Failed, Trigger::Failed)
        | (Uploading | Finalizing, Retry)
        | (Canceled, Cancel)
        | (Expired, Expire)
        | (Completed, Close) => Ok(Outcome::Unchanged(from)),
        (Uploading, FinalizeIntent) => Ok(Outcome::Moved(Finalizing)),
        (Uploading | Finalizing, Trigger::Failed) => Ok(Outcome::Moved(Failed)),
        (Failed, Retry) => Ok(Outcome::Moved(Uploading)),
        (Created | Uploading | Finalizing | Failed, Cancel) => Ok(Outcome::Moved(Canceled)),
        (Created | Uploading | Finalizing | Failed, Expire) => Ok(Outcome::Moved(Expired)),
        (Created | Uploading | Finalizing | Failed, Close) => Ok(Outcome::Moved(Completed)),
        _ => Err(InvalidTransition),
    }
}

pub const fn item_transition(
    from: ItemState,
    trigger: Trigger,
) -> Result<Outcome<ItemState>, InvalidTransition> {
    use ItemState::{
        Canceled, Completed, Expired, Failed, Finalizing, Pending, Skipped, Uploading,
    };
    use Trigger::{Cancel, Committed, Expire, FinalizeIntent, ProtocolStarted, Retry};

    match (from, trigger) {
        (Uploading, ProtocolStarted)
        | (Finalizing | Completed, FinalizeIntent)
        | (Completed, Committed)
        | (Failed, Trigger::Failed)
        | (Uploading, Retry)
        | (Canceled | Skipped, Cancel)
        | (Expired, Expire) => Ok(Outcome::Unchanged(from)),
        (Pending, ProtocolStarted) => Ok(Outcome::Moved(Uploading)),
        (Uploading, FinalizeIntent) => Ok(Outcome::Moved(Finalizing)),
        (Finalizing, Committed) => Ok(Outcome::Moved(Completed)),
        (Uploading | Finalizing, Trigger::Failed) => Ok(Outcome::Moved(Failed)),
        (Failed, Retry) => Ok(Outcome::Moved(Uploading)),
        (Pending | Uploading | Finalizing | Failed, Cancel) => Ok(Outcome::Moved(Canceled)),
        (Pending | Uploading | Finalizing | Failed, Expire) => Ok(Outcome::Moved(Expired)),
        _ => Err(InvalidTransition),
    }
}

pub fn has_expired(now: Timestamp, expires_at: Timestamp) -> bool {
    now >= expires_at
}

#[cfg(test)]
mod tests {
    use super::{
        has_expired, item_transition, session_transition, InvalidTransition, ItemState, Outcome,
        TransferSessionState, Trigger,
    };
    use crate::domain::time::Timestamp;

    const TRIGGERS: [Trigger; 8] = [
        Trigger::ProtocolStarted,
        Trigger::FinalizeIntent,
        Trigger::Committed,
        Trigger::Failed,
        Trigger::Retry,
        Trigger::Cancel,
        Trigger::Expire,
        Trigger::Close,
    ];

    #[test]
    fn unit_session_state_vocabulary_is_the_durable_set() {
        let names: Vec<&str> = TransferSessionState::ALL
            .iter()
            .map(|state| state.as_str())
            .collect();
        for state in TransferSessionState::ALL {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_str());
        }
        assert_eq!(
            names,
            [
                "created",
                "uploading",
                "finalizing",
                "completed",
                "failed",
                "canceled",
                "expired"
            ]
        );
        for client_only in ["queued", "preparing", "paused", "resumable", "pending", ""] {
            assert_eq!(
                TransferSessionState::parse(client_only),
                None,
                "{client_only}"
            );
        }
        for state in TransferSessionState::ALL {
            assert_eq!(TransferSessionState::parse(state.as_str()), Some(state));
        }
        let terminal: Vec<TransferSessionState> = TransferSessionState::ALL
            .into_iter()
            .filter(|state| state.is_terminal())
            .collect();
        assert_eq!(
            terminal,
            [
                TransferSessionState::Completed,
                TransferSessionState::Canceled,
                TransferSessionState::Expired
            ]
        );
    }

    #[test]
    fn unit_item_state_wire_spells_pending_as_created() {
        for state in ItemState::ALL {
            assert_eq!(ItemState::parse(state.as_str()), Some(state));
            let wire = serde_json::to_value(state.wire()).unwrap();
            if state == ItemState::Pending {
                assert_eq!(wire, "created");
            } else {
                assert_eq!(wire, state.as_str());
            }
        }
        assert_eq!(ItemState::parse("created"), None);
        assert_eq!(ItemState::parse("queued"), None);
    }

    #[test]
    fn unit_session_transition_table_matches_the_engine_contract() {
        use TransferSessionState::{
            Canceled, Completed, Created, Expired, Failed, Finalizing, Uploading,
        };
        let moved = |state| Ok(Outcome::Moved(state));
        let same = |state| Ok(Outcome::Unchanged(state));
        let cases: [(
            TransferSessionState,
            Trigger,
            Result<Outcome<TransferSessionState>, InvalidTransition>,
        ); 39] = [
            (Created, Trigger::ProtocolStarted, moved(Uploading)),
            (Uploading, Trigger::ProtocolStarted, same(Uploading)),
            (Uploading, Trigger::FinalizeIntent, moved(Finalizing)),
            (Finalizing, Trigger::FinalizeIntent, same(Finalizing)),
            (Uploading, Trigger::Failed, moved(Failed)),
            (Finalizing, Trigger::Failed, moved(Failed)),
            (Failed, Trigger::Failed, same(Failed)),
            (Failed, Trigger::Retry, moved(Uploading)),
            (Uploading, Trigger::Retry, same(Uploading)),
            (Finalizing, Trigger::Retry, same(Finalizing)),
            (Created, Trigger::Cancel, moved(Canceled)),
            (Uploading, Trigger::Cancel, moved(Canceled)),
            (Finalizing, Trigger::Cancel, moved(Canceled)),
            (Failed, Trigger::Cancel, moved(Canceled)),
            (Canceled, Trigger::Cancel, same(Canceled)),
            (Created, Trigger::Expire, moved(Expired)),
            (Uploading, Trigger::Expire, moved(Expired)),
            (Finalizing, Trigger::Expire, moved(Expired)),
            (Failed, Trigger::Expire, moved(Expired)),
            (Expired, Trigger::Expire, same(Expired)),
            (Created, Trigger::Close, moved(Completed)),
            (Uploading, Trigger::Close, moved(Completed)),
            (Finalizing, Trigger::Close, moved(Completed)),
            (Failed, Trigger::Close, moved(Completed)),
            (Finalizing, Trigger::Committed, moved(Completed)),
            (Completed, Trigger::Close, same(Completed)),
            (Completed, Trigger::Committed, same(Completed)),
            (Completed, Trigger::Cancel, Err(InvalidTransition)),
            (Completed, Trigger::Expire, Err(InvalidTransition)),
            (Completed, Trigger::Retry, Err(InvalidTransition)),
            (Canceled, Trigger::Close, Err(InvalidTransition)),
            (Canceled, Trigger::Expire, Err(InvalidTransition)),
            (Canceled, Trigger::Retry, Err(InvalidTransition)),
            (Expired, Trigger::Cancel, Err(InvalidTransition)),
            (Expired, Trigger::Close, Err(InvalidTransition)),
            (Expired, Trigger::Retry, Err(InvalidTransition)),
            (Created, Trigger::Retry, Err(InvalidTransition)),
            (Created, Trigger::FinalizeIntent, Err(InvalidTransition)),
            (Failed, Trigger::ProtocolStarted, Err(InvalidTransition)),
        ];
        for (from, trigger, expected) in cases {
            assert_eq!(
                session_transition(from, trigger),
                expected,
                "{from:?} {trigger:?}"
            );
        }
    }

    #[test]
    fn unit_terminal_session_states_accept_only_their_own_repeat() {
        for terminal in [
            TransferSessionState::Completed,
            TransferSessionState::Canceled,
            TransferSessionState::Expired,
        ] {
            for trigger in TRIGGERS {
                if let Ok(outcome) = session_transition(terminal, trigger) {
                    assert_eq!(
                        outcome,
                        Outcome::Unchanged(terminal),
                        "{terminal:?} {trigger:?} must not leave the terminal state"
                    );
                }
            }
        }
    }

    #[test]
    fn unit_item_transition_table_matches_the_engine_contract() {
        use ItemState::{
            Canceled, Completed, Expired, Failed, Finalizing, Pending, Skipped, Uploading,
        };
        let moved = |state| Ok(Outcome::Moved(state));
        let same = |state| Ok(Outcome::Unchanged(state));
        let cases: [(
            ItemState,
            Trigger,
            Result<Outcome<ItemState>, InvalidTransition>,
        ); 30] = [
            (Pending, Trigger::ProtocolStarted, moved(Uploading)),
            (Uploading, Trigger::ProtocolStarted, same(Uploading)),
            (Uploading, Trigger::FinalizeIntent, moved(Finalizing)),
            (Finalizing, Trigger::FinalizeIntent, same(Finalizing)),
            (Completed, Trigger::FinalizeIntent, same(Completed)),
            (Finalizing, Trigger::Committed, moved(Completed)),
            (Completed, Trigger::Committed, same(Completed)),
            (Uploading, Trigger::Failed, moved(Failed)),
            (Finalizing, Trigger::Failed, moved(Failed)),
            (Failed, Trigger::Failed, same(Failed)),
            (Failed, Trigger::Retry, moved(Uploading)),
            (Uploading, Trigger::Retry, same(Uploading)),
            (Pending, Trigger::Cancel, moved(Canceled)),
            (Uploading, Trigger::Cancel, moved(Canceled)),
            (Finalizing, Trigger::Cancel, moved(Canceled)),
            (Failed, Trigger::Cancel, moved(Canceled)),
            (Canceled, Trigger::Cancel, same(Canceled)),
            (Skipped, Trigger::Cancel, same(Skipped)),
            (Pending, Trigger::Expire, moved(Expired)),
            (Failed, Trigger::Expire, moved(Expired)),
            (Expired, Trigger::Expire, same(Expired)),
            (Completed, Trigger::Cancel, Err(InvalidTransition)),
            (Completed, Trigger::Expire, Err(InvalidTransition)),
            (Expired, Trigger::Cancel, Err(InvalidTransition)),
            (Pending, Trigger::Retry, Err(InvalidTransition)),
            (Pending, Trigger::Failed, Err(InvalidTransition)),
            (Pending, Trigger::Committed, Err(InvalidTransition)),
            (Canceled, Trigger::Retry, Err(InvalidTransition)),
            (Skipped, Trigger::Retry, Err(InvalidTransition)),
            (Pending, Trigger::Close, Err(InvalidTransition)),
        ];
        for (from, trigger, expected) in cases {
            assert_eq!(
                item_transition(from, trigger),
                expected,
                "{from:?} {trigger:?}"
            );
        }
    }

    #[test]
    fn unit_reservation_holders_are_the_non_terminal_items_and_failed() {
        let holders: Vec<ItemState> = ItemState::ALL
            .into_iter()
            .filter(|state| state.holds_reservation())
            .collect();
        assert_eq!(
            holders,
            [
                ItemState::Pending,
                ItemState::Uploading,
                ItemState::Finalizing,
                ItemState::Failed
            ]
        );
    }

    #[test]
    fn unit_expiry_is_inclusive_of_the_deadline() {
        let at = |text: &str| text.parse::<Timestamp>().unwrap();
        let deadline = at("2026-10-09T12:00:00.000Z");
        assert!(!has_expired(at("2026-10-09T11:59:59.999Z"), deadline));
        assert!(has_expired(deadline, deadline));
        assert!(has_expired(at("2026-10-09T12:00:00.001Z"), deadline));
    }
}
