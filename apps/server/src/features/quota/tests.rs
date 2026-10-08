use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::Row;
use tempfile::TempDir;
use time::macros::datetime;
use time::OffsetDateTime;
use tokio::task::JoinSet;

use super::error::QuotaError;
use super::model::{
    Hold, HoldRequest, ItemSettlement, ReleaseReason, ReservationContext, ReservationRow,
    ReservationState, Settlement, SizeCoverage, TransferSessionId, QUOTA_ADMISSION_MARGIN,
};
use super::service::{decrement_used, increment_used, QuotaService};
use crate::config::SqliteSynchronous;
use crate::domain::bytes::ByteSize;
use crate::domain::clock::{Clock, TestClock};
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::settings::model::AppSettings;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, DbPools, MIGRATOR};

pub(super) const START: OffsetDateTime = datetime!(2026-10-08 12:00 UTC);
const HOUR: Duration = Duration::from_secs(60 * 60);

pub(super) fn bytes(value: i64) -> ByteSize {
    ByteSize::try_from(value).unwrap()
}

#[derive(Debug, Clone, Copy)]
pub(super) struct UserSpec {
    pub role: &'static str,
    pub mode: &'static str,
    pub quota: Option<i64>,
    pub used: i64,
    pub active: bool,
}

impl UserSpec {
    pub(super) const fn inherit(used: i64) -> Self {
        Self {
            role: "user",
            mode: "inherit",
            quota: None,
            used,
            active: true,
        }
    }

    pub(super) const fn capped(quota: i64, used: i64) -> Self {
        Self {
            role: "user",
            mode: "bytes",
            quota: Some(quota),
            used,
            active: true,
        }
    }

    pub(super) const fn admin(self) -> Self {
        Self {
            role: "admin",
            ..self
        }
    }

    pub(super) const fn unlimited(used: i64) -> Self {
        Self {
            role: "user",
            mode: "unlimited",
            quota: None,
            used,
            active: true,
        }
    }
}

pub(super) struct Harness {
    pub(super) _root: TempDir,
    pub(super) pools: DbPools,
    pub(super) clock: TestClock,
    sequence: AtomicU32,
}

impl Harness {
    pub(super) async fn open() -> Arc<Self> {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        Arc::new(Self {
            _root: root,
            pools,
            clock: TestClock::new(START),
            sequence: AtomicU32::new(1),
        })
    }

    pub(super) fn service(&self, default_quota: Option<i64>) -> QuotaService {
        let mut settings = AppSettings::defaults();
        settings.quotas.default_user_quota_bytes = default_quota.map(bytes);
        QuotaService::new(SettingsHandle::new(settings), Arc::new(self.clock.clone()))
    }

    pub(super) fn now(&self) -> Timestamp {
        Timestamp::try_from(self.clock.now()).unwrap()
    }

    pub(super) fn after(&self, by: Duration) -> Timestamp {
        Timestamp::try_from(self.clock.now() + by).unwrap()
    }

