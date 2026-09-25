use tempfile::TempDir;
use time::macros::datetime;

use super::error::UserError;
use super::model::{NewUser, NormalizedIdentifier, QuotaOverride, User, UserId};
use super::repo;
use super::service::{
    assert_active_admin_remains, effective_quota, AccountPasswordPolicy, PASSWORD_MIN_LENGTH_FLOOR,
};
use crate::config::SqliteSynchronous;
use crate::domain::bytes::ByteSize;
use crate::domain::clock::TestClock;
use crate::domain::email::Email;
use crate::domain::role::Role;
use crate::domain::secret::{Secret, REDACTED};
use crate::domain::username::Username;
use crate::features::settings::model::AppSettings;
use crate::infra::crypto::password::hash_password;
use crate::infra::db::{DbPools, WriteTx, MIGRATOR};

const START: time::OffsetDateTime = datetime!(2026-09-25 12:00 UTC);
const GIB: i64 = 1024 * 1024 * 1024;

struct Harness {
    _root: TempDir,
    pools: DbPools,
    clock: TestClock,
}

impl Harness {
    async fn open() -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        Self {
            _root: root,
            pools,
            clock: TestClock::new(START),
        }
    }

    async fn insert(&self, new: NewUser) -> Result<User, UserError> {
        self.pools
            .write_tx(&self.clock, "users.test_insert", async |tx| {
                repo::insert(tx, &self.clock, &new).await
            })
            .await
    }

    async fn user(&self, email: &str, username: &str, role: Role, is_active: bool) -> UserId {
        self.insert(new_user(email, username, role, is_active))
            .await
            .unwrap()
            .id
    }

    async fn guard(&self, target: UserId) -> Result<(), UserError> {
        self.pools
            .write_tx(&self.clock, "users.test_guard", async |tx| {
                assert_active_admin_remains(tx, target).await
            })
            .await
    }

    async fn demote_if_allowed(&self, target: UserId) -> Result<(), UserError> {
        self.pools
            .write_tx(&self.clock, "users.test_demote", async |tx| {
                assert_active_admin_remains(tx, target).await?;
                set_role(tx, target, Role::User).await
            })
            .await
    }

    async fn active_admins(&self) -> i64 {
        sqlx::query_scalar(repo::COUNT_ACTIVE_ADMINS)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }
}

async fn set_role(tx: &mut WriteTx<'_>, target: UserId, role: Role) -> Result<(), UserError> {
    sqlx::query("UPDATE users SET role = ?2 WHERE id = ?1")
        .bind(target.to_string())
        .bind(role.as_str())
        .execute(tx.executor())
        .await?;
    Ok(())
}

fn new_user(email: &str, username: &str, role: Role, is_active: bool) -> NewUser {
    NewUser {
        email: Email::parse(email).unwrap(),
        username: Username::parse(username).unwrap(),
        first_name: String::new(),
        last_name: String::new(),
        password_hash: None,
        must_change_password: false,
        role,
        is_active,
        quota: QuotaOverride::Inherit,
        created_by: None,
    }
}

fn bytes(value: i64) -> ByteSize {
    ByteSize::try_from(value).unwrap()
}

fn password_of_length(length: usize) -> String {
    "a".repeat(length)
}

