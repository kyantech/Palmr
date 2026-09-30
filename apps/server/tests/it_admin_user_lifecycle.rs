pub mod support;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::header::SET_COOKIE;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio::task::JoinSet;

use support::client::{Creds, Db, Http, Reply, EPOCH, PASSWORD};
use support::TestApplication;

const USERS: &str = "/api/v1/admin/users";
const LOGIN: &str = "/api/v1/auth/login";
const ME: &str = "/api/v1/auth/me";
const ABSENT_ID: &str = "0192f3a1-0000-7000-8000-00000000abcd";

struct World {
    app: TestApplication,
    http: Arc<Http>,
    db: Db,
    admin: Creds,
    admin_id: String,
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

    async fn shutdown(self) {
        self.app.shutdown().await;
    }

    async fn member(&self, username: &str, role: &str) -> Result<String> {
        let created = self
            .http
            .send(
                Method::POST,
                USERS,
                Some(&self.admin),
                Some(json!({
                    "firstName": "Test",
                    "lastName": username,
                    "username": username,
                    "email": format!("{username}@example.test"),
                    "role": role,
                    "password": PASSWORD,
                    "requirePasswordChange": false,
                    "locale": "en-US",
                })),
            )
            .await?
            .expect(StatusCode::CREATED)?
            .json()?;
        Ok(created["id"].as_str().context("created id")?.to_owned())
    }

    async fn login(&self, identifier: &str) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                LOGIN,
                None,
                Some(json!({ "identifier": identifier, "password": PASSWORD })),
            )
            .await
    }

    async fn sign_in(&self, identifier: &str) -> Result<Creds> {
        self.login(identifier)
            .await?
            .expect(StatusCode::OK)?
            .creds()
    }

    async fn status_of(&self, creds: &Creds) -> Result<StatusCode> {
        Ok(self.http.get(ME, Some(creds)).await?.status)
    }

    async fn put_role(&self, actor: &Creds, id: &str, role: &str) -> Result<Reply> {
        self.http
            .send(
                Method::PUT,
                &format!("{USERS}/{id}/role"),
                Some(actor),
                Some(json!({ "role": role })),
            )
            .await
    }

    async fn deactivate(&self, actor: &Creds, id: &str) -> Result<Reply> {
        self.post_action(actor, id, "deactivate").await
    }

    async fn activate(&self, actor: &Creds, id: &str) -> Result<Reply> {
        self.post_action(actor, id, "activate").await
    }

    async fn post_action(&self, actor: &Creds, id: &str, action: &str) -> Result<Reply> {
        self.http
            .send(
                Method::POST,
                &format!("{USERS}/{id}/{action}"),
                Some(actor),
                None,
            )
            .await
    }

    async fn count(&self, sql: &str) -> Result<i64> {
        self.db.scalar_i64(sql).await
    }

    async fn audit(&self, action: &str) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
    }

    async fn metadata(&self, action: &str) -> Result<Value> {
        let text = self
            .db
            .scalar_string(&format!(
                "SELECT metadata_json FROM audit_events WHERE action = '{action}'
                  ORDER BY id DESC LIMIT 1"
            ))
            .await?;
        Ok(serde_json::from_str(&text)?)
    }

    async fn active_admins(&self) -> Result<i64> {
        self.count("SELECT COUNT(*) FROM users WHERE role = 'admin' AND is_active = 1")
            .await
    }

    async fn live_sessions(&self, id: &str) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM sessions WHERE user_id = '{id}' AND state = 'active'"
        ))
        .await
    }

    async fn revoked_sessions(&self, id: &str, reason: &str) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM sessions
              WHERE user_id = '{id}' AND state = 'revoked' AND revoked_reason = '{reason}'
                AND revoked_at IS NOT NULL"
        ))
        .await
    }

    async fn live_devices(&self, id: &str) -> Result<i64> {
        self.count(&format!(
            "SELECT COUNT(*) FROM trusted_devices WHERE user_id = '{id}' AND revoked_at IS NULL"
        ))
        .await
    }

    async fn device(&self, user: &str, n: u64) -> Result<()> {
        self.db
            .execute_bound(
                "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, '2030-01-01T00:00:00.000Z')",
                &[
                    &support::client::v7(5_000 + n),
                    user,
                    &format!("{n:064x}"),
                    EPOCH,
                ],
            )
            .await
    }

    async fn provider(&self, n: u64) -> Result<String> {
        let id = support::client::v7(6_000 + n);
        self.db
            .execute_bound(
                "INSERT INTO identity_providers (id, key, display_name, kind, client_id,
                    client_secret_ciphertext, client_secret_nonce, created_at, updated_at)
                 VALUES (?1, ?2, ?2, 'oidc', 'client-id', x'deadbeefdeadbeef', zeroblob(24),
                    ?3, ?3)",
                &[&id, &format!("provider-{n}"), EPOCH],
            )
            .await?;
        Ok(id)
    }

    async fn link(&self, user: &str, provider: &str, n: u64) -> Result<()> {
        self.db
            .execute_bound(
                "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method,
                    state, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'auto_verified_email', 'active', ?5)",
                &[
                    &support::client::v7(7_000 + n),
                    user,
                    provider,
                    &format!("subject-{n}"),
                    EPOCH,
                ],
            )
            .await
    }

    async fn reverse_share(&self, owner: &str, n: u64) -> Result<()> {
        self.db
            .execute_bound(
                "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                &[
                    &support::client::v7(3_000 + n),
                    owner,
                    &format!("public-reverse-{n:08}"),
                    &format!("reverse-{n}"),
                    EPOCH,
                ],
            )
            .await
    }

    async fn link_states(&self, user: &str) -> Result<Vec<(String, Option<String>)>> {
        let mut connection = self.db.writer().await?;
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT state, suspended_at FROM identity_links WHERE user_id = ?1 ORDER BY id",
        )
        .bind(user)
        .fetch_all(&mut connection)
        .await?;
        Ok(rows)
    }

    async fn account(
        &self,
        id: &str,
    ) -> Result<(String, i64, Option<String>, Option<String>, String)> {
        let mut connection = self.db.writer().await?;
        let row = sqlx::query_as(
            "SELECT role, is_active, deactivated_at, deactivated_by, updated_at
               FROM users WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&mut connection)
        .await?;
        Ok(row)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    action: String,
    actor_type: String,
    actor_user_id: Option<String>,
    actor_label: String,
    target_id: Option<String>,
    target_type: String,
    result: String,
    metadata_json: String,
}

