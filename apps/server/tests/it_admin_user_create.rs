pub mod support;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sqlx::{Column, Row};
use tokio::task::JoinSet;

use support::client::{Creds, Db, Http, Reply, PASSWORD};
use support::TestApplication;

const USERS: &str = "/api/v1/admin/users";
const LOGIN: &str = "/api/v1/auth/login";
const ME: &str = "/api/v1/auth/me";
const PROFILE: &str = "/api/v1/profile";
const PROFILE_PASSWORD: &str = "/api/v1/profile/password";
const TEMPORARY: &str = "a temporary passphrase";
const REPLACEMENT: &str = "a replacement passphrase";
const KEY: &str = "admin-user-key-0001-aaaaaaaaaaaaa";
const ABSENT_ID: &str = "0192f3a1-0000-7000-8000-00000000abcd";
const STORED_KEY: &str = "route_template = '/api/v1/admin/users'";

struct World {
    app: TestApplication,
    http: Arc<Http>,
    db: Db,
    admin: Creds,
    admin_id: String,
}

fn body(username: &str) -> Value {
    json!({
        "firstName": "Grace",
        "lastName": "Hopper",
        "username": username,
        "email": format!("{username}@example.test"),
        "role": "user",
        "password": TEMPORARY,
        "locale": "en-US",
    })
}

fn fields(reply: &Reply) -> Result<Vec<String>> {
    Ok(reply.json()?["error"]["details"]["fields"]
        .as_array()
        .context("validation fields")?
        .iter()
        .filter_map(|field| field.as_str().map(str::to_owned))
        .collect())
}

fn files_under(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files_under(&path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

fn file_carries(path: &Path, needle: &[u8]) -> Result<bool> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Ok(false);
    };
    let mut window: Vec<u8> = Vec::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(false);
        }
        window.extend_from_slice(&chunk[..read]);
        if window
            .windows(needle.len())
            .any(|candidate| candidate == needle)
        {
            return Ok(true);
        }
        let keep = needle.len().saturating_sub(1).min(window.len());
        window.drain(..window.len() - keep);
    }
}

