pub mod support;

use std::collections::BTreeSet;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::Value;

use support::client::{v7, Creds, Db, Http, SessionSpec, EPOCH, FAR_FUTURE};
use support::TestApplication;

const USERS: &str = "/api/v1/admin/users";
const ABSENT_ID: &str = "0192f3a1-0000-7000-8000-00000000abcd";
const PAST: &str = "2025-12-31T00:00:00.000Z";
const SOON: &str = "2026-01-01T00:10:00.000Z";
const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$aGFzaGhhc2hoYXNo";

#[derive(Clone)]
struct Seed {
    id: String,
    username: String,
    role: &'static str,
    active: bool,
    quota_mode: &'static str,
    quota_bytes: Option<i64>,
    used_bytes: i64,
    created_at: String,
    password: bool,
    totp: bool,
    must_change: bool,
    last_login_at: Option<String>,
}

impl Seed {
    fn new(n: u64, username: &str) -> Self {
        Self {
            id: v7(n),
            username: username.to_owned(),
            role: "user",
            active: true,
            quota_mode: "inherit",
            quota_bytes: None,
            used_bytes: 0,
            created_at: EPOCH.to_owned(),
            password: true,
            totp: false,
            must_change: false,
            last_login_at: None,
        }
    }

    fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }

    fn inactive(mut self) -> Self {
        self.active = false;
        self
    }

    fn quota(mut self, mode: &'static str, bytes: Option<i64>, used: i64) -> Self {
        self.quota_mode = mode;
        self.quota_bytes = bytes;
        self.used_bytes = used;
        self
    }

    fn created(mut self, at: &str) -> Self {
        self.created_at = at.to_owned();
        self
    }
}

struct World {
    app: TestApplication,
    http: Http,
    db: Db,
    admin: Creds,
    admin_id: String,
}

impl World {
    async fn start(name: &str) -> Result<Self> {
        let app = TestApplication::start(name).await?;
        let http = Http::new(app.url("/")?)?;
        let db = Db::new(app.data_dir());
        let admin = http.setup_admin().await?;
        let admin_id = db
            .scalar_string("SELECT id FROM users WHERE username = 'ada'")
            .await?;
        Ok(Self {
            app,
            http,
            db,
            admin,
            admin_id,
        })
    }