fn sets_cookies(reply: &Reply) -> bool {
    reply.headers.get_all(SET_COOKIE).iter().next().is_some()
}

fn fields(reply: &Reply) -> Result<Vec<String>> {
    Ok(reply.json()?["error"]["details"]["fields"]
        .as_array()
        .context("validation fields")?
        .iter()
        .filter_map(|field| field.as_str().map(str::to_owned))
        .collect())
}

fn ensure_code(reply: &Reply, status: StatusCode, code: &str) -> Result<()> {
    ensure!(
        reply.status == status && reply.error_code()? == code,
        "expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_role_promotes_and_demotes_with_the_role_read_from_the_database() -> Result<()> {
    let world = World::start("it_admin_role_promote_demote").await?;
    let bea_id = world.member("bea", "user").await?;
    let bea = world.sign_in("bea").await?;
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    let forbidden = world.http.get(USERS, Some(&bea)).await?;
    ensure_code(&forbidden, StatusCode::FORBIDDEN, "FORBIDDEN")?;

    let promoted = world
        .put_role(&world.admin, &bea_id, "admin")
        .await?
        .expect(StatusCode::OK)?;
    ensure!(!sets_cookies(&promoted), "no session is minted");
    let item = promoted.json()?;
    ensure!(
        item["id"] == bea_id.as_str() && item["role"] == "admin",
        "{item}"
    );
    ensure!(item["isActive"] == true, "{item}");
    ensure!(
        world.status_of(&bea).await? == StatusCode::UNAUTHORIZED,
        "the pre-promotion session is revoked"
    );
    ensure!(world.revoked_sessions(&bea_id, "role_changed").await? == 1);
    ensure!(world.active_admins().await? == 2);
    let meta = world.metadata("USER_ROLE_CHANGED").await?;
    ensure!(
        meta == json!({ "from": "user", "to": "admin", "sessions_revoked": 1 }),
        "{meta}"
    );

    let admin_bea = world.sign_in("bea").await?;
    world
        .http
        .get(USERS, Some(&admin_bea))
        .await?
        .expect(StatusCode::OK)?;

    let demoted = world
        .put_role(&world.admin, &bea_id, "user")
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(demoted["role"] == "user", "{demoted}");
    ensure!(world.status_of(&admin_bea).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.revoked_sessions(&bea_id, "role_changed").await? == 2);
    let after = world.sign_in("bea").await?;
    let denied = world.http.get(USERS, Some(&after)).await?;
    ensure_code(&denied, StatusCode::FORBIDDEN, "FORBIDDEN")?;
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 2);
    world
        .http
        .get(ME, Some(&world.admin))
        .await?
        .expect(StatusCode::OK)?;
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_role_change_to_the_current_role_is_a_no_op() -> Result<()> {
    let world = World::start("it_admin_role_same_value").await?;
    let bea_id = world.member("bea", "user").await?;
    let bea = world.sign_in("bea").await?;
    world.device(&bea_id, 1).await?;
    let before = world.account(&bea_id).await?;

    world.app.clock().advance(Duration::from_secs(60));
    let same = world
        .put_role(&world.admin, &bea_id, "user")
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(same["role"] == "user", "{same}");
    ensure!(world.account(&bea_id).await? == before, "no column moves");
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    ensure!(world.live_sessions(&bea_id).await? == 1);
    ensure!(world.live_devices(&bea_id).await? == 1);
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);

    let ada_same = world
        .put_role(&world.admin, &world.admin_id, "admin")
        .await?
        .expect(StatusCode::OK)?;
    ensure!(ada_same.json()?["role"] == "admin");
    ensure!(world.status_of(&world.admin).await? == StatusCode::OK);
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_deactivation_revokes_sessions_and_blocks_login() -> Result<()> {
    let world = World::start("it_deactivation_revokes_and_blocks").await?;
    let bea_id = world.member("bea", "user").await?;
    let cyd_id = world.member("cyd", "user").await?;
    let first = world.sign_in("bea").await?;
    let second = world.sign_in("bea").await?;
    let cyd = world.sign_in("cyd").await?;
    let provider = world.provider(1).await?;
    world.link(&bea_id, &provider, 1).await?;
    let other_provider = world.provider(2).await?;
    world.link(&cyd_id, &other_provider, 2).await?;
    world.device(&bea_id, 1).await?;
    world.device(&bea_id, 2).await?;
    world.device(&cyd_id, 3).await?;
    world.reverse_share(&bea_id, 1).await?;
    let wrong = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            Some(json!({ "identifier": "bea", "password": "not the password at all" })),
        )
        .await?;
    ensure!(wrong.status == StatusCode::UNAUTHORIZED);
    ensure!(world.live_sessions(&bea_id).await? == 2);
    ensure!(world.live_devices(&bea_id).await? == 2);

    world.app.clock().advance(Duration::from_secs(5));
    let reply = world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(!sets_cookies(&reply));
    let item = reply.json()?;
    ensure!(
        item["id"] == bea_id.as_str() && item["isActive"] == false,
        "{item}"
    );

    ensure!(world.status_of(&first).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.status_of(&second).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.live_sessions(&bea_id).await? == 0);
    ensure!(world.revoked_sessions(&bea_id, "deactivated").await? == 2);
    ensure!(world.live_devices(&bea_id).await? == 0);
    let (role, active, at, by, _) = world.account(&bea_id).await?;
    ensure!(role == "user" && active == 0, "{role} {active}");
    ensure!(at.is_some(), "deactivated_at is set");
    ensure!(by.as_deref() == Some(world.admin_id.as_str()), "{by:?}");
    let links = world.link_states(&bea_id).await?;
    ensure!(
        links.len() == 1 && links[0].0 == "suspended" && links[0].1 == at,
        "{links:?} {at:?}"
    );

    let refused = world.login("bea").await?;
    ensure!(
        refused.status == StatusCode::UNAUTHORIZED
            && refused.error_code()? == wrong.error_code()?,
        "an inactive account answers exactly like a wrong password: {} {}",
        refused.status,
        refused.body
    );
    ensure!(!sets_cookies(&refused));
    ensure!(world.live_sessions(&bea_id).await? == 0);
    let by_email = world
        .http
        .send(
            Method::POST,
            LOGIN,
            None,
            Some(json!({ "identifier": "BEA@example.test", "password": PASSWORD })),
        )
        .await?;
    ensure!(by_email.status == StatusCode::UNAUTHORIZED);

    ensure!(world.status_of(&cyd).await? == StatusCode::OK);
    ensure!(world.live_devices(&cyd_id).await? == 1);
    ensure!(
        world.link_states(&cyd_id).await? == vec![("active".to_owned(), None)],
        "an unrelated user's identity link is untouched"
    );
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM reverse_shares
                  WHERE owner_id = '{bea_id}' AND suspended_at IS NULL"
            ))
            .await?
            == 1,
        "public capabilities are suspended by the live owner-active predicate, not a batch write"
    );
    ensure!(world.audit("USER_DEACTIVATED").await? == 1);
    let meta = world.metadata("USER_DEACTIVATED").await?;
    ensure!(
        meta == json!({
            "sessions_revoked": 2,
            "trusted_devices_revoked": 2,
            "identity_links_suspended": 1
        }),
        "{meta}"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_activation_restores_login_and_links_without_restoring_sessions() -> Result<()> {
    let world = World::start("it_activation_restores_login").await?;
    let bea_id = world.member("bea", "user").await?;
    let old = world.sign_in("bea").await?;
    let provider = world.provider(1).await?;
    world.link(&bea_id, &provider, 1).await?;
    world.device(&bea_id, 1).await?;
    world.reverse_share(&bea_id, 1).await?;
    world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.login("bea").await?.status == StatusCode::UNAUTHORIZED);

    let reply = world
        .activate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(!sets_cookies(&reply), "activation mints no session");
    let item = reply.json()?;
    ensure!(
        item["isActive"] == true && item["id"] == bea_id.as_str(),
        "{item}"
    );

    let (role, active, at, by, _) = world.account(&bea_id).await?;
    ensure!(role == "user" && active == 1 && at.is_none() && by.is_none());
    ensure!(
        world.link_states(&bea_id).await? == vec![("active".to_owned(), None)],
        "the suspended identity link is restored"
    );
    ensure!(world.live_sessions(&bea_id).await? == 0);
    ensure!(world.status_of(&old).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.revoked_sessions(&bea_id, "deactivated").await? == 1);
    ensure!(
        world.live_devices(&bea_id).await? == 0,
        "revoked trusted devices are not resurrected"
    );
    ensure!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM reverse_shares
                  WHERE owner_id = '{bea_id}' AND suspended_at IS NULL"
            ))
            .await?
            == 1
    );

    let fresh = world.sign_in("bea").await?;
    ensure!(world.status_of(&fresh).await? == StatusCode::OK);
    ensure!(world.audit("USER_ACTIVATED").await? == 1);
    ensure!(world.metadata("USER_ACTIVATED").await? == json!({ "identity_links_restored": 1 }));
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_lifecycle_requests_are_idempotent() -> Result<()> {
    let world = World::start("it_admin_lifecycle_idempotent").await?;
    let bea_id = world.member("bea", "user").await?;
    let provider = world.provider(1).await?;
    world.link(&bea_id, &provider, 1).await?;
    let bea = world.sign_in("bea").await?;

    let untouched = world.account(&bea_id).await?;
    world.app.clock().advance(Duration::from_secs(30));
    world
        .activate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.account(&bea_id).await? == untouched);
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    ensure!(world.audit("USER_ACTIVATED").await? == 0);

    world.app.clock().advance(Duration::from_secs(30));
    world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    let once = world.account(&bea_id).await?;
    let revoked_at = world
        .db
        .scalar_string(&format!(
            "SELECT revoked_at FROM sessions WHERE user_id = '{bea_id}'"
        ))
        .await?;
    let suspended_at = world.link_states(&bea_id).await?;

    world.app.clock().advance(Duration::from_secs(30));
    let again = world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(again["isActive"] == false, "{again}");
    ensure!(
        world.account(&bea_id).await? == once,
        "metadata is not rewritten"
    );
    ensure!(
        world
            .db
            .scalar_string(&format!(
                "SELECT revoked_at FROM sessions WHERE user_id = '{bea_id}'"
            ))
            .await?
            == revoked_at
    );
    ensure!(world.link_states(&bea_id).await? == suspended_at);
    ensure!(world.audit("USER_DEACTIVATED").await? == 1);

    world.app.clock().advance(Duration::from_secs(30));
    world
        .activate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    let active_once = world.account(&bea_id).await?;
    world.app.clock().advance(Duration::from_secs(30));
    world
        .activate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.account(&bea_id).await? == active_once);
    ensure!(world.audit("USER_ACTIVATED").await? == 1);
    ensure!(world.audit("USER_DEACTIVATED").await? == 1);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_last_admin_cannot_self_demote_or_self_deactivate() -> Result<()> {
    let world = World::start("it_last_admin_self_actions").await?;
    let before = world.account(&world.admin_id).await?;

    let demote = world
        .put_role(&world.admin, &world.admin_id, "user")
        .await?;
    ensure_code(&demote, StatusCode::CONFLICT, "LAST_ADMIN_PROTECTED")?;
    let deactivate = world.deactivate(&world.admin, &world.admin_id).await?;
    ensure_code(&deactivate, StatusCode::CONFLICT, "LAST_ADMIN_PROTECTED")?;

    ensure!(world.account(&world.admin_id).await? == before);
    ensure!(world.status_of(&world.admin).await? == StatusCode::OK);
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);
    ensure!(world.audit("USER_DEACTIVATED").await? == 0);

    let bea_id = world.member("bea", "admin").await?;
    world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.active_admins().await? == 1);
    let still = world
        .put_role(&world.admin, &world.admin_id, "user")
        .await?;
    ensure_code(&still, StatusCode::CONFLICT, "LAST_ADMIN_PROTECTED")?;

    world
        .put_role(&world.admin, &bea_id, "user")
        .await?
        .expect(StatusCode::OK)?;
    ensure!(
        world.active_admins().await? == 1,
        "an inactive Admin never counts toward the guard"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_self_demotion_and_self_deactivation_succeed_with_another_admin() -> Result<()> {
    let world = World::start("it_admin_self_actions_allowed").await?;
    let bea_id = world.member("bea", "admin").await?;
    let bea = world.sign_in("bea").await?;

    let second_session = world.sign_in("ada").await?;
    let demote = world
        .put_role(&world.admin, &world.admin_id, "user")
        .await?
        .expect(StatusCode::OK)?;
    ensure!(demote.json()?["role"] == "user");
    ensure!(world.status_of(&world.admin).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.status_of(&second_session).await? == StatusCode::UNAUTHORIZED);
    ensure!(
        world
            .revoked_sessions(&world.admin_id, "role_changed")
            .await?
            == 2,
        "the acting session is revoked like every other target session"
    );
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    let refused = world.put_role(&bea, &bea_id, "user").await?;
    ensure_code(&refused, StatusCode::CONFLICT, "LAST_ADMIN_PROTECTED")?;
    world.shutdown().await;

    let world = World::start("it_admin_self_deactivation_allowed").await?;
    let bea_id = world.member("bea", "admin").await?;
    let bea = world.sign_in("bea").await?;
    let deactivated = world
        .deactivate(&world.admin, &world.admin_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(deactivated.json()?["isActive"] == false);
    ensure!(world.status_of(&world.admin).await? == StatusCode::UNAUTHORIZED);
    ensure!(world.login("ada").await?.status == StatusCode::UNAUTHORIZED);
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    let refused = world.deactivate(&bea, &bea_id).await?;
    ensure_code(&refused, StatusCode::CONFLICT, "LAST_ADMIN_PROTECTED")?;
    let revived = world
        .activate(&bea, &world.admin_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(revived.json()?["isActive"] == true);
    ensure!(world.sign_in("ada").await.is_ok());
    world.shutdown().await;
    Ok(())
}

async fn race(name: &str, demote_first: bool, demote_second: bool) -> Result<()> {
    let world = World::start(name).await?;
    let bea_id = world.member("bea", "admin").await?;
    let bea = world.sign_in("bea").await?;

    let mut tasks = JoinSet::new();
    for (actor, target, demote) in [
        (world.admin.clone(), bea_id.clone(), demote_first),
        (bea.clone(), world.admin_id.clone(), demote_second),
    ] {
        let http = Arc::clone(&world.http);
        tasks.spawn(async move {
            if demote {
                http.send(
                    Method::PUT,
                    &format!("{USERS}/{target}/role"),
                    Some(&actor),
                    Some(json!({ "role": "user" })),
                )
                .await
            } else {
                http.send(
                    Method::POST,
                    &format!("{USERS}/{target}/deactivate"),
                    Some(&actor),
                    None,
                )
                .await
            }
        });
    }
    let mut succeeded = 0;
    while let Some(joined) = tasks.join_next().await {
        let reply = joined??;
        match reply.status {
            StatusCode::OK => succeeded += 1,
            StatusCode::CONFLICT => ensure!(
                reply.error_code()? == "LAST_ADMIN_PROTECTED",
                "{name}: {}",
                reply.body
            ),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {}
            other => anyhow::bail!("{name}: unexpected {other}: {}", reply.body),
        }
    }
    ensure!(succeeded == 1, "{name}: {succeeded} removals succeeded");
    ensure!(world.active_admins().await? == 1, "{name}");
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_http_concurrent_lifecycle_requests_never_remove_the_last_admin() -> Result<()> {
    race("it_http_race_demote_demote", true, true).await?;
    race("it_http_race_deactivate_deactivate", false, false).await?;
    race("it_http_race_demote_deactivate", true, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_lifecycle_routes_reject_unknown_users_and_bad_bodies() -> Result<()> {
    let world = World::start("it_admin_lifecycle_unknown_and_invalid").await?;
    let bea_id = world.member("bea", "user").await?;
    let before = world.account(&bea_id).await?;

    for id in [ABSENT_ID, "not-a-uuid"] {
        ensure_code(
            &world.put_role(&world.admin, id, "user").await?,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        )?;
        ensure_code(
            &world.deactivate(&world.admin, id).await?,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        )?;
        ensure_code(
            &world.activate(&world.admin, id).await?,
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
        )?;
    }
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);

    let path = format!("{USERS}/{bea_id}/role");
    for payload in [
        json!({ "role": "superuser" }),
        json!({ "role": "Admin" }),
        json!({ "role": "" }),
    ] {
        let reply = world
            .http
            .send(
                Method::PUT,
                &path,
                Some(&world.admin),
                Some(payload.clone()),
            )
            .await?;
        ensure_code(&reply, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR")?;
        ensure!(fields(&reply)? == ["role"], "{payload}");
    }
    for payload in [
        json!({}),
        json!({ "role": "admin", "isActive": false }),
        json!({ "role": 1 }),
        json!({ "role": null }),
    ] {
        let reply = world
            .http
            .send(
                Method::PUT,
                &path,
                Some(&world.admin),
                Some(payload.clone()),
            )
            .await?;
        ensure_code(&reply, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR")?;
    }
    ensure!(world.account(&bea_id).await? == before);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_lifecycle_routes_enforce_role_and_recent_authentication() -> Result<()> {
    let world = World::start("it_admin_lifecycle_authorization").await?;
    let bea_id = world.member("bea", "user").await?;
    let cyd_id = world.member("cyd", "user").await?;
    let bea = world.sign_in("bea").await?;
    let before = world.account(&cyd_id).await?;

    ensure_code(
        &world.put_role(&bea, &cyd_id, "admin").await?,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    )?;
    ensure_code(
        &world.deactivate(&bea, &cyd_id).await?,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    )?;
    ensure_code(
        &world.activate(&bea, &cyd_id).await?,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    )?;
    ensure_code(
        &world.put_role(&bea, &bea_id, "admin").await?,
        StatusCode::FORBIDDEN,
        "FORBIDDEN",
    )?;
    ensure!(world.account(&cyd_id).await? == before);
    ensure!(world.account(&bea_id).await?.0 == "user");

    world.app.clock().advance(Duration::from_secs(31 * 60));
    ensure_code(
        &world.put_role(&world.admin, &cyd_id, "admin").await?,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    )?;
    ensure_code(
        &world.deactivate(&world.admin, &cyd_id).await?,
        StatusCode::FORBIDDEN,
        "AUTH_RECENT_AUTH_REQUIRED",
    )?;
    ensure!(world.account(&cyd_id).await? == before);
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);
    ensure!(world.audit("USER_DEACTIVATED").await? == 0);

    world
        .db
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '{EPOCH}' WHERE id = '{cyd_id}'"
        ))
        .await?;
    world
        .activate(&world.admin, &cyd_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.account(&cyd_id).await?.1 == 1);
    ensure!(world.audit("USER_ACTIVATED").await? == 1);
    world.shutdown().await;
    Ok(())
}

async fn inject_audit_failure(world: &World, action: &str) -> Result<()> {
    world
        .db
        .execute(&format!(
            "CREATE TRIGGER lifecycle_injected_audit_failure BEFORE INSERT ON audit_events
             WHEN NEW.action = '{action}'
             BEGIN SELECT RAISE(ABORT, 'injected lifecycle audit failure'); END"
        ))
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_lifecycle_audit_is_atomic_with_every_side_effect() -> Result<()> {
    let world = World::start("it_admin_lifecycle_atomic_audit").await?;
    let bea_id = world.member("bea", "admin").await?;
    let bea = world.sign_in("bea").await?;
    let provider = world.provider(1).await?;
    world.link(&bea_id, &provider, 1).await?;
    world.device(&bea_id, 1).await?;
    let account = world.account(&bea_id).await?;
    let links = world.link_states(&bea_id).await?;

    inject_audit_failure(&world, "USER_ROLE_CHANGED").await?;
    let failed = world.put_role(&world.admin, &bea_id, "user").await?;
    ensure_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")?;
    world
        .db
        .execute("DROP TRIGGER lifecycle_injected_audit_failure")
        .await?;
    ensure!(world.account(&bea_id).await? == account, "role not changed");
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    ensure!(world.live_sessions(&bea_id).await? == 1);
    ensure!(world.audit("USER_ROLE_CHANGED").await? == 0);

    inject_audit_failure(&world, "USER_DEACTIVATED").await?;
    let failed = world.deactivate(&world.admin, &bea_id).await?;
    ensure_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")?;
    world
        .db
        .execute("DROP TRIGGER lifecycle_injected_audit_failure")
        .await?;
    ensure!(world.account(&bea_id).await? == account, "still active");
    ensure!(world.status_of(&bea).await? == StatusCode::OK);
    ensure!(world.live_sessions(&bea_id).await? == 1);
    ensure!(world.live_devices(&bea_id).await? == 1);
    ensure!(world.link_states(&bea_id).await? == links);
    ensure!(world.audit("USER_DEACTIVATED").await? == 0);

    world
        .deactivate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    let deactivated = world.account(&bea_id).await?;
    let suspended = world.link_states(&bea_id).await?;
    inject_audit_failure(&world, "USER_ACTIVATED").await?;
    let failed = world.activate(&world.admin, &bea_id).await?;
    ensure_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")?;
    world
        .db
        .execute("DROP TRIGGER lifecycle_injected_audit_failure")
        .await?;
    ensure!(
        world.account(&bea_id).await? == deactivated,
        "still inactive"
    );
    ensure!(world.link_states(&bea_id).await? == suspended);
    ensure!(world.audit("USER_ACTIVATED").await? == 0);

    world
        .activate(&world.admin, &bea_id)
        .await?
        .expect(StatusCode::OK)?;
    ensure!(world.audit("USER_ACTIVATED").await? == 1);
    world.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_lifecycle_audit_records_actor_target_and_no_secret_material() -> Result<()> {
    let world = World::start("it_admin_lifecycle_audit_shape").await?;
    let bea_id = world.member("bea", "user").await?;
    let _bea = world.sign_in("bea").await?;
    world.put_role(&world.admin, &bea_id, "admin").await?;
    world.deactivate(&world.admin, &bea_id).await?;
    world.activate(&world.admin, &bea_id).await?;

    let mut connection = world.db.writer().await?;
    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT action, actor_type, actor_user_id, actor_label, target_id, target_type,
                result, metadata_json
           FROM audit_events
          WHERE action IN ('USER_ROLE_CHANGED', 'USER_DEACTIVATED', 'USER_ACTIVATED')
          ORDER BY id",
    )
    .fetch_all(&mut connection)
    .await?;
    ensure!(rows.len() == 3, "{rows:?}");
    for row in &rows {
        let action = &row.action;
        let metadata = &row.metadata_json;
        ensure!(
            row.actor_type == "user"
                && row.actor_user_id.as_deref() == Some(world.admin_id.as_str())
        );
        ensure!(row.actor_label == "ada" && row.target_id.as_deref() == Some(bea_id.as_str()));
        ensure!(
            row.target_type == "user" && row.result == "success",
            "{action}"
        );
        ensure!(metadata.len() < 512, "{action}: {metadata}");
        for forbidden in ["token", "hash", "password", "secret", "cookie", "csrf"] {
            ensure!(!metadata.contains(forbidden), "{action}: {metadata}");
        }
    }
    let actions: Vec<&str> = rows.iter().map(|row| row.action.as_str()).collect();
    ensure!(
        actions == ["USER_ROLE_CHANGED", "USER_DEACTIVATED", "USER_ACTIVATED"],
        "{actions:?}"
    );
    world.shutdown().await;
    Ok(())
}
