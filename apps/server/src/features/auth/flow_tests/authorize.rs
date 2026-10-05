use base64ct::{Base64UrlUnpadded, Encoding};
use http::header::HOST;
use url::Url;

use super::admin_providers::{oauth2_body, oidc_body};
use super::profile::Call;
use super::*;
use crate::domain::clock::Clock;
use crate::features::identity_providers::authorize::{
    authorization_request_aad, AUTH_REQUEST_TTL_SECONDS, DEFAULT_RETURN_TO,
};
use crate::features::identity_providers::model::AuthRequestId;
use crate::features::identity_providers::test_support::FakeIdp;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hash::{sha256_base64url, sha256_hex};
use crate::infra::crypto::hkdf::SealPurpose;

const LIST: &str = "/api/v1/auth/providers";

async fn authorize(stack: &Stack, slug: &str, body: &Value, host: u8) -> Fetched {
    stack
        .post_json(
            &format!("{LIST}/{slug}/authorize"),
            &body.to_string(),
            host,
            None,
        )
        .await
}

fn authorization_url(fetched: &Fetched) -> Url {
    Url::parse(fetched.json()["authorizationUrl"].as_str().unwrap()).unwrap()
}

fn query_param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

async fn latest_scalar<T>(stack: &Stack, sql: &str) -> T
where
    T: Send + Unpin + for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    sqlx::query_scalar(sql)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap()
}

async fn auth_request_count(stack: &Stack) -> i64 {
    stack
        .scalar_i64("SELECT COUNT(*) FROM oauth_auth_requests")
        .await
}