    async fn seed(&self, seed: &Seed) -> Result<()> {
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO users (id, email, email_normalized, username, username_normalized,
                first_name, last_name, password_hash, must_change_password, role, is_active,
                deactivated_at, totp_enabled, quota_override_mode, quota_bytes, used_bytes,
                last_login_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'First', 'Last', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?16)",
        )
        .bind(&seed.id)
        .bind(format!("{}@Example.test", seed.username))
        .bind(format!("{}@example.test", seed.username.to_lowercase()))
        .bind(&seed.username)
        .bind(seed.username.to_lowercase())
        .bind(seed.password.then_some(HASH))
        .bind(i64::from(seed.must_change))
        .bind(seed.role)
        .bind(i64::from(seed.active))
        .bind((!seed.active).then_some(EPOCH))
        .bind(i64::from(seed.totp))
        .bind(seed.quota_mode)
        .bind(seed.quota_bytes)
        .bind(seed.used_bytes)
        .bind(seed.last_login_at.as_deref())
        .bind(&seed.created_at)
        .execute(&mut connection)
        .await
        .context("seed user")?;
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let reply = self
            .http
            .get(path, Some(&self.admin))
            .await?
            .expect(StatusCode::OK)?;
        reply.json()
    }

    async fn fails(&self, path: &str, status: StatusCode, code: &str) -> Result<Value> {
        let reply = self.http.get(path, Some(&self.admin)).await?;
        ensure!(
            reply.status == status && reply.error_code()? == code,
            "{path}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        reply.json()
    }

    async fn walk(&self, query: &str) -> Result<(Vec<Value>, Vec<Value>)> {
        let mut items = Vec::new();
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let path = match &cursor {
                Some(cursor) => format!("{USERS}?{query}&cursor={cursor}"),
                None => format!("{USERS}?{query}"),
            };
            let page = self.get(&path).await?;
            items.extend(page["items"].as_array().context("items")?.clone());
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            pages.push(page);
            if cursor.is_none() {
                return Ok((items, pages));
            }
            ensure!(pages.len() < 100, "pagination does not terminate");
        }
    }

    async fn storage_object(&self, n: u64, size: i64) -> Result<String> {
        let id = format!("object-{n}");
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                created_at, updated_at, finalized_at)
             VALUES (?1, ?2, 'local', ?3, 'active', 1, ?4, ?4, ?4)",
        )
        .bind(&id)
        .bind(format!("objects/00/00/{n:032x}"))
        .bind(size)
        .bind(EPOCH)
        .execute(&mut connection)
        .await
        .context("seed storage object")?;
        Ok(id)
    }

    async fn file(&self, owner: &str, n: u64, size: i64) -> Result<()> {
        let object = self.storage_object(n, size).await?;
        self.db
            .execute_bound(
                "INSERT INTO files (id, owner_id, storage_object_id, name, name_normalized,
                    size_bytes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?6)",
                &[
                    &v7(1_000 + n),
                    owner,
                    &object,
                    &format!("file-{n}.bin"),
                    &size.to_string(),
                    EPOCH,
                ],
            )
            .await
    }

    async fn share(&self, owner: &str, n: u64) -> Result<()> {
        self.db
            .execute_bound(
                "INSERT INTO shares (id, owner_id, public_id, alias, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                &[
                    &v7(2_000 + n),
                    owner,
                    &format!("public-share-{n:08}"),
                    &format!("share-{n}"),
                    EPOCH,
                ],
            )
            .await
    }

    async fn reverse_share(&self, owner: &str, n: u64) -> Result<String> {
        let id = v7(3_000 + n);
        self.db
            .execute_bound(
                "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                &[
                    &id,
                    owner,
                    &format!("public-reverse-{n:08}"),
                    &format!("reverse-{n}"),
                    EPOCH,
                ],
            )
            .await?;
        Ok(id)
    }

    async fn received(&self, owner: &str, reverse_share: &str, n: u64, size: i64) -> Result<()> {
        let object = self.storage_object(n, size).await?;
        self.db
            .execute_bound(
                "INSERT INTO received_files (id, owner_id, reverse_share_id, storage_object_id,
                    name, name_normalized, size_bytes, received_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?7)",
                &[
                    &v7(4_000 + n),
                    owner,
                    reverse_share,
                    &object,
                    &format!("received-{n}.bin"),
                    &size.to_string(),
                    EPOCH,
                ],
            )
            .await
    }

    async fn trusted_device(&self, user: &str, n: u64, expires: &str, revoked: bool) -> Result<()> {
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at,
                revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(v7(5_000 + n))
        .bind(user)
        .bind(format!("{n:064x}"))
        .bind(EPOCH)
        .bind(expires)
        .bind(revoked.then_some(EPOCH))
        .execute(&mut connection)
        .await
        .context("seed trusted device")?;
        Ok(())
    }

    async fn lockout(&self, user: &str, failed: u32, locked_until: Option<&str>) -> Result<()> {
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO account_lockouts (user_id, failed_count, locked_until, lock_count,
                updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(user)
        .bind(i64::from(failed))
        .bind(locked_until)
        .bind(i64::from(locked_until.is_some()))
        .bind(EPOCH)
        .execute(&mut connection)
        .await
        .context("seed lockout")?;
        Ok(())
    }

    async fn provider(&self, n: u64, key: &str) -> Result<String> {
        let id = v7(6_000 + n);
        self.db
            .execute_bound(
                "INSERT INTO identity_providers (id, key, display_name, kind, client_id,
                    client_secret_ciphertext, client_secret_nonce, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'oidc', 'client-id-visible-to-nobody',
                    x'deadbeefdeadbeef', zeroblob(24), ?4, ?4)",
                &[&id, key, &format!("Provider {key}"), EPOCH],
            )
            .await?;
        Ok(id)
    }

    async fn link(&self, user: &str, provider: &str, n: u64, state: &str) -> Result<()> {
        self.db
            .execute_bound(
                "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method,
                    state, created_at, suspended_at)
                 VALUES (?1, ?2, ?3, 'subject-secret-value-' || ?1, 'auto_verified_email', ?4, ?5,
                    CASE WHEN ?4 = 'suspended' THEN ?5 ELSE NULL END)",
                &[&v7(7_000 + n), user, provider, state, EPOCH],
            )
            .await
    }
}

fn ids(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default())
        .collect()
}

fn usernames(items: &[Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| item["username"].as_str().unwrap_or_default())
        .collect()
}

