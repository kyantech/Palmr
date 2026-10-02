use std::io::Read;

use tracing_subscriber::filter::EnvFilter;

use super::admin_settings::assert_detail;
use super::password_reset::Capture;
use super::profile::{assert_code, Call};
use super::*;
use crate::config::LogFormat;
use crate::features::identity_providers::model::{client_secret_aad, ProviderId};
use crate::features::identity_providers::test_support::{FakeIdp, Reply};
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::SealPurpose;
use crate::infra::telemetry::build_dispatch;

pub(super) const PROVIDERS: &str = "/api/v1/admin/providers";
pub(super) const SECRET: &str = "palmr-idp-flow-sentinel-4c9a17e0";
pub(super) const REPLACEMENT: &str = "palmr-idp-flow-replacement-83be02d1";
const CALLBACK: &str = "https://files.example.test/api/v1/auth/providers";

pub(super) type ProviderAudit = (String, String, String, String, String, String);
type SecretRow = (Option<Vec<u8>>, Option<Vec<u8>>, i64);

pub(super) fn oidc_body(idp: &FakeIdp, slug: &str) -> Value {
    json!({
        "slug": slug,
        "displayName": format!("Provider {slug}"),
        "protocol": "oidc",
        "issuerUrl": idp.issuer(),
        "clientId": format!("client-{slug}"),
        "clientSecret": SECRET,
    })
}

pub(super) fn oauth2_body(idp: &FakeIdp, slug: &str) -> Value {
    let base = idp.base();
    json!({
        "slug": slug,
        "displayName": format!("OAuth {slug}"),
        "protocol": "oauth2",
        "clientId": format!("client-{slug}"),
        "clientSecret": SECRET,
        "endpoints": {
            "authorization": format!("{base}/oauth/authorize"),
            "token": format!("{base}/oauth/token"),
            "userinfo": format!("{base}/oauth/userinfo"),
        },
    })
}

impl Stack {
    pub(super) async fn providers_call(
        &self,
        method: Method,
        suffix: &str,
        creds: &Credentials,
        body: Option<&Value>,
    ) -> Fetched {
        self.clock.advance(Duration::from_secs(1));
        let path = format!("{PROVIDERS}{suffix}");
        let mut call = Call::new(method, &path, creds);
        if let Some(body) = body {
            call = call.json(body);
        }
        self.call(call, 10).await
    }

    pub(super) async fn create_provider(&self, creds: &Credentials, body: &Value) -> Fetched {
        self.providers_call(Method::POST, "", creds, Some(body))
            .await
    }