#[test]
fn unit_password_policy_floor_8() {
    assert_eq!(PASSWORD_MIN_LENGTH_FLOOR, 8);
    for (configured, effective) in [(0, 8), (1, 8), (7, 8), (8, 8), (12, 12), (64, 64)] {
        let policy = AccountPasswordPolicy::with_configured_min_length(configured);
        assert_eq!(policy.min_length(), effective, "configured {configured}");

        let below = password_of_length(usize::try_from(effective - 1).unwrap());
        match policy.check(&below) {
            Err(UserError::PasswordPolicyViolation { min_length }) => {
                assert_eq!(min_length, effective, "configured {configured}");
            }
            other => panic!("configured {configured}: expected a violation, got {other:?}"),
        }
        let exact = password_of_length(usize::try_from(effective).unwrap());
        assert!(policy.check(&exact).is_ok(), "configured {configured}");
        let above = password_of_length(usize::try_from(effective + 1).unwrap());
        assert!(policy.check(&above).is_ok(), "configured {configured}");
        assert!(matches!(
            policy.check(""),
            Err(UserError::PasswordPolicyViolation { .. })
        ));
    }

    let mut settings = AppSettings::defaults();
    assert_eq!(
        AccountPasswordPolicy::from_settings(&settings).min_length(),
        8
    );
    settings.security.password_min_length = 3;
    assert_eq!(
        AccountPasswordPolicy::from_settings(&settings).min_length(),
        8
    );
    settings.security.password_min_length = 14;
    assert_eq!(
        AccountPasswordPolicy::from_settings(&settings).min_length(),
        14
    );

    let floor = AccountPasswordPolicy::with_configured_min_length(1);
    for without_classes in ["aaaaaaaa", "12345678", "        ", "AAAAAAAA", "--------"] {
        assert!(floor.check(without_classes).is_ok());
    }
    assert!(floor.check(" padded ").is_ok());
    assert!(floor
        .check("\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}")
        .is_err());
    assert!(floor
        .check("\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}")
        .is_ok());
    assert!(floor
        .check("\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}")
        .is_err());
    assert!(floor.check("e\u{0301}e\u{0301}e\u{0301}e\u{0301}").is_ok());
}

#[test]
fn unit_effective_quota_resolution() {
    let hundred = Some(bytes(100 * GIB));
    let twenty_five = QuotaOverride::Bytes(bytes(25 * GIB));
    let zero = QuotaOverride::Bytes(ByteSize::ZERO);
    for (quota, instance_default, expected) in [
        (QuotaOverride::Inherit, hundred, hundred),
        (QuotaOverride::Inherit, None, None),
        (QuotaOverride::Unlimited, hundred, None),
        (QuotaOverride::Unlimited, None, None),
        (twenty_five, hundred, Some(bytes(25 * GIB))),
        (twenty_five, None, Some(bytes(25 * GIB))),
        (zero, hundred, Some(ByteSize::ZERO)),
        (zero, None, Some(ByteSize::ZERO)),
        (
            QuotaOverride::Inherit,
            Some(ByteSize::ZERO),
            Some(ByteSize::ZERO),
        ),
    ] {
        assert_eq!(
            effective_quota(quota, instance_default),
            expected,
            "{quota:?} with default {instance_default:?}"
        );
    }
    assert_ne!(effective_quota(zero, None), None);
    assert_ne!(Some(ByteSize::ZERO), None::<ByteSize>);

    for (mode, stored, expected) in [
        ("inherit", None, Ok(QuotaOverride::Inherit)),
        ("unlimited", None, Ok(QuotaOverride::Unlimited)),
        ("bytes", Some(0), Ok(QuotaOverride::Bytes(ByteSize::ZERO))),
        ("bytes", Some(25), Ok(QuotaOverride::Bytes(bytes(25)))),
    ] {
        let quota = QuotaOverride::from_columns(mode, stored);
        assert_eq!(quota, expected, "{mode} {stored:?}");
        let quota = quota.unwrap();
        assert_eq!(quota.mode(), mode);
        assert_eq!(quota.quota_bytes().map(ByteSize::to_i64), stored);
    }
    for (mode, stored) in [
        ("bytes", None),
        ("bytes", Some(-1)),
        ("inherit", Some(0)),
        ("unlimited", Some(100)),
        ("Inherit", None),
        ("", None),
    ] {
        assert!(
            QuotaOverride::from_columns(mode, stored).is_err(),
            "{mode} {stored:?}"
        );
    }
}

