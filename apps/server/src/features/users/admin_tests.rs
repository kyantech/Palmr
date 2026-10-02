use serde_json::{json, Value};
use sqlx::{QueryBuilder, Row, Sqlite};
use tempfile::TempDir;

use super::admin_model::{
    AdminPasswordReset, AdminUserDetail, AdminUserItem, AdminUserQuota, AdminUserRecord,
    DetailParts, IdentityLinkMethod, IdentityLinkRecord, IdentityLinkState, ResourceCounts,
    UserStatus,
};
use super::admin_repo::{
    push_count, push_grouped_count, push_list, Counted, SearchPrefix, UserFilter,
};
use super::admin_service::{check_reset_target, AdminUserError, USER_SORT, USER_SORT_FIELDS};
use super::model::{QuotaOverride, User, UserId};
use crate::config::SqliteSynchronous;
use crate::domain::bytes::ByteSize;
use crate::domain::clock::TestClock;
use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::features::auth::lockout::LockState;
use crate::features::auth::trusted_devices::repo::COUNT_LISTED;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::{DbPools, MIGRATOR};
use crate::infra::http::pagination::{
    encode_cursor, CursorKey, PageRequest, QueryParams, SearchQuery, SortValue,
};

const NOW: &str = "2026-09-25T12:00:00.000Z";
const LATER: &str = "2026-09-25T12:10:00.000Z";
const EARLIER: &str = "2026-09-25T11:50:00.000Z";
const SAFE_MAX: u64 = 9_007_199_254_740_991;

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn bytes(value: i64) -> ByteSize {
    ByteSize::try_from(value).unwrap()
}

fn record(role: Role, quota: QuotaOverride, used: i64) -> AdminUserRecord {
    let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
    AdminUserRecord {
        id: UserId::generate(&clock),
        first_name: "Ada".to_owned(),
        last_name: "Lovelace".to_owned(),
        username: "Ada".to_owned(),
        username_normalized: "ada".to_owned(),
        email: "Ada@Example.test".to_owned(),
        email_normalized: "ada@example.test".to_owned(),
        pending_email: None,
        role,
        is_active: true,
        must_change_password: false,
        totp_enabled: false,
        has_local_password: true,
        quota,
        used_bytes: bytes(used),
        lock: None,
        last_login_at: None,
        created_at: at(EARLIER),
    }
}

fn item(record: &AdminUserRecord, instance_default: Option<i64>) -> Value {
    serde_json::to_value(AdminUserItem::new(
        record,
        ResourceCounts::default(),
        instance_default.map(bytes),
        at(NOW),
    ))
    .unwrap()
}

fn detail(record: AdminUserRecord, instance_default: Option<i64>) -> AdminUserDetail {
    AdminUserDetail::new(
        DetailParts {
            record,
            counts: ResourceCounts::default(),
            session_count: 0,
            trusted_device_count: 0,
            identity_links: Vec::new(),
        },
        instance_default.map(bytes),
        at(NOW),
    )
}

fn keys() -> KeyRing {
    let dir = TempDir::new().unwrap();
    let (key, _) = InstanceKey::load_or_create(dir.path()).unwrap();
    KeyRing::new(&key)
}

fn object_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn unit_admin_user_effective_quota_follows_the_override_mode() {
    for role in Role::ALL {
        let bytes_mode = item(&record(role, QuotaOverride::Bytes(bytes(100)), 40), Some(7));
        assert_eq!(bytes_mode["quotaBytes"], 100);
        assert_eq!(bytes_mode["effectiveQuotaBytes"], 100);

        let unlimited = item(&record(role, QuotaOverride::Unlimited, 40), Some(7));
        assert_eq!(unlimited["quotaBytes"], Value::Null);
        assert_eq!(unlimited["effectiveQuotaBytes"], Value::Null);

        let inherit_default = item(&record(role, QuotaOverride::Inherit, 40), Some(7));
        assert_eq!(inherit_default["quotaBytes"], Value::Null);
        assert_eq!(inherit_default["effectiveQuotaBytes"], 7);

        let inherit_unlimited = item(&record(role, QuotaOverride::Inherit, 40), None);
        assert_eq!(inherit_unlimited["quotaBytes"], Value::Null);
        assert_eq!(inherit_unlimited["effectiveQuotaBytes"], Value::Null);
    }
}