async fn decrypted_verifier(stack: &Stack) -> (AuthRequestId, String) {
    let id_text: String = latest_scalar(
        stack,
        "SELECT id FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    let id: AuthRequestId = id_text.parse().unwrap();
    let (ciphertext, nonce, key_version): (Vec<u8>, Vec<u8>, i64) = sqlx::query_as(
        "SELECT pkce_verifier_ciphertext, pkce_verifier_nonce, key_version
           FROM oauth_auth_requests WHERE id = ?1",
    )
    .bind(&id_text)
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    let sealed = SealedSecret::from_parts(ciphertext, &nonce, key_version).unwrap();
    let opened = stack
        .settings
        .keys()
        .open(SealPurpose::Oidc, &authorization_request_aad(id), &sealed)
        .unwrap();
    (
        id,
        String::from_utf8(opened.expose_secret().clone()).unwrap(),
    )
}

async fn oidc_provider(stack: &Stack, admin: &Credentials, slug: &str) -> Value {
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    stack
        .created(admin, &with_enabled(oidc_body(&idp, slug)))
        .await
}

fn with_enabled(mut body: Value) -> Value {
    body["enabled"] = json!(true);
    body
}

async fn patch_security(stack: &Stack, admin: &Credentials, body: &Value, host: u8) -> Fetched {
    stack
        .call(
            Call::new(Method::PATCH, "/api/v1/admin/settings/security", admin).json(body),
            host,
        )
        .await
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R040")]
#[tokio::test]
async fn regression_R040_oauth_redirect_uri_allowlist() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let idp = FakeIdp::start().await;
    idp.serve_healthy_oidc();
    stack
        .created(&admin, &with_enabled(oidc_body(&idp, "corp")))
        .await;

    let fetched = authorize(
        &stack,
        "corp",
        &json!({ "purpose": "login", "returnTo": "/overview" }),
        10,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let url = authorization_url(&fetched);
    assert_eq!(
        query_param(&url, "redirect_uri").as_deref(),
        Some("https://files.example.test/api/v1/auth/providers/corp/callback")
    );

    // Hostile host/proxy headers must never influence the derived redirect URI.
    for headers in [
        vec![(HOST.as_str(), "evil.example")],
        vec![("x-forwarded-host", "evil.example")],
        vec![("forwarded", "host=evil.example;proto=https")],
        vec![
            (HOST.as_str(), "evil.example"),
            ("x-forwarded-host", "evil.example"),
            ("forwarded", "host=evil.example"),
        ],
    ] {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("{LIST}/corp/authorize"))
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, BASE_URL);
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        let fetched = stack
            .send(with_peer(
                builder
                    .body(Body::from(json!({ "purpose": "login" }).to_string()))
                    .unwrap(),
                11,
            ))
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        assert_eq!(
            query_param(&authorization_url(&fetched), "redirect_uri").as_deref(),
            Some("https://files.example.test/api/v1/auth/providers/corp/callback")
        );
    }

    // A client-supplied redirect field is rejected by the strict DTO.
    for field in [
        "redirectUri",
        "redirect_uri",
        "redirect",
        "next",
        "callbackUrl",
    ] {
        let before = auth_request_count(&stack).await;
        let fetched = authorize(
            &stack,
            "corp",
            &json!({ "purpose": "login", field: "/evil" }),
            12,
        )
        .await;
        assert_eq!(fetched.status, StatusCode::UNPROCESSABLE_ENTITY, "{field}");
        assert_eq!(fetched.error_code(), "VALIDATION_ERROR", "{field}");
        assert!(fetched.set_cookies().is_empty(), "{field}");
        assert_eq!(auth_request_count(&stack).await, before, "{field}");
    }

    // The callback URI cannot be smuggled in as the destination either: it is a
    // post-auth path, validated separately, and never used as a redirect target.
    let fetched = authorize(
        &stack,
        "corp",
        &json!({ "purpose": "login", "returnTo": "https://evil.example/callback" }),
        13,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK);
    let stored: Option<String> = latest_scalar(
        &stack,
        "SELECT post_auth_path FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    assert_eq!(stored.as_deref(), Some(DEFAULT_RETURN_TO));

    stack.stop().await;
}

#[tokio::test]
async fn it_return_to_validation_table() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let _ = oidc_provider(&stack, &admin, "corp").await;

    let long_ok = format!("/files/{}", "a".repeat(512 - "/files/".len()));
    let long_bad = format!("/files/{}", "a".repeat(513 - "/files/".len()));
    assert_eq!(long_ok.len(), 512);
    assert_eq!(long_bad.len(), 513);

    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("overview", Some("/overview"), "/overview"),
        ("files", Some("/files"), "/files"),
        ("files-query", Some("/files?foo=bar"), "/files?foo=bar"),
        ("settings", Some("/settings/security"), "/settings/security"),
        ("admin", Some("/admin/providers"), "/admin/providers"),
        (
            "protocol-relative",
            Some("//evil.example"),
            DEFAULT_RETURN_TO,
        ),
        ("backslash-second", Some("/\\evil"), DEFAULT_RETURN_TO),
        ("absolute", Some("https://evil.example"), DEFAULT_RETURN_TO),
        (
            "scheme-in-path",
            Some("/https://evil.example"),
            DEFAULT_RETURN_TO,
        ),
        ("control", Some("/files\u{0001}"), DEFAULT_RETURN_TO),
        ("crlf", Some("/files\r\n"), DEFAULT_RETURN_TO),
        ("nul", Some("/files\u{0000}"), DEFAULT_RETURN_TO),
        ("too-long", Some(long_bad.as_str()), DEFAULT_RETURN_TO),
        ("unknown-route", Some("/unknown"), DEFAULT_RETURN_TO),
        ("empty", Some(""), DEFAULT_RETURN_TO),
        ("max-length", Some(long_ok.as_str()), long_ok.as_str()),
    ];

    for (index, (case, value, expected)) in cases.into_iter().enumerate() {
        let body = match value {
            Some(value) => json!({ "purpose": "login", "returnTo": value }),
            None => json!({ "purpose": "login" }),
        };
        let host = 20 + u8::try_from(index).unwrap();
        let fetched = authorize(&stack, "corp", &body, host).await;
        assert_eq!(fetched.status, StatusCode::OK, "{case}: {}", fetched.text());
        let stored: Option<String> = latest_scalar(
            &stack,
            "SELECT post_auth_path FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
        )
        .await;
        assert_eq!(stored.as_deref(), Some(expected), "{case}");
    }

    // Missing returnTo defaults to /overview.
    let fetched = authorize(&stack, "corp", &json!({ "purpose": "login" }), 21).await;
    assert_eq!(fetched.status, StatusCode::OK);
    let stored: Option<String> = latest_scalar(
        &stack,
        "SELECT post_auth_path FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    assert_eq!(stored.as_deref(), Some(DEFAULT_RETURN_TO));

    stack.stop().await;
}