#[tokio::test]
async fn it_last_admin_guard_in_tx() {
    let db = Harness::open().await;
    let first = db
        .user("root@example.test", "root", Role::Admin, true)
        .await;
    let member = db
        .user("member@example.test", "member", Role::User, true)
        .await;
    let retired = db
        .user("retired@example.test", "retired", Role::Admin, false)
        .await;

    assert!(matches!(
        db.guard(first).await,
        Err(UserError::LastAdminProtected)
    ));
    db.guard(member).await.unwrap();
    db.guard(retired).await.unwrap();
    assert!(matches!(
        db.guard(UserId::generate(&db.clock)).await,
        Err(UserError::NotFound)
    ));
    assert!(matches!(
        db.demote_if_allowed(first).await,
        Err(UserError::LastAdminProtected)
    ));
    assert_eq!(db.active_admins().await, 1);

    let second = db
        .user("second@example.test", "second", Role::Admin, true)
        .await;
    db.guard(first).await.unwrap();
    db.guard(second).await.unwrap();

    let own_write = db
        .pools
        .write_tx(&db.clock, "users.test_guard_sees_own_write", async |tx| {
            set_role(tx, first, Role::User).await?;
            assert_active_admin_remains(tx, second).await
        })
        .await;
    assert!(matches!(own_write, Err(UserError::LastAdminProtected)));
    assert_eq!(db.active_admins().await, 2);

    let (demote_first, demote_second) =
        tokio::join!(db.demote_if_allowed(first), db.demote_if_allowed(second));
    let outcomes = [demote_first, demote_second];
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(UserError::LastAdminProtected)))
            .count(),
        1
    );
    assert_eq!(db.active_admins().await, 1);

    let plan: Vec<(i64, i64, i64, String)> =
        sqlx::query_as(&format!("EXPLAIN QUERY PLAN {}", repo::COUNT_ACTIVE_ADMINS))
            .fetch_all(db.pools.reader().executor())
            .await
            .unwrap();
    assert!(
        plan.iter()
            .any(|(_, _, _, detail)| detail.contains("ix_users_active_admins")),
        "{plan:?}"
    );
}