#[test]
fn unit_admin_user_over_quota_compares_committed_usage_only() {
    for role in Role::ALL {
        let below = detail(record(role, QuotaOverride::Bytes(bytes(100)), 99), None);
        let equal = detail(record(role, QuotaOverride::Bytes(bytes(100)), 100), None);
        let above = detail(record(role, QuotaOverride::Bytes(bytes(100)), 101), None);
        assert!(!below.over_quota);
        assert!(!equal.over_quota);
        assert!(above.over_quota);

        let unlimited = detail(record(role, QuotaOverride::Unlimited, i64::MAX), Some(1));
        assert!(!unlimited.over_quota);
        let inherited_unlimited = detail(record(role, QuotaOverride::Inherit, i64::MAX), None);
        assert!(!inherited_unlimited.over_quota);
        let inherited_limit = detail(record(role, QuotaOverride::Inherit, 11), Some(10));
        assert!(inherited_limit.over_quota);
        let zero_quota = detail(record(role, QuotaOverride::Bytes(bytes(0)), 1), None);
        assert!(zero_quota.over_quota);
    }
}

#[test]
fn unit_admin_user_bytes_beyond_the_json_safe_range_are_clamped() {
    let huge = record(Role::User, QuotaOverride::Bytes(ByteSize::MAX), i64::MAX);
    let value = item(&huge, Some(i64::MAX));
    assert_eq!(value["usedBytes"], SAFE_MAX);
    assert_eq!(value["quotaBytes"], SAFE_MAX);
    assert_eq!(value["effectiveQuotaBytes"], SAFE_MAX);
}

#[test]
fn unit_admin_user_lock_status_uses_the_active_window() {
    let mut locked = record(Role::User, QuotaOverride::Inherit, 0);
    locked.lock = Some(LockState {
        failed_count: 5,
        locked_until: Some(at(LATER)),
        lock_count: 2,
    });
    assert_eq!(item(&locked, None)["isLockedOut"], true);
    let view = detail(locked.clone(), None);
    assert_eq!(view.lockout.locked_until.as_deref(), Some(LATER));
    assert_eq!(view.lockout.failed_count, 5);
    assert_eq!(view.lockout.lock_count, 2);

    locked.lock = Some(LockState {
        failed_count: 5,
        locked_until: Some(at(EARLIER)),
        lock_count: 2,
    });
    assert_eq!(item(&locked, None)["isLockedOut"], false);
    let lapsed = detail(locked.clone(), None);
    assert_eq!(lapsed.lockout.locked_until, None);
    assert_eq!(lapsed.lockout.failed_count, 5);

    locked.lock = Some(LockState {
        failed_count: 2,
        locked_until: None,
        lock_count: 0,
    });
    assert_eq!(item(&locked, None)["isLockedOut"], false);

    locked.lock = None;
    assert_eq!(item(&locked, None)["isLockedOut"], false);
    let clean = detail(locked, None);
    assert_eq!(
        (
            clean.lockout.locked_until,
            clean.lockout.failed_count,
            clean.lockout.lock_count
        ),
        (None, 0, 0)
    );
}

#[test]
fn unit_admin_user_row_exposes_exactly_the_documented_fields() {
    let mut row = record(Role::Admin, QuotaOverride::Inherit, 5);
    row.pending_email = Some("new@example.test".to_owned());
    let value = item(&row, None);
    assert_eq!(
        object_keys(&value),
        [
            "counts",
            "createdAt",
            "effectiveQuotaBytes",
            "email",
            "firstName",
            "hasLocalPassword",
            "id",
            "identityLinkCount",
            "isActive",
            "isLockedOut",
            "lastLoginAt",
            "lastName",
            "mustChangePassword",
            "pendingEmail",
            "quotaBytes",
            "role",
            "twoFactorEnabled",
            "usedBytes",
            "username",
        ]
    );
    assert_eq!(
        object_keys(&value["counts"]),
        ["files", "receivedFiles", "reverseShares", "shares"]
    );
    assert_eq!(value["role"], "admin");
    assert_eq!(value["email"], "Ada@Example.test");
    assert_eq!(value["pendingEmail"], "new@example.test");
}