#[tokio::test]
async fn it_pkce_s256_always_sent() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let oidc_idp = FakeIdp::start().await;
    oidc_idp.serve_healthy_oidc();
    stack
        .created(&admin, &with_enabled(oidc_body(&oidc_idp, "corp")))
        .await;
    let oauth_idp = FakeIdp::start().await;
    stack
        .created(&admin, &with_enabled(oauth2_body(&oauth_idp, "custom")))
        .await;

    for slug in ["corp", "custom"] {
        let fetched = authorize(&stack, slug, &json!({ "purpose": "login" }), 30).await;
        assert_eq!(fetched.status, StatusCode::OK, "{slug}: {}", fetched.text());
        let url = authorization_url(&fetched);
        assert_eq!(
            query_param(&url, "code_challenge_method").as_deref(),
            Some("S256"),
            "{slug}"
        );
        let challenge = query_param(&url, "code_challenge").expect(slug);
        assert!(!challenge.is_empty(), "{slug}");
        assert_ne!(challenge, "plain", "{slug}");
        assert!(
            url.query_pairs().all(|(_, value)| value != "plain"),
            "{slug}"
        );

        let (_id, verifier) = decrypted_verifier(&stack).await;
        let raw = Base64UrlUnpadded::decode_vec(&verifier).unwrap();
        assert_eq!(raw.len(), 96, "{slug}");
        assert_eq!(sha256_base64url(verifier.as_bytes()), challenge, "{slug}");

        // The plaintext verifier is never persisted: only ciphertext bytes exist.
        let ciphertext: Vec<u8> = latest_scalar(
            &stack,
            "SELECT pkce_verifier_ciphertext FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
        )
        .await;
        assert!(
            !ciphertext
                .windows(verifier.len())
                .any(|window| window == verifier.as_bytes()),
            "{slug}"
        );
    }

    stack.stop().await;
}

