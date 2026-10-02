use super::admin_providers::{
    oauth2_body, oidc_body, ProviderAudit, PROVIDERS, REPLACEMENT, SECRET,
};
use super::admin_settings::assert_detail;
use super::profile::{assert_code, Call};
use super::*;
use crate::domain::clock::Clock;
use crate::features::audit::model::ClientMetadata;
use crate::features::identity_providers::input::{UpdateInput, UpdateProviderRequest};
use crate::features::identity_providers::model::ProviderId;
use crate::features::identity_providers::repo;
use crate::features::identity_providers::test_support::{FakeIdp, Reply};
use crate::infra::http::json::parse;

fn check_names(result: &Value) -> Vec<String> {
    result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| check["name"].as_str().unwrap().to_owned())
        .collect()
}

fn failing(fetched: &Fetched) -> Vec<(String, String)> {
    fetched.json()["error"]["details"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["ok"] == false)
        .map(|check| {
            (
                check["name"].as_str().unwrap().to_owned(),
                check["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

impl Stack {
    async fn test_provider(&self, creds: &Credentials, id: &str) -> Fetched {
        self.providers_call(Method::POST, &format!("/{id}/test"), creds, None)
            .await
    }

    async fn tested_ok(&self, creds: &Credentials, id: &str) -> Value {
        let fetched = self.test_provider(creds, id).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn validated_at(&self, id: &str) -> Option<String> {
        self.provider_scalar(id, "validated_at").await
    }

    async fn validation_error(&self, id: &str) -> Option<String> {
        self.provider_scalar(id, "validation_error").await
    }

    async fn audit_with_actor(&self) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT action, actor_user_id FROM audit_events
              WHERE action LIKE 'IDENTITY_PROVIDER_%' ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }
}

fn actions(rows: &[ProviderAudit]) -> Vec<&str> {
    rows.iter().map(|row| row.0.as_str()).collect()
}

#[tokio::test]
async fn it_provider_routes_enforce_their_documented_auth_classes() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let plain = stack.settings_user("plain", 11).await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fake_id = "0192f3a1-0000-7000-8000-00000000ffff";
    let discover = json!({ "issuerUrl": idp.issuer() });
    let create = oidc_body(&idp, "second");
    let order = json!({ "order": [id] });
    let routes: Vec<(Method, String, Option<&Value>)> = vec![
        (Method::GET, String::new(), None),
        (Method::POST, String::new(), Some(&create)),
        (Method::PATCH, format!("/{fake_id}"), Some(&order)),
        (Method::DELETE, format!("/{fake_id}"), None),
        (Method::PUT, "/order".to_owned(), Some(&order)),
        (Method::POST, "/discover".to_owned(), Some(&discover)),
        (Method::POST, format!("/{fake_id}/test"), None),
        (Method::GET, "/presets".to_owned(), None),
    ];
    let before = (stack.provider_count().await, stack.provider_audit().await);

    for (method, suffix, body) in &routes {
        let path = format!("{PROVIDERS}{suffix}");
        let mut call = Call::new(method.clone(), &path, &admin);
        call.session = None;
        if let Some(body) = body {
            call = call.json(body);
        }
        let anonymous = stack.call(call, 20).await;
        assert_code(&anonymous, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");

        let forbidden = stack
            .providers_call(method.clone(), suffix, &plain, *body)
            .await;
        assert_eq!(
            forbidden.status,
            StatusCode::FORBIDDEN,
            "{method} {suffix}: {}",
            forbidden.text()
        );
        assert_ne!(forbidden.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    }
    assert_eq!(
        before,
        (stack.provider_count().await, stack.provider_audit().await)
    );

    clock.advance(Duration::from_secs(6 * 60));
    for (method, suffix, body) in [
        (Method::POST, String::new(), Some(&create)),
        (
            Method::PATCH,
            format!("/{id}"),
            Some(&json!({ "displayName": "Nope" })),
        ),
        (Method::DELETE, format!("/{id}"), None),
    ] {
        let stale = stack
            .providers_call(method.clone(), &suffix, &admin, body)
            .await;
        assert_code(&stale, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    }
    assert_eq!(stack.provider_count().await, 1);
    assert_eq!(
        stack.provider_scalar(&id, "display_name").await.as_deref(),
        Some("Provider authentik")
    );
    for (method, suffix, body, status) in [
        (Method::GET, String::new(), None, StatusCode::OK),
        (Method::GET, "/presets".to_owned(), None, StatusCode::OK),
        (
            Method::PUT,
            "/order".to_owned(),
            Some(&order),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::POST,
            "/discover".to_owned(),
            Some(&discover),
            StatusCode::OK,
        ),
        (Method::POST, format!("/{id}/test"), None, StatusCode::OK),
    ] {
        let allowed = stack
            .providers_call(method.clone(), &suffix, &admin, body)
            .await;
        assert_eq!(
            allowed.status,
            status,
            "{method} {suffix}: {}",
            allowed.text()
        );
    }

    let reauth = stack.reauth_with_password(&admin, 10).await;
    assert_eq!(reauth.status, StatusCode::NO_CONTENT, "{}", reauth.text());
    assert_eq!(
        stack
            .patch_provider(&admin, &id, &json!({ "displayName": "Fresh" }))
            .await
            .status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_success_stamps_validated_at_with_safe_checks() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(stack.validated_at(&id).await, None);
    let requests_before = idp.requests().len();

    let result = stack.tested_ok(&admin, &id).await;

    let now = Timestamp::try_from(stack.clock.now()).unwrap().to_string();
    assert_eq!(
        result,
        json!({
            "ok": true,
            "checks": [
                { "name": "discovery", "ok": true, "detail": null },
                { "name": "jwks", "ok": true, "detail": null },
                { "name": "token_endpoint", "ok": true, "detail": null },
                { "name": "redirect_uri", "ok": true, "detail": null },
            ],
            "validatedAt": now,
        })
    );
    assert_eq!(stack.validated_at(&id).await.as_deref(), Some(now.as_str()));
    assert_eq!(stack.validation_error(&id).await, None);
    let listed = stack.list_providers(&admin).await;
    assert_eq!(listed["items"][0]["validatedAt"], now);
    assert_eq!(listed["items"][0]["validationError"], Value::Null);

    let probes: Vec<_> = idp.requests().into_iter().skip(requests_before).collect();
    let paths: std::collections::BTreeSet<&str> =
        probes.iter().map(|request| request.path.as_str()).collect();
    assert_eq!(
        paths,
        std::collections::BTreeSet::from([
            "/realm/.well-known/openid-configuration",
            "/realm/jwks",
            "/realm/token",
        ])
    );
    for request in &probes {
        assert_eq!(request.method, "GET");
        for forbidden in ["authorization", "cookie", "x-palmr-csrf"] {
            assert_eq!(request.header(forbidden), None, "{forbidden}");
        }
        assert!(!format!("{request:?}").contains(SECRET));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_failure_never_leaves_a_current_validation() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack.tested_ok(&admin, &id).await;
    assert!(stack.validated_at(&id).await.is_some());

    let leaked = "UPSTREAM-SECRET-DIAGNOSTIC-<html>stack trace</html>";
    let mut failing_jwks = Reply::raw(leaked.as_bytes().to_vec());
    failing_jwks.status = 500;
    idp.set("/realm/jwks", failing_jwks);
    idp.set("/realm/token", Reply::status(503));
    let failed = stack.test_provider(&admin, &id).await;
    assert_code(
        &failed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    assert_eq!(
        failing(&failed),
        [
            ("jwks".to_owned(), "upstream_error".to_owned()),
            ("token_endpoint".to_owned(), "upstream_error".to_owned()),
        ]
    );
    let checks = failed.json()["error"]["details"]["checks"].clone();
    assert_eq!(
        checks[0],
        json!({ "name": "discovery", "ok": true, "detail": null })
    );
    assert_eq!(
        checks[3],
        json!({ "name": "redirect_uri", "ok": true, "detail": null })
    );
    for needle in [leaked, SECRET, "stack trace", "<html>"] {
        assert!(!failed.text().contains(needle), "{needle}");
    }
    assert_eq!(
        stack.validated_at(&id).await,
        None,
        "a failed test must clear the current validation"
    );
    assert_eq!(
        stack.validation_error(&id).await.as_deref(),
        Some("jwks:upstream_error,token_endpoint:upstream_error")
    );
    let listed = stack.list_providers(&admin).await;
    assert_eq!(listed["items"][0]["validatedAt"], Value::Null);
    assert_eq!(
        listed["items"][0]["validationError"],
        "jwks:upstream_error,token_endpoint:upstream_error"
    );
    assert!(!stack.pools_text_contains(leaked).await);

    for (path, reply, name, detail) in [
        (
            "/realm/jwks",
            Reply::raw(b"{not json".to_vec()),
            "jwks",
            "malformed_document",
        ),
        (
            "/realm/jwks",
            Reply::json(&json!({ "keys": [] })),
            "jwks",
            "empty_key_set",
        ),
        (
            "/realm/jwks",
            Reply::json(&json!({ "nokeys": true })),
            "jwks",
            "malformed_document",
        ),
        (
            "/realm/jwks",
            Reply::status(404),
            "jwks",
            "unexpected_status",
        ),
        (
            "/realm/.well-known/openid-configuration",
            Reply::status(404),
            "discovery",
            "unexpected_status",
        ),
        (
            "/realm/.well-known/openid-configuration",
            Reply::raw(b"<html>login</html>".to_vec()),
            "discovery",
            "malformed_document",
        ),
    ] {
        idp.serve_healthy_oidc();
        idp.set(path, reply);
        let failed = stack.test_provider(&admin, &id).await;
        assert_code(
            &failed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "PROVIDER_VALIDATION_FAILED",
        );
        assert!(
            failing(&failed).contains(&(name.to_owned(), detail.to_owned())),
            "{name}/{detail}: {}",
            failed.text()
        );
        assert_eq!(stack.validated_at(&id).await, None);
    }

    idp.serve_healthy_oidc();
    stack.tested_ok(&admin, &id).await;
    assert!(stack.validated_at(&id).await.is_some());
    assert_eq!(stack.validation_error(&id).await, None);

    let reachable_but_unauthenticated = [400_u16, 401, 403, 404, 405];
    for status in reachable_but_unauthenticated {
        idp.set("/realm/token", Reply::status(status));
        assert_eq!(
            stack.test_provider(&admin, &id).await.status,
            StatusCode::OK,
            "a token endpoint answering {status} is reachable"
        );
    }
    drop(idp);
    let unreachable = stack.test_provider(&admin, &id).await;
    assert_code(
        &unreachable,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    assert!(failing(&unreachable)
        .iter()
        .all(|(_, detail)| detail == "unreachable"));
    stack.stop().await;
}

impl Stack {
    async fn pools_text_contains(&self, needle: &str) -> bool {
        let found: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM identity_providers
              WHERE COALESCE(validation_error, '') LIKE '%' || ?1 || '%'",
        )
        .bind(needle)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        found > 0
    }
}

#[tokio::test]
async fn it_provider_oauth2_test_checks_only_what_the_provider_configures() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    idp.set("/oauth/authorize", Reply::status(200));
    idp.set("/oauth/token", Reply::status(400));
    idp.set("/oauth/userinfo", Reply::status(401));
    let id = stack
        .created(&admin, &oauth2_body(&idp, "custom-oauth"))
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let result = stack.tested_ok(&admin, &id).await;
    assert_eq!(
        check_names(&result),
        [
            "authorization_endpoint",
            "token_endpoint",
            "userinfo_endpoint",
            "redirect_uri"
        ]
    );
    assert_eq!(
        idp.requests_to("/realm/.well-known/openid-configuration"),
        0
    );
    assert_eq!(idp.requests_to("/realm/jwks"), 0);
    assert!(stack.validated_at(&id).await.is_some());

    idp.set("/oauth/userinfo", Reply::status(502));
    let failed = stack.test_provider(&admin, &id).await;
    assert_code(
        &failed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    assert_eq!(
        failing(&failed),
        [("userinfo_endpoint".to_owned(), "upstream_error".to_owned())]
    );
    assert_eq!(stack.validated_at(&id).await, None);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_patch_clears_validation_only_for_connection_critical_changes() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let base = idp.base();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    stack.tested_ok(&admin, &id).await;
    let stamp = stack.validated_at(&id).await.unwrap();
    for body in [
        json!({ "displayName": "Renamed" }),
        json!({ "scopes": ["openid", "email"] }),
        json!({ "claimMapping": { "email": "mail" } }),
        json!({ "autoProvision": true }),
        json!({ "allowEmailLinking": false }),
        json!({ "enabled": true }),
        json!({ "sortOrder": 9 }),
        json!({ "preset": "authentik" }),
        json!({ "displayName": "Renamed", "clientId": "client-authentik" }),
    ] {
        let patched = stack.patched(&admin, &id, &body).await;
        assert_eq!(patched["validatedAt"], stamp.as_str(), "{body}");
        assert_eq!(
            stack.validated_at(&id).await.as_deref(),
            Some(stamp.as_str()),
            "{body}"
        );
    }

    idp.set(
        "/realm2/.well-known/openid-configuration",
        Reply::json(&{
            let mut document = idp.discovery_document();
            document["issuer"] = json!(format!("{base}/realm2"));
            document["token_endpoint"] = json!(format!("{base}/realm2/token"));
            document["authorization_endpoint"] = json!(format!("{base}/realm2/authorize"));
            document["jwks_uri"] = json!(format!("{base}/realm2/jwks"));
            document
        }),
    );
    let critical = [
        json!({ "clientId": "another-client" }),
        json!({ "clientSecret": REPLACEMENT }),
        json!({ "endpoints": { "token": format!("{base}/realm/token2") } }),
        json!({ "tokenAuthMethod": "client_secret_basic" }),
        json!({ "issuerUrl": format!("{base}/realm2") }),
    ];
    for body in critical {
        stack.tested_ok(&admin, &id).await;
        assert!(stack.validated_at(&id).await.is_some());
        let patched = stack.patched(&admin, &id, &body).await;
        assert_eq!(patched["validatedAt"], Value::Null, "{body}");
        assert_eq!(stack.validated_at(&id).await, None, "{body}");
        assert_eq!(stack.validation_error(&id).await, None, "{body}");
        idp.set("/realm/token2", Reply::status(400));
        idp.set(
            "/realm2/jwks",
            Reply::json(&json!({ "keys": [{ "kty": "RSA" }] })),
        );
        idp.set("/realm2/token", Reply::status(400));
    }
    let issuer_after = stack.provider_scalar(&id, "issuer").await;
    assert_eq!(
        issuer_after.as_deref(),
        Some(format!("{base}/realm2").as_str())
    );
    assert_eq!(
        stack
            .provider_scalar(&id, "token_endpoint")
            .await
            .as_deref(),
        Some(format!("{base}/realm2/token").as_str()),
        "an issuer change must discard the cached endpoints and rediscover"
    );
    stack.stop().await;

    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    idp.serve_healthy_oidc();
    idp.set("/oauth/authorize", Reply::status(200));
    idp.set("/oauth/token", Reply::status(400));
    idp.set("/oauth/userinfo", Reply::status(401));
    let id = stack.created(&admin, &oidc_body(&idp, "switch")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack.tested_ok(&admin, &id).await;
    let switched = stack
        .patched(
            &admin,
            &id,
            &json!({
                "protocol": "oauth2", "issuerUrl": null,
                "endpoints": {
                    "authorization": format!("{base}/oauth/authorize"),
                    "token": format!("{base}/oauth/token"),
                    "userinfo": format!("{base}/oauth/userinfo"),
                },
            }),
        )
        .await;
    assert_eq!(switched["protocol"], "oauth2");
    assert_eq!(switched["validatedAt"], Value::Null);
    assert_eq!(switched["issuerUrl"], Value::Null);
    assert_eq!(switched["endpoints"]["jwks"], Value::Null);
    let incomplete = stack
        .patch_provider(&admin, &id, &json!({ "protocol": "oidc" }))
        .await;
    assert_code(
        &incomplete,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        stack.provider_scalar(&id, "kind").await.as_deref(),
        Some("oauth2")
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_does_not_stamp_a_provider_that_changed_while_it_ran() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    idp.set(
        "/realm/jwks",
        Reply::json(&json!({ "keys": [{ "kty": "RSA" }] })).delayed(Duration::from_millis(1500)),
    );

    let provider = id.parse::<ProviderId>().unwrap();
    let principal = stack.principal(&admin.session).await;
    let client = ClientMetadata::none();
    let running = stack.providers.test(&principal, provider, &client);
    let changing = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        stack
            .patch_provider(&admin, &id, &json!({ "displayName": "Changed meanwhile" }))
            .await
    };
    let (result, patched) = tokio::join!(running, changing);

    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let error = result.unwrap_err();
    assert_eq!(
        error.api_error().code(),
        crate::domain::error_code::ErrorCode::DatabaseBusy
    );
    assert_eq!(
        stack.validated_at(&id).await,
        None,
        "a stale test must not stamp the new configuration"
    );

    let stale = stack
        .pools
        .write_tx(&stack.clock, "test.stale_stamp", async |tx| {
            repo::stamp_validation(
                tx,
                provider,
                Timestamp::try_from(START).unwrap(),
                Some(Timestamp::try_from(START).unwrap()),
                None,
            )
            .await
        })
        .await
        .unwrap();
    assert!(!stale);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_discover_previews_without_persisting_or_mutating() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let existing = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let id = existing["id"].as_str().unwrap().to_owned();
    let (rows, audit) = (stack.provider_count().await, stack.provider_audit().await);
    let before = idp.requests().len();

    let preview = stack
        .providers_call(
            Method::POST,
            "/discover",
            &admin,
            Some(&json!({ "issuerUrl": idp.issuer() })),
        )
        .await;

    assert_eq!(preview.status, StatusCode::OK, "{}", preview.text());
    let base = idp.base();
    assert_eq!(
        preview.json(),
        json!({
            "issuerUrl": idp.issuer(),
            "endpoints": {
                "authorization": format!("{base}/realm/authorize"),
                "token": format!("{base}/realm/token"),
                "userinfo": format!("{base}/realm/userinfo"),
                "jwks": format!("{base}/realm/jwks"),
            },
            "scopesSupported": ["openid", "email", "profile"],
            "tokenEndpointAuthMethodsSupported": ["client_secret_basic", "client_secret_post"],
        })
    );
    assert_eq!(
        preview
            .headers
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap(),
        "no-store"
    );
    let fetched: Vec<_> = idp.requests().into_iter().skip(before).collect();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].path, "/realm/.well-known/openid-configuration");
    assert_eq!(fetched[0].method, "GET");
    for forbidden in ["authorization", "cookie", "x-palmr-csrf"] {
        assert_eq!(fetched[0].header(forbidden), None, "{forbidden}");
    }
    assert_eq!(
        (stack.provider_count().await, stack.provider_audit().await),
        (rows, audit)
    );
    assert_eq!(
        stack.provider_scalar(&id, "updated_at").await.unwrap(),
        existing["updatedAt"].as_str().unwrap()
    );

    let direct = stack.providers.discover(&idp.issuer()).await.unwrap();
    assert_eq!(direct.issuer_url, idp.issuer());

    for (body, expected) in [
        (json!({}), vec!["issuerUrl"]),
        (json!({ "issuerUrl": "" }), vec!["issuerUrl"]),
        (
            json!({ "issuerUrl": "http://sso.example.test/" }),
            vec!["issuerUrl"],
        ),
        (
            json!({ "issuerUrl": "https://u:p@sso.example.test/" }),
            vec!["issuerUrl"],
        ),
        (
            json!({ "issuerUrl": "https://sso.example.test/?a=b" }),
            vec!["issuerUrl"],
        ),
        (json!({ "issuerUrl": 7 }), vec!["issuerUrl"]),
        (
            json!({ "issuerUrl": idp.issuer(), "extra": 1 }),
            vec!["body"],
        ),
    ] {
        let rejected = stack
            .providers_call(Method::POST, "/discover", &admin, Some(&body))
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_detail(&rejected, "fields", &json!(expected));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_discover_failures_are_classified_and_bounded() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    let well_known = "/realm/.well-known/openid-configuration";
    let issuer = json!({ "issuerUrl": idp.issuer() });

    let mut incomplete = idp.discovery_document();
    incomplete.as_object_mut().unwrap().remove("jwks_uri");
    let mut scheme = idp.discovery_document();
    scheme["token_endpoint"] = json!("http://sso.example.test/token");
    for (reply, reason) in [
        (Reply::status(404), "unexpected_status"),
        (Reply::status(500), "upstream_error"),
        (
            Reply::raw(b"<html>sign in</html>".to_vec()),
            "malformed_document",
        ),
        (Reply::raw(vec![b' '; 256 * 1024 + 1]), "response_too_large"),
        (
            Reply::raw(vec![b' '; 300 * 1024]).unsized_body(),
            "response_too_large",
        ),
        (Reply::json(&incomplete), "incomplete_document"),
        (Reply::json(&scheme), "malformed_document"),
        (
            Reply::redirect("https://169.254.169.254/latest/meta-data/"),
            "blocked_address",
        ),
        (
            Reply::redirect(&format!("{}/elsewhere", FakeIdp::start().await.base())),
            "blocked_address",
        ),
    ] {
        idp.set(well_known, reply);
        let failed = stack
            .providers_call(Method::POST, "/discover", &admin, Some(&issuer))
            .await;
        assert_code(
            &failed,
            StatusCode::BAD_GATEWAY,
            "PROVIDER_DISCOVERY_FAILED",
        );
        assert_detail(&failed, "reason", &json!(reason));
        assert!(!failed.text().contains("sign in"));
    }
    assert_eq!(idp.requests_to("/elsewhere"), 0);

    idp.set(well_known, Reply::raw(vec![b' '; 256 * 1024]));
    let at_cap = stack
        .providers_call(Method::POST, "/discover", &admin, Some(&issuer))
        .await;
    assert_detail(&at_cap, "reason", &json!("malformed_document"));
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_discover_and_test_share_the_provider_test_rate_limit() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let issuer = json!({ "issuerUrl": idp.issuer() });

    for attempt in 0..15 {
        let discover = stack
            .providers_call(Method::POST, "/discover", &admin, Some(&issuer))
            .await;
        assert_eq!(
            discover.status,
            StatusCode::OK,
            "discover {attempt}: {}",
            discover.text()
        );
        let test = stack.test_provider(&admin, &id).await;
        assert_eq!(
            test.status,
            StatusCode::OK,
            "test {attempt}: {}",
            test.text()
        );
    }
    let limited = stack
        .providers_call(Method::POST, "/discover", &admin, Some(&issuer))
        .await;
    assert_code(&limited, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    assert!(limited.headers.contains_key("retry-after"));
    let limited_test = stack.test_provider(&admin, &id).await;
    assert_code(&limited_test, StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED");
    let list = stack.providers_call(Method::GET, "", &admin, None).await;
    assert_eq!(list.status, StatusCode::OK, "reads use their own class");
    stack.stop().await;
}

async fn three_providers(stack: &Stack, admin: &Credentials, idp: &FakeIdp) -> [String; 3] {
    let mut ids = Vec::new();
    for slug in ["alpha", "bravo", "charlie"] {
        ids.push(
            stack.created(admin, &oidc_body(idp, slug)).await["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    ids.try_into().unwrap()
}

async fn order_of(stack: &Stack, admin: &Credentials) -> Vec<String> {
    stack.list_providers(admin).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["slug"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn it_provider_order_is_atomic_validated_and_exact() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let [a, b, c] = three_providers(&stack, &admin, &idp).await;
    assert_eq!(
        order_of(&stack, &admin).await,
        ["alpha", "bravo", "charlie"]
    );
    let updated_before = stack.provider_scalar(&a, "updated_at").await;

    let reordered = stack
        .providers_call(
            Method::PUT,
            "/order",
            &admin,
            Some(&json!({ "order": [c, a, b] })),
        )
        .await;
    assert_eq!(
        reordered.status,
        StatusCode::NO_CONTENT,
        "{}",
        reordered.text()
    );
    assert!(reordered.body.is_empty());
    assert_eq!(
        order_of(&stack, &admin).await,
        ["charlie", "alpha", "bravo"]
    );
    let items = stack.list_providers(&admin).await;
    let orders: Vec<i64> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["sortOrder"].as_i64().unwrap())
        .collect();
    assert_eq!(orders, [0, 1, 2]);
    assert_eq!(
        stack.provider_scalar(&a, "updated_at").await,
        updated_before
    );

    let unknown = "0192f3a1-0000-7000-8000-00000000ffff";
    for body in [
        json!({ "order": [c, a, unknown] }),
        json!({ "order": [c, a, a] }),
        json!({ "order": [c, a, "nope"] }),
        json!({ "order": [c, a] }),
        json!({ "order": [c, a, b, unknown] }),
        json!({ "order": [] }),
        json!({ "order": [c.to_uppercase(), a, b] }),
        json!({ "order": "not-a-list" }),
        json!({}),
        json!({ "order": [c, a, b], "extra": true }),
        json!([c, a, b]),
    ] {
        let rejected = stack
            .providers_call(Method::PUT, "/order", &admin, Some(&body))
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            order_of(&stack, &admin).await,
            ["charlie", "alpha", "bravo"],
            "{body}"
        );
    }

    stack
        .execute(&format!(
            "CREATE TRIGGER refuse_third_move BEFORE UPDATE OF sort_order ON identity_providers
               WHEN NEW.id = '{b}' BEGIN SELECT RAISE(ABORT, 'refused'); END"
        ))
        .await;
    let failed = stack
        .providers_call(
            Method::PUT,
            "/order",
            &admin,
            Some(&json!({ "order": [b, c, a] })),
        )
        .await;
    assert_ne!(failed.status, StatusCode::NO_CONTENT);
    assert_eq!(
        order_of(&stack, &admin).await,
        ["charlie", "alpha", "bravo"],
        "a failure part-way must roll back every move"
    );
    stack.stop().await;

    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let empty = stack
        .providers_call(Method::PUT, "/order", &admin, Some(&json!({ "order": [] })))
        .await;
    assert_eq!(empty.status, StatusCode::NO_CONTENT);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_list_is_paginated_deterministic_and_counted() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let mut created = Vec::new();
    for slug in ["delta", "echo", "foxtrot"] {
        let mut body = oidc_body(&idp, slug);
        body["sortOrder"] = json!(5);
        created.push(
            stack.created(&admin, &body).await["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let mut sorted = created.clone();
    sorted.sort();

    let first = stack
        .providers_call(Method::GET, "?limit=2", &admin, None)
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    let first = first.json();
    assert_eq!(first["totalCount"], 3);
    let page_one: Vec<&str> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(page_one, &sorted[..2], "ties on sortOrder are broken by id");
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = stack
        .providers_call(
            Method::GET,
            &format!("?limit=2&cursor={cursor}"),
            &admin,
            None,
        )
        .await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.text());
    assert_eq!(second.json()["items"].as_array().unwrap().len(), 1);
    assert_eq!(second.json()["items"][0]["id"], sorted[2].as_str());
    assert_eq!(second.json()["nextCursor"], Value::Null);
    assert_eq!(second.json()["totalCount"], 3);

    let tampered = stack
        .providers_call(Method::GET, &format!("?cursor={cursor}x"), &admin, None)
        .await;
    assert_code(&tampered, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let bad_sort = stack
        .providers_call(Method::GET, "?sort=slug", &admin, None)
        .await;
    assert_eq!(
        bad_sort.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        bad_sort.text()
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_mutations_write_allowlisted_audit_in_the_same_transaction() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, admin_id) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    let created = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let rows = stack.provider_audit().await;
    assert_eq!(actions(&rows), ["IDENTITY_PROVIDER_CREATED"]);
    assert_eq!(
        (
            rows[0].1.as_str(),
            rows[0].2.as_str(),
            rows[0].3.as_str(),
            rows[0].4.as_str()
        ),
        ("provider", id.as_str(), "authentik", "success")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&rows[0].5).unwrap(),
        json!({
            "protocol": "oidc", "preset": "generic", "enabled": false, "auto_provision": false,
            "allow_email_linking": true, "client_secret": "set",
        })
    );

    stack
        .patched(&admin, &id, &json!({ "displayName": "Renamed" }))
        .await;
    stack
        .patched(&admin, &id, &json!({ "enabled": true }))
        .await;
    stack
        .patched(
            &admin,
            &id,
            &json!({ "enabled": false, "autoProvision": true }),
        )
        .await;
    stack
        .patched(&admin, &id, &json!({ "clientSecret": REPLACEMENT }))
        .await;
    stack
        .patched(
            &admin,
            &id,
            &json!({ "displayName": "Renamed", "enabled": false }),
        )
        .await;
    let taken = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_eq!(taken.status, StatusCode::CONFLICT);
    let invalid = stack
        .patch_provider(&admin, &id, &json!({ "clientId": "" }))
        .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    let removed = stack
        .providers_call(Method::DELETE, &format!("/{id}"), &admin, None)
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);

    let rows = stack.provider_audit().await;
    assert_eq!(
        actions(&rows),
        [
            "IDENTITY_PROVIDER_CREATED",
            "IDENTITY_PROVIDER_UPDATED",
            "IDENTITY_PROVIDER_ENABLED",
            "IDENTITY_PROVIDER_UPDATED",
            "IDENTITY_PROVIDER_DISABLED",
            "IDENTITY_PROVIDER_UPDATED",
            "IDENTITY_PROVIDER_DELETED",
        ]
    );
    let metadata = |index: usize| serde_json::from_str::<Value>(&rows[index].5).unwrap();
    assert_eq!(
        metadata(1),
        json!({ "changed": ["display_name"], "validation_reset": false })
    );
    assert_eq!(metadata(2), json!({}));
    assert_eq!(
        metadata(3),
        json!({ "changed": ["auto_provision"], "validation_reset": false })
    );
    assert_eq!(metadata(4), json!({}));
    assert_eq!(
        metadata(5),
        json!({ "changed": [], "validation_reset": true, "client_secret": { "from": "set", "to": "set" } })
    );
    assert_eq!(
        metadata(6),
        json!({ "protocol": "oidc", "preset": "generic" })
    );
    for row in &rows {
        assert_eq!(
            (
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str()
            ),
            ("provider", id.as_str(), "authentik", "success")
        );
    }
    let text = rows
        .iter()
        .map(|row| format!("{row:?}"))
        .collect::<String>();
    for needle in [SECRET, REPLACEMENT, "ciphertext", "nonce"] {
        assert!(!text.contains(needle), "{needle}");
    }
    assert!(stack
        .audit_with_actor()
        .await
        .iter()
        .all(|(_, actor)| actor.as_deref() == Some(admin_id.as_str())));
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_audit_failure_rolls_the_mutation_back() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    stack
        .execute(
            "CREATE TRIGGER refuse_create_audit BEFORE INSERT ON audit_events
               WHEN NEW.action = 'IDENTITY_PROVIDER_CREATED' BEGIN SELECT RAISE(ABORT, 'refused'); END",
        )
        .await;
    let failed = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_ne!(failed.status, StatusCode::CREATED, "{}", failed.text());
    assert_eq!(
        stack.provider_count().await,
        0,
        "a provider must not exist without its audit record"
    );
    stack.execute("DROP TRIGGER refuse_create_audit").await;

    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack
        .execute(
            "CREATE TRIGGER refuse_update_audit BEFORE INSERT ON audit_events
               WHEN NEW.action IN ('IDENTITY_PROVIDER_UPDATED', 'IDENTITY_PROVIDER_DELETED')
               BEGIN SELECT RAISE(ABORT, 'refused'); END",
        )
        .await;
    let update = stack
        .patch_provider(&admin, &id, &json!({ "displayName": "Changed" }))
        .await;
    assert_ne!(update.status, StatusCode::OK, "{}", update.text());
    assert_eq!(
        stack.provider_scalar(&id, "display_name").await.as_deref(),
        Some("Provider authentik")
    );
    let delete = stack
        .providers_call(Method::DELETE, &format!("/{id}"), &admin, None)
        .await;
    assert_ne!(delete.status, StatusCode::NO_CONTENT, "{}", delete.text());
    assert_eq!(stack.provider_count().await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_errors_use_catalogue_codes_and_keep_the_request_id() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let unknown = "0192f3a1-0000-7000-8000-00000000ffff";

    for (method, suffix, body) in [
        (
            Method::PATCH,
            format!("/{unknown}"),
            Some(json!({ "displayName": "x" })),
        ),
        (
            Method::PATCH,
            "/not-an-id".to_owned(),
            Some(json!({ "displayName": "x" })),
        ),
        (Method::DELETE, format!("/{unknown}"), None),
        (Method::DELETE, "/not-an-id".to_owned(), None),
        (Method::POST, format!("/{unknown}/test"), None),
        (Method::POST, "/not-an-id/test".to_owned(), None),
    ] {
        let missing = stack
            .providers_call(method.clone(), &suffix, &admin, body.as_ref())
            .await;
        assert_code(&missing, StatusCode::NOT_FOUND, "PROVIDER_NOT_FOUND");
        let error = &missing.json()["error"];
        assert_eq!(
            error["requestId"].as_str().unwrap(),
            missing
                .headers
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "{method} {suffix}"
        );
        assert!(!error["requestId"].as_str().unwrap().is_empty());
    }

    let empty = stack.patch_provider(&admin, &id, &json!({})).await;
    assert_code(&empty, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_detail(&empty, "fields", &json!(["body"]));
    let nulls = stack
        .patch_provider(&admin, &id, &json!({ "displayName": null }))
        .await;
    assert_code(&nulls, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_detail(&nulls, "fields", &json!(["displayName"]));

    stack.clock.advance(Duration::from_secs(1));
    let path = PROVIDERS.to_owned();
    let broken = stack
        .call(Call::new(Method::POST, &path, &admin).raw("{\"slug\":"), 10)
        .await;
    assert_code(&broken, StatusCode::BAD_REQUEST, "INVALID_JSON");
    let mut wrong_type = Call::new(Method::POST, &path, &admin).json(&oidc_body(&idp, "other"));
    wrong_type.content_type = Some("text/plain");
    let wrong_type = stack.call(wrong_type, 10).await;
    assert_code(
        &wrong_type,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );
    for body in [json!([]), json!("text"), json!(7)] {
        let shape = stack.create_provider(&admin, &body).await;
        assert_code(&shape, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    assert_eq!(stack.provider_count().await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_update_that_races_a_concurrent_change_is_retryable() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let principal = stack.principal(&admin.session).await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let base = idp.base();
    let mut moved = idp.discovery_document();
    moved["issuer"] = json!(format!("{base}/realm2"));
    moved["authorization_endpoint"] = json!(format!("{base}/realm2/authorize"));
    moved["token_endpoint"] = json!(format!("{base}/realm2/token"));
    moved["jwks_uri"] = json!(format!("{base}/realm2/jwks"));
    idp.set(
        "/realm2/.well-known/openid-configuration",
        Reply::json(&moved).delayed(Duration::from_millis(1500)),
    );

    let input = UpdateInput::parse(
        parse::<UpdateProviderRequest>(json!({ "issuerUrl": format!("{base}/realm2") })).unwrap(),
    )
    .unwrap();
    let provider = id.parse::<ProviderId>().unwrap();
    let client = ClientMetadata::none();
    let running = stack.providers.update(&principal, provider, input, &client);
    let changing = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        stack
            .patch_provider(&admin, &id, &json!({ "displayName": "Changed meanwhile" }))
            .await
    };
    let (result, patched) = tokio::join!(running, changing);

    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    assert_eq!(
        result.unwrap_err().api_error().code(),
        crate::domain::error_code::ErrorCode::DatabaseBusy
    );
    assert_eq!(
        stack.provider_scalar(&id, "issuer").await.as_deref(),
        Some(idp.issuer().as_str())
    );
    assert_eq!(
        stack.provider_scalar(&id, "display_name").await.as_deref(),
        Some("Changed meanwhile")
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_created_enabled_is_one_created_event() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let mut body = oidc_body(&idp, "authentik");
    body["enabled"] = json!(true);

    let created = stack.created(&admin, &body).await;

    assert_eq!(created["enabled"], true);
    let rows = stack.provider_audit().await;
    assert_eq!(actions(&rows), ["IDENTITY_PROVIDER_CREATED"]);
    assert_eq!(
        serde_json::from_str::<Value>(&rows[0].5).unwrap()["enabled"],
        true
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_invalid_or_duplicate_requests_never_reach_the_network() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let before = idp.requests().len();

    let mut no_secret = oidc_body(&idp, "second");
    no_secret.as_object_mut().unwrap().remove("clientSecret");
    let mut no_openid = oidc_body(&idp, "third");
    no_openid["scopes"] = json!(["email"]);
    for (body, fields) in [
        (no_secret, json!(["clientSecret"])),
        (no_openid, json!(["scopes"])),
    ] {
        let rejected = stack.create_provider(&admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_detail(&rejected, "fields", &fields);
    }
    let duplicate = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_code(&duplicate, StatusCode::CONFLICT, "PROVIDER_SLUG_TAKEN");
    assert_eq!(
        idp.requests().len(),
        before,
        "a rejected request must not trigger discovery"
    );

    let id = stack.list_providers(&admin).await["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected = stack
        .patch_provider(
            &admin,
            &id,
            &json!({ "issuerUrl": format!("{}/elsewhere", idp.base()), "clientSecret": null }),
        )
        .await;
    assert_code(
        &rejected,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(idp.requests().len(), before);
    stack.stop().await;
}

fn field_sets(rows: &[ProviderAudit]) -> Vec<Value> {
    rows.iter()
        .filter(|row| row.0 == "IDENTITY_PROVIDER_UPDATED")
        .map(|row| serde_json::from_str::<Value>(&row.5).unwrap())
        .collect()
}

#[tokio::test]
async fn it_provider_order_audits_each_moved_provider_in_the_same_transaction() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, admin_id) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let [a, b, c] = three_providers(&stack, &admin, &idp).await;
    let created = stack.provider_audit().await.len();

    let moved = stack
        .providers_call(
            Method::PUT,
            "/order",
            &admin,
            Some(&json!({ "order": [a, c, b] })),
        )
        .await;
    assert_eq!(moved.status, StatusCode::NO_CONTENT, "{}", moved.text());
    let rows = stack.provider_audit().await;
    let new_rows = &rows[created..];
    assert_eq!(
        actions(new_rows),
        ["IDENTITY_PROVIDER_UPDATED", "IDENTITY_PROVIDER_UPDATED"]
    );
    let mut targets: Vec<(&str, &str)> = new_rows
        .iter()
        .map(|row| (row.2.as_str(), row.3.as_str()))
        .collect();
    targets.sort_unstable();
    let mut expected = vec![(c.as_str(), "charlie"), (b.as_str(), "bravo")];
    expected.sort_unstable();
    assert_eq!(targets, expected);
    for row in new_rows {
        assert_eq!(row.1, "provider");
        assert_eq!(
            serde_json::from_str::<Value>(&row.5).unwrap(),
            json!({ "fields": ["sortOrder"] })
        );
    }
    assert!(stack
        .audit_with_actor()
        .await
        .iter()
        .all(|(_, actor)| actor.as_deref() == Some(admin_id.as_str())));

    let same = stack
        .providers_call(
            Method::PUT,
            "/order",
            &admin,
            Some(&json!({ "order": [a, c, b] })),
        )
        .await;
    assert_eq!(same.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.provider_audit().await.len(),
        rows.len(),
        "a no-op reorder writes no audit rows"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_order_audit_failure_rolls_the_order_back() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let [a, b, c] = three_providers(&stack, &admin, &idp).await;
    let before = stack.provider_audit().await.len();
    stack
        .execute(&format!(
            "CREATE TRIGGER refuse_order_audit BEFORE INSERT ON audit_events
               WHEN NEW.action = 'IDENTITY_PROVIDER_UPDATED' AND NEW.target_id = '{b}'
               BEGIN SELECT RAISE(ABORT, 'refused'); END"
        ))
        .await;

    let failed = stack
        .providers_call(
            Method::PUT,
            "/order",
            &admin,
            Some(&json!({ "order": [b, a, c] })),
        )
        .await;

    assert_ne!(failed.status, StatusCode::NO_CONTENT, "{}", failed.text());
    assert_eq!(
        order_of(&stack, &admin).await,
        ["alpha", "bravo", "charlie"]
    );
    assert_eq!(stack.provider_audit().await.len(), before);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_audits_validation_state_changes_only() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = stack.provider_audit().await.len();

    stack.tested_ok(&admin, &id).await;
    let rows = stack.provider_audit().await;
    assert_eq!(
        field_sets(&rows[created..]),
        [json!({ "fields": ["validatedAt"] })]
    );
    assert_eq!(
        (
            rows[created].1.as_str(),
            rows[created].2.as_str(),
            rows[created].3.as_str()
        ),
        ("provider", id.as_str(), "authentik")
    );

    stack.tested_ok(&admin, &id).await;
    let rows = stack.provider_audit().await;
    assert_eq!(
        field_sets(&rows[created..]).len(),
        2,
        "a later success moves validatedAt and is a persisted change"
    );

    idp.set("/realm/jwks", Reply::status(500));
    let failed = stack.test_provider(&admin, &id).await;
    assert_code(
        &failed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    let rows = stack.provider_audit().await;
    assert_eq!(
        field_sets(&rows[created..]).last().unwrap(),
        &json!({ "fields": ["validatedAt", "validationError"] })
    );

    let after_failure = stack.provider_audit().await.len();
    let repeated = stack.test_provider(&admin, &id).await;
    assert_code(
        &repeated,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    assert_eq!(
        stack.provider_audit().await.len(),
        after_failure,
        "a repeated failure with the same persisted state writes no audit row"
    );

    idp.serve_healthy_oidc();
    stack.tested_ok(&admin, &id).await;
    let rows = stack.provider_audit().await;
    assert_eq!(
        field_sets(&rows[created..]).last().unwrap(),
        &json!({ "fields": ["validatedAt", "validationError"] })
    );
    let text = rows
        .iter()
        .map(|row| format!("{row:?}"))
        .collect::<String>();
    for needle in [SECRET, "upstream_error", "jwks:"] {
        assert!(!text.contains(needle), "{needle}");
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_test_audit_failure_rolls_the_validation_state_back() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack
        .execute(
            "CREATE TRIGGER refuse_test_audit BEFORE INSERT ON audit_events
               WHEN NEW.action = 'IDENTITY_PROVIDER_UPDATED' BEGIN SELECT RAISE(ABORT, 'refused'); END",
        )
        .await;

    let failed = stack.test_provider(&admin, &id).await;

    assert_ne!(failed.status, StatusCode::OK, "{}", failed.text());
    assert_eq!(
        stack.validated_at(&id).await,
        None,
        "no validation may persist without its audit row"
    );
    stack.execute("DROP TRIGGER refuse_test_audit").await;
    stack.tested_ok(&admin, &id).await;
    stack
        .execute(
            "CREATE TRIGGER refuse_test_audit BEFORE INSERT ON audit_events
               WHEN NEW.action = 'IDENTITY_PROVIDER_UPDATED' BEGIN SELECT RAISE(ABORT, 'refused'); END",
        )
        .await;
    let stamp = stack.validated_at(&id).await;
    idp.set("/realm/jwks", Reply::status(500));
    let failed = stack.test_provider(&admin, &id).await;
    assert_ne!(
        failed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        failed.text()
    );
    assert_eq!(
        stack.validated_at(&id).await,
        stamp,
        "the previous validation must survive a rolled-back failure"
    );
    assert_eq!(stack.validation_error(&id).await, None);
    stack.stop().await;
}