#[test]
fn unit_admin_user_detail_extends_the_row_without_secret_material() {
    let row = record(Role::User, QuotaOverride::Inherit, 5);
    let link = IdentityLinkRecord {
        id: "link-1".to_owned(),
        provider_key: "corp".to_owned(),
        provider_name: "Corporate SSO".to_owned(),
        state: IdentityLinkState::Suspended,
        method: IdentityLinkMethod::AutoVerifiedEmail,
        created_at: at(EARLIER),
        last_login_at: None,
    };
    let view = AdminUserDetail::new(
        DetailParts {
            record: row.clone(),
            counts: ResourceCounts {
                files: 3,
                shares: 2,
                reverse_shares: 1,
                received_files: 4,
                identity_links: 1,
            },
            session_count: 6,
            trusted_device_count: 2,
            identity_links: vec![link],
        },
        None,
        at(NOW),
    );
    let value = serde_json::to_value(&view).unwrap();
    let row_value = item(&row, None);
    let mut expected: Vec<&str> = object_keys(&row_value);
    expected.extend([
        "identityLinks",
        "lockout",
        "overQuota",
        "quotaOverrideMode",
        "sessionCount",
        "trustedDeviceCount",
    ]);
    expected.sort_unstable();
    assert_eq!(object_keys(&value), expected);
    assert_eq!(
        object_keys(&value["lockout"]),
        ["failedCount", "lockCount", "lockedUntil"]
    );
    assert_eq!(value["quotaOverrideMode"], "inherit");
    assert_eq!(value["identityLinkCount"], 1);
    assert_eq!(value["counts"]["receivedFiles"], 4);
    assert_eq!(value["sessionCount"], 6);
    assert_eq!(value["trustedDeviceCount"], 2);
    assert_eq!(
        value["identityLinks"],
        json!([{
            "id": "link-1",
            "providerKey": "corp",
            "providerName": "Corporate SSO",
            "state": "suspended",
            "linkMethod": "auto_verified_email",
            "createdAt": EARLIER,
            "lastLoginAt": null,
        }])
    );
    let text = value.to_string().to_ascii_lowercase();
    for forbidden in ["hash", "secret", "ciphertext", "nonce", "token", "subject"] {
        assert!(!text.contains(forbidden), "{forbidden} leaked into {text}");
    }
}

#[test]
fn unit_admin_user_sort_allowlist_is_closed() {
    assert_eq!(USER_SORT_FIELDS.len(), 4);
    assert_eq!(
        USER_SORT.values(),
        [
            "createdAt:asc",
            "createdAt:desc",
            "usedBytes:asc",
            "usedBytes:desc",
            "username:asc",
            "username:desc",
            "email:asc",
            "email:desc",
        ]
    );
    assert_eq!(USER_SORT.default_spec().wire(), "createdAt:desc");
    for rejected in [
        "passwordHash:asc",
        "lastLoginAt:desc",
        "createdAt",
        "createdAt:sideways",
        "createdat:asc",
        "u.created_at:asc",
        "createdAt:asc,usedBytes:desc",
        "",
    ] {
        assert!(USER_SORT.parse(Some(rejected)).is_err(), "{rejected}");
    }
}

#[test]
fn unit_admin_user_status_values_are_closed() {
    assert_eq!(UserStatus::parse("active"), Some(UserStatus::Active));
    assert_eq!(UserStatus::parse("inactive"), Some(UserStatus::Inactive));
    for rejected in ["", "Active", "disabled", "all", "1"] {
        assert_eq!(UserStatus::parse(rejected), None, "{rejected}");
    }
    assert!(UserStatus::Active.is_active());
    assert!(!UserStatus::Inactive.is_active());
}

#[test]
fn unit_admin_user_search_prefix_is_normalized_and_bounded() {
    let query = SearchQuery::parse(Some("  ÀDA ")).unwrap();
    let prefix = SearchPrefix::parse(query.as_ref()).unwrap().unwrap();
    let mut sql = QueryBuilder::<Sqlite>::new("");
    push_count(
        &mut sql,
        &UserFilter {
            role: None,
            status: None,
            search: Some(prefix),
        },
    );
    assert!(sql.sql().contains("u.username_normalized >="));
    assert!(sql.sql().contains("u.email_normalized <"));
    assert!(!sql.sql().to_ascii_lowercase().contains(" like "));

    let blank = SearchQuery::parse(Some("   ")).unwrap();
    let error = SearchPrefix::parse(blank.as_ref()).unwrap_err();
    assert_eq!(
        serde_json::to_value(error.details()).unwrap(),
        json!({ "fields": ["q"] })
    );
    assert!(SearchQuery::parse(Some("a")).is_err());
    assert!(SearchQuery::parse(Some(&"x".repeat(129))).is_err());
}

struct Database {
    _root: TempDir,
    pools: DbPools,
}

impl Database {
    async fn open() -> Self {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        Self { _root: root, pools }
    }