    fn next(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) async fn try_execute(
        &self,
        sql: &'static str,
        binds: Vec<String>,
    ) -> Result<(), DbError> {
        self.pools
            .write_tx(&self.clock, "quota.test_fixture", async |tx| {
                let mut query = sqlx::query(sql);
                for bind in &binds {
                    query = query.bind(bind);
                }
                query.execute(tx.executor()).await.map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
    }

    pub(super) async fn execute(&self, sql: &'static str, binds: Vec<String>) {
        self.try_execute(sql, binds).await.unwrap();
    }

    pub(super) async fn user(&self, spec: UserSpec) -> UserId {
        let id = UserId::generate(&self.clock);
        let seq = self.next();
        let name = format!("user{seq}");
        let now = self.now().to_string();
        let deactivated = (!spec.active).then(|| now.clone());
        self.pools
            .write_tx(&self.clock, "quota.test_user", async |tx| {
                sqlx::query(
                    "INSERT INTO users
                         (id, email, email_normalized, username, username_normalized, role,
                          is_active, deactivated_at, quota_override_mode, quota_bytes, used_bytes,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                )
                .bind(id.to_string())
                .bind(format!("{name}@example.test"))
                .bind(&name)
                .bind(spec.role)
                .bind(i64::from(spec.active))
                .bind(&deactivated)
                .bind(spec.mode)
                .bind(spec.quota)
                .bind(spec.used)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn session_expiring(
        &self,
        owner: UserId,
        expires: Timestamp,
    ) -> TransferSessionId {
        let id = TransferSessionId::generate(&self.clock);
        let now = self.now().to_string();
        self.pools
            .write_tx(&self.clock, "quota.test_session", async |tx| {
                sqlx::query(
                    "INSERT INTO transfer_sessions
                         (id, context, user_id, provider, state, created_at, updated_at, expires_at)
                     VALUES (?1, 'my_files', ?2, 'local', 'created', ?3, ?3, ?4)",
                )
                .bind(id.to_string())
                .bind(owner.to_string())
                .bind(&now)
                .bind(expires.to_string())
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn session(&self, owner: UserId) -> TransferSessionId {
        self.session_expiring(owner, self.after(HOUR)).await
    }

    pub(super) async fn reverse_share(&self, owner: UserId) -> String {
        let seq = self.next();
        let id = Id::<ReverseShareMarker>::generate(&self.clock).to_string();
        let now = self.now().to_string();
        let public_id = format!("public{seq:010}");
        let alias = format!("alias{seq}");
        let share = id.clone();
        self.pools
            .write_tx(&self.clock, "quota.test_reverse_share", async |tx| {
                sqlx::query(
                    "INSERT INTO reverse_shares
                         (id, owner_id, public_id, alias, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                )
                .bind(&share)
                .bind(owner.to_string())
                .bind(&public_id)
                .bind(&alias)
                .bind(&now)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn reverse_share_session(&self, owner: UserId) -> TransferSessionId {
        let share = self.reverse_share(owner).await;
        let upload = Id::<ReverseShareMarker>::generate(&self.clock).to_string();
        let id = TransferSessionId::generate(&self.clock);
        let now = self.now().to_string();
        let expires = self.after(HOUR).to_string();
        let token = format!("{:0>64}", upload.replace('-', ""));
        self.pools
            .write_tx(&self.clock, "quota.test_reverse_share_session", async |tx| {
                sqlx::query(
                    "INSERT INTO reverse_share_upload_sessions
                         (id, reverse_share_id, token_hash, created_at, expires_at, last_activity_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
                )
                .bind(&upload)
                .bind(&share)
                .bind(&token)
                .bind(&now)
                .bind(&expires)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                sqlx::query(
                    "INSERT INTO transfer_sessions
                         (id, context, user_id, reverse_share_upload_session_id, provider, state,
                          created_at, updated_at, expires_at)
                     VALUES (?1, 'reverse_share', NULL, ?2, 'local', 'created', ?3, ?3, ?4)",
                )
                .bind(id.to_string())
                .bind(&upload)
                .bind(&now)
                .bind(&expires)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    pub(super) fn request(
        &self,
        owner: UserId,
        session: TransferSessionId,
        reserved: i64,
    ) -> HoldRequest {
        HoldRequest {
            owner,
            session,
            context: ReservationContext::MyFiles,
            reserved: bytes(reserved),
            expires_at: self.after(HOUR),
        }
    }

    pub(super) async fn hold(
        &self,
        service: &QuotaService,
        request: HoldRequest,
    ) -> Result<Hold, QuotaError> {
        self.pools
            .write_tx(&self.clock, "quota.test_hold", async |tx| {
                service.hold(tx, request).await
            })
            .await
    }

    pub(super) async fn held(
        &self,
        service: &QuotaService,
        owner: UserId,
        reserved: i64,
    ) -> TransferSessionId {
        let session = self.session(owner).await;
        self.hold(service, self.request(owner, session, reserved))
            .await
            .unwrap();
        session
    }

    pub(super) async fn used(&self, owner: UserId) -> i64 {
        sqlx::query_scalar("SELECT used_bytes FROM users WHERE id = ?1")
            .bind(owner.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn held_sum(&self, owner: UserId) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(SUM(reserved_bytes), 0) FROM quota_reservations
              WHERE user_id = ?1 AND state = 'held'",
        )
        .bind(owner.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn reservation_count(&self, owner: UserId) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM quota_reservations WHERE user_id = ?1")
            .bind(owner.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn raw_state(&self, session: TransferSessionId) -> RawReservation {
        let row = sqlx::query(
            "SELECT state, reserved_bytes, committed_bytes, settled_at, release_reason
               FROM quota_reservations WHERE transfer_session_id = ?1",
        )
        .bind(session.to_string())
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        RawReservation {
            state: row.get("state"),
            reserved: row.get("reserved_bytes"),
            committed: row.get("committed_bytes"),
            settled_at: row.get("settled_at"),
            release_reason: row.get("release_reason"),
        }
    }

    pub(super) async fn commit(
        &self,
        service: &QuotaService,
        session: TransferSessionId,
        committed: i64,
    ) -> Result<Settlement, QuotaError> {
        self.pools
            .write_tx(&self.clock, "quota.test_commit", async |tx| {
                service.commit(tx, session, bytes(committed)).await
            })
            .await
    }

    pub(super) async fn release(
        &self,
        service: &QuotaService,
        session: TransferSessionId,
        reason: ReleaseReason,
    ) -> Result<Settlement, QuotaError> {
        self.pools
            .write_tx(&self.clock, "quota.test_release", async |tx| {
                service.release(tx, session, reason).await
            })
            .await
    }
}

pub(super) enum ReverseShareMarker {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawReservation {
    pub state: String,
    pub reserved: i64,
    pub committed: Option<i64>,
    pub settled_at: Option<String>,
    pub release_reason: Option<String>,
}

async fn race(
    harness: &Arc<Harness>,
    spec: UserSpec,
    preheld: i64,
    request: i64,
    contenders: usize,
) -> (usize, usize, UserId) {
    let service = harness.service(None);
    let owner = harness.user(spec).await;
    if preheld > 0 {
        harness.held(&service, owner, preheld).await;
    }
    let mut sessions = Vec::new();
    for _ in 0..contenders {
        sessions.push(harness.session(owner).await);
    }
    let mut racers = JoinSet::new();
    for session in sessions {
        let harness = Arc::clone(harness);
        let service = service.clone();
        racers.spawn(async move {
            let request = harness.request(owner, session, request);
            harness.hold(&service, request).await
        });
    }
    let outcomes = racers.join_all().await;
    let admitted = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    let refused = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Err(QuotaError::Exceeded(_))))
        .count();
    assert_eq!(admitted + refused, contenders, "{outcomes:?}");
    (admitted, refused, owner)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_quota_admission_concurrent_never_exceeds() {
    let harness = Harness::open().await;

    let (admitted, refused, owner) = race(&harness, UserSpec::capped(100, 0), 0, 30, 12).await;
    assert_eq!(admitted, 3);
    assert_eq!(refused, 9);
    let rows = harness.reservation_count(owner).await;
    assert_eq!(rows, 3);
    assert!(harness.held_sum(owner).await <= 100);
    assert_eq!(harness.held_sum(owner).await, 90);
    assert_eq!(harness.used(owner).await, 0);

    let (admitted, refused, owner) = race(&harness, UserSpec::capped(100, 40), 0, 30, 10).await;
    assert_eq!(admitted, 2);
    assert_eq!(refused, 8);
    assert_eq!(harness.reservation_count(owner).await, 2);
    assert_eq!(harness.held_sum(owner).await, 60);
    assert!(harness.used(owner).await + harness.held_sum(owner).await <= 100);

    let (admitted, refused, owner) = race(&harness, UserSpec::capped(100, 40), 10, 30, 10).await;
    assert_eq!(admitted, 1);
    assert_eq!(refused, 9);
    assert_eq!(harness.reservation_count(owner).await, 2);
    assert_eq!(harness.held_sum(owner).await, 40);
    assert!(harness.used(owner).await + harness.held_sum(owner).await <= 100);

    let (admitted, refused, owner) = race(&harness, UserSpec::capped(100, 0), 0, 100, 6).await;
    assert_eq!((admitted, refused), (1, 5));
    assert_eq!(harness.held_sum(owner).await, 100);
}

#[tokio::test]
async fn it_quota_unlimited_null() {
    let harness = Harness::open().await;

    let unlimited_default = harness.service(None);
    let finite_default = harness.service(Some(100));

    let inherits = harness.user(UserSpec::inherit(0)).await;
    let override_unlimited = harness.user(UserSpec::unlimited(0)).await;
    let override_finite = harness.user(UserSpec::capped(50, 0)).await;
    let override_zero = harness.user(UserSpec::capped(0, 0)).await;
    let big = harness.user(UserSpec::capped(i64::MAX, 0)).await;

    async fn effective(
        harness: &Harness,
        service: &QuotaService,
        owner: UserId,
    ) -> Option<ByteSize> {
        let mut connection = harness.pools.reader().executor().acquire().await.unwrap();
        service
            .effective_quota(&mut connection, owner)
            .await
            .unwrap()
    }

    assert_eq!(
        effective(&harness, &unlimited_default, inherits).await,
        None
    );
    assert_eq!(
        effective(&harness, &finite_default, inherits).await,
        Some(bytes(100))
    );
    assert_eq!(
        effective(&harness, &finite_default, override_unlimited).await,
        None
    );
    assert_eq!(
        effective(&harness, &unlimited_default, override_finite).await,
        Some(bytes(50))
    );
    assert_eq!(
        effective(&harness, &finite_default, override_zero).await,
        Some(ByteSize::ZERO)
    );
    assert_eq!(
        effective(&harness, &unlimited_default, big).await,
        Some(ByteSize::MAX)
    );

    let admit = |service: QuotaService, owner: UserId, requested: i64| {
        let harness = Arc::clone(&harness);
        async move {
            harness
                .pools
                .write_tx(&harness.clock, "quota.test_admit", async |tx| {
                    service.admit(tx, owner, bytes(requested)).await
                })
                .await
        }
    };

    let huge = admit(unlimited_default.clone(), inherits, i64::MAX / 2)
        .await
        .unwrap();
    assert_eq!(huge.usage.quota, None);
    assert!(
        admit(finite_default.clone(), override_unlimited, i64::MAX / 2)
            .await
            .is_ok()
    );

    assert!(admit(unlimited_default.clone(), override_finite, 50)
        .await
        .is_ok());
    assert!(matches!(
        admit(unlimited_default.clone(), override_finite, 51).await,
        Err(QuotaError::Exceeded(exceeded)) if exceeded.quota == Some(bytes(50))
    ));
    assert!(admit(finite_default.clone(), inherits, 100).await.is_ok());
    assert!(matches!(
        admit(finite_default.clone(), inherits, 101).await,
        Err(QuotaError::Exceeded(_))
    ));

    assert!(admit(finite_default.clone(), override_zero, 0)
        .await
        .is_ok());
    assert!(matches!(
        admit(finite_default.clone(), override_zero, 1).await,
        Err(QuotaError::Exceeded(exceeded)) if exceeded.quota == Some(ByteSize::ZERO)
    ));
    let zero_default = harness.service(Some(0));
    assert!(matches!(
        admit(zero_default.clone(), inherits, 1).await,
        Err(QuotaError::Exceeded(exceeded)) if exceeded.quota == Some(ByteSize::ZERO)
    ));
    assert!(admit(zero_default, inherits, 0).await.is_ok());

    let admin_inherits = harness.user(UserSpec::inherit(0).admin()).await;
    let admin_capped = harness.user(UserSpec::capped(50, 0).admin()).await;
    let admin_unlimited = harness.user(UserSpec::unlimited(0).admin()).await;
    assert!(matches!(
        admit(finite_default.clone(), admin_inherits, 101).await,
        Err(QuotaError::Exceeded(_))
    ));
    assert!(admit(finite_default.clone(), admin_inherits, 100)
        .await
        .is_ok());
    assert!(matches!(
        admit(finite_default.clone(), admin_capped, 51).await,
        Err(QuotaError::Exceeded(_))
    ));
    assert!(admit(finite_default, admin_unlimited, i64::MAX / 2)
        .await
        .is_ok());

    let stored = sqlx::query(
        "SELECT id, quota_override_mode, quota_bytes FROM users WHERE id IN (?1, ?2, ?3)
          ORDER BY quota_override_mode, quota_bytes",
    )
    .bind(inherits.to_string())
    .bind(override_unlimited.to_string())
    .bind(override_zero.to_string())
    .fetch_all(harness.pools.reader().executor())
    .await
    .unwrap();
    let persisted: Vec<(String, Option<i64>)> = stored
        .iter()
        .map(|row| (row.get("quota_override_mode"), row.get("quota_bytes")))
        .collect();
    assert_eq!(
        persisted,
        [
            ("bytes".to_owned(), Some(0)),
            ("inherit".to_owned(), None),
            ("unlimited".to_owned(), None),
        ]
    );
}

#[tokio::test]
async fn it_quota_hold_persists_a_held_reservation() {
    let harness = Harness::open().await;
    let service = harness.service(Some(1_000));
    let owner = harness.user(UserSpec::inherit(100)).await;
    let session = harness.session(owner).await;
    let request = harness.request(owner, session, 300);

    let hold = harness.hold(&service, request).await.unwrap();
    let Hold::Created(row) = hold else {
        panic!("a first hold is created");
    };
    assert_eq!(row.state, ReservationState::Held);
    assert_eq!(row.reserved, bytes(300));
    assert_eq!(row.committed, None);
    assert_eq!(row.settled_at, None);
    assert_eq!(row.release_reason, None);
    assert_eq!(row.context, ReservationContext::MyFiles);
    assert_eq!(row.created_at, harness.now());
    assert_eq!(row.expires_at, request.expires_at);

    let raw = harness.raw_state(session).await;
    assert_eq!(
        raw,
        RawReservation {
            state: "held".to_owned(),
            reserved: 300,
            committed: None,
            settled_at: None,
            release_reason: None,
        }
    );
    assert_eq!(harness.used(owner).await, 100);
    assert_eq!(harness.held_sum(owner).await, 300);
}

#[tokio::test]
async fn it_quota_hold_rejection_persists_nothing() {
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(60)).await;
    let session = harness.session(owner).await;

    let refused = harness
        .hold(&service, harness.request(owner, session, 41))
        .await;
    let Err(QuotaError::Exceeded(exceeded)) = refused else {
        panic!("expected QUOTA_EXCEEDED, got {refused:?}");
    };
    assert_eq!(exceeded.used, bytes(60));
    assert_eq!(exceeded.held, ByteSize::ZERO);
    assert_eq!(exceeded.requested, bytes(41));
    assert_eq!(exceeded.quota, Some(bytes(100)));
    assert_eq!(harness.reservation_count(owner).await, 0);
    assert_eq!(harness.used(owner).await, 60);

    let admitted = harness
        .hold(&service, harness.request(owner, session, 40))
        .await;
    assert!(admitted.is_ok());
}

#[tokio::test]
async fn it_quota_hold_retry_same_session_reuses_the_reservation() {
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(0)).await;
    let session = harness.session(owner).await;
    let request = harness.request(owner, session, 70);

    let first = harness.hold(&service, request).await.unwrap();
    let second = harness.hold(&service, request).await.unwrap();
    assert!(matches!(first, Hold::Created(_)));
    assert!(matches!(second, Hold::Reused(_)));
    assert_eq!(first.reservation().id, second.reservation().id);
    assert_eq!(harness.reservation_count(owner).await, 1);
    assert_eq!(harness.held_sum(owner).await, 70);
}

#[tokio::test]
async fn it_quota_hold_retry_with_different_claim_is_refused() {
    let harness = Harness::open().await;
    let service = harness.service(Some(1_000));
    let owner = harness.user(UserSpec::inherit(0)).await;
    let session = harness.session(owner).await;
    let request = harness.request(owner, session, 70);
    harness.hold(&service, request).await.unwrap();

    let different_amount = HoldRequest {
        reserved: bytes(71),
        ..request
    };
    let different_expiry = HoldRequest {
        expires_at: harness.after(HOUR * 2),
        ..request
    };
    for mismatch in [different_amount, different_expiry] {
        assert!(matches!(
            harness.hold(&service, mismatch).await,
            Err(QuotaError::ReservationMismatch)
        ));
    }
    assert_eq!(harness.held_sum(owner).await, 70);
    assert_eq!(harness.reservation_count(owner).await, 1);
}

#[tokio::test]
async fn it_quota_hold_after_settlement_is_refused() {
    let harness = Harness::open().await;
    let service = harness.service(None);
    let owner = harness.user(UserSpec::inherit(0)).await;

    let committed = harness.held(&service, owner, 10).await;
    harness.commit(&service, committed, 10).await.unwrap();
    let released = harness.held(&service, owner, 10).await;
    harness
        .release(&service, released, ReleaseReason::Canceled)
        .await
        .unwrap();

    for session in [committed, released] {
        let retry = harness
            .hold(&service, harness.request(owner, session, 10))
            .await;
        assert!(
            matches!(retry, Err(QuotaError::SessionAlreadySettled)),
            "{retry:?}"
        );
    }
    assert_eq!(harness.held_sum(owner).await, 0);
    assert_eq!(harness.reservation_count(owner).await, 2);
    assert_eq!(harness.raw_state(committed).await.state, "committed");
    assert_eq!(harness.raw_state(released).await.state, "released");
}

#[tokio::test]
async fn it_quota_hold_validates_owner_context_and_session() {
    let harness = Harness::open().await;
    let service = harness.service(None);
    let owner = harness.user(UserSpec::inherit(0)).await;
    let other = harness.user(UserSpec::inherit(0)).await;
    let session = harness.session(owner).await;

    assert!(matches!(
        harness
            .hold(&service, harness.request(other, session, 1))
            .await,
        Err(QuotaError::OwnerContextMismatch)
    ));
    let wrong_context = HoldRequest {
        context: ReservationContext::ReverseShare,
        ..harness.request(owner, session, 1)
    };
    assert!(matches!(
        harness.hold(&service, wrong_context).await,
        Err(QuotaError::OwnerContextMismatch)
    ));

    let unknown = TransferSessionId::generate(&harness.clock);
    assert!(matches!(
        harness
            .hold(&service, harness.request(owner, unknown, 1))
            .await,
        Err(QuotaError::SessionNotFound)
    ));

    let past = HoldRequest {
        expires_at: harness.now(),
        ..harness.request(owner, session, 1)
    };
    assert!(matches!(
        harness.hold(&service, past).await,
        Err(QuotaError::InvalidExpiry)
    ));
    assert_eq!(harness.reservation_count(owner).await, 0);
    assert_eq!(harness.reservation_count(other).await, 0);
}

#[tokio::test]
async fn it_quota_hold_reverse_share_charges_the_share_owner() {
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(30)).await;
    let bystander = harness.user(UserSpec::inherit(0)).await;
    let session = harness.reverse_share_session(owner).await;

    let as_my_files = HoldRequest {
        context: ReservationContext::MyFiles,
        ..harness.request(owner, session, 10)
    };
    assert!(matches!(
        harness.hold(&service, as_my_files).await,
        Err(QuotaError::OwnerContextMismatch)
    ));
    let wrong_owner = HoldRequest {
        context: ReservationContext::ReverseShare,
        ..harness.request(bystander, session, 10)
    };
    assert!(matches!(
        harness.hold(&service, wrong_owner).await,
        Err(QuotaError::OwnerContextMismatch)
    ));

    let correct = HoldRequest {
        context: ReservationContext::ReverseShare,
        ..harness.request(owner, session, 70)
    };
    let hold = harness.hold(&service, correct).await.unwrap();
    assert_eq!(hold.reservation().user_id, owner);
    assert_eq!(hold.reservation().context, ReservationContext::ReverseShare);
    assert_eq!(harness.held_sum(owner).await, 70);
    assert_eq!(harness.held_sum(bystander).await, 0);

    let second = harness.reverse_share_session(owner).await;
    let overflow = HoldRequest {
        context: ReservationContext::ReverseShare,
        ..harness.request(owner, second, 1)
    };
    assert!(matches!(
        harness.hold(&service, overflow).await,
        Err(QuotaError::Exceeded(_))
    ));
}

#[tokio::test]
async fn it_quota_hold_requires_an_active_existing_owner() {
    let harness = Harness::open().await;
    let service = harness.service(None);
    let inactive = harness
        .user(UserSpec {
            active: false,
            ..UserSpec::inherit(0)
        })
        .await;
    let session = harness.session(inactive).await;
    let refused = harness
        .hold(&service, harness.request(inactive, session, 1))
        .await;
    assert!(
        matches!(refused, Err(QuotaError::OwnerInactive)),
        "{refused:?}"
    );
    assert_eq!(harness.reservation_count(inactive).await, 0);

    let ghost = UserId::generate(&harness.clock);
    let missing = harness
        .pools
        .write_tx(&harness.clock, "quota.test_ghost", async |tx| {
            service.admit(tx, ghost, ByteSize::ZERO).await
        })
        .await;
    assert!(matches!(missing, Err(QuotaError::OwnerNotFound)));
}

#[tokio::test]
async fn it_quota_foreign_key_requires_a_real_transfer_session() {
    let harness = Harness::open().await;
    let owner = harness.user(UserSpec::inherit(0)).await;
    let orphan = harness
        .try_execute(
            "INSERT INTO quota_reservations
                 (id, user_id, transfer_session_id, context, reserved_bytes, state,
                  created_at, expires_at)
             VALUES ('01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d', ?1, 'missing-session', 'my_files',
                     1, 'held', ?2, ?2)",
            vec![owner.to_string(), harness.now().to_string()],
        )
        .await;
    assert!(matches!(orphan, Err(DbError::ForeignKeyViolation(_))));
}

#[tokio::test]
async fn it_quota_commit_moves_a_hold_to_committed_once() {
    let harness = Harness::open().await;
    let service = harness.service(Some(1_000));
    let owner = harness.user(UserSpec::inherit(0)).await;
    let session = harness.held(&service, owner, 300).await;

    let settlement = harness.commit(&service, session, 280).await.unwrap();
    let Settlement::Applied(row) = settlement else {
        panic!("first commit applies");
    };
    assert_eq!(row.state, ReservationState::Committed);
    assert_eq!(row.committed, Some(bytes(280)));
    assert_eq!(row.reserved, bytes(300));
    assert_eq!(row.release_reason, None);

    let raw = harness.raw_state(session).await;
    assert_eq!(raw.state, "committed");
    assert_eq!(raw.committed, Some(280));
    assert!(raw.settled_at.is_some());
    assert_eq!(raw.release_reason, None);
    assert_eq!(harness.held_sum(owner).await, 0);
    assert_eq!(harness.used(owner).await, 0);

    assert!(matches!(
        harness.commit(&service, session, 280).await,
        Ok(Settlement::AlreadySettled(_))
    ));
    assert!(matches!(
        harness.commit(&service, session, 281).await,
        Err(QuotaError::StateConflict {
            current: ReservationState::Committed
        })
    ));
    assert!(matches!(
        harness
            .release(&service, session, ReleaseReason::Canceled)
            .await,
        Err(QuotaError::StateConflict {
            current: ReservationState::Committed
        })
    ));
    assert_eq!(harness.raw_state(session).await, raw);
    assert_eq!(harness.used(owner).await, 0);

    let unknown = TransferSessionId::generate(&harness.clock);
    assert!(matches!(
        harness.commit(&service, unknown, 1).await,
        Err(QuotaError::ReservationNotFound)
    ));
}

#[tokio::test]
async fn it_quota_release_records_every_reason_and_is_idempotent() {
    let harness = Harness::open().await;
    let service = harness.service(Some(1_000));
    let owner = harness.user(UserSpec::inherit(40)).await;

    for reason in ReleaseReason::ALL {
        let session = harness.held(&service, owner, 25).await;
        assert_eq!(harness.held_sum(owner).await, 25);
        let settlement = harness.release(&service, session, reason).await.unwrap();
        assert!(matches!(settlement, Settlement::Applied(_)));
        assert_eq!(settlement.reservation().release_reason, Some(reason));

        let raw = harness.raw_state(session).await;
        assert_eq!(raw.state, "released");
        assert_eq!(raw.release_reason.as_deref(), Some(reason.as_str()));
        assert!(raw.settled_at.is_some());
        assert_eq!(raw.committed, None);
        assert_eq!(raw.reserved, 25);
        assert_eq!(harness.held_sum(owner).await, 0);
        assert_eq!(harness.used(owner).await, 40);

        harness.clock.advance(Duration::from_secs(5));
        let repeated = harness
            .release(&service, session, ReleaseReason::Failed)
            .await
            .unwrap();
        assert!(matches!(repeated, Settlement::AlreadySettled(_)));
        assert_eq!(harness.raw_state(session).await, raw);
        assert!(matches!(
            harness.commit(&service, session, 25).await,
            Err(QuotaError::StateConflict {
                current: ReservationState::Released
            })
        ));
        assert_eq!(harness.used(owner).await, 40);
    }
    assert_eq!(harness.reservation_count(owner).await, 6);
}

#[tokio::test]
async fn it_quota_adjust_increase_decrease_and_denial() {
    let harness = Harness::open().await;
    let service = harness.service(Some(200));
    let owner = harness.user(UserSpec::inherit(20)).await;
    let session = harness.held(&service, owner, 100).await;

    let adjust = |target: i64| {
        let harness = Arc::clone(&harness);
        let service = service.clone();
        async move {
            harness
                .pools
                .write_tx(&harness.clock, "quota.test_adjust", async |tx| {
                    service.adjust(tx, session, bytes(target)).await
                })
                .await
        }
    };

    let grown = adjust(150).await.unwrap();
    assert_eq!((grown.previous, grown.current), (bytes(100), bytes(150)));
    assert_eq!(harness.held_sum(owner).await, 150);
    assert_eq!(harness.reservation_count(owner).await, 1);

    let shrunk = adjust(60).await.unwrap();
    assert_eq!((shrunk.previous, shrunk.current), (bytes(150), bytes(60)));
    assert_eq!(harness.held_sum(owner).await, 60);

    let unchanged = adjust(60).await.unwrap();
    assert_eq!(unchanged.previous, unchanged.current);

    assert!(matches!(adjust(181).await, Err(QuotaError::Exceeded(_))));
    assert_eq!(harness.raw_state(session).await.reserved, 60);
    assert_eq!(harness.held_sum(owner).await, 60);
    assert!(adjust(180).await.is_ok());
    assert!(harness.used(owner).await + harness.held_sum(owner).await <= 200);

    harness.commit(&service, session, 10).await.unwrap();
    assert!(matches!(
        adjust(5).await,
        Err(QuotaError::StateConflict {
            current: ReservationState::Committed
        })
    ));
    let other = harness.held(&service, owner, 1).await;
    harness
        .release(&service, other, ReleaseReason::Canceled)
        .await
        .unwrap();
    let resurrected = harness
        .pools
        .write_tx(&harness.clock, "quota.test_adjust_released", async |tx| {
            service.adjust(tx, other, bytes(2)).await
        })
        .await;
    assert!(matches!(
        resurrected,
        Err(QuotaError::StateConflict {
            current: ReservationState::Released
        })
    ));
    assert_eq!(harness.raw_state(other).await.state, "released");
    assert_eq!(harness.held_sum(owner).await, 0);
}

#[tokio::test]
async fn it_quota_settle_item_moves_share_into_used_exactly_once() {
    let harness = Harness::open().await;
    let service = harness.service(None);
    let owner = harness.user(UserSpec::inherit(10)).await;
    let session = harness.held(&service, owner, 300).await;

    let settle = |share: i64, real: i64, coverage: SizeCoverage| {
        let harness = Arc::clone(&harness);
        let service = service.clone();
        async move {
            harness
                .pools
                .write_tx(&harness.clock, "quota.test_settle_item", async |tx| {
                    service
                        .settle_item(
                            tx,
                            ItemSettlement {
                                session,
                                share: bytes(share),
                                authoritative: bytes(real),
                                coverage,
                            },
                        )
                        .await
                })
                .await
        }
    };

    let first = settle(100, 90, SizeCoverage::Reserved).await.unwrap();
    assert_eq!(first.released_share, bytes(100));
    assert_eq!(first.used, bytes(100));
    assert_eq!(harness.used(owner).await, 100);
    assert_eq!(harness.held_sum(owner).await, 200);
    assert_eq!(harness.raw_state(session).await.state, "held");

    let overrun_within_margin = 100 + QUOTA_ADMISSION_MARGIN.to_i64();
    let second = settle(100, overrun_within_margin, SizeCoverage::Reserved)
        .await
        .unwrap();
    assert_eq!(second.used, bytes(100 + overrun_within_margin));
    assert_eq!(harness.held_sum(owner).await, 100);

    let beyond_margin = 100 + QUOTA_ADMISSION_MARGIN.to_i64() + 1;
    let used_before = harness.used(owner).await;
    assert!(matches!(
        settle(100, beyond_margin, SizeCoverage::Reserved).await,
        Err(QuotaError::Exceeded(_))
    ));
    assert_eq!(harness.used(owner).await, used_before);
    assert_eq!(harness.held_sum(owner).await, 100);

    let capped = harness.service(Some(used_before + 100 + 5));
    let refused = harness
        .pools
        .write_tx(&harness.clock, "quota.test_settle_quota", async |tx| {
            capped
                .settle_item(
                    tx,
                    ItemSettlement {
                        session,
                        share: bytes(100),
                        authoritative: bytes(100 + 6),
                        coverage: SizeCoverage::RunningCheck,
                    },
                )
                .await
        })
        .await;
    assert!(matches!(refused, Err(QuotaError::Exceeded(_))));
    assert_eq!(harness.used(owner).await, used_before);

    let running = settle(0, beyond_margin, SizeCoverage::RunningCheck)
        .await
        .unwrap();
    assert_eq!(running.released_share, ByteSize::ZERO);
    assert_eq!(running.used, bytes(used_before + beyond_margin));

    let total = harness.used(owner).await - 10;
    let committed = harness.commit(&service, session, total).await.unwrap();
    assert!(matches!(committed, Settlement::Applied(_)));
    assert_eq!(harness.used(owner).await, 10 + total);
    assert!(matches!(
        harness.commit(&service, session, total).await,
        Ok(Settlement::AlreadySettled(_))
    ));
    assert_eq!(harness.used(owner).await, 10 + total);
}

#[tokio::test]
async fn it_quota_release_share_is_bounded_by_the_hold() {
    let harness = Harness::open().await;
    let service = harness.service(None);
    let owner = harness.user(UserSpec::inherit(0)).await;
    let session = harness.held(&service, owner, 50).await;

    let released = harness
        .pools
        .write_tx(&harness.clock, "quota.test_release_share", async |tx| {
            let partial = service.release_share(tx, session, bytes(20)).await?;
            let rest = service.release_share(tx, session, bytes(1_000)).await?;
            Ok::<_, QuotaError>((partial, rest))
        })
        .await
        .unwrap();
    assert_eq!(released, (bytes(20), bytes(30)));
    assert_eq!(harness.raw_state(session).await.reserved, 0);
    assert_eq!(harness.used(owner).await, 0);
}

#[tokio::test]
async fn it_quota_counter_helpers_are_checked() {
    let harness = Harness::open().await;
    let owner = harness.user(UserSpec::inherit(0)).await;

    let apply = |increment: bool, owner: UserId, delta: i64| {
        let harness = Arc::clone(&harness);
        async move {
            harness
                .pools
                .write_tx(&harness.clock, "quota.test_counter", async |tx| {
                    if increment {
                        increment_used(tx, owner, bytes(delta)).await
                    } else {
                        decrement_used(tx, owner, bytes(delta)).await
                    }
                })
                .await
        }
    };

    assert_eq!(apply(true, owner, 1).await.unwrap(), bytes(1));
    assert_eq!(harness.used(owner).await, 1);
    assert_eq!(apply(false, owner, 1).await.unwrap(), ByteSize::ZERO);
    assert_eq!(harness.used(owner).await, 0);
    assert!(matches!(
        apply(false, owner, 1).await,
        Err(QuotaError::Underflow { .. })
    ));
    assert_eq!(harness.used(owner).await, 0);

    let maxed = harness.user(UserSpec::inherit(i64::MAX)).await;
    assert!(matches!(
        apply(true, maxed, 1).await,
        Err(QuotaError::Overflow { .. })
    ));
    assert_eq!(harness.used(maxed).await, i64::MAX);
    assert_eq!(apply(true, maxed, 0).await.unwrap(), ByteSize::MAX);
    assert_eq!(apply(false, maxed, i64::MAX).await.unwrap(), ByteSize::ZERO);

    let ghost = UserId::generate(&harness.clock);
    assert!(matches!(
        apply(true, ghost, 1).await,
        Err(QuotaError::OwnerNotFound)
    ));
    assert!(matches!(
        apply(false, ghost, 1).await,
        Err(QuotaError::OwnerNotFound)
    ));
}

#[tokio::test]
async fn it_quota_counter_change_rolls_back_with_its_transaction() {
    let harness = Harness::open().await;
    let owner = harness.user(UserSpec::inherit(5)).await;

    let failed = harness
        .pools
        .write_tx(&harness.clock, "quota.test_rollback", async |tx| {
            increment_used(tx, owner, bytes(100)).await?;
            sqlx::query("INSERT INTO users (id) VALUES ('incomplete-row')")
                .execute(tx.executor())
                .await?;
            Ok::<(), QuotaError>(())
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(harness.used(owner).await, 5);
}

#[tokio::test]
async fn it_quota_arithmetic_overflow_is_not_a_quota_refusal() {
    let harness = Harness::open().await;
    let service = harness.service(None);

    let near_max = harness.user(UserSpec::inherit(i64::MAX - 5)).await;
    let session = harness.session(near_max).await;
    let overflowing = harness
        .hold(&service, harness.request(near_max, session, 10))
        .await;
    assert!(
        matches!(overflowing, Err(QuotaError::Overflow { .. })),
        "{overflowing:?}"
    );
    assert_eq!(harness.reservation_count(near_max).await, 0);
    assert_eq!(harness.used(near_max).await, i64::MAX - 5);

    let owner = harness.user(UserSpec::inherit(0)).await;
    for reserved in [i64::MAX, 1, i64::MAX] {
        let seeded = harness.session(owner).await;
        harness
            .execute(
                "INSERT INTO quota_reservations
                     (id, user_id, transfer_session_id, context, reserved_bytes, state,
                      created_at, expires_at)
                 VALUES (?1, ?2, ?3, 'my_files', ?4, 'held', ?5, ?5)",
                vec![
                    Id::<super::model::Reservation>::generate(&harness.clock).to_string(),
                    owner.to_string(),
                    seeded.to_string(),
                    reserved.to_string(),
                    harness.after(HOUR).to_string(),
                ],
            )
            .await;
    }
    let next = harness.session(owner).await;
    let refused = harness
        .hold(&service, harness.request(owner, next, 1))
        .await;
    assert!(
        matches!(refused, Err(QuotaError::Overflow { .. })),
        "{refused:?}"
    );
    assert_eq!(harness.reservation_count(owner).await, 3);
    let api = refused.unwrap_err().api_error();
    assert_ne!(
        api.code(),
        crate::domain::error_code::ErrorCode::QuotaExceeded
    );
}

#[tokio::test]
async fn it_quota_over_quota_account_keeps_data_and_refuses_growth() {
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(150)).await;
    let session = harness.session(owner).await;

    assert!(matches!(
        harness
            .hold(&service, harness.request(owner, session, 1))
            .await,
        Err(QuotaError::Exceeded(_))
    ));
    assert!(matches!(
        harness
            .hold(&service, harness.request(owner, session, 0))
            .await,
        Err(QuotaError::Exceeded(_))
    ));
    assert_eq!(harness.used(owner).await, 150);

    let reduced = harness
        .pools
        .write_tx(&harness.clock, "quota.test_over_quota_delete", async |tx| {
            decrement_used(tx, owner, bytes(60)).await
        })
        .await
        .unwrap();
    assert_eq!(reduced, bytes(90));
    assert!(harness
        .hold(&service, harness.request(owner, session, 10))
        .await
        .is_ok());
}

#[tokio::test]
async fn it_quota_running_check_excludes_the_checked_session() {
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(30)).await;
    let own = harness.held(&service, owner, 40).await;
    harness.held(&service, owner, 10).await;

    let check = |in_flight: i64| {
        let harness = Arc::clone(&harness);
        let service = service.clone();
        async move {
            let mut connection = harness.pools.reader().executor().acquire().await.unwrap();
            service
                .running_check(&mut connection, owner, own, bytes(in_flight))
                .await
        }
    };
    assert!(check(60).await.is_ok());
    assert!(
        matches!(check(61).await, Err(QuotaError::Exceeded(exceeded))
        if exceeded.held == bytes(10) && exceeded.used == bytes(30))
    );
}

#[tokio::test]
async fn it_quota_error_maps_to_the_507_catalogue_entry() {
    use crate::domain::error_code::ErrorCode;
    let harness = Harness::open().await;
    let service = harness.service(Some(100));
    let owner = harness.user(UserSpec::inherit(60)).await;
    let session = harness.session(owner).await;
    let refused = harness
        .hold(&service, harness.request(owner, session, 41))
        .await
        .unwrap_err();
    let api = refused.api_error();
    assert_eq!(api.code(), ErrorCode::QuotaExceeded);
    assert_eq!(api.status().as_u16(), 507);
    assert!(!api.retryable());
    assert_eq!(api.code().as_str(), "QUOTA_EXCEEDED");
    assert_eq!(api.details().len(), 4);
    let body = serde_json::to_value(api.details()).unwrap();
    assert_eq!(body["usedBytes"], 60);
    assert_eq!(body["heldBytes"], 0);
    assert_eq!(body["requestedBytes"], 41);
    assert_eq!(body["quotaBytes"], 100);
}

#[test]
fn unit_reservation_row_fields_cover_the_schema() {
    let _: Option<ReservationRow> = None;
    assert_eq!(ReleaseReason::ALL.len(), 6);
    for reason in ReleaseReason::ALL {
        assert_eq!(ReleaseReason::parse(reason.as_str()), Some(reason));
    }
    for state in [
        ReservationState::Held,
        ReservationState::Committed,
        ReservationState::Released,
    ] {
        assert_eq!(ReservationState::parse(state.as_str()), Some(state));
    }
    assert_eq!(ReservationState::parse("expired"), None);
    assert_eq!(ReleaseReason::parse("held"), None);
    assert_eq!(QUOTA_ADMISSION_MARGIN.to_i64(), 1_048_576);
}