    pub(super) async fn created(&self, creds: &Credentials, body: &Value) -> Value {
        let fetched = self.create_provider(creds, body).await;
        assert_eq!(fetched.status, StatusCode::CREATED, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn patch_provider(
        &self,
        creds: &Credentials,
        id: &str,
        body: &Value,
    ) -> Fetched {
        self.providers_call(Method::PATCH, &format!("/{id}"), creds, Some(body))
            .await
    }

    pub(super) async fn patched(&self, creds: &Credentials, id: &str, body: &Value) -> Value {
        let fetched = self.patch_provider(creds, id, body).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn list_providers(&self, creds: &Credentials) -> Value {
        let fetched = self.providers_call(Method::GET, "", creds, None).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn provider_secret_row(&self, id: &str) -> SecretRow {
        sqlx::query_as(
            "SELECT client_secret_ciphertext, client_secret_nonce, key_version
               FROM identity_providers WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn provider_scalar(&self, id: &str, column: &str) -> Option<String> {
        sqlx::query_scalar(&format!(
            "SELECT CAST({column} AS TEXT) FROM identity_providers WHERE id = ?1"
        ))
        .bind(id)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn provider_count(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM identity_providers")
            .await
    }

    pub(super) async fn provider_audit(&self) -> Vec<ProviderAudit> {
        sqlx::query_as(
            "SELECT action, COALESCE(target_type, ''), COALESCE(target_id, ''),
                    COALESCE(target_label, ''), result, COALESCE(metadata_json, '')
               FROM audit_events WHERE action LIKE 'IDENTITY_PROVIDER_%' ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn link_user_to(&self, provider_id: &str, username: &str) -> String {
        let hash = password_hash();
        let user = self
            .user(UserSpec::local(
                username,
                &format!("{username}@example.test"),
                &hash,
            ))
            .await;
        let link = format!(
            "0192f3a1-0000-7000-8000-0000000{:05}",
            user.to_string().len()
        );
        self.execute(&format!(
            "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method, created_at)
             VALUES ('{link}-{username}', '{user}', '{provider_id}', 'subject-{username}', 'manual',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .await;
        user.to_string()
    }
}

pub(super) fn database_files_containing(root: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for name in ["palmr.db", "palmr.db-wal", "palmr.db-shm"] {
        let Ok(file) = std::fs::File::open(root.join(name)) else {
            continue;
        };
        let mut bytes = Vec::new();
        file.take(1 << 30).read_to_end(&mut bytes).unwrap();
        if bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
        {
            found.push(name.to_owned());
        }
    }
    found
}

fn audit_text(rows: &[ProviderAudit]) -> String {
    rows.iter()
        .map(|row| format!("{row:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn it_provider_secret_never_returned() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let capture = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("trace"),
        LogFormat::Json,
        capture.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let mut responses: Vec<(String, Fetched)> = Vec::new();
    let created = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let id = created.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(created.json()["clientSecretConfigured"], true);
    for forbidden in [
        "clientSecret",
        "clientSecretCiphertext",
        "clientSecretNonce",
        "keyVersion",
        "nonce",
    ] {
        assert!(created.json().get(forbidden).is_none(), "{forbidden}");
    }
    responses.push(("create".to_owned(), created));

    let listed = stack.providers_call(Method::GET, "", &admin, None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    assert_eq!(listed.json()["items"][0]["clientSecretConfigured"], true);
    responses.push(("list".to_owned(), listed));

    let patched = stack
        .patch_provider(&admin, &id, &json!({ "clientSecret": REPLACEMENT }))
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    responses.push(("patch".to_owned(), patched));
    let renamed = stack
        .patch_provider(&admin, &id, &json!({ "displayName": "Renamed" }))
        .await;
    responses.push(("rename".to_owned(), renamed));

    let tested = stack
        .providers_call(Method::POST, &format!("/{id}/test"), &admin, None)
        .await;
    assert_eq!(tested.status, StatusCode::OK, "{}", tested.text());
    responses.push(("test".to_owned(), tested));

    for (name, body) in [
        (
            "invalid-create",
            json!({ "slug": "Bad Slug", "clientSecret": SECRET }),
        ),
        ("unknown-member", {
            let mut body = oidc_body(&idp, "other");
            body["bogus"] = json!(SECRET);
            body
        }),
        ("secret-on-public-client", {
            let mut body = oidc_body(&idp, "public");
            body["tokenAuthMethod"] = json!("none");
            body
        }),
    ] {
        let rejected = stack.create_provider(&admin, &body).await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}: {}",
            rejected.text()
        );
        responses.push((name.to_owned(), rejected));
    }
    let taken = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_code(&taken, StatusCode::CONFLICT, "PROVIDER_SLUG_TAKEN");
    responses.push(("slug-taken".to_owned(), taken));
    let invalid_patch = stack
        .patch_provider(&admin, &id, &json!({ "clientSecret": "", "bogus": SECRET }))
        .await;
    assert_eq!(invalid_patch.status, StatusCode::UNPROCESSABLE_ENTITY);
    responses.push(("invalid-patch".to_owned(), invalid_patch));

    let document = serde_json::to_string(
        &crate::app::router::application_routes()
            .build()
            .unwrap()
            .openapi,
    )
    .unwrap();
    assert!(document.contains("clientSecretConfigured"));
    assert!(!document.contains(SECRET) && !document.contains(REPLACEMENT));

    for (name, fetched) in &responses {
        let text = fetched.text();
        for needle in [SECRET, REPLACEMENT] {
            assert!(
                !text.contains(needle),
                "{name} echoed a client secret: {text}"
            );
        }
    }
    let row = stack.provider_secret_row(&id).await;
    let ciphertext = row.0.unwrap();
    let encoded: String = ciphertext
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for (name, fetched) in &responses {
        assert!(
            !fetched.text().contains(&encoded),
            "{name} returned the ciphertext"
        );
    }

    let audit = audit_text(&stack.provider_audit().await);
    assert!(!audit.is_empty());
    for needle in [SECRET, REPLACEMENT, &encoded] {
        assert!(!audit.contains(needle), "audit leaked a secret: {audit}");
    }
    let logs = capture.text();
    for needle in [SECRET, REPLACEMENT, &encoded] {
        assert!(!logs.contains(needle), "a log line leaked a secret");
    }
    for needle in [SECRET, REPLACEMENT] {
        assert!(
            database_files_containing(root.path(), needle).is_empty(),
            "plaintext secret found in the database files"
        );
    }
    let loaded = crate::features::identity_providers::repo::find(
        stack.pools.reader(),
        id.parse::<ProviderId>().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let debug = format!("{loaded:?}");
    for needle in [SECRET, REPLACEMENT, &encoded] {
        assert!(!debug.contains(needle), "Debug rendered a secret: {debug}");
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_delete_restricted_with_links() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let created = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let user = stack.link_user_to(&id, "linked").await;
    assert_eq!(
        stack.list_providers(&admin).await["items"][0]["linkedUserCount"],
        1
    );

    let blocked = stack
        .providers_call(Method::DELETE, &format!("/{id}"), &admin, None)
        .await;
    assert_code(&blocked, StatusCode::CONFLICT, "PROVIDER_HAS_LINKS");
    assert_eq!(blocked.json()["error"]["details"], json!({}));
    let wire = blocked.text().to_ascii_lowercase();
    for raw in ["foreign key", "constraint", "sqlite", "1811"] {
        assert!(!wire.contains(raw), "{raw}: {wire}");
    }
    assert_eq!(stack.provider_count().await, 1);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM identity_links")
            .await,
        1,
        "deleting a provider must never remove identity links"
    );
    assert!(
        !stack
            .provider_audit()
            .await
            .iter()
            .any(|row| row.0 == "IDENTITY_PROVIDER_DELETED"),
        "a refused delete must not be audited as done"
    );

    let direct = stack
        .pools
        .write_tx(&stack.clock, "test.provider_delete_restrict", async |tx| {
            crate::features::identity_providers::repo::delete(tx, id.parse::<ProviderId>().unwrap())
                .await
        })
        .await;
    assert!(matches!(
        direct,
        Err(crate::features::identity_providers::error::ProviderError::HasLinks)
    ));

    stack
        .execute(&format!(
            "DELETE FROM identity_links WHERE user_id = '{user}'"
        ))
        .await;
    let removed = stack
        .providers_call(Method::DELETE, &format!("/{id}"), &admin, None)
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.text());
    assert!(removed.body.is_empty());
    assert_eq!(stack.provider_count().await, 0);
    let audit = stack.provider_audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|row| row.0 == "IDENTITY_PROVIDER_DELETED")
            .count(),
        1
    );
    let gone = stack
        .providers_call(Method::DELETE, &format!("/{id}"), &admin, None)
        .await;
    assert_code(&gone, StatusCode::NOT_FOUND, "PROVIDER_NOT_FOUND");
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_discovery_issuer_mismatch_rejected() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    let mut document = idp.discovery_document();
    document["issuer"] = json!(format!("{}/other-realm", idp.base()));
    idp.set(
        "/realm/.well-known/openid-configuration",
        Reply::json(&document),
    );

    let preview = stack
        .providers_call(
            Method::POST,
            "/discover",
            &admin,
            Some(&json!({ "issuerUrl": idp.issuer() })),
        )
        .await;
    assert_code(
        &preview,
        StatusCode::BAD_GATEWAY,
        "PROVIDER_DISCOVERY_FAILED",
    );
    assert_detail(&preview, "reason", &json!("issuer_mismatch"));

    let create = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_code(
        &create,
        StatusCode::BAD_GATEWAY,
        "PROVIDER_DISCOVERY_FAILED",
    );
    assert_eq!(stack.provider_count().await, 0);

    for variant in [
        format!("{}/realm/", idp.base()),
        format!("{}/realm", idp.base().replace("127.0.0.1", "localhost")),
    ] {
        let mut document = idp.discovery_document();
        document["issuer"] = json!(idp.issuer());
        idp.set(
            "/realm/.well-known/openid-configuration",
            Reply::json(&document),
        );
        let mismatched = stack
            .providers_call(
                Method::POST,
                "/discover",
                &admin,
                Some(&json!({ "issuerUrl": variant })),
            )
            .await;
        assert_eq!(
            mismatched.status,
            StatusCode::BAD_GATEWAY,
            "{variant}: {}",
            mismatched.text()
        );
    }

    idp.set(
        "/realm/.well-known/openid-configuration",
        Reply::json(&idp.discovery_document()),
    );
    idp.serve_healthy_oidc();
    let id = stack.created(&admin, &oidc_body(&idp, "authentik")).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut moved = idp.discovery_document();
    moved["issuer"] = json!("https://elsewhere.example.test/realm");
    idp.set(
        "/realm/.well-known/openid-configuration",
        Reply::json(&moved),
    );
    let patched = stack
        .patch_provider(
            &admin,
            &id,
            &json!({ "issuerUrl": format!("{}/realm/", idp.base()) }),
        )
        .await;
    assert_code(
        &patched,
        StatusCode::BAD_GATEWAY,
        "PROVIDER_DISCOVERY_FAILED",
    );
    assert_eq!(
        stack.provider_scalar(&id, "issuer").await.as_deref(),
        Some(idp.issuer().as_str())
    );
    let tested = stack
        .providers_call(Method::POST, &format!("/{id}/test"), &admin, None)
        .await;
    assert_code(
        &tested,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PROVIDER_VALIDATION_FAILED",
    );
    let checks = tested.json()["error"]["details"]["checks"].clone();
    assert_eq!(
        checks[0],
        json!({ "name": "discovery", "ok": false, "detail": "issuer_mismatch" })
    );
    assert_eq!(stack.provider_scalar(&id, "validated_at").await, None);
    stack.stop().await;
}

#[tokio::test]
async fn it_auto_provision_default_off() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let users_before = stack.scalar_i64("SELECT COUNT(*) FROM users").await;

    let oidc = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let oauth2 = stack
        .created(&admin, &oauth2_body(&idp, "custom-oauth"))
        .await;
    for provider in [&oidc, &oauth2] {
        assert_eq!(provider["autoProvision"], false);
        assert_eq!(provider["enabled"], false);
        let id = provider["id"].as_str().unwrap();
        assert_eq!(
            stack.provider_scalar(id, "auto_provision").await.as_deref(),
            Some("0")
        );
        for member in [
            "autoProvisionRole",
            "defaultRole",
            "role",
            "adminEmailDomains",
        ] {
            assert!(provider.get(member).is_none(), "{member}");
        }
    }

    let id = oidc["id"].as_str().unwrap();
    let enabled = stack
        .patched(&admin, id, &json!({ "autoProvision": true }))
        .await;
    assert_eq!(enabled["autoProvision"], true);
    let listed = stack.list_providers(&admin).await;
    let reloaded = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == oidc["id"])
        .unwrap();
    assert_eq!(reloaded["autoProvision"], true);
    let other = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == oauth2["id"])
        .unwrap();
    assert_eq!(other["autoProvision"], false);

    let mut explicit = oauth2_body(&idp, "explicit-on");
    explicit["autoProvision"] = json!(true);
    assert_eq!(
        stack.created(&admin, &explicit).await["autoProvision"],
        true
    );

    for member in [
        "autoProvisionRole",
        "defaultRole",
        "role",
        "adminEmailDomains",
        "adminDomains",
    ] {
        let mut body = oauth2_body(&idp, "role-attempt");
        body[member] = json!("admin");
        let rejected = stack.create_provider(&admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        let rejected = stack
            .patch_provider(&admin, id, &json!({ member: ["example.test"] }))
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM users").await,
        users_before,
        "configuring a provider must never create an account"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_email_linking_defaults_follow_the_protocol_and_persist_overrides() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    let oidc = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let oauth2 = stack
        .created(&admin, &oauth2_body(&idp, "custom-oauth"))
        .await;
    assert_eq!(oidc["allowEmailLinking"], true);
    assert_eq!(oauth2["allowEmailLinking"], false);
    let (oidc_id, oauth2_id) = (oidc["id"].as_str().unwrap(), oauth2["id"].as_str().unwrap());
    assert_eq!(
        stack
            .provider_scalar(oidc_id, "allow_email_linking")
            .await
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        stack
            .provider_scalar(oauth2_id, "allow_email_linking")
            .await
            .as_deref(),
        Some("0")
    );

    let flipped_oidc = stack
        .patched(&admin, oidc_id, &json!({ "allowEmailLinking": false }))
        .await;
    let flipped_oauth2 = stack
        .patched(&admin, oauth2_id, &json!({ "allowEmailLinking": true }))
        .await;
    assert_eq!(flipped_oidc["allowEmailLinking"], false);
    assert_eq!(flipped_oauth2["allowEmailLinking"], true);
    let listed = stack.list_providers(&admin).await;
    let by_id = |id: &str| {
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_id(oidc_id)["allowEmailLinking"], false);
    assert_eq!(by_id(oauth2_id)["allowEmailLinking"], true);

    let mut overridden = oauth2_body(&idp, "oauth-linking");
    overridden["allowEmailLinking"] = json!(true);
    assert_eq!(
        stack.created(&admin, &overridden).await["allowEmailLinking"],
        true
    );
    let mut disabled = oidc_body(&idp, "oidc-no-linking");
    disabled["allowEmailLinking"] = json!(false);
    assert_eq!(
        stack.created(&admin, &disabled).await["allowEmailLinking"],
        false
    );

    stack
        .execute(&format!(
            "UPDATE identity_providers SET allow_email_linking = 1 WHERE id = '{oauth2_id}'"
        ))
        .await;
    stack
        .execute(&format!(
            "UPDATE identity_providers SET allow_email_linking = 0 WHERE id = '{oidc_id}'"
        ))
        .await;
    let reread = stack.list_providers(&admin).await;
    let item = |id: &str| {
        reread["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        item(oauth2_id)["allowEmailLinking"],
        true,
        "the value is stored, never inferred from the protocol"
    );
    assert_eq!(item(oidc_id)["allowEmailLinking"], false);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_secret_encrypts_at_rest_with_the_idp_purpose_and_row_aad() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    let keys = stack.settings.keys();

    let created = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let (ciphertext, nonce, version) = stack.provider_secret_row(&id).await;
    let (ciphertext, nonce) = (ciphertext.unwrap(), nonce.unwrap());
    assert_eq!(nonce.len(), 24);
    assert_eq!(version, 1);
    assert_ne!(ciphertext, SECRET.as_bytes());
    assert_eq!(ciphertext.len(), SECRET.len() + 16);
    let provider_id = id.parse::<ProviderId>().unwrap();
    let sealed = SealedSecret::from_parts(ciphertext.clone(), &nonce, version).unwrap();
    let opened = keys
        .open(SealPurpose::Idp, &client_secret_aad(provider_id), &sealed)
        .unwrap();
    assert_eq!(opened.expose_secret(), SECRET.as_bytes());
    assert!(keys
        .open(SealPurpose::Smtp, &client_secret_aad(provider_id), &sealed)
        .is_err());
    let other = stack.created(&admin, &oidc_body(&idp, "second")).await;
    let other_id = other["id"].as_str().unwrap().parse::<ProviderId>().unwrap();
    assert!(
        keys.open(SealPurpose::Idp, &client_secret_aad(other_id), &sealed)
            .is_err(),
        "a ciphertext moved to another provider row must not open"
    );

    let kept = stack
        .patched(&admin, &id, &json!({ "displayName": "Renamed" }))
        .await;
    assert_eq!(kept["clientSecretConfigured"], true);
    let after_rename = stack.provider_secret_row(&id).await;
    assert_eq!(
        (
            after_rename.0.clone().unwrap(),
            after_rename.1.clone().unwrap()
        ),
        (ciphertext.clone(), nonce.clone())
    );

    let replaced = stack
        .patched(&admin, &id, &json!({ "clientSecret": REPLACEMENT }))
        .await;
    assert_eq!(replaced["clientSecretConfigured"], true);
    let (next_ciphertext, next_nonce, _) = stack.provider_secret_row(&id).await;
    let (next_ciphertext, next_nonce) = (next_ciphertext.unwrap(), next_nonce.unwrap());
    assert_ne!(next_ciphertext, ciphertext);
    assert_ne!(next_nonce, nonce);
    let resealed = SealedSecret::from_parts(next_ciphertext, &next_nonce, 1).unwrap();
    assert_eq!(
        keys.open(SealPurpose::Idp, &client_secret_aad(provider_id), &resealed)
            .unwrap()
            .expose_secret(),
        REPLACEMENT.as_bytes()
    );

    let empty = stack
        .patch_provider(&admin, &id, &json!({ "clientSecret": "" }))
        .await;
    assert_code(&empty, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert!(
        stack.provider_secret_row(&id).await.0.is_some(),
        "an empty value must never clear the secret"
    );
    let clear_confidential = stack
        .patch_provider(&admin, &id, &json!({ "clientSecret": null }))
        .await;
    assert_code(
        &clear_confidential,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert!(stack.provider_secret_row(&id).await.0.is_some());

    let public = stack
        .patched(
            &admin,
            &id,
            &json!({ "clientSecret": null, "tokenAuthMethod": "none" }),
        )
        .await;
    assert_eq!(public["clientSecretConfigured"], false);
    assert_eq!(stack.provider_secret_row(&id).await, (None, None, 1));
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_public_client_needs_no_secret_and_confidential_client_does() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    let mut public = oidc_body(&idp, "public");
    public.as_object_mut().unwrap().remove("clientSecret");
    public["tokenAuthMethod"] = json!("none");
    let created = stack.created(&admin, &public).await;
    assert_eq!(created["clientSecretConfigured"], false);
    assert_eq!(created["tokenAuthMethod"], "none");

    let mut confidential = oidc_body(&idp, "confidential");
    confidential.as_object_mut().unwrap().remove("clientSecret");
    let missing = stack.create_provider(&admin, &confidential).await;
    assert_code(
        &missing,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        missing.json()["error"]["details"]["fields"],
        json!(["clientSecret"])
    );

    let mut contradictory = oidc_body(&idp, "contradictory");
    contradictory["tokenAuthMethod"] = json!("none");
    let rejected = stack.create_provider(&admin, &contradictory).await;
    assert_code(
        &rejected,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(stack.provider_count().await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_slug_is_unique_lowercase_and_immutable() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    let first = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let id = first["id"].as_str().unwrap().to_owned();
    assert_eq!(first["slug"], "authentik");
    let duplicate = stack
        .create_provider(&admin, &oidc_body(&idp, "authentik"))
        .await;
    assert_code(&duplicate, StatusCode::CONFLICT, "PROVIDER_SLUG_TAKEN");
    assert_eq!(stack.provider_count().await, 1);

    for invalid in [
        "Authentik",
        "a",
        "has space",
        "dot.ted",
        "",
        &"x".repeat(41),
    ] {
        let rejected = stack
            .create_provider(&admin, &oidc_body(&idp, invalid))
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    for accepted in ["ab", "my-sso_2", &"y".repeat(40)] {
        let mut body = oidc_body(&idp, accepted);
        body["clientId"] = json!("c");
        assert_eq!(
            stack.create_provider(&admin, &body).await.status,
            StatusCode::CREATED,
            "{accepted}"
        );
    }

    let rename = stack
        .patch_provider(&admin, &id, &json!({ "slug": "renamed" }))
        .await;
    assert_code(
        &rename,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    let combined = stack
        .patch_provider(
            &admin,
            &id,
            &json!({ "slug": "renamed", "displayName": "Changed" }),
        )
        .await;
    assert_code(
        &combined,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    let listed = stack.list_providers(&admin).await;
    let item = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id.as_str())
        .unwrap();
    assert_eq!(item["slug"], "authentik");
    assert_eq!(item["displayName"], "Provider authentik");
    assert_eq!(
        item["redirectUri"],
        format!("{CALLBACK}/authentik/callback")
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_redirect_uri_is_derived_from_the_base_url_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    stack.clock.advance(Duration::from_secs(1));
    let body = oidc_body(&idp, "authentik").to_string();
    let hostile = Request::builder()
        .method(Method::POST)
        .uri(PROVIDERS)
        .header("content-type", "application/json")
        .header("origin", BASE_URL)
        .header("host", "evil.example.test")
        .header("x-forwarded-host", "evil.example.test")
        .header("x-forwarded-proto", "http")
        .header("forwarded", "host=evil.example.test;proto=http")
        .header(
            "cookie",
            format!("palmr_session={}; palmr_csrf={}", admin.session, admin.csrf),
        )
        .header("x-palmr-csrf", &admin.csrf)
        .body(Body::from(body))
        .unwrap();
    let created = stack.send(with_peer(hostile, 10)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(
        created.json()["redirectUri"],
        format!("{CALLBACK}/authentik/callback")
    );
    assert!(!created.text().contains("evil.example.test"));

    for body in [
        json!({ "redirectUri": "https://evil.example.test/cb" }),
        json!({ "redirect_uri": "https://evil.example.test/cb" }),
    ] {
        let id = created.json()["id"].as_str().unwrap().to_owned();
        let patch = stack.patch_provider(&admin, &id, &body).await;
        assert_code(&patch, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    }
    let mut writable = oidc_body(&idp, "second");
    writable["redirectUri"] = json!("https://evil.example.test/cb");
    let rejected = stack.create_provider(&admin, &writable).await;
    assert_code(
        &rejected,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_create_discovers_endpoints_and_applies_defaults() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    let created = stack.created(&admin, &oidc_body(&idp, "authentik")).await;
    let base = idp.base();
    assert_eq!(created["protocol"], "oidc");
    assert_eq!(created["preset"], "generic");
    assert_eq!(created["issuerUrl"], idp.issuer());
    assert_eq!(
        created["endpoints"],
        json!({
            "authorization": format!("{base}/realm/authorize"),
            "token": format!("{base}/realm/token"),
            "userinfo": format!("{base}/realm/userinfo"),
            "jwks": format!("{base}/realm/jwks"),
        })
    );
    assert_eq!(created["scopes"], json!(["openid", "profile", "email"]));
    assert_eq!(created["tokenAuthMethod"], "client_secret_post");
    assert_eq!(
        created["claimMapping"],
        json!({
            "subject": "sub", "email": "email", "emailVerified": "email_verified",
            "username": "preferred_username", "name": "name", "picture": "picture",
        })
    );
    assert_eq!(created["enabled"], false);
    assert_eq!(created["validatedAt"], Value::Null);
    assert_eq!(created["linkedUserCount"], 0);
    assert_eq!(created["sortOrder"], 0);
    assert_eq!(
        created["redirectUri"],
        format!("{CALLBACK}/authentik/callback")
    );
    let requests = idp.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/realm/.well-known/openid-configuration");
    assert_eq!(
        stack
            .provider_scalar(created["id"].as_str().unwrap(), "discovery_url")
            .await
            .as_deref(),
        Some(format!("{base}/realm/.well-known/openid-configuration").as_str())
    );

    let mut explicit = oidc_body(&idp, "explicit");
    explicit["endpoints"] = json!({
        "authorization": format!("{base}/custom/authorize"),
        "token": format!("{base}/custom/token"),
        "jwks": format!("{base}/custom/jwks"),
    });
    explicit["scopes"] = json!(["openid", "email"]);
    explicit["claimMapping"] = json!({ "email": "mail" });
    let before = idp.requests().len();
    let created = stack.created(&admin, &explicit).await;
    assert_eq!(
        idp.requests().len(),
        before,
        "a complete explicit endpoint set needs no discovery"
    );
    assert_eq!(
        created["endpoints"]["token"],
        format!("{base}/custom/token")
    );
    assert_eq!(created["endpoints"]["userinfo"], Value::Null);
    assert_eq!(created["claimMapping"]["email"], "mail");
    assert_eq!(created["claimMapping"]["subject"], "sub");
    assert_eq!(created["sortOrder"], 1);

    let mut partial = oidc_body(&idp, "partial");
    partial["endpoints"] = json!({ "token": format!("{base}/override/token") });
    let created = stack.created(&admin, &partial).await;
    assert_eq!(
        created["endpoints"]["token"],
        format!("{base}/override/token")
    );
    assert_eq!(
        created["endpoints"]["authorization"],
        format!("{base}/realm/authorize")
    );

    let mut no_openid = oidc_body(&idp, "no-openid");
    no_openid["scopes"] = json!(["email"]);
    assert_code(
        &stack.create_provider(&admin, &no_openid).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_oauth2_requires_explicit_endpoints_and_never_discovers() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();

    for missing in ["authorization", "token", "userinfo"] {
        let mut body = oauth2_body(&idp, "custom-oauth");
        body["endpoints"].as_object_mut().unwrap().remove(missing);
        let rejected = stack.create_provider(&admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            rejected.json()["error"]["details"]["fields"],
            json!(["endpoints"]),
            "{missing}"
        );
    }
    let mut with_issuer = oauth2_body(&idp, "custom-oauth");
    with_issuer["issuerUrl"] = json!(idp.issuer());
    assert_code(
        &stack.create_provider(&admin, &with_issuer).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    let mut with_jwks = oauth2_body(&idp, "custom-oauth");
    with_jwks["endpoints"]["jwks"] = json!(format!("{}/jwks", idp.base()));
    assert_code(
        &stack.create_provider(&admin, &with_jwks).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );

    let created = stack
        .created(&admin, &oauth2_body(&idp, "custom-oauth"))
        .await;
    assert_eq!(created["protocol"], "oauth2");
    assert_eq!(created["issuerUrl"], Value::Null);
    assert_eq!(created["endpoints"]["jwks"], Value::Null);
    assert_eq!(created["scopes"], json!([]));
    assert!(
        idp.requests().is_empty(),
        "an OAuth2 provider must not trigger OIDC discovery: {:?}",
        idp.requests()
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_provider_presets_fill_defaults_without_creating_providers() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let idp = FakeIdp::start().await;

    let catalogue = stack
        .providers_call(Method::GET, "/presets", &admin, None)
        .await;
    assert_eq!(catalogue.status, StatusCode::OK, "{}", catalogue.text());
    let items = catalogue.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 11);
    let named: Vec<&str> = items
        .iter()
        .filter_map(|item| item["preset"].as_str())
        .collect();
    for expected in [
        "google",
        "github",
        "discord",
        "pocket_id",
        "authentik",
        "zitadel",
        "auth0",
        "kinde",
        "frontegg",
        "generic",
    ] {
        assert!(named.contains(&expected), "{expected}");
    }
    let github = items
        .iter()
        .find(|item| item["preset"] == "github")
        .unwrap();
    assert_eq!(github["protocol"], "oauth2");
    assert_eq!(github["allowEmailLinking"], false);
    assert_eq!(
        github["endpoints"]["userinfo"],
        "https://api.github.com/user"
    );
    let google = items
        .iter()
        .find(|item| item["preset"] == "google")
        .unwrap();
    assert_eq!(google["protocol"], "oidc");
    assert_eq!(google["allowEmailLinking"], true);
    assert_eq!(google["issuerUrl"], "https://accounts.google.com");
    assert_eq!(stack.provider_count().await, 0);
    assert!(idp.requests().is_empty());

    let created = stack
        .created(
            &admin,
            &json!({
                "slug": "github", "displayName": "GitHub", "protocol": "oauth2", "preset": "github",
                "clientId": "gh-client", "clientSecret": SECRET,
            }),
        )
        .await;
    assert_eq!(created["preset"], "github");
    assert_eq!(created["scopes"], json!(["read:user", "user:email"]));
    assert_eq!(
        created["endpoints"]["token"],
        "https://github.com/login/oauth/access_token"
    );
    assert_eq!(created["claimMapping"]["subject"], "id");
    let mismatched = stack
        .create_provider(
            &admin,
            &json!({
                "slug": "gh2", "displayName": "GitHub", "protocol": "oidc", "preset": "github",
                "issuerUrl": idp.issuer(), "clientId": "c", "clientSecret": SECRET,
            }),
        )
        .await;
    assert_code(
        &mismatched,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(stack.provider_count().await, 1);
    stack.stop().await;
}