    async fn plan(&self, build: impl FnOnce(&mut QueryBuilder<'_, Sqlite>)) -> String {
        let mut query = QueryBuilder::<Sqlite>::new("EXPLAIN QUERY PLAN ");
        build(&mut query);
        query
            .build()
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn page(keys: &KeyRing, raw_query: &str) -> PageRequest {
    PageRequest::from_query(&QueryParams::parse(Some(raw_query)), &USER_SORT, keys).unwrap()
}

fn cursor_for(keys: &KeyRing, sort: &str, value: SortValue) -> String {
    let spec = USER_SORT.parse(Some(sort)).unwrap();
    let id = UserId::generate(&TestClock::new(
        time::macros::datetime!(2026-09-25 12:00 UTC),
    ));
    encode_cursor(keys, &spec, &CursorKey::new(value, id))
}

#[tokio::test]
async fn it_admin_user_page_queries_use_the_sort_indexes() {
    let database = Database::open().await;
    let keys = keys();
    let filter = UserFilter::default();

    let default_plan = database
        .plan(|query| push_list(query, &filter, &page(&keys, "")))
        .await;
    assert!(
        default_plan.contains("USING INDEX ix_users_created_at"),
        "{default_plan}"
    );
    assert!(!default_plan.contains("SCAN l"), "{default_plan}");

    let cursor = cursor_for(&keys, "createdAt:desc", SortValue::Text(NOW.to_owned()));
    let next_plan = database
        .plan(|query| push_list(query, &filter, &page(&keys, &format!("cursor={cursor}"))))
        .await;
    assert!(next_plan.contains("ix_users_created_at"), "{next_plan}");

    let used_plan = database
        .plan(|query| push_list(query, &filter, &page(&keys, "sort=usedBytes:desc")))
        .await;
    assert!(
        used_plan.contains("USING INDEX ix_users_used_bytes"),
        "{used_plan}"
    );

    let username_plan = database
        .plan(|query| push_list(query, &filter, &page(&keys, "sort=username:asc")))
        .await;
    assert!(
        username_plan.contains("ux_users_username_normalized"),
        "{username_plan}"
    );

    let email_plan = database
        .plan(|query| push_list(query, &filter, &page(&keys, "sort=email:asc")))
        .await;
    assert!(
        email_plan.contains("ux_users_email_normalized"),
        "{email_plan}"
    );

    let joined = database
        .plan(|query| push_list(query, &filter, &page(&keys, "")))
        .await;
    assert!(
        joined.contains("SEARCH l USING") || joined.contains("account_lockouts"),
        "{joined}"
    );
}

#[tokio::test]
async fn it_admin_user_search_uses_the_normalized_identity_indexes() {
    let database = Database::open().await;
    let keys = keys();
    let query = SearchQuery::parse(Some("ada")).unwrap();
    let filter = UserFilter {
        role: Some(Role::User),
        status: Some(UserStatus::Active),
        search: SearchPrefix::parse(query.as_ref()).unwrap(),
    };
    let list = database
        .plan(|builder| push_list(builder, &filter, &page(&keys, "")))
        .await;
    assert!(list.contains("ux_users_username_normalized"), "{list}");
    assert!(list.contains("ux_users_email_normalized"), "{list}");

    let count = database.plan(|builder| push_count(builder, &filter)).await;
    assert!(count.contains("ux_users_username_normalized"), "{count}");
    assert!(count.contains("ux_users_email_normalized"), "{count}");

    let unfiltered = database
        .plan(|builder| push_count(builder, &UserFilter::default()))
        .await;
    assert!(unfiltered.contains("COVERING INDEX"), "{unfiltered}");
}

#[tokio::test]
async fn it_admin_user_resource_counts_are_grouped_indexed_lookups() {
    let database = Database::open().await;
    let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
    let ids: Vec<UserId> = (0..3).map(|_| UserId::generate(&clock)).collect();
    let expectations = [
        (Counted::Files, "COVERING INDEX ix_files_owner_"),
        (Counted::Shares, "ix_shares_owner"),
        (Counted::ReverseShares, "ix_reverse_shares_owner"),
        (Counted::ReceivedFiles, "ix_received_owner"),
        (Counted::IdentityLinks, "identity_links"),
    ];
    assert_eq!(expectations.len(), Counted::ALL.len());
    for (counted, index) in expectations {
        let plan = database
            .plan(|query| push_grouped_count(query, counted, &ids))
            .await;
        assert!(plan.contains("SEARCH"), "{counted:?}: {plan}");
        assert!(plan.contains(index), "{counted:?}: {plan}");
        assert!(!plan.contains("SCAN"), "{counted:?}: {plan}");
    }
}

#[tokio::test]
async fn it_admin_user_detail_counts_use_the_user_indexes() {
    let database = Database::open().await;
    let trusted: Vec<String> = sqlx::query(&format!("EXPLAIN QUERY PLAN {COUNT_LISTED}"))
        .bind("user")
        .bind(NOW)
        .fetch_all(database.pools.reader().executor())
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect();
    let trusted = trusted.join("\n");
    assert!(trusted.contains("ix_trusted_devices_user"), "{trusted}");
    assert!(!trusted.contains("SCAN"), "{trusted}");
}

fn account(password_hash: Option<&str>) -> User {
    let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
    User {
        id: UserId::generate(&clock),
        email: "bea@example.test".to_owned(),
        email_normalized: "bea@example.test".to_owned(),
        username: "bea".to_owned(),
        username_normalized: "bea".to_owned(),
        first_name: "Bea".to_owned(),
        last_name: "Baker".to_owned(),
        password_hash: password_hash
            .map(|hash| crate::domain::secret::Secret::new(hash.to_owned())),
        password_updated_at: None,
        must_change_password: false,
        role: Role::User,
        is_active: true,
        deactivated_at: None,
        totp_enabled: false,
        quota: QuotaOverride::Inherit,
        used_bytes: ByteSize::ZERO,
        created_at: at(EARLIER),
        updated_at: at(EARLIER),
        created_by: None,
    }
}

#[test]
fn unit_admin_password_reset_precondition_errors_map_to_canonical_codes() {
    let local = account(Some("$argon2id$stored"));
    let sso = account(None);

    assert!(check_reset_target(&local, true).is_ok());
    assert!(matches!(
        check_reset_target(&local, false),
        Err(AdminUserError::PasswordLoginDisabled)
    ));
    for password_login in [true, false] {
        assert!(matches!(
            check_reset_target(&sso, password_login),
            Err(AdminUserError::NoLocalAuth)
        ));
    }
    for (error, status, code) in [
        (AdminUserError::NoLocalAuth, 409, "USER_HAS_NO_LOCAL_AUTH"),
        (
            AdminUserError::PasswordLoginDisabled,
            403,
            "AUTH_PASSWORD_LOGIN_DISABLED",
        ),
    ] {
        let api = error.api_error();
        assert_eq!(api.status().as_u16(), status, "{code}");
        assert_eq!(api.code().as_str(), code);
    }
}

#[test]
fn unit_admin_password_reset_response_never_debug_prints_the_password() {
    let reset = AdminPasswordReset::new("temporary-sentinel-Zx81".to_owned());
    assert!(!format!("{reset:?}").contains("temporary-sentinel"));
    assert_eq!(
        serde_json::to_value(&reset).unwrap(),
        json!({ "temporaryPassword": "temporary-sentinel-Zx81", "mustChangePassword": true })
    );
}

#[test]
fn unit_admin_quota_policy_keeps_inherit_unlimited_and_bytes_distinct() {
    let cases = [
        (
            QuotaOverride::Inherit,
            None,
            0,
            json!({
                "mode": "inherit", "quotaBytes": null, "instanceDefaultQuotaBytes": null,
                "effectiveQuotaBytes": null, "belowCurrentUsage": false
            }),
        ),
        (
            QuotaOverride::Inherit,
            Some(1000),
            2000,
            json!({
                "mode": "inherit", "quotaBytes": null, "instanceDefaultQuotaBytes": 1000,
                "effectiveQuotaBytes": 1000, "belowCurrentUsage": true
            }),
        ),
        (
            QuotaOverride::Unlimited,
            Some(1000),
            i64::MAX,
            json!({
                "mode": "unlimited", "quotaBytes": null, "instanceDefaultQuotaBytes": 1000,
                "effectiveQuotaBytes": null, "belowCurrentUsage": false
            }),
        ),
        (
            QuotaOverride::Bytes(bytes(0)),
            Some(1000),
            0,
            json!({
                "mode": "bytes", "quotaBytes": 0, "instanceDefaultQuotaBytes": 1000,
                "effectiveQuotaBytes": 0, "belowCurrentUsage": false
            }),
        ),
        (
            QuotaOverride::Bytes(bytes(0)),
            None,
            1,
            json!({
                "mode": "bytes", "quotaBytes": 0, "instanceDefaultQuotaBytes": null,
                "effectiveQuotaBytes": 0, "belowCurrentUsage": true
            }),
        ),
    ];
    for (quota, default, used, expected) in cases {
        let policy = AdminUserQuota::new(quota, default.map(bytes), bytes(used));
        assert_eq!(serde_json::to_value(&policy).unwrap(), expected);
    }
}
