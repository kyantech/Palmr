pub mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use palmr_server::TestClock;
use support::client::{v7, Creds, Db, Http, SessionSpec};
use support::TestApplication;

const RATE_WINDOW: Duration = Duration::from_secs(3600);
const PLACEHOLDER_ID: &str = "0192f3a1-0000-7000-8000-00000000ffff";
const ADMIN_CLASSES: [&str; 2] = ["admin", "admin+recent-auth"];
const METHODS: [(&str, &str); 6] = [
    ("get", "GET"),
    ("head", "HEAD"),
    ("post", "POST"),
    ("put", "PUT"),
    ("patch", "PATCH"),
    ("delete", "DELETE"),
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AdminRoute {
    method: String,
    template: String,
    class: String,
}

impl AdminRoute {
    fn path(&self) -> String {
        let mut path = String::new();
        let mut rest = self.template.as_str();
        while let Some(open) = rest.find('{') {
            path.push_str(&rest[..open]);
            let close = rest[open..].find('}').map_or(rest.len(), |at| open + at);
            path.push_str(PLACEHOLDER_ID);
            rest = rest.get(close + 1..).unwrap_or_default();
        }
        path.push_str(rest);
        path
    }

    fn method(&self) -> Result<Method> {
        Method::from_bytes(self.method.as_bytes()).context("route method")
    }

    fn body(&self) -> Option<Value> {
        matches!(self.method.as_str(), "POST" | "PUT" | "PATCH").then(|| json!({}))
    }

    fn label(&self) -> String {
        format!("{} {} ({})", self.method, self.template, self.class)
    }
}

fn admin_routes() -> Result<Vec<AdminRoute>> {
    let document: Value = serde_json::from_slice(
        &palmr_server::openapi::export_document()
            .map_err(|error| anyhow::anyhow!("export OpenAPI document: {error}"))?,
    )?;
    let paths = document["paths"].as_object().context("paths object")?;
    let mut routes = Vec::new();
    for (template, item) in paths {
        for (key, method) in METHODS {
            let Some(operation) = item.get(key) else {
                continue;
            };
            let tags: Vec<&str> = operation["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(class) = ADMIN_CLASSES.iter().find(|class| tags.contains(class)) {
                routes.push(AdminRoute {
                    method: method.to_owned(),
                    template: template.clone(),
                    class: (*class).to_owned(),
                });
            }
        }
    }
    routes.sort();
    Ok(routes)
}

fn state_changing(route: &AdminRoute) -> bool {
    matches!(route.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE")
}

async fn assert_anonymous_rejected(
    http: &Http,
    clock: &TestClock,
    routes: &[AdminRoute],
    state: &str,
) -> Result<()> {
    for route in routes {
        clock.advance(RATE_WINDOW);
        let reply = if state_changing(route) {
            http.send_with_anonymous_csrf(route.method()?, &route.path(), route.body())
                .await?
        } else {
            http.send(route.method()?, &route.path(), None, route.body())
                .await?
        };
        ensure!(
            reply.status == StatusCode::UNAUTHORIZED,
            "{state}: anonymous {} answered {} {}",
            route.label(),
            reply.status,
            reply.body
        );
        ensure!(
            reply.error_code()? == "AUTH_REQUIRED",
            "{state}: anonymous {} answered {}",
            route.label(),
            reply.body
        );
        if state_changing(route) {
            let without_proof = http
                .send(route.method()?, &route.path(), None, route.body())
                .await?;
            ensure!(
                without_proof.status == StatusCode::FORBIDDEN
                    && without_proof.error_code()? == "CSRF_TOKEN_MISSING",
                "{state}: anonymous {} without a CSRF proof answered {} {}",
                route.label(),
                without_proof.status,
                without_proof.body
            );
        }
    }
    Ok(())
}

async fn assert_user_forbidden(
    http: &Http,
    clock: &TestClock,
    routes: &[AdminRoute],
    creds: &Creds,
    state: &str,
) -> Result<()> {
    for route in routes {
        clock.advance(RATE_WINDOW);
        let reply = http
            .send(route.method()?, &route.path(), Some(creds), route.body())
            .await?;
        ensure!(
            reply.status == StatusCode::FORBIDDEN && reply.error_code()? == "FORBIDDEN",
            "{state}: role user on {} answered {} {}",
            route.label(),
            reply.status,
            reply.body
        );
    }
    Ok(())
}

#[test]
fn it_r031_route_registry_enumerates_every_admin_route() -> Result<()> {
    let routes = admin_routes()?;
    let labels: BTreeSet<String> = routes.iter().map(AdminRoute::label).collect();
    for expected in [
        "GET /api/v1/admin/users (admin)",
        "GET /api/v1/admin/users/{id} (admin)",
        "GET /api/v1/admin/users/{userId}/sessions (admin)",
        "POST /api/v1/admin/users (admin)",
        "PATCH /api/v1/admin/users/{id} (admin)",
        "PUT /api/v1/admin/users/{id}/role (admin+recent-auth)",
        "POST /api/v1/admin/users/{id}/activate (admin)",
        "POST /api/v1/admin/users/{id}/deactivate (admin+recent-auth)",
        "POST /api/v1/admin/users/{id}/password-reset (admin+recent-auth)",
        "POST /api/v1/admin/users/{id}/unlock (admin)",
        "DELETE /api/v1/admin/users/{userId}/sessions (admin+recent-auth)",
        "PUT /api/v1/admin/users/{id}/quota (admin)",
        "POST /api/v1/admin/users/{id}/email (admin+recent-auth)",
        "POST /api/v1/admin/users/{id}/email/resend (admin+recent-auth)",
        "DELETE /api/v1/admin/users/{id}/email (admin+recent-auth)",
        "GET /api/v1/admin/settings (admin)",
        "GET /api/v1/admin/settings/general (admin)",
        "PATCH /api/v1/admin/settings/general (admin)",
        "GET /api/v1/admin/settings/security (admin)",
        "PATCH /api/v1/admin/settings/security (admin+recent-auth)",
        "GET /api/v1/admin/settings/quotas (admin)",
        "PATCH /api/v1/admin/settings/quotas (admin)",
        "GET /api/v1/admin/settings/public-links (admin)",
        "PATCH /api/v1/admin/settings/public-links (admin)",
        "GET /api/v1/admin/invites (admin)",
        "POST /api/v1/admin/invites (admin)",
        "POST /api/v1/admin/invites/{id}/resend (admin)",
        "DELETE /api/v1/admin/invites/{id} (admin)",
    ] {
        ensure!(labels.contains(expected), "{expected} is not enumerated");
    }
    for route in &routes {
        ensure!(
            route.template.starts_with("/api/v1/admin/"),
            "{} is admin-classed outside the admin namespace",
            route.label()
        );
        ensure!(!route.path().contains('{'), "{}", route.path());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R031_admin_endpoints_never_unauthenticated() -> Result<()> {
    let routes = admin_routes()?;
    ensure!(!routes.is_empty(), "no admin route was enumerated");

    let empty = TestApplication::start("regression_R031_empty_instance").await?;
    let http = Http::new(empty.url("/")?)?;
    let db = Db::new(empty.data_dir());
    ensure!(db.scalar_i64("SELECT COUNT(*) FROM users").await? == 0);
    assert_anonymous_rejected(&http, empty.clock(), &routes, "zero users").await?;
    empty.shutdown().await;

    let mid_setup = TestApplication::start("regression_R031_mid_setup").await?;
    let http = Http::new(mid_setup.url("/")?)?;
    let db = Db::new(mid_setup.data_dir());
    db.insert_user(&v7(1), "preexisting", "admin").await?;
    ensure!(db.scalar_i64("SELECT COUNT(*) FROM users").await? == 1);
    ensure!(
        db.scalar_i64(
            "SELECT COUNT(*) FROM app_settings WHERE key = 'setup_completed' AND value_json = 'true'"
        )
        .await?
            == 0,
        "setup must still be incomplete"
    );
    assert_anonymous_rejected(
        &http,
        mid_setup.clock(),
        &routes,
        "mid-setup with one admin row",
    )
    .await?;
    mid_setup.shutdown().await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R031_exactly_one_admin_grants_no_anonymous_privilege() -> Result<()> {
    let routes = admin_routes()?;
    let app = TestApplication::start("regression_R031_single_admin").await?;
    let http = Http::new(app.url("/")?)?;
    let admin = http.setup_admin().await?;
    let db = Db::new(app.data_dir());
    ensure!(db.scalar_i64("SELECT COUNT(*) FROM users").await? == 1);
    ensure!(
        db.scalar_i64("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .await?
            == 1
    );
    ensure!(
        db.scalar_i64(
            "SELECT COUNT(*) FROM app_settings WHERE key = 'setup_completed' AND value_json = 'true'"
        )
        .await?
            == 1
    );

    assert_anonymous_rejected(&http, app.clock(), &routes, "exactly one user, an Admin").await?;

    let listed = http
        .get("/api/v1/admin/users", Some(&admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(listed["totalCount"] == 1, "{listed}");
    ensure!(listed["items"][0]["role"] == "admin", "{listed}");

    let cookieless_with_forged = http
        .send(
            Method::GET,
            "/api/v1/admin/users",
            Some(&Creds {
                session: "forged".to_owned(),
                csrf: "forged".to_owned(),
            }),
            None,
        )
        .await?;
    ensure!(
        cookieless_with_forged.status == StatusCode::UNAUTHORIZED
            && cookieless_with_forged.error_code()? == "AUTH_REQUIRED",
        "{}",
        cookieless_with_forged.body
    );
    app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
async fn regression_R031_authenticated_user_is_forbidden_on_every_admin_route() -> Result<()> {
    let routes = admin_routes()?;

    let app = TestApplication::start("regression_R031_user_matrix").await?;
    let http = Http::new(app.url("/")?)?;
    let admin = http.setup_admin().await?;
    let user = http.invite_and_accept(&admin, "bea").await?;
    assert_user_forbidden(&http, app.clock(), &routes, &user, "admin plus user").await?;
    assert_anonymous_rejected(&http, app.clock(), &routes, "admin plus user").await?;
    http.get("/api/v1/admin/users", Some(&admin))
        .await?
        .expect(StatusCode::OK)?;
    app.shutdown().await;

    let only_user = TestApplication::start("regression_R031_only_a_user").await?;
    let http = Http::new(only_user.url("/")?)?;
    let db = Db::new(only_user.data_dir());
    let id = v7(7);
    db.insert_user(&id, "lonely", "user").await?;
    let (_, creds) = db.insert_session(&SessionSpec::active(&id)).await?;
    assert_user_forbidden(
        &http,
        only_user.clock(),
        &routes,
        &creds,
        "one user, role user",
    )
    .await?;
    assert_anonymous_rejected(&http, only_user.clock(), &routes, "one user, role user").await?;
    only_user.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_admin_role_is_read_from_the_database_on_every_request() -> Result<()> {
    let app = TestApplication::start("it_admin_role_read_per_request").await?;
    let http = Http::new(app.url("/")?)?;
    let db = Db::new(app.data_dir());
    let admin = http.setup_admin().await?;
    http.get("/api/v1/admin/users", Some(&admin))
        .await?
        .expect(StatusCode::OK)?;

    db.execute("UPDATE users SET role = 'user' WHERE username = 'ada'")
        .await?;
    let demoted = http.get("/api/v1/admin/users", Some(&admin)).await?;
    ensure!(
        demoted.status == StatusCode::FORBIDDEN && demoted.error_code()? == "FORBIDDEN",
        "{} {}",
        demoted.status,
        demoted.body
    );

    db.execute("UPDATE users SET role = 'admin' WHERE username = 'ada'")
        .await?;
    http.get("/api/v1/admin/users", Some(&admin))
        .await?
        .expect(StatusCode::OK)?;
    app.shutdown().await;
    Ok(())
}