#[tokio::test]
async fn it_public_provider_list_is_enabled_ordered_and_safe() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let a = FakeIdp::start().await;
    a.serve_healthy_oidc();
    let b = FakeIdp::start().await;
    b.serve_healthy_oidc();
    stack
        .created(&admin, &with_enabled(oidc_body(&a, "first")))
        .await;
    let second = stack
        .created(&admin, &with_enabled(oidc_body(&b, "second")))
        .await;
    let second_id = second["id"].as_str().unwrap().to_owned();

    // Both new HTTP routes are public with exactly one AuthClass each.
    let inventory = application_routes().build().unwrap().inventory;
    let list_route = inventory.get(&Method::GET, LIST).unwrap().policy();
    assert_eq!(list_route.auth(), AuthClass::Public);
    assert_eq!(list_route.rate_limit(), RateLimitClass::PublicRead);
    let authorize_route = inventory
        .get(&Method::POST, "/api/v1/auth/providers/{slug}/authorize")
        .unwrap()
        .policy();
    assert_eq!(authorize_route.auth(), AuthClass::Public);
    assert_eq!(authorize_route.rate_limit(), RateLimitClass::AuthLogin);

    let fetched = stack.get(LIST, None, 40).await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let body = fetched.json();
    let items = body["providers"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["slug"], "first");
    assert_eq!(items[1]["slug"], "second");
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["displayName", "iconKey", "slug"]);
    }
    assert_eq!(items[0]["iconKey"], "generic");
    assert!(!fetched.text().contains("client-first"));
    assert!(!fetched.text().contains("issuer"));

    // A disabled provider is excluded.
    let patched = stack
        .patch_provider(&admin, &second_id, &json!({ "enabled": false }))
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let fetched = stack.get(LIST, None, 41).await;
    let items = &fetched.json()["providers"];
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["slug"], "first");

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_login_only_and_rejects_link_reauth() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let _ = oidc_provider(&stack, &admin, "corp").await;

    let ok = authorize(&stack, "corp", &json!({ "purpose": "login" }), 50).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(auth_request_count(&stack).await, 1);

    for purpose in ["link", "reauth", "signup", ""] {
        let before = auth_request_count(&stack).await;
        let fetched = authorize(&stack, "corp", &json!({ "purpose": purpose }), 51).await;
        assert_eq!(
            fetched.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{purpose}"
        );
        assert_eq!(fetched.error_code(), "VALIDATION_ERROR", "{purpose}");
        assert!(fetched.set_cookies().is_empty(), "{purpose}");
        assert_eq!(auth_request_count(&stack).await, before, "{purpose}");
    }

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_rejects_unknown_and_disabled_provider() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let provider = oidc_provider(&stack, &admin, "corp").await;
    let id = provider["id"].as_str().unwrap().to_owned();

    let unknown = authorize(&stack, "missing", &json!({ "purpose": "login" }), 60).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "PROVIDER_NOT_FOUND");
    assert!(unknown.set_cookies().is_empty());

    let patched = stack
        .patch_provider(&admin, &id, &json!({ "enabled": false }))
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let disabled = authorize(&stack, "corp", &json!({ "purpose": "login" }), 61).await;
    assert_eq!(disabled.status, StatusCode::FORBIDDEN);
    assert_eq!(disabled.error_code(), "PROVIDER_DISABLED");
    assert!(disabled.set_cookies().is_empty());
    assert_eq!(auth_request_count(&stack).await, 0);

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_state_binding_and_nonce_are_hashed_and_independent() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let _ = oidc_provider(&stack, &admin, "corp").await;

    let first = authorize(&stack, "corp", &json!({ "purpose": "login" }), 70).await;
    assert_eq!(first.status, StatusCode::OK);
    let first_url = authorization_url(&first);
    let first_cookie = first.cookie("palmr_oauth");
    let second = authorize(&stack, "corp", &json!({ "purpose": "login" }), 71).await;
    assert_eq!(second.status, StatusCode::OK);
    let second_url = authorization_url(&second);
    let second_cookie = second.cookie("palmr_oauth");

    let first_state = query_param(&first_url, "state").unwrap();
    let second_state = query_param(&second_url, "state").unwrap();
    assert_eq!(first_state.len(), 43);
    assert_eq!(second_state.len(), 43);
    assert_ne!(first_state, second_state);
    assert_ne!(first_state, first_cookie);
    assert_ne!(first_cookie, second_cookie);

    // Stored only as lowercase-hex SHA-256 of the raw 256-bit value, never raw.
    let stored: String = latest_scalar(
        &stack,
        "SELECT state_hash FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    let state_raw = Base64UrlUnpadded::decode_vec(&second_state).unwrap();
    assert_eq!(state_raw.len(), 32);
    assert_eq!(stored, sha256_hex(&state_raw).as_str());
    let raw_rows: i64 = stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM oauth_auth_requests WHERE state_hash = '{second_state}'"
        ))
        .await;
    assert_eq!(raw_rows, 0);

    let binding_hash: String = latest_scalar(
        &stack,
        "SELECT binding_cookie_hash FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    let cookie_raw = Base64UrlUnpadded::decode_vec(&second_cookie).unwrap();
    assert_eq!(cookie_raw.len(), 32);
    assert_eq!(binding_hash, sha256_hex(&cookie_raw).as_str());

    let nonce: String = latest_scalar(
        &stack,
        "SELECT nonce FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    assert_eq!(nonce.len(), 43);
    assert_ne!(nonce, second_state);
    assert_eq!(
        query_param(&second_url, "nonce").as_deref(),
        Some(nonce.as_str())
    );

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_cookie_attributes_and_ten_minute_lifetime() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let _ = oidc_provider(&stack, &admin, "corp").await;

    let fetched = authorize(&stack, "corp", &json!({ "purpose": "login" }), 80).await;
    assert_eq!(fetched.status, StatusCode::OK);
    let cookies = fetched.set_cookies();
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    let cookie = &cookies[0];
    assert!(cookie.starts_with("palmr_oauth="), "{cookie}");
    assert!(cookie.contains("; Path=/api/v1/auth/providers"), "{cookie}");
    assert!(cookie.contains("; SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("; Max-Age=600"), "{cookie}");
    assert!(cookie.contains("; Secure"), "{cookie}");
    assert!(cookie.contains("; HttpOnly"), "{cookie}");
    assert!(!cookie.contains("Domain="), "{cookie}");

    let (created, expires): (String, String) = sqlx::query_as(
        "SELECT created_at, expires_at FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    let now = stack.clock.now();
    let expected_created = Timestamp::try_from(now).unwrap().to_string();
    let expected_expires =
        Timestamp::try_from(now + time::Duration::seconds(AUTH_REQUEST_TTL_SECONDS as i64))
            .unwrap()
            .to_string();
    assert_eq!(created, expected_created);
    assert_eq!(expires, expected_expires);
    assert_eq!(AUTH_REQUEST_TTL_SECONDS, 600);

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_nonce_is_persisted_for_oauth2_but_not_sent() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let idp = FakeIdp::start().await;
    stack
        .created(&admin, &with_enabled(oauth2_body(&idp, "custom")))
        .await;

    let fetched = authorize(&stack, "custom", &json!({ "purpose": "login" }), 90).await;
    assert_eq!(fetched.status, StatusCode::OK);
    let url = authorization_url(&fetched);
    assert_eq!(query_param(&url, "nonce"), None);

    let nonce: String = latest_scalar(
        &stack,
        "SELECT nonce FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    assert_eq!(nonce.len(), 43);

    stack.stop().await;
}

#[tokio::test]
async fn it_authorize_persists_everything_the_callback_needs() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let provider = oidc_provider(&stack, &admin, "corp").await;
    let provider_id = provider["id"].as_str().unwrap().to_owned();

    let fetched = authorize(
        &stack,
        "corp",
        &json!({ "purpose": "login", "returnTo": "/settings/security" }),
        95,
    )
    .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let url = authorization_url(&fetched);
    let state = query_param(&url, "state").unwrap();

    // T04 consumes the row with `UPDATE ... RETURNING` before token exchange: it
    // needs a state-hash lookup, a binding to compare, provider equality, an
    // unconsumed unexpired row, a nonce, a decryptable verifier and the exact
    // redirect URI captured at request time.
    let (id, stored_provider, state_hash, binding_hash, nonce, redirect_uri, consumed): (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT id, provider_id, state_hash, binding_cookie_hash, nonce, redirect_uri, consumed_at
           FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(id.parse::<AuthRequestId>().is_ok());
    assert_eq!(stored_provider, provider_id);
    assert_eq!(state_hash, sha256_hex(&Base64UrlUnpadded::decode_vec(&state).unwrap()).as_str());
    assert_eq!(binding_hash.len(), 64);
    assert_eq!(nonce.len(), 43);
    assert_eq!(
        redirect_uri,
        "https://files.example.test/api/v1/auth/providers/corp/callback"
    );
    assert_eq!(consumed, None);

    let stored_path: Option<String> = latest_scalar(
        &stack,
        "SELECT post_auth_path FROM oauth_auth_requests ORDER BY rowid DESC LIMIT 1",
    )
    .await;
    assert_eq!(stored_path.as_deref(), Some("/settings/security"));

    let (_id, verifier) = decrypted_verifier(&stack).await;
    assert_eq!(Base64UrlUnpadded::decode_vec(&verifier).unwrap().len(), 96);

    stack.stop().await;
}

#[tokio::test]
async fn it_global_provider_toggle_takes_effect_immediately() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admin = stack.operator(60).await;
    let _ = oidc_provider(&stack, &admin, "corp").await;

    let listed = stack.get(LIST, None, 100).await;
    assert_eq!(listed.json()["providers"].as_array().unwrap().len(), 1);

    let off = patch_security(
        &stack,
        &admin,
        &json!({ "authProvidersEnabled": false }),
        101,
    )
    .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text());
    assert_eq!(off.json()["authProvidersEnabled"], false);

    let listed = stack.get(LIST, None, 102).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["providers"].as_array().unwrap().len(), 0);

    let before = auth_request_count(&stack).await;
    let blocked = authorize(&stack, "corp", &json!({ "purpose": "login" }), 103).await;
    assert_eq!(blocked.status, StatusCode::FORBIDDEN);
    assert_eq!(blocked.error_code(), "PROVIDER_DISABLED");
    assert!(blocked.set_cookies().is_empty());
    assert_eq!(auth_request_count(&stack).await, before);

    let on = patch_security(
        &stack,
        &admin,
        &json!({ "authProvidersEnabled": true }),
        104,
    )
    .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.text());
    let listed = stack.get(LIST, None, 105).await;
    assert_eq!(listed.json()["providers"].as_array().unwrap().len(), 1);
    let allowed = authorize(&stack, "corp", &json!({ "purpose": "login" }), 106).await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text());

    stack.stop().await;
}