fn find<'a>(items: &'a [Value], username: &str) -> Result<&'a Value> {
    items
        .iter()
        .find(|item| item["username"] == username)
        .with_context(|| format!("{username} is not listed"))
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    keys
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_usage_fields() -> Result<()> {
    let world = World::start("it_admin_user_usage_fields").await?;
    world
        .db
        .execute("UPDATE users SET quota_override_mode = 'bytes', quota_bytes = 1000, used_bytes = 400 WHERE username = 'ada'")
        .await?;
    let below = Seed::new(1, "below").quota("bytes", Some(1_000_000), 250_000);
    let over = Seed::new(2, "over").quota("bytes", Some(100_000), 250_000);
    let unlimited = Seed::new(3, "unlimited").quota("unlimited", None, 5_000_000_000_000);
    let inherit = Seed::new(4, "inherit").quota("inherit", None, 42);
    let materialized = Seed::new(5, "materialized").quota("bytes", Some(2_000), 777);
    let admin_over = Seed::new(6, "adminover")
        .role("admin")
        .quota("bytes", Some(50), 51);
    let admin_unlimited =
        Seed::new(7, "boundless")
            .role("admin")
            .quota("unlimited", None, i64::MAX);
    for seed in [
        &below,
        &over,
        &unlimited,
        &inherit,
        &materialized,
        &admin_over,
        &admin_unlimited,
    ] {
        world.seed(seed).await?;
    }
    world.file(&materialized.id, 1, 10).await?;
    let reverse = world.reverse_share(&materialized.id, 1).await?;
    world.received(&materialized.id, &reverse, 2, 5).await?;

    let audit_before = world
        .db
        .scalar_i64("SELECT COUNT(*) FROM audit_events")
        .await?;

    let listed = world.get(&format!("{USERS}?limit=200")).await?;
    let items = listed["items"].as_array().context("items")?;
    let expectations: [(&str, Value, Value, i64, bool); 8] = [
        ("ada", 1000.into(), 1000.into(), 400, false),
        ("below", 1_000_000.into(), 1_000_000.into(), 250_000, false),
        ("over", 100_000.into(), 100_000.into(), 250_000, true),
        (
            "unlimited",
            Value::Null,
            Value::Null,
            5_000_000_000_000,
            false,
        ),
        ("inherit", Value::Null, Value::Null, 42, false),
        ("materialized", 2_000.into(), 2_000.into(), 777, false),
        ("adminover", 50.into(), 50.into(), 51, true),
        (
            "boundless",
            Value::Null,
            Value::Null,
            9_007_199_254_740_991,
            false,
        ),
    ];
    for (username, quota, effective, used, over_quota) in expectations {
        let row = find(items, username)?;
        assert_eq!(row["quotaBytes"], quota, "{username}");
        assert_eq!(row["effectiveQuotaBytes"], effective, "{username}");
        assert_eq!(row["usedBytes"], used, "{username}");
        assert!(
            row["usedBytes"].is_u64(),
            "{username}: bytes are JSON numbers"
        );

        let id = row["id"].as_str().context("id")?;
        let detail = world.get(&format!("{USERS}/{id}")).await?;
        assert_eq!(detail["quotaBytes"], quota, "{username}");
        assert_eq!(detail["effectiveQuotaBytes"], effective, "{username}");
        assert_eq!(detail["usedBytes"], used, "{username}");
        assert_eq!(detail["overQuota"], over_quota, "{username}");
        for field in ["quotaBytes", "effectiveQuotaBytes"] {
            assert!(detail[field].is_null() || detail[field].is_u64());
        }
    }

    let stored: i64 = world
        .db
        .scalar_i64(&format!(
            "SELECT used_bytes FROM users WHERE id = '{}'",
            materialized.id
        ))
        .await?;
    assert_eq!(stored, 777);
    assert_eq!(
        world
            .db
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM files WHERE owner_id = '{}'",
                materialized.id
            ))
            .await?,
        1
    );
    let row = find(items, "materialized")?;
    assert_eq!(
        row["usedBytes"], 777,
        "usedBytes is the materialized counter, not files (10) + received (5)"
    );
    assert_eq!(row["counts"]["files"], 1);
    assert_eq!(row["counts"]["receivedFiles"], 1);

    let text = listed.to_string().to_lowercase();
    for forbidden in ["disk", "capacity", "free", "statvfs", "bucket"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events")
            .await?,
        audit_before,
        "read-only user administration writes no audit rows"
    );
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_list_contract_and_headers() -> Result<()> {
    let world = World::start("it_admin_user_list_contract").await?;
    let mut bea = Seed::new(1, "Bea").role("admin");
    bea.totp = true;
    bea.must_change = true;
    bea.last_login_at = Some(SOON.to_owned());
    world.seed(&bea).await?;
    let mut sso = Seed::new(2, "sso");
    sso.password = false;
    world.seed(&sso).await?;

    let reply = world
        .http
        .get(USERS, Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?;
    assert_eq!(
        reply
            .headers
            .get("cache-control")
            .map(|value| value.as_bytes()),
        Some(b"no-store".as_slice())
    );
    let page = reply.json()?;
    assert_eq!(keys(&page), ["items", "nextCursor", "totalCount"]);
    assert_eq!(page["totalCount"], 3);
    assert!(page["nextCursor"].is_null());
    let items = page["items"].as_array().context("items")?;
    assert_eq!(
        keys(&items[0]),
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
    let row = find(items, "Bea")?;
    assert_eq!(row["email"], "Bea@Example.test");
    assert_eq!(row["role"], "admin");
    assert_eq!(row["twoFactorEnabled"], true);
    assert_eq!(row["mustChangePassword"], true);
    assert_eq!(row["hasLocalPassword"], true);
    assert_eq!(row["lastLoginAt"], SOON);
    assert_eq!(row["pendingEmail"], Value::Null);
    let sso_row = find(items, "sso")?;
    assert_eq!(sso_row["hasLocalPassword"], false);
    let text = page.to_string().to_lowercase();
    for forbidden in ["argon2", "password_hash", "passwordhash", "token", "secret"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_list_is_keyset_stable_under_concurrent_inserts() -> Result<()> {
    let world = World::start("it_admin_user_list_keyset").await?;
    let stamps = [
        "2026-01-01T00:00:01.000Z",
        "2026-01-01T00:00:02.000Z",
        "2026-01-01T00:00:03.000Z",
        "2026-01-01T00:00:04.000Z",
        "2026-01-01T00:00:04.000Z",
        "2026-01-01T00:00:04.000Z",
        "2026-01-01T00:00:05.000Z",
        "2026-01-01T00:00:06.000Z",
    ];
    let mut original = BTreeSet::new();
    original.insert(world.admin_id.clone());
    for (index, stamp) in stamps.iter().enumerate() {
        let seed = Seed::new(10 + index as u64, &format!("member{index}")).created(stamp);
        original.insert(seed.id.clone());
        world.seed(&seed).await?;
    }
    let total = original.len();

    let first = world.get(&format!("{USERS}?limit=3")).await?;
    assert_eq!(first["totalCount"], total);
    let cursor = first["nextCursor"]
        .as_str()
        .context("first page has a cursor")?
        .to_owned();
    let mut visited: Vec<String> = first["items"]
        .as_array()
        .context("items")?
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(visited.len(), 3);

    let newer = Seed::new(100, "newer").created("2026-01-01T00:59:00.000Z");
    let older = Seed::new(101, "older").created("2025-12-30T00:00:00.000Z");
    world.seed(&newer).await?;
    world.seed(&older).await?;

    let mut next = Some(cursor);
    while let Some(cursor) = next {
        let page = world
            .get(&format!("{USERS}?limit=3&cursor={cursor}"))
            .await?;
        assert_eq!(page["totalCount"], total + 2);
        visited.extend(
            page["items"]
                .as_array()
                .context("items")?
                .iter()
                .map(|item| item["id"].as_str().unwrap_or_default().to_owned()),
        );
        next = page["nextCursor"].as_str().map(str::to_owned);
    }

    let unique: BTreeSet<&String> = visited.iter().collect();
    assert_eq!(unique.len(), visited.len(), "a row was returned twice");
    for id in &original {
        assert!(visited.contains(id), "{id} was skipped");
    }
    assert!(visited.contains(&older.id), "a later, older row is reached");
    assert!(
        !visited.contains(&newer.id),
        "a row inserted before the cursor is not revisited"
    );
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_list_sorts_by_every_allowlisted_field_with_id_tiebreak() -> Result<()> {
    let world = World::start("it_admin_user_list_sorts").await?;
    for (index, (name, used, stamp)) in [
        ("alpha", 30, "2026-01-01T00:00:03.000Z"),
        ("Bravo", 10, "2026-01-01T00:00:03.000Z"),
        ("charlie", 30, "2026-01-01T00:00:01.000Z"),
        ("delta", 20, "2026-01-01T00:00:02.000Z"),
        ("echo", 10, "2026-01-01T00:00:03.000Z"),
    ]
    .into_iter()
    .enumerate()
    {
        let seed = Seed::new(20 + index as u64, name)
            .quota("inherit", None, used)
            .created(stamp);
        world.seed(&seed).await?;
    }
    for field in ["createdAt", "usedBytes", "username", "email"] {
        for direction in ["asc", "desc"] {
            let sort = format!("{field}:{direction}");
            let (items, pages) = world.walk(&format!("limit=2&sort={sort}")).await?;
            assert_eq!(items.len(), 6, "{sort}");
            assert!(pages.len() >= 3, "{sort}");
            let mut expected = items.clone();
            expected.sort_by(|left, right| {
                let key = |item: &Value| -> (String, u64, String) {
                    let text = |name: &str| item[name].as_str().unwrap_or_default().to_lowercase();
                    match field {
                        "usedBytes" => (
                            String::new(),
                            item["usedBytes"].as_u64().unwrap_or_default(),
                            text("id"),
                        ),
                        "username" => (text("username"), 0, text("id")),
                        "email" => (text("email"), 0, text("id")),
                        _ => (text("createdAt"), 0, text("id")),
                    }
                };
                let ordering = key(left).cmp(&key(right));
                if direction == "asc" {
                    ordering
                } else {
                    ordering.reverse()
                }
            });
            assert_eq!(ids(&items), ids(&expected), "{sort}");
            let unique: BTreeSet<&str> = ids(&items).into_iter().collect();
            assert_eq!(unique.len(), 6, "{sort}");
            for page in &pages {
                assert_eq!(page["totalCount"], 6, "{sort}");
            }
            assert!(pages.last().context("pages")?["nextCursor"].is_null());
        }
    }
    let default = world.get(USERS).await?;
    let explicit = world.get(&format!("{USERS}?sort=createdAt:desc")).await?;
    assert_eq!(default, explicit);
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_list_rejects_tampered_and_mismatched_cursors_and_bad_queries() -> Result<()>
{
    let world = World::start("it_admin_user_list_rejections").await?;
    for index in 0..4 {
        world
            .seed(&Seed::new(30 + index, &format!("user{index}")))
            .await?;
    }
    let page = world.get(&format!("{USERS}?limit=2")).await?;
    let cursor = page["nextCursor"].as_str().context("cursor")?.to_owned();
    let followed = world
        .get(&format!("{USERS}?limit=2&cursor={cursor}"))
        .await?;
    assert_eq!(followed["items"].as_array().map(Vec::len), Some(2));

    let mut tampered = cursor.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered)?;
    let rejected = world
        .fails(
            &format!("{USERS}?limit=2&cursor={tampered}"),
            StatusCode::BAD_REQUEST,
            "CURSOR_INVALID",
        )
        .await?;
    assert_eq!(rejected["error"]["details"], serde_json::json!({}));
    world
        .fails(
            &format!("{USERS}?limit=2&sort=usedBytes:desc&cursor={cursor}"),
            StatusCode::BAD_REQUEST,
            "CURSOR_INVALID",
        )
        .await?;
    world
        .fails(
            &format!("{USERS}?limit=2&sort=createdAt:asc&cursor={cursor}"),
            StatusCode::BAD_REQUEST,
            "CURSOR_INVALID",
        )
        .await?;
    world
        .fails(
            &format!("{USERS}?cursor=not-a-cursor"),
            StatusCode::BAD_REQUEST,
            "CURSOR_INVALID",
        )
        .await?;

    for (query, field) in [
        ("limit=0", "limit"),
        ("limit=201", "limit"),
        ("limit=abc", "limit"),
        ("limit=1&limit=2", "limit"),
        ("sort=passwordHash:asc", "sort"),
        ("sort=createdAt", "sort"),
        ("sort=createdAt:up", "sort"),
        ("sort=lastLoginAt:asc", "sort"),
        ("role=root", "role"),
        ("role=Admin", "role"),
        ("role=admin&role=user", "role"),
        ("status=disabled", "status"),
        ("status=", "status"),
        ("q=a", "q"),
        ("q=%20%20", "q"),
    ] {
        let error = world
            .fails(
                &format!("{USERS}?{query}"),
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_ERROR",
            )
            .await?;
        assert_eq!(
            error["error"]["details"]["fields"],
            serde_json::json!([field]),
            "{query}"
        );
    }
    let long = "x".repeat(129);
    world
        .fails(
            &format!("{USERS}?q={long}"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        )
        .await?;
    let maximum = world.get(&format!("{USERS}?limit=200")).await?;
    assert_eq!(maximum["items"].as_array().map(Vec::len), Some(5));
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_filters_and_total_count_apply_before_pagination() -> Result<()> {
    let world = World::start("it_admin_user_filters").await?;
    world.seed(&Seed::new(1, "bea")).await?;
    world.seed(&Seed::new(2, "cyrus").inactive()).await?;
    world
        .seed(&Seed::new(3, "dee").role("admin").inactive())
        .await?;
    world.seed(&Seed::new(4, "Beatrix")).await?;
    world.seed(&Seed::new(5, "zed")).await?;

    let names = |page: &Value| -> Vec<String> {
        let mut names: Vec<String> = page["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item["username"].as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    };
    for (query, expected) in [
        ("", vec!["Beatrix", "ada", "bea", "cyrus", "dee", "zed"]),
        ("role=user", vec!["Beatrix", "bea", "cyrus", "zed"]),
        ("role=admin", vec!["ada", "dee"]),
        ("status=active", vec!["Beatrix", "ada", "bea", "zed"]),
        ("status=inactive", vec!["cyrus", "dee"]),
        ("role=user&status=inactive", vec!["cyrus"]),
        ("role=admin&status=inactive", vec!["dee"]),
        ("role=admin&status=active", vec!["ada"]),
        ("q=be", vec!["Beatrix", "bea"]),
        ("q=BEA", vec!["Beatrix", "bea"]),
        ("q=bea&status=active&role=user", vec!["Beatrix", "bea"]),
        ("q=cyrus%40ex", vec!["cyrus"]),
        ("q=ADA%40", vec!["ada"]),
        ("q=trix", vec![]),
        ("q=zz", vec![]),
    ] {
        let page = world.get(&format!("{USERS}?{query}")).await?;
        assert_eq!(names(&page), expected, "{query}");
        assert_eq!(page["totalCount"], expected.len(), "{query}");
    }

    let (items, pages) = world.walk("role=user&limit=1").await?;
    assert_eq!(items.len(), 4);
    assert_eq!(pages.len(), 4);
    for page in &pages {
        assert_eq!(page["totalCount"], 4);
    }
    assert_eq!(usernames(&items).len(), 4);
    let (items, pages) = world.walk("status=inactive&limit=1").await?;
    assert_eq!(items.len(), 2);
    assert_eq!(pages[0]["totalCount"], 2);
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_counts_lockout_and_links_agree_between_list_and_detail() -> Result<()> {
    let world = World::start("it_admin_user_detail").await?;
    let target = Seed::new(1, "target");
    let other = Seed::new(2, "other");
    let empty = Seed::new(3, "empty");
    let lapsed = Seed::new(4, "lapsed");
    let mut sso = Seed::new(5, "ssoonly");
    sso.password = false;
    for seed in [&target, &other, &empty, &lapsed, &sso] {
        world.seed(seed).await?;
    }

    world.file(&target.id, 1, 10).await?;
    world.file(&target.id, 2, 20).await?;
    for n in 1..=3 {
        world.share(&target.id, n).await?;
    }
    let target_reverse = world.reverse_share(&target.id, 1).await?;
    for n in 3..=6 {
        world.received(&target.id, &target_reverse, n, 1).await?;
    }
    for n in 10..=14 {
        world.file(&other.id, n, 1).await?;
    }
    for n in 10..=11 {
        world.share(&other.id, n).await?;
    }
    let other_reverse = world.reverse_share(&other.id, 10).await?;
    world.reverse_share(&other.id, 11).await?;
    world.received(&other.id, &other_reverse, 20, 1).await?;

    let google = world.provider(1, "google").await?;
    let corp = world.provider(2, "corp").await?;
    world.link(&target.id, &google, 1, "active").await?;
    world.link(&target.id, &corp, 2, "suspended").await?;
    world.link(&other.id, &google, 3, "active").await?;

    world.lockout(&target.id, 5, Some(SOON)).await?;
    world.lockout(&lapsed.id, 5, Some(PAST)).await?;
    world.lockout(&other.id, 2, None).await?;

    for _ in 0..3 {
        world
            .db
            .insert_session(&SessionSpec::active(&target.id))
            .await?;
    }
    world
        .db
        .insert_session(&SessionSpec {
            idle_expires_at: PAST,
            ..SessionSpec::active(&target.id)
        })
        .await?;
    world
        .db
        .insert_session(&SessionSpec {
            state: "revoked",
            ..SessionSpec::active(&target.id)
        })
        .await?;
    world
        .db
        .insert_session(&SessionSpec {
            state: "mfa_pending",
            ..SessionSpec::active(&target.id)
        })
        .await?;
    world
        .db
        .insert_session(&SessionSpec::active(&other.id))
        .await?;

    world
        .trusted_device(&target.id, 1, FAR_FUTURE, false)
        .await?;
    world
        .trusted_device(&target.id, 2, FAR_FUTURE, false)
        .await?;
    world
        .trusted_device(&target.id, 3, FAR_FUTURE, true)
        .await?;
    world.trusted_device(&target.id, 4, PAST, false).await?;
    world
        .trusted_device(&other.id, 5, FAR_FUTURE, false)
        .await?;

    let listed = world.get(&format!("{USERS}?limit=200")).await?;
    let items = listed["items"].as_array().context("items")?;
    let row = find(items, "target")?;
    assert_eq!(
        row["counts"],
        serde_json::json!({ "files": 2, "shares": 3, "reverseShares": 1, "receivedFiles": 4 })
    );
    assert_eq!(row["identityLinkCount"], 2);
    assert_eq!(row["isLockedOut"], true);
    assert_eq!(
        find(items, "other")?["counts"],
        serde_json::json!({ "files": 5, "shares": 2, "reverseShares": 2, "receivedFiles": 1 })
    );
    assert_eq!(find(items, "other")?["identityLinkCount"], 1);
    assert_eq!(find(items, "other")?["isLockedOut"], false);
    assert_eq!(
        find(items, "empty")?["counts"],
        serde_json::json!({ "files": 0, "shares": 0, "reverseShares": 0, "receivedFiles": 0 })
    );
    assert_eq!(find(items, "empty")?["identityLinkCount"], 0);
    assert_eq!(find(items, "lapsed")?["isLockedOut"], false);
    assert_eq!(find(items, "ssoonly")?["hasLocalPassword"], false);
    assert_eq!(find(items, "ada")?["hasLocalPassword"], true);

    for username in ["target", "other", "empty", "lapsed", "ssoonly", "ada"] {
        let list_row = find(items, username)?;
        let id = list_row["id"].as_str().context("id")?;
        let detail = world.get(&format!("{USERS}/{id}")).await?;
        for (field, value) in list_row.as_object().context("row")? {
            assert_eq!(
                &detail[field], value,
                "{username}.{field} differs in detail"
            );
        }
    }

    let detail = world.get(&format!("{USERS}/{}", target.id)).await?;
    assert_eq!(detail["sessionCount"], 3);
    assert_eq!(detail["trustedDeviceCount"], 2);
    assert_eq!(detail["overQuota"], false);
    assert_eq!(
        detail["lockout"],
        serde_json::json!({ "lockedUntil": SOON, "failedCount": 5, "lockCount": 1 })
    );
    let links = detail["identityLinks"].as_array().context("links")?;
    assert_eq!(links.len(), 2);
    assert_eq!(
        links[0],
        serde_json::json!({
            "id": v7(7_001),
            "providerKey": "google",
            "providerName": "Provider google",
            "state": "active",
            "linkMethod": "auto_verified_email",
            "createdAt": EPOCH,
            "lastLoginAt": null,
        })
    );
    assert_eq!(links[1]["providerKey"], "corp");
    assert_eq!(links[1]["state"], "suspended");

    let other_detail = world.get(&format!("{USERS}/{}", other.id)).await?;
    assert_eq!(other_detail["sessionCount"], 1);
    assert_eq!(other_detail["trustedDeviceCount"], 1);
    assert_eq!(
        other_detail["lockout"],
        serde_json::json!({ "lockedUntil": null, "failedCount": 2, "lockCount": 0 })
    );
    let lapsed_detail = world.get(&format!("{USERS}/{}", lapsed.id)).await?;
    assert_eq!(lapsed_detail["lockout"]["lockedUntil"], Value::Null);
    assert_eq!(lapsed_detail["lockout"]["failedCount"], 5);
    let empty_detail = world.get(&format!("{USERS}/{}", empty.id)).await?;
    assert_eq!(empty_detail["identityLinks"], serde_json::json!([]));
    assert_eq!(empty_detail["sessionCount"], 0);
    assert_eq!(empty_detail["trustedDeviceCount"], 0);
    assert_eq!(
        empty_detail["lockout"],
        serde_json::json!({ "lockedUntil": null, "failedCount": 0, "lockCount": 0 })
    );

    let text = detail.to_string().to_lowercase();
    for forbidden in [
        "deadbeef",
        "ciphertext",
        "nonce",
        "client-id",
        "clientid",
        "subject-secret-value",
        "argon2",
        "hash",
        "secret",
        "token",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_sessions_are_target_scoped_paginated_and_safe() -> Result<()> {
    let world = World::start("it_admin_user_sessions").await?;
    let target = Seed::new(1, "target");
    let other = Seed::new(2, "other");
    world.seed(&target).await?;
    world.seed(&other).await?;

    let stamps = [
        "2026-01-01T00:00:01.000Z",
        "2026-01-01T00:00:02.000Z",
        "2026-01-01T00:00:03.000Z",
        "2026-01-01T00:00:04.000Z",
        "2026-01-01T00:00:05.000Z",
    ];
    let mut target_sessions = Vec::new();
    for (index, stamp) in stamps.iter().enumerate() {
        let (id, _) = world
            .db
            .insert_session(&SessionSpec {
                last_seen_at: stamp,
                ip: Some("203.0.113.9"),
                user_agent: Some("Mozilla/5.0 (test)"),
                auth_method: if index == 4 { "external" } else { "password" },
                ..SessionSpec::active(&target.id)
            })
            .await?;
        target_sessions.push(id);
    }
    world
        .db
        .insert_session(&SessionSpec {
            idle_expires_at: PAST,
            ..SessionSpec::active(&target.id)
        })
        .await?;
    world
        .db
        .insert_session(&SessionSpec {
            state: "revoked",
            ..SessionSpec::active(&target.id)
        })
        .await?;
    let (foreign, _) = world
        .db
        .insert_session(&SessionSpec::active(&other.id))
        .await?;

    let path = format!("{USERS}/{}/sessions", target.id);
    let first = world.get(&format!("{path}?limit=2")).await?;
    assert_eq!(first["totalCount"], 5);
    assert_eq!(first["items"].as_array().map(Vec::len), Some(2));
    let cursor = first["nextCursor"].as_str().context("cursor")?.to_owned();
    assert_eq!(
        keys(&first["items"][0]),
        [
            "absoluteExpiresAt",
            "createdAt",
            "expiresAt",
            "id",
            "ipAddress",
            "isCurrent",
            "lastSeenAt",
            "origin",
            "userAgent",
        ]
    );

    let mut seen: Vec<Value> = first["items"].as_array().context("items")?.clone();
    let mut next = Some(cursor);
    while let Some(cursor) = next {
        let page = world
            .get(&format!("{path}?limit=2&cursor={cursor}"))
            .await?;
        assert_eq!(page["totalCount"], 5);
        seen.extend(page["items"].as_array().context("items")?.clone());
        next = page["nextCursor"].as_str().map(str::to_owned);
    }
    let seen_ids: Vec<&str> = ids(&seen);
    let mut expected: Vec<&str> = target_sessions.iter().map(String::as_str).collect();
    expected.reverse();
    assert_eq!(seen_ids, expected, "lastSeenAt:desc is the default order");
    assert!(!seen_ids.contains(&foreign.as_str()));
    for item in &seen {
        assert_eq!(item["isCurrent"], false);
        assert_eq!(item["ipAddress"], "203.0.113.9");
        assert_eq!(item["userAgent"], "Mozilla/5.0 (test)");
    }
    assert_eq!(seen[0]["origin"], "external");
    assert_eq!(seen[1]["origin"], "password");

    let ascending = world.get(&format!("{path}?sort=lastSeenAt:asc")).await?;
    let mut ascending_ids: Vec<&str> = ids(ascending["items"].as_array().context("items")?);
    ascending_ids.reverse();
    assert_eq!(ascending_ids, seen_ids);

    let body = serde_json::to_string(&seen)?;
    let hashes = world
        .db
        .scalar_string("SELECT group_concat(token_hash || csrf_token_hash, ',') FROM sessions")
        .await?;
    for hash in hashes.split(',').flat_map(|pair| {
        [
            pair.get(..64).unwrap_or_default(),
            pair.get(64..).unwrap_or_default(),
        ]
    }) {
        assert!(!body.contains(hash), "a session hash leaked");
    }
    let lowered = body.to_lowercase();
    for forbidden in ["hash", "token", "csrf", "userid"] {
        assert!(!lowered.contains(forbidden), "{forbidden} in {lowered}");
    }

    let own = world
        .get(&format!("{USERS}/{}/sessions", world.admin_id))
        .await?;
    assert_eq!(own["totalCount"], 1);
    assert_eq!(own["items"][0]["isCurrent"], true);

    world
        .fails(
            &format!("{path}?sort=createdAt:asc"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        )
        .await?;
    world
        .fails(
            &format!("{path}?limit=0"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        )
        .await?;
    world
        .fails(
            &format!("{path}?cursor=bogus"),
            StatusCode::BAD_REQUEST,
            "CURSOR_INVALID",
        )
        .await?;
    let sessions_before = world.db.scalar_i64("SELECT COUNT(*) FROM sessions").await?;
    let deleted = world
        .http
        .send(Method::DELETE, &path, Some(&world.admin), None)
        .await?;
    assert!(
        deleted.status == StatusCode::METHOD_NOT_ALLOWED,
        "DELETE on the admin sessions collection is not part of M11-T01: {} {}",
        deleted.status,
        deleted.body
    );
    assert_eq!(
        world.db.scalar_i64("SELECT COUNT(*) FROM sessions").await?,
        sessions_before
    );
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_unknown_targets_answer_user_not_found() -> Result<()> {
    let world = World::start("it_admin_user_not_found").await?;
    for path in [
        format!("{USERS}/{ABSENT_ID}"),
        format!("{USERS}/{ABSENT_ID}/sessions"),
        format!("{USERS}/not-a-uuid"),
        format!("{USERS}/not-a-uuid/sessions"),
        format!("{USERS}/{}", ABSENT_ID.to_uppercase()),
    ] {
        let error = world
            .fails(&path, StatusCode::NOT_FOUND, "USER_NOT_FOUND")
            .await?;
        assert_eq!(error["error"]["details"], serde_json::json!({}), "{path}");
    }
    let known = world.get(&format!("{USERS}/{}", world.admin_id)).await?;
    assert_eq!(known["username"], "ada");
    assert_eq!(known["id"], world.admin_id.as_str());
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_reads_are_rate_limited_per_session() -> Result<()> {
    let world = World::start("it_admin_user_rate_limit").await?;
    let path = format!("{USERS}/{ABSENT_ID}");
    for attempt in 0..300 {
        let reply = world.http.get(&path, Some(&world.admin)).await?;
        ensure!(
            reply.status == StatusCode::NOT_FOUND,
            "request {attempt} answered {} {}",
            reply.status,
            reply.body
        );
    }
    let limited = world.http.get(&path, Some(&world.admin)).await?;
    ensure!(
        limited.status == StatusCode::TOO_MANY_REQUESTS && limited.error_code()? == "RATE_LIMITED",
        "{} {}",
        limited.status,
        limited.body
    );
    ensure!(limited.json()?["error"]["details"]["scope"] == "rl.read");

    let (_, second) = world
        .db
        .insert_session(&SessionSpec::active(&world.admin_id))
        .await?;
    world
        .http
        .get(USERS, Some(&second))
        .await?
        .expect(StatusCode::OK)?;
    world.app.shutdown().await;
    Ok(())
}