impl World {
    async fn start(name: &str) -> Result<Self> {
        let app = TestApplication::start(name).await?;
        let http = Arc::new(Http::new(app.url("/")?)?);
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

    async fn create(&self, creds: &Creds, payload: Value, key: Option<&str>) -> Result<Reply> {
        let headers: Vec<(&str, &str)> = key
            .map(|key| ("idempotency-key", key))
            .into_iter()
            .collect();
        self.http
            .send_with_headers(Method::POST, USERS, Some(creds), Some(payload), &headers)
            .await
    }

    async fn created(&self, payload: Value) -> Result<Value> {
        self.create(&self.admin, payload, None)
            .await?
            .expect(StatusCode::CREATED)?
            .json()
    }

    async fn patch(&self, id: &str, payload: Value) -> Result<Reply> {
        self.http
            .send(
                Method::PATCH,
                &format!("{USERS}/{id}"),
                Some(&self.admin),
                Some(payload),
            )
            .await
    }

    async fn login(&self, identifier: &str, password: &str) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                LOGIN,
                None,
                Some(json!({ "identifier": identifier, "password": password })),
            )
            .await
    }

    async fn count(&self, sql: &str) -> Result<i64> {
        self.db.scalar_i64(sql).await
    }

    async fn users(&self) -> Result<i64> {
        self.count("SELECT COUNT(*) FROM users").await
    }

    async fn preferences(&self) -> Result<i64> {
        self.count("SELECT COUNT(*) FROM user_preferences").await
    }

    async fn audit(&self, action: &str) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
    }

    async fn idempotency_rows(&self) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM idempotency_records WHERE {STORED_KEY}"
        ))
        .await
    }

    async fn column_of(&self, username: &str, column: &str) -> Result<Option<String>> {
        let mut connection = self.db.writer().await?;
        let value: Option<String> = sqlx::query_scalar(&format!(
            "SELECT CAST({column} AS TEXT) FROM users WHERE username = ?1"
        ))
        .bind(username)
        .fetch_one(&mut connection)
        .await
        .with_context(|| format!("read {column} of {username}"))?;
        Ok(value)
    }

    async fn id_of(&self, username: &str) -> Result<String> {
        self.column_of(username, "id")
            .await?
            .context("user id is not null")
    }

    async fn snapshot(&self, id: &str) -> Result<BTreeMap<String, String>> {
        let mut connection = self.db.writer().await?;
        let row = sqlx::query("SELECT * FROM users WHERE id = ?1")
            .bind(id)
            .fetch_one(&mut connection)
            .await
            .context("snapshot user")?;
        let mut columns = BTreeMap::new();
        for (index, column) in row.columns().iter().enumerate() {
            let text = if let Ok(value) = row.try_get::<Option<String>, _>(index) {
                value.unwrap_or_else(|| "<null>".to_owned())
            } else if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
                value.map_or_else(|| "<null>".to_owned(), |value| value.to_string())
            } else {
                "<blob>".to_owned()
            };
            columns.insert(column.name().to_owned(), text);
        }
        Ok(columns)
    }

    async fn security_state(&self, id: &str) -> Result<Vec<i64>> {
        let mut counts = Vec::new();
        for table in [
            "sessions",
            "trusted_devices",
            "totp_secrets",
            "totp_backup_codes",
            "account_lockouts",
            "identity_links",
            "user_preferences",
        ] {
            counts.push(
                self.count(&format!(
                    "SELECT COUNT(*) FROM {table} WHERE user_id = '{id}'"
                ))
                .await?,
            );
        }
        Ok(counts)
    }

    fn data_dir_carries(&self, needle: &str) -> Result<bool> {
        let mut files = Vec::new();
        files_under(self.app.data_dir(), &mut files)?;
        ensure!(!files.is_empty(), "the data directory holds no files");
        for file in &files {
            if file_carries(file, needle.as_bytes())? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn assert_data_dir_free_of(&self, needles: &[&str]) -> Result<()> {
        ensure!(
            self.data_dir_carries("$argon2id$")?,
            "the scan must be able to see stored credential hashes"
        );
        for needle in needles {
            ensure!(
                !self.data_dir_carries(needle)?,
                "a stored file carries {needle:?}"
            );
        }
        Ok(())
    }

    async fn shutdown(self) {
        self.app.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_forces_password_change_by_default() -> Result<()> {
    let world = World::start("it_admin_create_user_forces_password_change_by_default").await?;
    let created = world
        .create(&world.admin, body("grace"), None)
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(!created.body.contains(TEMPORARY), "{}", created.body);
    let user = created.json()?;
    ensure!(user["mustChangePassword"] == true, "{user}");
    ensure!(user["hasLocalPassword"] == true, "{user}");
    ensure!(user["isActive"] == true && user["role"] == "user", "{user}");
    ensure!(user["username"] == "grace" && user["email"] == "grace@example.test");

    let hash = world
        .column_of("grace", "password_hash")
        .await?
        .context("a local account stores a hash")?;
    ensure!(hash.starts_with("$argon2id$"), "{hash}");
    ensure!(!hash.contains(TEMPORARY));
    ensure!(
        world
            .column_of("grace", "must_change_password")
            .await?
            .as_deref()
            == Some("1")
    );
    ensure!(world
        .column_of("grace", "password_updated_at")
        .await?
        .is_some());
    world.assert_data_dir_free_of(&[TEMPORARY, KEY])?;

    let login = world
        .login("grace", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(
        login.json()?["mustChangePassword"] == true,
        "{}",
        login.body
    );
    let restricted = login.creds()?;
    let me = world
        .http
        .get(ME, Some(&restricted))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(me["restriction"] == "must_change_password", "{me}");

    let blocked = world.http.get(PROFILE, Some(&restricted)).await?;
    ensure!(
        blocked.status == StatusCode::FORBIDDEN
            && blocked.error_code()? == "AUTH_PASSWORD_CHANGE_REQUIRED",
        "{} {}",
        blocked.status,
        blocked.body
    );
    let blocked_admin = world.http.get(USERS, Some(&restricted)).await?;
    ensure!(blocked_admin.status == StatusCode::FORBIDDEN);

    let by_email = world
        .login("GRACE@Example.TEST", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(by_email.json()?["mustChangePassword"] == true);

    let changed = world
        .http
        .send(
            Method::POST,
            PROFILE_PASSWORD,
            Some(&restricted),
            Some(json!({ "newPassword": REPLACEMENT })),
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    let released = changed.creds()?;
    world
        .http
        .get(PROFILE, Some(&released))
        .await?
        .expect(StatusCode::OK)?;
    ensure!(
        world
            .column_of("grace", "must_change_password")
            .await?
            .as_deref()
            == Some("0")
    );
    let again = world
        .login("grace", REPLACEMENT)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(again.json()?["mustChangePassword"] == false);
    let stale = world.login("grace", TEMPORARY).await?;
    ensure!(
        stale.status == StatusCode::UNAUTHORIZED
            && stale.error_code()? == "AUTH_INVALID_CREDENTIALS"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_duplicate_normalized_identity_409() -> Result<()> {
    let world = World::start("it_admin_create_user_duplicate_normalized_identity_409").await?;
    world
        .created(json!({
            "firstName": "Grace", "lastName": "Hopper", "username": "Grace",
            "email": "Grace@Example.test", "role": "user", "password": TEMPORARY,
        }))
        .await?;
    world
        .created(json!({
            "firstName": "Kel", "lastName": "Vin", "username": "kelvin",
            "email": "kelvin@Straße.test", "role": "user", "password": TEMPORARY,
        }))
        .await?;
    world
        .created(json!({
            "firstName": "Str", "lastName": "Asse", "username": "Straße",
            "email": "strasse@example.test", "role": "user", "password": TEMPORARY,
        }))
        .await?;
    let users = world.users().await?;
    let preferences = world.preferences().await?;
    let created_audits = world.audit("USER_CREATED").await?;

    let duplicate_emails = [
        "grace@example.test",
        "GRACE@EXAMPLE.TEST",
        "gRaCe@ExAmPlE.tEsT",
        "\u{ff47}\u{ff52}\u{ff41}\u{ff43}\u{ff45}@example.test",
        "grace@\u{ff45}xample.test",
        "kelvin@STRAßE.test",
        "kelvin@STRAẞE.test",
        "ADA@example.TEST",
    ];
    for (index, email) in duplicate_emails.into_iter().enumerate() {
        let reply = world
            .create(
                &world.admin,
                json!({
                    "firstName": "Dup", "lastName": "Licate",
                    "username": format!("fresh-{index}"),
                    "email": email, "role": "user", "password": TEMPORARY,
                }),
                None,
            )
            .await?;
        ensure!(
            reply.status == StatusCode::CONFLICT && reply.error_code()? == "USER_EMAIL_TAKEN",
            "{email}: {} {}",
            reply.status,
            reply.body
        );
    }

    let duplicate_usernames = [
        "grace",
        "GRACE",
        "GrAcE",
        "\u{ff47}\u{ff52}\u{ff41}\u{ff43}\u{ff45}",
        "STRAẞE",
        "straße",
        "\u{212a}elvin",
        "ADA",
    ];
    for (index, username) in duplicate_usernames.into_iter().enumerate() {
        let reply = world
            .create(
                &world.admin,
                json!({
                    "firstName": "Dup", "lastName": "Licate", "username": username,
                    "email": format!("fresh-{index}@example.test"),
                    "role": "user", "password": TEMPORARY,
                }),
                None,
            )
            .await?;
        ensure!(
            reply.status == StatusCode::CONFLICT && reply.error_code()? == "USER_USERNAME_TAKEN",
            "{username}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(!reply.body.contains("UNIQUE") && !reply.body.contains("sqlite"));
    }

    ensure!(world.users().await? == users);
    ensure!(world.preferences().await? == preferences);
    ensure!(world.audit("USER_CREATED").await? == created_audits);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_may_claim_an_address_pending_for_another_user() -> Result<()> {
    let world = World::start("it_admin_create_user_pending_email_is_not_a_reservation").await?;
    world
        .db
        .execute(
            "UPDATE users SET pending_email = 'Claimed@Example.test',
                    pending_email_normalized = 'claimed@example.test'
              WHERE username = 'ada'",
        )
        .await?;

    let mut payload = body("claimer");
    payload["email"] = json!("CLAIMED@example.TEST");
    let created = world.created(payload).await?;
    ensure!(created["email"] == "CLAIMED@example.TEST", "{created}");
    ensure!(
        world
            .column_of("claimer", "email_normalized")
            .await?
            .as_deref()
            == Some("claimed@example.test")
    );
    ensure!(
        world.column_of("ada", "pending_email").await?.as_deref() == Some("Claimed@Example.test")
    );
    ensure!(
        world
            .column_of("ada", "pending_email_normalized")
            .await?
            .as_deref()
            == Some("claimed@example.test"),
        "the other user's pending change is left untouched"
    );
    ensure!(world.column_of("ada", "email").await?.as_deref() == Some("ada@example.test"));
    ensure!(world.users().await? == 2 && world.preferences().await? == 2);
    ensure!(world.audit("USER_CREATED").await? == 1);

    let mut again = body("secondclaimer");
    again["email"] = json!("claimed@example.test");
    let reply = world.create(&world.admin, again, None).await?;
    ensure!(
        reply.status == StatusCode::CONFLICT && reply.error_code()? == "USER_EMAIL_TAKEN",
        "a canonical address stays unique: {} {}",
        reply.status,
        reply.body
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_sso_only_placeholder_has_no_local_credential() -> Result<()> {
    let world = World::start("it_admin_create_user_sso_only_placeholder").await?;
    let mut payload = body("placeholder");
    payload
        .as_object_mut()
        .context("body object")?
        .remove("password");
    let user = world.created(payload).await?;
    ensure!(user["hasLocalPassword"] == false, "{user}");
    ensure!(user["mustChangePassword"] == false, "{user}");
    ensure!(user["isActive"] == true, "{user}");
    ensure!(user["identityLinkCount"] == 0, "{user}");
    let id = user["id"].as_str().context("id")?.to_owned();

    ensure!(world
        .column_of("placeholder", "password_hash")
        .await?
        .is_none());
    ensure!(world
        .column_of("placeholder", "password_updated_at")
        .await?
        .is_none());
    ensure!(
        world
            .column_of("placeholder", "must_change_password")
            .await?
            .as_deref()
            == Some("0")
    );
    ensure!(
        world
            .column_of("placeholder", "is_active")
            .await?
            .as_deref()
            == Some("1")
    );
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM identity_links WHERE user_id = '{id}'"
            ))
            .await?
            == 0
    );
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM sessions WHERE user_id = '{id}'"
            ))
            .await?
            == 0
    );

    for password in [TEMPORARY, PASSWORD] {
        let reply = world.login("placeholder", password).await?;
        ensure!(
            reply.status == StatusCode::UNAUTHORIZED
                && reply.error_code()? == "AUTH_INVALID_CREDENTIALS",
            "{} {}",
            reply.status,
            reply.body
        );
    }
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM sessions WHERE user_id = '{id}'"
            ))
            .await?
            == 0
    );

    let detail = world
        .http
        .get(&format!("{USERS}/{id}"), Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(detail["hasLocalPassword"] == false && detail["identityLinks"] == json!([]));
    ensure!(detail["isActive"] == true, "{detail}");

    let mut impossible = body("impossible");
    let object = impossible.as_object_mut().context("body object")?;
    object.remove("password");
    object.insert("requirePasswordChange".to_owned(), json!(true));
    let rejected = world.create(&world.admin, impossible, None).await?;
    ensure!(
        rejected.status == StatusCode::UNPROCESSABLE_ENTITY
            && rejected.error_code()? == "VALIDATION_ERROR",
        "{}",
        rejected.body
    );
    ensure!(fields(&rejected)? == ["requirePasswordChange"]);
    ensure!(world.column_of("impossible", "id").await.is_err());

    let mut explicit = body("explicitfalse");
    let object = explicit.as_object_mut().context("body object")?;
    object.remove("password");
    object.insert("requirePasswordChange".to_owned(), json!(false));
    let user = world.created(explicit).await?;
    ensure!(user["hasLocalPassword"] == false && user["mustChangePassword"] == false);

    let mut nulled = body("nullpassword");
    nulled["password"] = Value::Null;
    let user = world.created(nulled).await?;
    ensure!(user["hasLocalPassword"] == false && user["mustChangePassword"] == false);
    ensure!(world
        .column_of("nullpassword", "password_hash")
        .await?
        .is_none());
    ensure!(world.audit("USER_CREATED").await? == 3);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_can_opt_out_of_the_forced_password_change() -> Result<()> {
    let world = World::start("it_admin_create_user_opt_out_of_forced_change").await?;
    let mut payload = body("relaxed");
    payload["requirePasswordChange"] = json!(false);
    let user = world.created(payload).await?;
    ensure!(user["mustChangePassword"] == false && user["hasLocalPassword"] == true);
    ensure!(
        world
            .column_of("relaxed", "must_change_password")
            .await?
            .as_deref()
            == Some("0")
    );
    ensure!(world.column_of("relaxed", "password_hash").await?.is_some());

    let login = world
        .login("relaxed", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(
        login.json()?["mustChangePassword"] == false,
        "{}",
        login.body
    );
    let creds = login.creds()?;
    let me = world
        .http
        .get(ME, Some(&creds))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(me["restriction"] == Value::Null, "{me}");
    world
        .http
        .get(PROFILE, Some(&creds))
        .await?
        .expect(StatusCode::OK)?;

    let mut explicit = body("forced");
    explicit["requirePasswordChange"] = json!(true);
    let user = world.created(explicit).await?;
    ensure!(user["mustChangePassword"] == true);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_persists_initial_state_atomically() -> Result<()> {
    let world = World::start("it_admin_create_user_persists_initial_state").await?;

    let mut sized = body("sized");
    sized["quotaBytes"] = json!(5_368_709_120_u64);
    sized["locale"] = json!("pt-BR");
    let sized = world.created(sized).await?;
    ensure!(sized["quotaBytes"] == 5_368_709_120_u64, "{sized}");
    ensure!(sized["effectiveQuotaBytes"] == 5_368_709_120_u64, "{sized}");
    ensure!(
        world
            .column_of("sized", "quota_override_mode")
            .await?
            .as_deref()
            == Some("bytes")
    );
    ensure!(world.column_of("sized", "quota_bytes").await?.as_deref() == Some("5368709120"));
    ensure!(
        world.column_of("sized", "created_by").await?.as_deref() == Some(world.admin_id.as_str())
    );
    let sized_id = sized["id"].as_str().context("id")?;
    ensure!(
        world
            .db
            .scalar_string(&format!(
                "SELECT locale FROM user_preferences WHERE user_id = '{sized_id}'"
            ))
            .await?
            == "pt-BR"
    );
    let detail = world
        .http
        .get(&format!("{USERS}/{sized_id}"), Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(
        detail["quotaBytes"] == 5_368_709_120_u64
            && detail["effectiveQuotaBytes"] == 5_368_709_120_u64
    );
    ensure!(detail["role"] == "user" && detail["isActive"] == true && detail["overQuota"] == false);
    let listed = world
        .http
        .get(&format!("{USERS}?q=sized"), Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(
        listed["items"][0] == sized,
        "the create response is the persisted list row: {} != {sized}",
        listed["items"][0]
    );

    let mut inherited = body("inherited");
    inherited["quotaBytes"] = Value::Null;
    inherited
        .as_object_mut()
        .context("object")?
        .remove("locale");
    let inherited = world.created(inherited).await?;
    ensure!(
        inherited["quotaBytes"] == Value::Null && inherited["effectiveQuotaBytes"] == Value::Null
    );
    ensure!(
        world
            .column_of("inherited", "quota_override_mode")
            .await?
            .as_deref()
            == Some("inherit")
    );
    ensure!(world.column_of("inherited", "quota_bytes").await?.is_none());
    let inherited_id = inherited["id"].as_str().context("id")?;
    ensure!(
        world
            .db
            .scalar_string(&format!(
                "SELECT locale FROM user_preferences WHERE user_id = '{inherited_id}'"
            ))
            .await?
            == "en-US",
        "an omitted locale falls back to the instance default"
    );

    let mut zero = body("zeroquota");
    zero["quotaBytes"] = json!(0);
    let zero = world.created(zero).await?;
    ensure!(
        zero["quotaBytes"] == 0 && zero["effectiveQuotaBytes"] == 0,
        "{zero}"
    );
    ensure!(
        world
            .column_of("zeroquota", "quota_override_mode")
            .await?
            .as_deref()
            == Some("bytes")
    );
    ensure!(
        world
            .column_of("zeroquota", "quota_bytes")
            .await?
            .as_deref()
            == Some("0")
    );

    let mut admin = body("newadmin");
    admin["role"] = json!("admin");
    admin["requirePasswordChange"] = json!(false);
    let admin = world.created(admin).await?;
    ensure!(admin["role"] == "admin", "{admin}");
    ensure!(world.column_of("newadmin", "role").await?.as_deref() == Some("admin"));
    let creds = world
        .login("newadmin", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?
        .creds()?;
    world
        .http
        .get(USERS, Some(&creds))
        .await?
        .expect(StatusCode::OK)?;

    let mut inactive = body("dormant");
    inactive["isActive"] = json!(false);
    let inactive = world.created(inactive).await?;
    ensure!(inactive["isActive"] == false, "{inactive}");
    ensure!(world.column_of("dormant", "is_active").await?.as_deref() == Some("0"));
    ensure!(world
        .column_of("dormant", "deactivated_at")
        .await?
        .is_some());
    let refused = world.login("dormant", TEMPORARY).await?;
    ensure!(
        refused.status == StatusCode::UNAUTHORIZED
            && refused.error_code()? == "AUTH_INVALID_CREDENTIALS",
        "{} {}",
        refused.status,
        refused.body
    );
    let inactive_id = inactive["id"].as_str().context("id")?;
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM sessions WHERE user_id = '{inactive_id}'"
            ))
            .await?
            == 0
    );
    let listed = world
        .http
        .get(&format!("{USERS}?status=inactive"), Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(listed["totalCount"] == 1 && listed["items"][0]["username"] == "dormant");

    for username in ["sized", "inherited", "zeroquota", "newadmin", "dormant"] {
        ensure!(world
            .column_of(username, "email_verified_at")
            .await?
            .is_none());
        ensure!(world.column_of(username, "pending_email").await?.is_none());
        ensure!(
            world.column_of(username, "created_by").await?.as_deref()
                == Some(world.admin_id.as_str())
        );
    }
    ensure!(
        world
            .count("SELECT COUNT(*) FROM email_verifications")
            .await?
            == 0
    );
    ensure!(world.count("SELECT COUNT(*) FROM email_outbox").await? == 0);
    ensure!(
        world.users().await? == world.preferences().await?,
        "every user has exactly one preference row"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_rejects_invalid_requests_without_side_effects() -> Result<()> {
    let world = World::start("it_admin_create_user_rejects_invalid_requests").await?;
    let with = |field: &str, value: Value| {
        let mut payload = body("invalid");
        payload[field] = value;
        payload
    };
    let without = |field: &str| {
        let mut payload = body("invalid");
        payload.as_object_mut().map(|object| object.remove(field));
        payload
    };
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "missing first name",
            without("firstName"),
            vec!["firstName"],
        ),
        ("missing role", without("role"), vec!["role"]),
        ("missing email", without("email"), vec!["email"]),
        (
            "unknown member",
            with("createdBy", json!("x")),
            vec!["body"],
        ),
        (
            "unknown id member",
            with("id", json!(ABSENT_ID)),
            vec!["body"],
        ),
        (
            "unknown admin flag",
            with("isAdmin", json!(true)),
            vec!["body"],
        ),
        ("bad role", with("role", json!("owner")), vec!["role"]),
        ("bad locale", with("locale", json!("xx-XX")), vec!["locale"]),
        ("bad email", with("email", json!("nope")), vec!["email"]),
        (
            "short username",
            with("username", json!("ab")),
            vec!["username"],
        ),
        (
            "negative quota",
            with("quotaBytes", json!(-1)),
            vec!["quotaBytes"],
        ),
        (
            "fractional quota",
            with("quotaBytes", json!(1.5)),
            vec!["quotaBytes"],
        ),
        (
            "string quota",
            with("quotaBytes", json!("5")),
            vec!["quotaBytes"],
        ),
        (
            "unsafe quota",
            with("quotaBytes", json!(9_007_199_254_740_992_u64)),
            vec!["quotaBytes"],
        ),
        (
            "huge quota",
            with("quotaBytes", json!(u64::MAX)),
            vec!["body"],
        ),
        (
            "blank first name",
            with("firstName", json!("  ")),
            vec!["firstName"],
        ),
        (
            "long last name",
            with("lastName", json!("x".repeat(101))),
            vec!["lastName"],
        ),
        (
            "non boolean active",
            with("isActive", json!("yes")),
            vec!["isActive"],
        ),
        (
            "non boolean require",
            with("requirePasswordChange", json!("yes")),
            vec!["requirePasswordChange"],
        ),
        (
            "non string password",
            with("password", json!(12345678)),
            vec!["password"],
        ),
    ];
    for (label, payload, expected) in cases {
        let reply = world.create(&world.admin, payload, Some(KEY)).await?;
        ensure!(
            reply.status == StatusCode::UNPROCESSABLE_ENTITY
                && reply.error_code()? == "VALIDATION_ERROR",
            "{label}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(fields(&reply)? == expected, "{label}: {}", reply.body);
    }
    let not_an_object = world.create(&world.admin, json!([1, 2]), Some(KEY)).await?;
    ensure!(not_an_object.status == StatusCode::UNPROCESSABLE_ENTITY);
    ensure!(fields(&not_an_object)? == ["body"]);

    for short in ["", "1234567", "sevench"] {
        let mut payload = body("shortpw");
        payload["password"] = json!(short);
        let reply = world.create(&world.admin, payload, Some(KEY)).await?;
        ensure!(
            reply.status == StatusCode::UNPROCESSABLE_ENTITY
                && reply.error_code()? == "PASSWORD_POLICY_VIOLATION",
            "{short:?}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(reply.json()?["error"]["details"]["minLength"] == 8);
    }

    ensure!(world.users().await? == 1, "only the setup Admin exists");
    ensure!(world.preferences().await? == 1);
    ensure!(world.audit("USER_CREATED").await? == 0);
    ensure!(
        world.idempotency_rows().await? == 0,
        "rejected requests release their claim"
    );

    let mut boundary = body("boundary");
    boundary["password"] = json!("12345678");
    let reply = world.create(&world.admin, boundary, Some(KEY)).await?;
    ensure!(reply.status == StatusCode::CREATED, "{}", reply.body);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_audit_row_is_allowlisted() -> Result<()> {
    let world = World::start("it_admin_create_user_audit_row").await?;
    let created = world
        .create(&world.admin, body("audited"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    let request_id = created
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .context("request id header")?
        .to_owned();
    let id = created.json()?["id"].as_str().context("id")?.to_owned();
    ensure!(world.audit("USER_CREATED").await? == 1);

    let mut connection = world.db.writer().await?;
    let row = sqlx::query(
        "SELECT actor_type, actor_user_id, actor_label, target_type, target_id, target_label,
                result, error_code, request_id, metadata_json
           FROM audit_events WHERE action = 'USER_CREATED'",
    )
    .fetch_one(&mut connection)
    .await?;
    ensure!(row.get::<String, _>("actor_type") == "user");
    ensure!(
        row.get::<Option<String>, _>("actor_user_id").as_deref() == Some(world.admin_id.as_str())
    );
    ensure!(row.get::<Option<String>, _>("actor_label").as_deref() == Some("ada"));
    ensure!(row.get::<Option<String>, _>("target_type").as_deref() == Some("user"));
    ensure!(row.get::<Option<String>, _>("target_id").as_deref() == Some(id.as_str()));
    ensure!(row.get::<Option<String>, _>("target_label").as_deref() == Some("audited"));
    ensure!(row.get::<String, _>("result") == "success");
    ensure!(row.get::<Option<String>, _>("error_code").is_none());
    ensure!(row.get::<Option<String>, _>("request_id").as_deref() == Some(request_id.as_str()));
    let metadata: Value = serde_json::from_str(&row.get::<String, _>("metadata_json"))?;
    ensure!(
        metadata
            == json!({
                "role": "user",
                "is_active": true,
                "local_password": true,
                "must_change_password": true,
                "quota_mode": "inherit",
            }),
        "{metadata}"
    );
    let dump = format!("{metadata} {}", row.get::<String, _>("metadata_json"));
    for forbidden in [TEMPORARY, KEY, "argon2", "password_hash"] {
        ensure!(!dump.contains(forbidden), "{forbidden}");
    }

    world
        .create(&world.admin, body("audited"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(
        world.audit("USER_CREATED").await? == 1,
        "a replay writes no second row"
    );
    world.shutdown().await;
    Ok(())
}

async fn assert_injected_failure_rolls_back(
    world: &World,
    trigger: &str,
    name: &str,
    username: &str,
) -> Result<()> {
    let users = world.users().await?;
    let preferences = world.preferences().await?;
    let audits = world.audit("USER_CREATED").await?;
    let idempotency = world.idempotency_rows().await?;
    world.db.execute(trigger).await?;

    let key = format!("admin-user-key-{name}-injected-fail");
    let failed = world
        .create(&world.admin, body(username), Some(&key))
        .await?;
    ensure!(
        failed.status == StatusCode::INTERNAL_SERVER_ERROR
            && failed.error_code()? == "INTERNAL_ERROR",
        "{name}: {} {}",
        failed.status,
        failed.body
    );
    ensure!(world.users().await? == users, "{name}: a user row survived");
    ensure!(
        world.preferences().await? == preferences,
        "{name}: a preference row survived"
    );
    ensure!(
        world.audit("USER_CREATED").await? == audits,
        "{name}: an audit row survived"
    );
    ensure!(
        world.idempotency_rows().await? == idempotency,
        "{name}: an idempotency record survived"
    );
    ensure!(
        world
            .count(&format!("SELECT COUNT(*) FROM idempotency_records WHERE state = 'completed' AND {STORED_KEY}"))
            .await?
            == idempotency
    );

    world.db.execute(&format!("DROP TRIGGER {name}")).await?;
    let retried = world
        .create(&world.admin, body(username), Some(&key))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(retried.headers.get("idempotency-replayed").is_none());
    ensure!(world.users().await? == users + 1);
    ensure!(world.preferences().await? == preferences + 1);
    ensure!(world.audit("USER_CREATED").await? == audits + 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_rolls_back_every_fact_on_injected_failure() -> Result<()> {
    let world = World::start("it_admin_create_user_rolls_back").await?;
    assert_injected_failure_rolls_back(
        &world,
        "CREATE TRIGGER inject_audit BEFORE INSERT ON audit_events
         WHEN NEW.action = 'USER_CREATED'
         BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END",
        "inject_audit",
        "auditfail",
    )
    .await?;
    assert_injected_failure_rolls_back(
        &world,
        "CREATE TRIGGER inject_preferences BEFORE INSERT ON user_preferences
         BEGIN SELECT RAISE(ABORT, 'injected preference failure'); END",
        "inject_preferences",
        "preffail",
    )
    .await?;
    assert_injected_failure_rolls_back(
        &world,
        "CREATE TRIGGER inject_completion BEFORE UPDATE ON idempotency_records
         WHEN NEW.state = 'completed'
         BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END",
        "inject_completion",
        "completionfail",
    )
    .await?;
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_idempotent_replay_returns_the_original_201() -> Result<()> {
    let world = World::start("it_admin_create_user_idempotent_replay").await?;
    let first = world
        .create(&world.admin, body("replayed"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(first.headers.get("idempotency-replayed").is_none());
    let original = first.json()?;
    let users = world.users().await?;

    let replay = world
        .create(&world.admin, body("replayed"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(
        replay
            .headers
            .get("idempotency-replayed")
            .and_then(|value| value.to_str().ok())
            == Some("true")
    );
    ensure!(
        replay.json()? == original,
        "{} != {}",
        replay.body,
        first.body
    );
    ensure!(
        replay
            .headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
            == Some("no-store")
    );
    ensure!(!replay.body.contains(TEMPORARY));
    ensure!(world.users().await? == users, "no second account");
    ensure!(world.preferences().await? == users);
    ensure!(
        world.audit("USER_CREATED").await? == 1,
        "no second audit row"
    );
    ensure!(world.idempotency_rows().await? == 1);

    let other_session = world
        .login("ada", PASSWORD)
        .await?
        .expect(StatusCode::OK)?
        .creds()?;
    let across_sessions = world
        .create(&other_session, body("replayed"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(across_sessions
        .headers
        .get("idempotency-replayed")
        .is_some());
    ensure!(across_sessions.json()? == original);
    ensure!(
        world.users().await? == users,
        "the scope is the principal, not the session"
    );

    let mut connection = world.db.writer().await?;
    let row = sqlx::query(
        "SELECT scope_kind, scope_id, request_hash, response_status, response_json,
                response_ciphertext, state
           FROM idempotency_records WHERE route_template = '/api/v1/admin/users'",
    )
    .fetch_one(&mut connection)
    .await?;
    ensure!(row.get::<String, _>("scope_kind") == "user");
    ensure!(row.get::<String, _>("scope_id") == world.admin_id);
    ensure!(row.get::<String, _>("state") == "completed");
    ensure!(row.get::<i64, _>("response_status") == 201);
    ensure!(row
        .get::<Option<Vec<u8>>, _>("response_ciphertext")
        .is_none());
    let stored = row
        .get::<Option<String>, _>("response_json")
        .context("plaintext envelope")?;
    ensure!(!stored.contains(TEMPORARY) && !stored.contains("argon2"));
    ensure!(row.get::<String, _>("request_hash").len() == 64);
    world.assert_data_dir_free_of(&[TEMPORARY, KEY])?;

    for short in ["short", &"k".repeat(129)] {
        let reply = world
            .create(&world.admin, body("badkey"), Some(short))
            .await?;
        ensure!(
            reply.status == StatusCode::UNPROCESSABLE_ENTITY
                && reply.error_code()? == "VALIDATION_ERROR",
            "{} {}",
            reply.status,
            reply.body
        );
    }
    ensure!(world.users().await? == users);

    let failing_key = "admin-user-key-0002-failing-attempt";
    for _ in 0..2 {
        let taken = world
            .create(&world.admin, body("REPLAYED"), Some(failing_key))
            .await?;
        ensure!(
            taken.status == StatusCode::CONFLICT && taken.error_code()? == "USER_EMAIL_TAKEN",
            "{}",
            taken.body
        );
        ensure!(taken.headers.get("idempotency-replayed").is_none());
        ensure!(
            world.idempotency_rows().await? == 1,
            "an expected failure does not consume its key"
        );
    }
    let recovered = world
        .create(&world.admin, body("recovered"), Some(failing_key))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(recovered.headers.get("idempotency-replayed").is_none());
    ensure!(world.users().await? == users + 1);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_idempotency_conflict_and_principal_scope() -> Result<()> {
    let world = World::start("it_admin_create_user_idempotency_scope").await?;
    let first = world
        .create(&world.admin, body("scoped"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    let users = world.users().await?;

    for variant in [body("otherperson"), body("SCOPED")] {
        let conflict = world.create(&world.admin, variant, Some(KEY)).await?;
        ensure!(
            conflict.status == StatusCode::CONFLICT
                && conflict.error_code()? == "IDEMPOTENCY_KEY_CONFLICT",
            "{} {}",
            conflict.status,
            conflict.body
        );
    }
    ensure!(
        world.users().await? == users,
        "a conflicting body changes nothing"
    );
    ensure!(world.audit("USER_CREATED").await? == 1);
    ensure!(world.column_of("scoped", "username").await?.as_deref() == Some("scoped"));

    let mut second_admin = body("secondadmin");
    second_admin["role"] = json!("admin");
    second_admin["requirePasswordChange"] = json!(false);
    world
        .create(
            &world.admin,
            second_admin,
            Some("admin-user-key-9999-second-admin"),
        )
        .await?
        .expect(StatusCode::CREATED)?;
    let second = world
        .login("secondadmin", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?
        .creds()?;
    let second_id = world.id_of("secondadmin").await?;

    let same_request = world.create(&second, body("scoped"), Some(KEY)).await?;
    ensure!(
        same_request.status == StatusCode::CONFLICT
            && same_request.error_code()? == "USER_EMAIL_TAKEN",
        "{} {}",
        same_request.status,
        same_request.body
    );
    ensure!(same_request.headers.get("idempotency-replayed").is_none());
    ensure!(!same_request
        .body
        .contains(first.json()?["id"].as_str().context("id")?));

    let own = world
        .create(&second, body("secondowned"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(own.headers.get("idempotency-replayed").is_none());
    ensure!(
        world
            .column_of("secondowned", "created_by")
            .await?
            .as_deref()
            == Some(second_id.as_str())
    );
    ensure!(
        world.column_of("scoped", "created_by").await?.as_deref() == Some(world.admin_id.as_str())
    );

    let replay_first = world
        .create(&world.admin, body("scoped"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(replay_first.headers.get("idempotency-replayed").is_some());
    ensure!(replay_first.json()? == first.json()?);
    ensure!(
        world.column_of("scoped", "created_by").await?.as_deref() == Some(world.admin_id.as_str()),
        "a replay does not change created_by"
    );
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(DISTINCT scope_id) FROM idempotency_records WHERE {STORED_KEY}"
            ))
            .await?
            == 2
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_concurrent_same_key_executes_once() -> Result<()> {
    let world = World::start("it_admin_create_user_concurrent_same_key").await?;
    let mut tasks = JoinSet::new();
    for _ in 0..8 {
        let http = Arc::clone(&world.http);
        let creds = world.admin.clone();
        tasks.spawn(async move {
            http.send_with_headers(
                Method::POST,
                USERS,
                Some(&creds),
                Some(body("racing")),
                &[("idempotency-key", KEY)],
            )
            .await
        });
    }
    let mut executed = 0;
    let mut replayed = 0;
    let mut in_progress = 0;
    while let Some(joined) = tasks.join_next().await {
        let reply = joined??;
        match reply.status {
            StatusCode::CREATED if reply.headers.get("idempotency-replayed").is_some() => {
                replayed += 1;
            }
            StatusCode::CREATED => executed += 1,
            StatusCode::CONFLICT => {
                ensure!(
                    reply.error_code()? == "IDEMPOTENCY_REQUEST_IN_PROGRESS",
                    "{}",
                    reply.body
                );
                ensure!(
                    reply
                        .headers
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        == Some("1")
                );
                in_progress += 1;
            }
            other => anyhow::bail!("unexpected {other}: {}", reply.body),
        }
    }
    ensure!(
        executed == 1,
        "executed {executed}, replayed {replayed}, in progress {in_progress}"
    );
    ensure!(executed + replayed + in_progress == 8);
    ensure!(world.users().await? == 2);
    ensure!(world.preferences().await? == 2);
    ensure!(world.audit("USER_CREATED").await? == 1);
    ensure!(world.idempotency_rows().await? == 1);

    let retry = world
        .create(&world.admin, body("racing"), Some(KEY))
        .await?
        .expect(StatusCode::CREATED)?;
    ensure!(retry.headers.get("idempotency-replayed").is_some());
    ensure!(world.users().await? == 2);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_create_user_concurrent_duplicate_identity_leaves_one_account() -> Result<()> {
    let world = World::start("it_admin_create_user_concurrent_identity").await?;
    for (label, payloads, expected) in [
        (
            "email",
            (0..8)
                .map(|index| {
                    let email = if index % 2 == 0 {
                        "Contested@Example.test"
                    } else {
                        "CONTESTED@example.TEST"
                    };
                    json!({
                        "firstName": "Race", "lastName": "Runner",
                        "username": format!("emailracer{index}"),
                        "email": email, "role": "user", "password": TEMPORARY,
                    })
                })
                .collect::<Vec<_>>(),
            "USER_EMAIL_TAKEN",
        ),
        (
            "username",
            (0..8)
                .map(|index| {
                    let username = if index % 2 == 0 {
                        "Contested"
                    } else {
                        "CONTESTED"
                    };
                    json!({
                        "firstName": "Race", "lastName": "Runner",
                        "username": username,
                        "email": format!("usernameracer{index}@example.test"),
                        "role": "user", "password": TEMPORARY,
                    })
                })
                .collect::<Vec<_>>(),
            "USER_USERNAME_TAKEN",
        ),
    ] {
        let before = world.users().await?;
        let mut tasks = JoinSet::new();
        for (index, payload) in payloads.into_iter().enumerate() {
            let http = Arc::clone(&world.http);
            let creds = world.admin.clone();
            let key = format!("admin-user-key-race-{label}-{index:04}");
            tasks.spawn(async move {
                http.send_with_headers(
                    Method::POST,
                    USERS,
                    Some(&creds),
                    Some(payload),
                    &[("idempotency-key", key.as_str())],
                )
                .await
            });
        }
        let mut created = 0;
        while let Some(joined) = tasks.join_next().await {
            let reply = joined??;
            match reply.status {
                StatusCode::CREATED => created += 1,
                StatusCode::CONFLICT => ensure!(
                    reply.error_code()? == expected,
                    "{label}: {} {}",
                    expected,
                    reply.body
                ),
                other => anyhow::bail!("{label}: unexpected {other}: {}", reply.body),
            }
        }
        ensure!(created == 1, "{label}: {created} accounts were created");
        ensure!(world.users().await? == before + 1, "{label}");
        ensure!(world.preferences().await? == before + 1, "{label}");
    }
    ensure!(world.audit("USER_CREATED").await? == 2);
    ensure!(
        world.idempotency_rows().await? == 2,
        "only the winning attempts hold a completed record"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_user_write_routes_need_no_recent_authentication() -> Result<()> {
    let world = World::start("it_admin_user_writes_need_no_recent_auth").await?;
    let target = world.created(body("target")).await?;
    let id = target["id"].as_str().context("id")?.to_owned();

    world.app.clock().advance(Duration::from_secs(31 * 60));
    let recent = world
        .http
        .send(
            Method::POST,
            PROFILE_PASSWORD,
            Some(&world.admin),
            Some(json!({ "currentPassword": PASSWORD, "newPassword": REPLACEMENT })),
        )
        .await?;
    ensure!(
        recent.status == StatusCode::FORBIDDEN
            && recent.error_code()? == "AUTH_RECENT_AUTH_REQUIRED",
        "the control route must demand recent authentication: {} {}",
        recent.status,
        recent.body
    );

    world
        .create(&world.admin, body("afterlapse"), None)
        .await?
        .expect(StatusCode::CREATED)?;
    world
        .patch(&id, json!({ "firstName": "Changed" }))
        .await?
        .expect(StatusCode::OK)?;
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_patch_user_updates_only_identity_fields() -> Result<()> {
    let world = World::start("it_admin_patch_user_identity_fields_only").await?;
    let mut payload = body("target");
    payload["requirePasswordChange"] = json!(false);
    payload["quotaBytes"] = json!(1_000_000);
    payload["locale"] = json!("de-DE");
    let target = world.created(payload).await?;
    let id = target["id"].as_str().context("id")?.to_owned();
    let session = world
        .login("target", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?
        .creds()?;
    world
        .db
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1, last_login_at = '2026-01-01T00:00:00.000Z' WHERE id = '{id}'"
        ))
        .await?;
    let before = world.snapshot(&id).await?;
    let security_before = world.security_state(&id).await?;
    let audits_before = world.count("SELECT COUNT(*) FROM audit_events").await?;

    world.app.clock().advance(Duration::from_secs(60));
    let reply = world
        .patch(
            &id,
            json!({ "firstName": "Grace M.", "lastName": "Hopper-Murray", "username": "AdmiralGrace" }),
        )
        .await?
        .expect(StatusCode::OK)?;
    let updated = reply.json()?;
    ensure!(updated["id"] == id.as_str());
    ensure!(updated["firstName"] == "Grace M." && updated["lastName"] == "Hopper-Murray");
    ensure!(updated["username"] == "AdmiralGrace", "{updated}");
    ensure!(updated["email"] == "target@example.test");
    ensure!(updated["role"] == "user" && updated["isActive"] == true);
    ensure!(updated["mustChangePassword"] == false && updated["hasLocalPassword"] == true);
    ensure!(updated["twoFactorEnabled"] == true);
    ensure!(updated["quotaBytes"] == 1_000_000 && updated["effectiveQuotaBytes"] == 1_000_000);
    ensure!(!reply.body.contains("argon2"));

    let after = world.snapshot(&id).await?;
    let changed: Vec<&str> = before
        .iter()
        .filter(|(column, value)| after.get(*column) != Some(*value))
        .map(|(column, _)| column.as_str())
        .collect();
    ensure!(
        changed
            == [
                "first_name",
                "last_name",
                "updated_at",
                "username",
                "username_normalized"
            ],
        "{changed:?}"
    );
    ensure!(after["username"] == "AdmiralGrace" && after["username_normalized"] == "admiralgrace");
    ensure!(after["updated_at"] > before["updated_at"]);
    ensure!(world.security_state(&id).await? == security_before);
    ensure!(world.count("SELECT COUNT(*) FROM audit_events").await? == audits_before);
    world
        .http
        .get(ME, Some(&session))
        .await?
        .expect(StatusCode::OK)?;
    ensure!(
        world
            .count(&format!("SELECT COUNT(*) FROM sessions WHERE user_id = '{id}' AND state = 'active' AND revoked_at IS NULL"))
            .await?
            == 1,
        "a profile edit revokes no session"
    );

    let detail = world
        .http
        .get(&format!("{USERS}/{id}"), Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    for field in [
        "firstName",
        "lastName",
        "username",
        "email",
        "role",
        "isActive",
        "mustChangePassword",
    ] {
        ensure!(detail[field] == updated[field], "{field}");
    }

    let partial = world
        .patch(&id, json!({ "lastName": "Hopper" }))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(partial["lastName"] == "Hopper" && partial["firstName"] == "Grace M.");
    ensure!(partial["username"] == "AdmiralGrace");

    let mut inactive = body("dormant");
    inactive["isActive"] = json!(false);
    let inactive = world.created(inactive).await?;
    let inactive_id = inactive["id"].as_str().context("id")?;
    let edited = world
        .patch(inactive_id, json!({ "firstName": "Sleeping" }))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(edited["firstName"] == "Sleeping" && edited["isActive"] == false);

    let with_key = world
        .http
        .send_with_headers(
            Method::PATCH,
            &format!("{USERS}/{id}"),
            Some(&world.admin),
            Some(json!({ "firstName": "Keyed" })),
            &[("idempotency-key", "short")],
        )
        .await?;
    ensure!(with_key.status == StatusCode::OK, "{}", with_key.body);
    ensure!(with_key.headers.get("idempotency-replayed").is_none());
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_patch_user_rejects_every_non_identity_field() -> Result<()> {
    let world = World::start("it_admin_patch_user_rejects_non_identity").await?;
    let target = world.created(body("target")).await?;
    let id = target["id"].as_str().context("id")?.to_owned();
    let before = world.snapshot(&id).await?;
    let security_before = world.security_state(&id).await?;

    let forbidden: Vec<(&str, Value)> = vec![
        ("email", json!("changed@example.test")),
        ("email", Value::Null),
        ("pendingEmail", json!("pending@example.test")),
        ("role", json!("admin")),
        ("isActive", json!(false)),
        ("deactivatedAt", json!("2026-01-01T00:00:00.000Z")),
        ("quotaBytes", json!(1)),
        ("quotaOverrideMode", json!("unlimited")),
        ("password", json!("another passphrase")),
        (
            "passwordHash",
            json!("$argon2id$v=19$m=1,t=1,p=1$c2FsdA$aGFzaA"),
        ),
        ("mustChangePassword", json!(false)),
        ("requirePasswordChange", json!(false)),
        ("totpEnabled", json!(false)),
        ("emailVerifiedAt", json!("2026-01-01T00:00:00.000Z")),
        ("id", json!(ABSENT_ID)),
        ("createdBy", json!(ABSENT_ID)),
        ("locale", json!("fr-FR")),
    ];
    for (field, value) in forbidden {
        let reply = world
            .patch(&id, json!({ "firstName": "Changed", field: value }))
            .await?;
        ensure!(
            reply.status == StatusCode::UNPROCESSABLE_ENTITY
                && reply.error_code()? == "VALIDATION_ERROR",
            "{field}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(fields(&reply)? == ["body"], "{field}: {}", reply.body);
        let alone = world.patch(&id, json!({ field: "x" })).await?;
        ensure!(alone.status == StatusCode::UNPROCESSABLE_ENTITY, "{field}");
    }
    ensure!(
        world.snapshot(&id).await? == before,
        "a rejected PATCH applies nothing"
    );
    ensure!(world.security_state(&id).await? == security_before);
    ensure!(world.audit("USER_CREATED").await? == 1);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_patch_user_validation_and_unknown_targets() -> Result<()> {
    let world = World::start("it_admin_patch_user_validation").await?;
    let target = world.created(body("target")).await?;
    let id = target["id"].as_str().context("id")?.to_owned();
    let before = world.snapshot(&id).await?;

    for (label, payload, expected) in [
        ("empty object", json!({}), vec!["body"]),
        ("null only", json!({ "firstName": null }), vec!["body"]),
        (
            "blank first name",
            json!({ "firstName": "" }),
            vec!["firstName"],
        ),
        (
            "long last name",
            json!({ "lastName": "x".repeat(101) }),
            vec!["lastName"],
        ),
        (
            "short username",
            json!({ "username": "ab" }),
            vec!["username"],
        ),
        ("wrong type", json!({ "username": 5 }), vec!["username"]),
        ("array", json!([]), vec!["body"]),
    ] {
        let reply = world.patch(&id, payload).await?;
        ensure!(
            reply.status == StatusCode::UNPROCESSABLE_ENTITY
                && reply.error_code()? == "VALIDATION_ERROR",
            "{label}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(fields(&reply)? == expected, "{label}: {}", reply.body);
    }
    ensure!(world.snapshot(&id).await? == before);

    for unknown in [ABSENT_ID, "not-a-uuid", "0"] {
        let reply = world
            .patch(unknown, json!({ "firstName": "Nobody" }))
            .await?;
        ensure!(
            reply.status == StatusCode::NOT_FOUND && reply.error_code()? == "USER_NOT_FOUND",
            "{unknown}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(!reply.body.to_lowercase().contains("sql"));
    }
    let malformed = world
        .http
        .send(
            Method::PATCH,
            &format!("{USERS}/{id}"),
            Some(&world.admin),
            None,
        )
        .await?;
    ensure!(malformed.status.is_client_error(), "{}", malformed.status);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_patch_user_username_collisions_are_normalized_and_race_safe() -> Result<()> {
    let world = World::start("it_admin_patch_user_username_collisions").await?;
    let alpha = world.created(body("alpha")).await?;
    let bravo = world.created(body("bravo")).await?;
    let charlie = world.created(body("charlie")).await?;
    world.created(body("kelvin")).await?;
    let alpha_id = alpha["id"].as_str().context("id")?.to_owned();
    let bravo_id = bravo["id"].as_str().context("id")?.to_owned();
    let charlie_id = charlie["id"].as_str().context("id")?.to_owned();
    let before = world.snapshot(&bravo_id).await?;

    for username in [
        "alpha",
        "ALPHA",
        "AlPhA",
        "\u{ff41}\u{ff4c}\u{ff50}\u{ff48}\u{ff41}",
        "\u{212a}ELVIN",
        "ada",
        "ADA",
    ] {
        let reply = world
            .patch(&bravo_id, json!({ "username": username }))
            .await?;
        ensure!(
            reply.status == StatusCode::CONFLICT && reply.error_code()? == "USER_USERNAME_TAKEN",
            "{username}: {} {}",
            reply.status,
            reply.body
        );
        ensure!(!reply.body.contains("UNIQUE") && !reply.body.contains("sqlite"));
    }
    ensure!(
        world.snapshot(&bravo_id).await? == before,
        "a collision applies nothing"
    );

    let recased = world
        .patch(&bravo_id, json!({ "username": "BRAVO" }))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(recased["username"] == "BRAVO", "{recased}");
    ensure!(
        world
            .column_of("BRAVO", "username_normalized")
            .await?
            .as_deref()
            == Some("bravo")
    );
    let same = world
        .patch(
            &bravo_id,
            json!({ "username": "BRAVO", "firstName": "Same" }),
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(same["firstName"] == "Same");

    let mut tasks = JoinSet::new();
    for (id, username) in [
        (bravo_id.clone(), "Contested"),
        (charlie_id.clone(), "CONTESTED"),
    ] {
        let http = Arc::clone(&world.http);
        let creds = world.admin.clone();
        tasks.spawn(async move {
            http.send(
                Method::PATCH,
                &format!("{USERS}/{id}"),
                Some(&creds),
                Some(json!({ "username": username })),
            )
            .await
        });
    }
    let mut won = 0;
    while let Some(joined) = tasks.join_next().await {
        let reply = joined??;
        match reply.status {
            StatusCode::OK => won += 1,
            StatusCode::CONFLICT => ensure!(reply.error_code()? == "USER_USERNAME_TAKEN"),
            other => anyhow::bail!("unexpected {other}: {}", reply.body),
        }
    }
    ensure!(won == 1, "{won} accounts took the contested username");
    ensure!(
        world
            .count("SELECT COUNT(*) FROM users WHERE username_normalized = 'contested'")
            .await?
            == 1
    );
    let _ = alpha_id;
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_patch_user_username_change_moves_the_login_identity() -> Result<()> {
    let world = World::start("it_admin_patch_user_login_transition").await?;
    let mut payload = body("oldname");
    payload["requirePasswordChange"] = json!(false);
    let target = world.created(payload).await?;
    let id = target["id"].as_str().context("id")?.to_owned();
    let existing = world
        .login("oldname", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?
        .creds()?;

    world
        .patch(&id, json!({ "username": "NewName" }))
        .await?
        .expect(StatusCode::OK)?;

    let old = world.login("oldname", TEMPORARY).await?;
    ensure!(
        old.status == StatusCode::UNAUTHORIZED && old.error_code()? == "AUTH_INVALID_CREDENTIALS",
        "{} {}",
        old.status,
        old.body
    );
    let renamed = world
        .login("NEWNAME", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(renamed.json()?["user"]["username"] == "NewName");
    world
        .login("oldname@example.test", TEMPORARY)
        .await?
        .expect(StatusCode::OK)?;

    let me = world
        .http
        .get(ME, Some(&existing))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(me["user"]["username"] == "NewName", "{me}");
    world.shutdown().await;
    Ok(())
}