#[tokio::test]
async fn it_user_repository_identity_and_credentials() {
    let db = Harness::open().await;
    let hash = hash_password(b"correct horse battery staple").unwrap();
    let phc = hash.expose_secret().clone();

    let mut local = new_user("Ada.Lovelace@Example.COM", "AdaL", Role::Admin, true);
    local.first_name = "Ada".to_owned();
    local.password_hash = Some(hash);
    local.quota = QuotaOverride::Bytes(ByteSize::ZERO);
    let created = db.insert(local).await.unwrap();
    assert_eq!(created.email, "Ada.Lovelace@Example.COM");
    assert_eq!(created.email_normalized, "ada.lovelace@example.com");
    assert_eq!(created.password_updated_at.map(|at| at.get()), Some(START));

    let mut sso = new_user("sso@example.test", "Sso.Only", Role::User, true);
    sso.quota = QuotaOverride::Unlimited;
    sso.created_by = Some(created.id);
    sso.must_change_password = true;
    let sso = db.insert(sso).await.unwrap();

    let reader = db.pools.reader();
    let by_email = repo::find_by_email_normalized(
        reader,
        &NormalizedIdentifier::from_input("  ADA.LOVELACE@example.com "),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(by_email.id, created.id);
    assert_eq!(by_email.email, "Ada.Lovelace@Example.COM");
    assert_eq!(by_email.username, "AdaL");
    assert_eq!(by_email.username_normalized, "adal");
    assert_eq!(by_email.first_name, "Ada");
    assert_eq!(by_email.role, Role::Admin);
    assert!(by_email.is_active);
    assert_eq!(by_email.quota, QuotaOverride::Bytes(ByteSize::ZERO));
    assert_eq!(by_email.used_bytes, ByteSize::ZERO);
    assert_eq!(
        by_email
            .password_hash
            .as_ref()
            .map(|hash| hash.expose_secret().as_str()),
        Some(phc.as_str())
    );

    let by_username = repo::find_by_username_normalized(
        reader,
        &NormalizedIdentifier::from_input(
            "\u{ff33}\u{ff33}\u{ff2f}.\u{ff2f}\u{ff2e}\u{ff2c}\u{ff39}",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(by_username.id, sso.id);
    assert_eq!(by_username.username, "Sso.Only");
    assert_eq!(by_username.role, Role::User);
    assert!(by_username.password_hash.is_none());
    assert!(by_username.password_updated_at.is_none());
    assert!(by_username.must_change_password);
    assert_eq!(by_username.quota, QuotaOverride::Unlimited);
    assert_eq!(by_username.created_by, Some(created.id));

    let username_as_email = repo::find_by_email_normalized(
        reader,
        &NormalizedIdentifier::from(&Username::parse("adal").unwrap()),
    )
    .await
    .unwrap();
    assert!(username_as_email.is_none());
    assert!(repo::find_by_id(reader, UserId::generate(&db.clock))
        .await
        .unwrap()
        .is_none());

    let stored_hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
            .bind(sso.id.to_string())
            .fetch_one(reader.executor())
            .await
            .unwrap();
    assert_eq!(stored_hash, None);

    for rendered in [format!("{by_email:?}"), format!("{by_email:#?}")] {
        assert!(!rendered.contains(&phc));
        assert!(!rendered.contains("$argon2id$"));
        assert!(rendered.contains(REDACTED));
    }

    for plan_query in [
        repo::SELECT_BY_EMAIL_NORMALIZED,
        repo::SELECT_BY_USERNAME_NORMALIZED,
    ] {
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {plan_query}"))
                .bind("x")
                .fetch_all(reader.executor())
                .await
                .unwrap();
        assert!(
            plan.iter()
                .any(|(_, _, _, detail)| detail.contains("ux_users_")),
            "{plan:?}"
        );
    }

    db.clock.advance(std::time::Duration::from_secs(60));
    let replacement = hash_password(b"another long passphrase").unwrap();
    db.pools
        .write_tx(&db.clock, "users.test_password", async |tx| {
            repo::replace_password_hash(tx, &db.clock, sso.id, &replacement).await?;
            repo::set_must_change_password(tx, &db.clock, sso.id, false).await
        })
        .await
        .unwrap();
    let updated = repo::find_by_id(reader, sso.id).await.unwrap().unwrap();
    assert_eq!(
        repo::password_hash(reader, sso.id)
            .await
            .unwrap()
            .map(|hash| hash.expose_secret().clone()),
        Some(replacement.expose_secret().clone())
    );
    assert!(!updated.must_change_password);
    assert_eq!(
        updated.password_updated_at.map(|at| at.get()),
        Some(START + time::Duration::seconds(60))
    );
    assert_eq!(
        updated.updated_at.get(),
        START + time::Duration::seconds(60)
    );

    let missing = UserId::generate(&db.clock);
    assert!(matches!(
        repo::password_hash(reader, missing).await,
        Err(UserError::NotFound)
    ));
    let missing_update = db
        .pools
        .write_tx(&db.clock, "users.test_missing", async |tx| {
            repo::replace_password_hash(tx, &db.clock, missing, &Secret::new(String::new())).await
        })
        .await;
    assert!(matches!(missing_update, Err(UserError::NotFound)));

    let inactive = db
        .insert(new_user("gone@example.test", "gone", Role::Admin, false))
        .await
        .unwrap();
    let inactive = repo::find_by_id(reader, inactive.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!inactive.is_active);
    assert_eq!(inactive.role, Role::Admin);
    assert!(inactive.deactivated_at.is_some());
}

#[tokio::test]
async fn it_user_repository_normalized_uniqueness() {
    let db = Harness::open().await;
    db.user("Daniel@Example.com", "Daniel", Role::User, true)
        .await;

    let email_clash = db
        .insert(new_user("DANIEL@example.COM", "someone", Role::User, true))
        .await;
    assert!(
        matches!(email_clash, Err(UserError::EmailTaken)),
        "{email_clash:?}"
    );

    let username_clash = db
        .insert(new_user(
            "other@example.com",
            "\u{ff24}\u{ff41}\u{ff4e}\u{ff49}\u{ff45}\u{ff4c}",
            Role::User,
            true,
        ))
        .await;
    assert!(
        matches!(username_clash, Err(UserError::UsernameTaken)),
        "{username_clash:?}"
    );

    let recovered = db
        .pools
        .write_tx(&db.clock, "users.test_recover", async |tx| {
            let clash = repo::insert(
                tx,
                &db.clock,
                &new_user("daniel@EXAMPLE.com", "third", Role::User, true),
            )
            .await;
            assert!(matches!(clash, Err(UserError::EmailTaken)));
            repo::insert(
                tx,
                &db.clock,
                &new_user("third@example.com", "third", Role::User, true),
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(recovered.username, "third");

    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(users, 2);
}
