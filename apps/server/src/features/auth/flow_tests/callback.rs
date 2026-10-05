use std::cell::Cell;
use std::time::{Duration as StdDuration, Instant};

use http::header::LOCATION;
use serde_json::{json, Value};
use url::Url;

use super::profile::Call;
use super::*;
use crate::domain::clock::Clock;
use crate::features::identity_providers::authorize::{authorization_request_aad, AuthorizeContext};
use crate::features::identity_providers::model::{AuthRequestId, IdentityLinkId};
use crate::features::identity_providers::provision::{
    deterministic_candidates, provision_with, random_candidate, RANDOM_SUFFIX_BYTES,
};
use crate::features::identity_providers::repo;
use crate::features::identity_providers::resolve::ExternalIdentity;
use crate::features::identity_providers::resolve::{
    create_link, LinkInsert, LinkMethod, ResolveInput,
};
use crate::infra::crypto::hash::sha256_base64url;

#[path = "../../../../tests/support/mock_idp.rs"]
mod mock_idp;

use mock_idp::{MockIdp, CLIENT_ID, RSA1_KID};

const PROVIDERS: &str = "/api/v1/auth/providers";
const SECRET: &str = "callback-test-client-secret";
const FAILURE_PREFIX: &str = "https://files.example.test/login?error=";
const OAUTH_CLEARED: &str =
    "palmr_oauth=; Path=/api/v1/auth/providers; SameSite=Lax; Max-Age=0; Secure; HttpOnly";

type TokenArm = Box<dyn Fn(&MockIdp)>;

struct Begun {
    state: String,
    binding: String,
    nonce: String,
}

struct Federation {
    stack: Stack,
    idp: MockIdp,
    admin: Credentials,
    peer: Cell<u8>,
    _root: TempDir,
}

impl Federation {
    async fn start() -> Self {
        let root = TempDir::new().unwrap();
        let stack = Stack::start(root.path(), &TestClock::new(START)).await;
        let admin = stack.operator(60).await;
        Self {
            stack,
            idp: MockIdp::start().await,
            admin,
            peer: Cell::new(1),
            _root: root,
        }
    }

    fn next_peer(&self) -> u8 {
        let current = self.peer.get();
        self.peer.set(if current >= 240 { 1 } else { current + 1 });
        current
    }

    fn now(&self) -> i64 {
        self.stack.clock.now().unix_timestamp()
    }

    async fn provider(&self, body: Value) -> Value {
        let mut body = body;
        body["enabled"] = json!(true);
        self.stack.created(&self.admin, &body).await
    }

    async fn oidc(&self, slug: &str, extra: Value) -> Value {
        let mut body = json!({
            "slug": slug,
            "displayName": format!("Provider {slug}"),
            "protocol": "oidc",
            "issuerUrl": self.idp.issuer(),
            "clientId": CLIENT_ID,
            "clientSecret": SECRET,
        });
        merge(&mut body, extra);
        self.provider(body).await
    }

    async fn oauth2(&self, slug: &str, extra: Value) -> Value {
        let mut body = json!({
            "slug": slug,
            "displayName": format!("OAuth {slug}"),
            "protocol": "oauth2",
            "clientId": CLIENT_ID,
            "clientSecret": SECRET,
            "endpoints": {
                "authorization": format!("{}/authorize", self.idp.issuer()),
                "token": self.idp.token_endpoint(),
                "userinfo": self.idp.userinfo_endpoint(),
            },
        });
        merge(&mut body, extra);
        self.provider(body).await
    }

    async fn begin(&self, slug: &str, return_to: Option<&str>) -> Begun {
        let mut body = json!({ "purpose": "login" });
        if let Some(path) = return_to {
            body["returnTo"] = json!(path);
        }
        let fetched = self
            .stack
            .post_json(
                &format!("{PROVIDERS}/{slug}/authorize"),
                &body.to_string(),
                self.next_peer(),
                None,
            )
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        let url = Url::parse(fetched.json()["authorizationUrl"].as_str().unwrap()).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let nonce: String =
            sqlx::query_scalar("SELECT nonce FROM oauth_auth_requests WHERE state_hash = ?1")
                .bind(digest(&state))
                .fetch_one(self.stack.pools.reader().executor())
                .await
                .unwrap();
        Begun {
            binding: fetched.cookie("palmr_oauth"),
            state,
            nonce,
        }
    }

    async fn callback(&self, slug: &str, query: &str, cookie: Option<&str>) -> Fetched {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(format!("{PROVIDERS}/{slug}/callback?{query}"));
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, format!("palmr_oauth={cookie}"));
        }
        self.stack
            .send(with_peer(
                builder.body(Body::empty()).unwrap(),
                self.next_peer(),
            ))
            .await
    }

    async fn finish(&self, slug: &str, begun: &Begun) -> Fetched {
        self.callback(
            slug,
            &format!("code=auth-code&state={}", begun.state),
            Some(&begun.binding),
        )
        .await
    }

    fn claims(&self, begun: &Begun, set: Value, remove: &[&str]) -> Value {
        let mut claims = self.idp.valid_claims(self.now(), &begun.nonce);
        merge(&mut claims, set);
        for key in remove {
            claims.as_object_mut().unwrap().remove(*key);
        }
        claims
    }

    fn arm_oidc(&self, begun: &Begun, set: Value, remove: &[&str]) {
        let claims = self.claims(begun, set, remove);
        self.idp.set_token_response(json!({
            "access_token": "access-token-value",
            "token_type": "Bearer",
            "id_token": self.idp.sign_rs256(RSA1_KID, &claims),
        }));
    }

    async fn login_oidc(&self, slug: &str, set: Value, remove: &[&str]) -> Fetched {
        let begun = self.begin(slug, None).await;
        self.arm_oidc(&begun, set, remove);
        self.finish(slug, &begun).await
    }

    async fn login_oauth2(&self, slug: &str, userinfo: Value) -> Fetched {
        let begun = self.begin(slug, None).await;
        self.idp.set_token_response(json!({
            "access_token": "oauth-access-token",
            "token_type": "bearer",
        }));
        self.idp.set_userinfo(userinfo);
        self.finish(slug, &begun).await
    }

    async fn count(&self, sql: &str) -> i64 {
        self.stack.scalar_i64(sql).await
    }

    async fn token_form(&self, index: usize) -> Vec<(String, String)> {
        let requests = self.idp.token_requests().await;
        url::form_urlencoded::parse(&requests[index].body)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    async fn user_by_email(&self, email: &str) -> UserId {
        let id: String = sqlx::query_scalar("SELECT id FROM users WHERE email_normalized = ?1")
            .bind(email)
            .fetch_one(self.stack.pools.reader().executor())
            .await
            .unwrap();
        id.parse().unwrap()
    }

    async fn provider_id(&self, slug: &str) -> String {
        sqlx::query_scalar("SELECT id FROM identity_providers WHERE key = ?1")
            .bind(slug)
            .fetch_one(self.stack.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn make_user(&self, username: &str, email: &str) -> UserId {
        self.stack
            .user(UserSpec {
                hash: None,
                ..UserSpec::local(username, email, &password_hash())
            })
            .await
    }

    async fn link(&self, slug: &str, user: UserId, subject: &str, state: &str) {
        let provider = self.provider_id(slug).await;
        let id = IdentityLinkId::generate(&self.stack.clock);
        let suspended = if state == "suspended" {
            "'2026-09-25T12:00:00.000Z'"
        } else {
            "NULL"
        };
        self.stack
            .execute(&format!(
                "INSERT INTO identity_links (id, user_id, provider_id, subject, email_at_link,
                    link_method, state, created_at, suspended_at)
                 VALUES ('{id}', '{user}', '{provider}', '{subject}', 'old@example.test',
                    'manual', '{state}', '2026-09-25T12:00:00.000Z', {suspended})"
            ))
            .await;
    }

    async fn links(&self) -> Vec<(String, String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT user_id, subject, link_method, last_login_at FROM identity_links ORDER BY rowid",
        )
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn session_count(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM sessions").await
    }
}

fn verified_identity(subject: &str, email: &str) -> ExternalIdentity {
    ExternalIdentity {
        subject: subject.to_owned(),
        email: Some(email.to_owned()),
        email_verified: true,
        username: None,
        name: None,
        picture: None,
    }
}

fn merge(target: &mut Value, extra: Value) {
    if let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) {
        for (key, value) in extra {
            target.insert(key, value);
        }
    }
}

fn location(fetched: &Fetched) -> String {
    fetched
        .headers
        .get(LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn assert_failure(fetched: &Fetched, code: &str) {
    assert_eq!(fetched.status, StatusCode::SEE_OTHER, "{code}");
    assert_eq!(location(fetched), format!("{FAILURE_PREFIX}{code}"));
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    assert!(fetched.body.is_empty());
    let cookies = fetched.set_cookies();
    assert_eq!(cookies, vec![OAUTH_CLEARED.to_owned()], "{code}");
}

fn assert_signed_in(fetched: &Fetched, destination: &str) -> Credentials {
    assert_eq!(
        fetched.status,
        StatusCode::SEE_OTHER,
        "{}",
        location(fetched)
    );
    assert_eq!(
        location(fetched),
        format!("https://files.example.test{destination}")
    );
    let cookies = fetched.set_cookies();
    assert_eq!(cookies.len(), 3, "{cookies:?}");
    assert!(cookies.iter().any(|cookie| cookie == OAUTH_CLEARED));
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with("palmr_session=") && cookie.contains("HttpOnly")));
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with("palmr_csrf=")));
    Credentials::from(fetched)
}

fn form_value<'a>(form: &'a [(String, String)], name: &str) -> Option<&'a str> {
    form.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn unit_callback_params_parse_the_documented_query_only() {
    use crate::features::identity_providers::callback::CallbackParams;

    let ok = CallbackParams::parse(Some("code=c&state=s&extra=1"));
    assert_eq!(ok.code.unwrap().expose_secret(), "c");
    assert_eq!(ok.state.unwrap().expose_secret(), "s");
    assert!(!ok.denied && !ok.malformed);

    let denied = CallbackParams::parse(Some(
        "error=access_denied&error_description=%3Cb%3E&state=s",
    ));
    assert!(denied.denied);

    let duplicated = CallbackParams::parse(Some("state=a&state=b&code=c"));
    assert!(duplicated.malformed);
    let duplicated = CallbackParams::parse(Some("state=a&code=c&code=d"));
    assert!(duplicated.malformed);

    let empty = CallbackParams::parse(None);
    assert!(empty.code.is_none() && empty.state.is_none() && !empty.denied);
}

#[test]
fn unit_callback_locations_are_built_from_the_trusted_base_url() {
    use crate::domain::error_code::ErrorCode;
    use crate::features::identity_providers::callback::{failure_location, success_location};

    let base = Url::parse("https://files.example.test").unwrap();
    assert_eq!(
        failure_location(&base, ErrorCode::ProviderStateInvalid),
        "https://files.example.test/login?error=PROVIDER_STATE_INVALID"
    );
    assert_eq!(
        success_location(&base, "/files?x=1"),
        "https://files.example.test/files?x=1"
    );
    let nested = Url::parse("https://example.com/palmr/").unwrap();
    assert_eq!(
        failure_location(&nested, ErrorCode::AuthLocked),
        "https://example.com/palmr/login?error=AUTH_LOCKED"
    );
    assert_eq!(
        success_location(&nested, "/overview"),
        "https://example.com/palmr/overview"
    );
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R041")]
#[tokio::test]
async fn regression_R041_oauth_state_nonce_pkce_single_use() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    let begun = f.begin("corp", None).await;
    let challenge: String = {
        let verifier = decrypted_verifier(&f, &begun).await;
        sha256_base64url(verifier.as_bytes())
    };
    f.arm_oidc(&begun, json!({}), &[]);

    let first = f.finish("corp", &begun).await;
    assert_signed_in(&first, "/overview");
    let form = f.token_form(0).await;
    assert_eq!(f.idp.token_requests().await.len(), 1);
    assert_eq!(form_value(&form, "grant_type"), Some("authorization_code"));
    assert_eq!(form_value(&form, "code"), Some("auth-code"));
    assert_eq!(
        form_value(&form, "redirect_uri"),
        Some("https://files.example.test/api/v1/auth/providers/corp/callback")
    );
    let sent_verifier = form_value(&form, "code_verifier").unwrap();
    assert_eq!(sha256_base64url(sent_verifier.as_bytes()), challenge);
    assert!(sent_verifier.len() >= 128);

    let replay = f.finish("corp", &begun).await;
    assert_failure(&replay, "PROVIDER_STATE_INVALID");
    assert_eq!(f.idp.token_requests().await.len(), 1);

    let rejected = f.begin("corp", None).await;
    f.idp
        .set_token_status(400, json!({ "error": "invalid_grant" }));
    let failed = f.finish("corp", &rejected).await;
    assert_failure(&failed, "PROVIDER_CODE_EXCHANGE_FAILED");
    assert_eq!(f.idp.token_requests().await.len(), 2);
    let retried = f.finish("corp", &rejected).await;
    assert_failure(&retried, "PROVIDER_STATE_INVALID");
    assert_eq!(
        f.idp.token_requests().await.len(),
        2,
        "a consumed state must never reach the token endpoint again"
    );

    let wrong_nonce = f.begin("corp", None).await;
    let other = Begun {
        nonce: "a-different-nonce-value".to_owned(),
        ..Begun {
            state: String::new(),
            binding: String::new(),
            nonce: String::new(),
        }
    };
    f.arm_oidc(&other, json!({}), &[]);
    assert_failure(
        &f.finish("corp", &wrong_nonce).await,
        "PROVIDER_ID_TOKEN_INVALID",
    );

    let racing = f.begin("corp", None).await;
    f.arm_oidc(
        &racing,
        json!({ "sub": "racer", "email": "racer@example.test" }),
        &[],
    );
    let before = f.idp.token_requests().await.len();
    let results = futures_util::future::join_all((0..6).map(|_| f.finish("corp", &racing))).await;
    let advanced = results
        .iter()
        .filter(|fetched| location(fetched) == "https://files.example.test/overview")
        .count();
    assert_eq!(advanced, 1, "exactly one concurrent callback may advance");
    for fetched in results
        .iter()
        .filter(|fetched| location(fetched) != "https://files.example.test/overview")
    {
        assert_eq!(
            location(fetched),
            format!("{FAILURE_PREFIX}PROVIDER_STATE_INVALID")
        );
    }
    assert_eq!(f.idp.token_requests().await.len(), before + 1);
    f.stack.stop().await;
}

async fn decrypted_verifier(f: &Federation, begun: &Begun) -> String {
    let (id, ciphertext, nonce, key_version): (String, Vec<u8>, Vec<u8>, i64) = sqlx::query_as(
        "SELECT id, pkce_verifier_ciphertext, pkce_verifier_nonce, key_version
           FROM oauth_auth_requests WHERE state_hash = ?1",
    )
    .bind(digest(&begun.state))
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    let id: AuthRequestId = id.parse().unwrap();
    let sealed =
        crate::infra::crypto::aead::SealedSecret::from_parts(ciphertext, &nonce, key_version)
            .unwrap();
    let opened = f
        .stack
        .settings
        .keys()
        .open(
            crate::infra::crypto::hkdf::SealPurpose::Oidc,
            &authorization_request_aad(id),
            &sealed,
        )
        .unwrap();
    String::from_utf8(opened.expose_secret().clone()).unwrap()
}

async fn consumed(f: &Federation, begun: &Begun) -> bool {
    let at: Option<String> =
        sqlx::query_scalar("SELECT consumed_at FROM oauth_auth_requests WHERE state_hash = ?1")
            .bind(digest(&begun.state))
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    at.is_some()
}

#[tokio::test]
async fn it_callback_state_and_browser_binding_fail_closed() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    f.oidc("other", json!({})).await;

    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({}), &[]);
    let missing = f
        .callback("corp", &format!("code=c&state={}", begun.state), None)
        .await;
    assert_failure(&missing, "PROVIDER_STATE_INVALID");
    assert!(!consumed(&f, &begun).await, "no cookie, nothing consumed");
    assert!(f.idp.token_requests().await.is_empty());

    let other_browser = f.begin("corp", None).await;
    let wrong = f
        .callback(
            "corp",
            &format!("code=c&state={}", begun.state),
            Some(&other_browser.binding),
        )
        .await;
    assert_failure(&wrong, "PROVIDER_STATE_INVALID");
    assert!(
        consumed(&f, &begun).await,
        "a wrong cookie still burns the state"
    );
    let genuine = f.finish("corp", &begun).await;
    assert_failure(&genuine, "PROVIDER_STATE_INVALID");

    let malformed_cookie = f.begin("corp", None).await;
    let result = f
        .callback(
            "corp",
            &format!("code=c&state={}", malformed_cookie.state),
            Some("not-a-token"),
        )
        .await;
    assert_failure(&result, "PROVIDER_STATE_INVALID");
    assert!(consumed(&f, &malformed_cookie).await);

    let unknown = f
        .callback(
            "corp",
            "code=c&state=AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s",
            Some(&begun.binding),
        )
        .await;
    assert_failure(&unknown, "PROVIDER_STATE_INVALID");
    for query in [
        "code=c&state=short",
        "code=c",
        "state=x",
        "",
        "code=&state=x",
    ] {
        assert_failure(
            &f.callback("corp", query, Some(&begun.binding)).await,
            "PROVIDER_STATE_INVALID",
        );
    }
    let duplicated = f.begin("corp", None).await;
    assert_failure(
        &f.callback(
            "corp",
            &format!(
                "code=c&state={}&state={}",
                duplicated.state, duplicated.state
            ),
            Some(&duplicated.binding),
        )
        .await,
        "PROVIDER_STATE_INVALID",
    );
    assert!(!consumed(&f, &duplicated).await);

    let expiring = f.begin("corp", None).await;
    f.stack.clock.advance(StdDuration::from_secs(601));
    let expired = f.finish("corp", &expiring).await;
    assert_failure(&expired, "PROVIDER_STATE_INVALID");

    let mismatch = f.begin("corp", None).await;
    let crossed = f.finish("other", &mismatch).await;
    assert_failure(&crossed, "PROVIDER_STATE_INVALID");
    assert!(consumed(&f, &mismatch).await);
    let unknown_slug = f.begin("corp", None).await;
    assert_failure(
        &f.finish("no-such-provider", &unknown_slug).await,
        "PROVIDER_STATE_INVALID",
    );

    assert!(f.idp.token_requests().await.is_empty());
    assert_eq!(f.session_count().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_provider_denial_never_echoes_or_exchanges() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let begun = f.begin("corp", None).await;

    let denied = f
        .callback(
            "corp",
            &format!(
                "error=access_denied&error_description=%3Cscript%3Ealert(1)%3C%2Fscript%3E&state={}",
                begun.state
            ),
            Some(&begun.binding),
        )
        .await;
    assert_failure(&denied, "PROVIDER_AUTH_DENIED");
    let rendered = format!("{:?}{}", location(&denied), denied.set_cookies().join(";"));
    assert!(!rendered.contains("script"));
    assert!(!rendered.contains("access_denied"));
    assert!(f.idp.token_requests().await.is_empty());

    let without_cookie = f.callback("corp", "error=server_error", None).await;
    assert_failure(&without_cookie, "PROVIDER_AUTH_DENIED");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_requires_the_exact_stored_redirect_uri_and_a_sound_verifier() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;

    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({}), &[]);
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests SET redirect_uri = 'https://evil.example/api/v1/auth/providers/corp/callback'
              WHERE state_hash = '{}'",
            digest(&begun.state)
        ))
        .await;
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_STATE_INVALID");
    assert!(f.idp.token_requests().await.is_empty());

    let trailing = f.begin("corp", None).await;
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests SET redirect_uri = redirect_uri || '/' WHERE state_hash = '{}'",
            digest(&trailing.state)
        ))
        .await;
    assert_failure(&f.finish("corp", &trailing).await, "PROVIDER_STATE_INVALID");

    let tampered = f.begin("corp", None).await;
    f.arm_oidc(&tampered, json!({}), &[]);
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests
                SET pkce_verifier_ciphertext = X'00' || substr(pkce_verifier_ciphertext, 2)
              WHERE state_hash = '{}'",
            digest(&tampered.state)
        ))
        .await;
    assert_failure(&f.finish("corp", &tampered).await, "PROVIDER_STATE_INVALID");

    let swapped = f.begin("corp", None).await;
    let donor = f.begin("corp", None).await;
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests SET
                pkce_verifier_ciphertext = (SELECT pkce_verifier_ciphertext FROM oauth_auth_requests WHERE state_hash = '{donor}'),
                pkce_verifier_nonce = (SELECT pkce_verifier_nonce FROM oauth_auth_requests WHERE state_hash = '{donor}')
              WHERE state_hash = '{swapped}'",
            donor = digest(&donor.state),
            swapped = digest(&swapped.state)
        ))
        .await;
    assert_failure(&f.finish("corp", &swapped).await, "PROVIDER_STATE_INVALID");
    assert!(
        f.idp.token_requests().await.is_empty(),
        "a verifier bound to another request row never reaches the provider"
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_non_login_requests_never_run_login_resolution() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let user = f.make_user("existing", "existing@example.test").await;

    for purpose in ["link", "reauth"] {
        let context = if purpose == "link" {
            AuthorizeContext::link(user, None)
        } else {
            AuthorizeContext::reauth(user, None)
        };
        let authorized = f.stack.providers.authorize("corp", context).await.unwrap();
        let url = Url::parse(&authorized.authorization_url).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let nonce: String =
            sqlx::query_scalar("SELECT nonce FROM oauth_auth_requests WHERE state_hash = ?1")
                .bind(digest(&state))
                .fetch_one(f.stack.pools.reader().executor())
                .await
                .unwrap();
        let begun = Begun {
            state,
            binding: authorized.binding.expose_secret().clone(),
            nonce,
        };
        f.arm_oidc(&begun, json!({ "email": "existing@example.test" }), &[]);
        let fetched = f.finish("corp", &begun).await;
        assert_failure(&fetched, "PROVIDER_STATE_INVALID");
        assert!(consumed(&f, &begun).await);
    }
    assert!(f.idp.token_requests().await.is_empty());
    assert_eq!(f.count("SELECT COUNT(*) FROM identity_links").await, 0);
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, 2);
    assert_eq!(f.session_count().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_token_endpoint_authentication_follows_the_provider_method() {
    let f = Federation::start().await;
    f.oidc(
        "basic",
        json!({ "tokenAuthMethod": "client_secret_basic", "autoProvision": true }),
    )
    .await;
    f.oidc(
        "post",
        json!({ "tokenAuthMethod": "client_secret_post", "autoProvision": true }),
    )
    .await;
    f.oidc(
        "public",
        json!({ "tokenAuthMethod": "none", "clientSecret": null, "autoProvision": true }),
    )
    .await;

    for slug in ["basic", "post", "public"] {
        let fetched = f.login_oidc(slug, json!({ "sub": slug }), &[]).await;
        assert_signed_in(&fetched, "/overview");
    }
    let requests = f.idp.token_requests().await;
    assert_eq!(requests.len(), 3);
    let authorization = |index: usize| {
        requests[index]
            .headers
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_owned())
    };

    let basic = f.token_form(0).await;
    let expected = {
        use base64ct::Encoding;
        format!(
            "Basic {}",
            base64ct::Base64::encode_string(format!("{CLIENT_ID}:{SECRET}").as_bytes())
        )
    };
    assert_eq!(authorization(0), Some(expected));
    assert_eq!(form_value(&basic, "client_secret"), None);
    assert_eq!(form_value(&basic, "client_id"), None);

    let post = f.token_form(1).await;
    assert_eq!(authorization(1), None);
    assert_eq!(form_value(&post, "client_id"), Some(CLIENT_ID));
    assert_eq!(form_value(&post, "client_secret"), Some(SECRET));

    let public = f.token_form(2).await;
    assert_eq!(authorization(2), None);
    assert_eq!(form_value(&public, "client_id"), Some(CLIENT_ID));
    assert_eq!(form_value(&public, "client_secret"), None);
    for index in 0..3 {
        let form = f.token_form(index).await;
        assert!(form_value(&form, "code_verifier").is_some());
    }
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_token_failures_map_to_the_exchange_code_and_leak_nothing() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;

    let variants: Vec<(&str, TokenArm)> = vec![
        (
            "rejected",
            Box::new(|idp| {
                idp.set_token_status(
                    400,
                    json!({"error": "invalid_grant", "error_description": "super-secret-detail"}),
                )
            }),
        ),
        (
            "server error",
            Box::new(|idp| idp.set_token_status(503, json!({"error": "temporarily_unavailable"}))),
        ),
        (
            "error with 200",
            Box::new(|idp| idp.set_token_response(json!({"error": "bad_verification_code"}))),
        ),
        (
            "not json",
            Box::new(|idp| idp.set_token_raw(200, "access_token=a&token_type=bearer")),
        ),
        (
            "not an object",
            Box::new(|idp| idp.set_token_raw(200, "[1]")),
        ),
        ("empty", Box::new(|idp| idp.set_token_raw(200, ""))),
        (
            "unsupported token type",
            Box::new(|idp| {
                idp.set_token_response(
                    json!({"access_token": "a", "token_type": "mac", "id_token": "x.y.z"}),
                )
            }),
        ),
        (
            "non-string token",
            Box::new(|idp| idp.set_token_response(json!({"access_token": 5}))),
        ),
    ];
    for (name, arm) in variants {
        let begun = f.begin("corp", None).await;
        arm(&f.idp);
        let fetched = f.finish("corp", &begun).await;
        assert_failure(&fetched, "PROVIDER_CODE_EXCHANGE_FAILED");
        let rendered = format!("{:?}{}", fetched.headers, fetched.text());
        for secret in [
            SECRET,
            "super-secret-detail",
            "auth-code",
            "invalid_grant",
            &begun.state,
        ] {
            assert!(!rendered.contains(secret), "{name}: {secret}");
        }
    }

    let begun = f.begin("corp", None).await;
    f.idp
        .set_token_response(json!({ "access_token": "only-access" }));
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_ID_TOKEN_INVALID");

    assert_eq!(f.session_count().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_oidc_round_trip_issues_a_normal_server_side_session() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    let begun = f.begin("corp", Some("/files?view=grid")).await;
    f.arm_oidc(&begun, json!({ "email": "ada@example.test" }), &[]);
    let fetched = f
        .callback(
            "corp",
            &format!(
                "code=auth-code&state={}&returnTo=https%3A%2F%2Fevil.example&redirect=/evil",
                begun.state
            ),
            Some(&begun.binding),
        )
        .await;
    let creds = assert_signed_in(&fetched, "/files?view=grid");

    let (method, state, user_id, link_id): (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT auth_method, state, user_id, identity_link_id FROM sessions WHERE token_hash = ?1",
    )
    .bind(digest(&creds.session))
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!((method.as_str(), state.as_str()), ("external", "active"));
    assert_eq!(
        user_id,
        f.user_by_email("ada@example.test").await.to_string()
    );
    assert!(link_id.is_some());

    let me = f
        .stack
        .get("/api/v1/auth/me", Some(&creds.session), 90)
        .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());

    let cookies = fetched.set_cookies().join(";");
    for secret in ["access-token-value", "external-subject", "palmr-client"] {
        assert!(!cookies.contains(secret), "{secret}");
    }
    assert!(!location(&fetched).contains("access-token"));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_oidc_validation_failures_keep_their_classification() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({ "nonce": "someone-elses-nonce" }), &[]);
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_ID_TOKEN_INVALID");

    for (name, set, remove) in [
        (
            "wrong issuer",
            json!({ "iss": "https://evil.example/" }),
            vec![],
        ),
        ("wrong audience", json!({ "aud": "other-client" }), vec![]),
        ("expired", json!({ "exp": 1 }), vec![]),
        (
            "bad at_hash",
            json!({ "at_hash": "AAAAAAAAAAAAAAAAAAAAAA" }),
            vec![],
        ),
    ] {
        let begun = f.begin("corp", None).await;
        f.arm_oidc(&begun, set, &remove);
        assert_failure(&f.finish("corp", &begun).await, "PROVIDER_ID_TOKEN_INVALID");
        assert!(consumed(&f, &begun).await, "{name}");
    }

    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({}), &["sub"]);
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_SUBJECT_MISSING");
    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({ "sub": "" }), &[]);
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_SUBJECT_MISSING");
    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({ "sub": "x".repeat(300) }), &[]);
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_SUBJECT_MISSING");

    let begun = f.begin("corp", None).await;
    let claims = f.claims(
        &begun,
        json!({ "at_hash": at_hash("access-token-value") }),
        &[],
    );
    f.idp.set_token_response(json!({
        "access_token": "access-token-value",
        "id_token": f.idp.sign_rs256(RSA1_KID, &claims),
    }));
    assert_signed_in(&f.finish("corp", &begun).await, "/overview");

    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, 2);
    f.stack.stop().await;
}

fn at_hash(access_token: &str) -> String {
    use base64ct::Encoding;
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(access_token.as_bytes());
    base64ct::Base64UrlUnpadded::encode_string(&digest[..16])
}

#[tokio::test]
async fn it_callback_oauth2_uses_userinfo_with_the_bearer_token() {
    let f = Federation::start().await;
    f.oauth2("social", json!({ "autoProvision": true })).await;

    let fetched = f
        .login_oauth2(
            "social",
            json!({ "sub": "social-1", "email": "sam@example.test", "email_verified": true,
                    "preferred_username": "sam", "name": "Sam Social" }),
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    assert_eq!(
        f.idp.userinfo_authorization().await.as_deref(),
        Some("Bearer oauth-access-token")
    );
    assert_eq!(f.idp.userinfo_cookie().await, None);
    assert_eq!(
        f.links().await.first().map(|link| link.1.as_str()),
        Some("social-1")
    );

    let begun = f.begin("social", None).await;
    f.idp
        .set_token_response(json!({ "access_token": "oauth-access-token" }));
    f.idp.set_userinfo_raw(500, "oops");
    assert_failure(
        &f.finish("social", &begun).await,
        "PROVIDER_USERINFO_FAILED",
    );

    let begun = f.begin("social", None).await;
    f.idp
        .set_userinfo(json!({ "email": "nobody@example.test", "email_verified": true }));
    assert_failure(
        &f.finish("social", &begun).await,
        "PROVIDER_SUBJECT_MISSING",
    );

    let begun = f.begin("social", None).await;
    f.idp.set_userinfo_raw(200, "[]");
    assert_failure(
        &f.finish("social", &begun).await,
        "PROVIDER_USERINFO_FAILED",
    );

    let begun = f.begin("social", None).await;
    f.idp.set_token_response(json!({ "token_type": "bearer" }));
    assert_failure(
        &f.finish("social", &begun).await,
        "PROVIDER_CODE_EXCHANGE_FAILED",
    );
    f.stack.stop().await;
}

fn unverified_matrix() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("false", Some(json!(false))),
        ("missing", None),
        ("null", Some(json!(null))),
        ("zero", Some(json!(0))),
        ("one", Some(json!(1))),
        ("empty string", Some(json!(""))),
        ("string false", Some(json!("false"))),
        ("string no", Some(json!("no"))),
        ("string TRUE", Some(json!("TRUE"))),
        ("string arbitrary", Some(json!("verified"))),
        ("array", Some(json!([true]))),
        ("object", Some(json!({"verified": true}))),
    ]
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R038")]
#[tokio::test]
async fn regression_R038_identity_link_requires_verified_email() {
    let f = Federation::start().await;
    f.oidc("linking", json!({ "allowEmailLinking": true }))
        .await;
    f.oidc(
        "provisioning",
        json!({ "allowEmailLinking": true, "autoProvision": true }),
    )
    .await;
    f.oidc("nolink", json!({ "allowEmailLinking": false }))
        .await;

    let admin_id = f.make_user("boss", "admin@example.test").await;
    f.stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{admin_id}'"
        ))
        .await;

    let mut index = 0;
    for slug in ["linking", "provisioning"] {
        for (name, claim) in unverified_matrix() {
            index += 1;
            let (set, remove): (Value, Vec<&str>) = match claim {
                Some(value) => (
                    json!({ "email": "admin@example.test", "email_verified": value, "sub": format!("s-{index}") }),
                    vec![],
                ),
                None => (
                    json!({ "email": "admin@example.test", "sub": format!("s-{index}") }),
                    vec!["email_verified"],
                ),
            };
            let fetched = f.login_oidc(slug, set, &remove).await;
            assert_failure(&fetched, "PROVIDER_EMAIL_UNVERIFIED");
            assert_eq!(
                f.count("SELECT COUNT(*) FROM identity_links").await,
                0,
                "{slug}/{name}"
            );
            assert_eq!(
                f.count("SELECT COUNT(*) FROM users").await,
                2,
                "{slug}/{name}"
            );
        }
        for (name, set, remove) in [
            (
                "invalid e-mail",
                json!({ "email": "not-an-email", "email_verified": true }),
                vec![],
            ),
            (
                "empty e-mail",
                json!({ "email": "", "email_verified": true }),
                vec![],
            ),
            (
                "missing e-mail",
                json!({ "email_verified": true }),
                vec!["email"],
            ),
            (
                "spaced e-mail",
                json!({ "email": "a b@example.test", "email_verified": true }),
                vec![],
            ),
        ] {
            index += 1;
            let mut set = set;
            set["sub"] = json!(format!("s-{index}"));
            let fetched = f.login_oidc(slug, set, &remove).await;
            assert_failure(&fetched, "PROVIDER_EMAIL_UNVERIFIED");
            assert_eq!(
                f.count("SELECT COUNT(*) FROM identity_links").await,
                0,
                "{slug}/{name}"
            );
        }
    }
    assert_eq!(f.session_count().await, 1);

    let refused = f
        .login_oidc(
            "nolink",
            json!({ "email": "admin@example.test", "email_verified": true, "sub": "nolink-1" }),
            &[],
        )
        .await;
    assert_failure(&refused, "PROVIDER_AUTO_PROVISION_DISABLED");
    assert_eq!(f.count("SELECT COUNT(*) FROM identity_links").await, 0);

    for (name, value) in [
        ("boolean true", json!(true)),
        ("string true", json!("true")),
    ] {
        let email = format!("{}@example.test", name.replace(' ', "-"));
        f.make_user(&name.replace(' ', "-"), &email).await;
        let fetched = f
            .login_oidc(
                "linking",
                json!({ "email": email, "email_verified": value, "sub": format!("ok-{name}") }),
                &[],
            )
            .await;
        assert_signed_in(&fetched, "/overview");
    }
    let methods: Vec<String> = f.links().await.into_iter().map(|link| link.2).collect();
    assert_eq!(methods, ["auto_verified_email", "auto_verified_email"]);
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?1")
        .bind(admin_id.to_string())
        .fetch_one(f.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(role, "admin");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_github_only_a_primary_verified_address_is_eligible() {
    let f = Federation::start().await;
    f.oauth2(
        "github",
        json!({ "preset": "github", "allowEmailLinking": true, "autoProvision": true }),
    )
    .await;
    let victim = f.make_user("victim", "victim@example.test").await;
    let github_user = |email: &str| {
        json!({ "id": 4242, "login": "octocat", "name": "The Octocat", "email": email,
                "avatar_url": "https://avatars.example/octocat.png" })
    };

    f.idp.set_emails(json!([
        { "email": "victim@example.test", "primary": false, "verified": true },
        { "email": "other@example.test", "primary": true, "verified": false }
    ]));
    let fetched = f
        .login_oauth2("github", github_user("victim@example.test"))
        .await;
    assert_failure(&fetched, "PROVIDER_EMAIL_UNVERIFIED");

    f.idp.set_emails(
        json!([{ "email": "victim@example.test", "primary": true, "verified": false }]),
    );
    let begun = f.begin("github", None).await;
    f.idp
        .set_token_response(json!({ "access_token": "gh", "token_type": "bearer" }));
    f.idp.set_userinfo(github_user("victim@example.test"));
    assert_failure(
        &f.finish("github", &begun).await,
        "PROVIDER_EMAIL_UNVERIFIED",
    );

    f.idp.set_emails_status(500);
    let begun = f.begin("github", None).await;
    assert_failure(
        &f.finish("github", &begun).await,
        "PROVIDER_USERINFO_FAILED",
    );

    f.idp.set_emails(json!([
        { "email": "other@example.test", "primary": false, "verified": true },
        { "email": "victim@example.test", "primary": true, "verified": true }
    ]));
    let begun = f.begin("github", None).await;
    let fetched = f.finish("github", &begun).await;
    assert_signed_in(&fetched, "/overview");
    let links = f.links().await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].0, victim.to_string());
    assert_eq!(links[0].1, "4242");
    assert_eq!(links[0].2, "auto_verified_email");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_generic_oauth2_does_not_inherit_github_rules() {
    let f = Federation::start().await;
    f.oauth2("custom", json!({ "allowEmailLinking": true }))
        .await;
    let user = f.make_user("sam", "sam@example.test").await;

    let fetched = f
        .login_oauth2(
            "custom",
            json!({ "sub": "c-1", "email": "sam@example.test", "email_verified": true }),
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    assert_eq!(f.links().await[0].0, user.to_string());
    assert_eq!(f.idp.userinfo_request_count().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_existing_link_always_wins_over_email() {
    let f = Federation::start().await;
    f.oidc(
        "corp",
        json!({ "allowEmailLinking": true, "autoProvision": true }),
    )
    .await;
    let first = f.make_user("first", "first@example.test").await;
    let second = f.make_user("second", "second@example.test").await;
    f.link("corp", first, "external-subject", "active").await;

    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "external-subject", "email": "second@example.test", "email_verified": true }),
            &[],
        )
        .await;
    let creds = assert_signed_in(&fetched, "/overview");
    let owner: String = sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash = ?1")
        .bind(digest(&creds.session))
        .fetch_one(f.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(owner, first.to_string());

    let links = f.links().await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].0, first.to_string());
    assert_ne!(links[0].0, second.to_string());
    assert!(
        links[0].3.is_some(),
        "a successful login stamps last_login_at"
    );
    let stored: (Option<String>, i64, String) = sqlx::query_as(
        "SELECT email_at_link, email_verified_at_link, link_method FROM identity_links",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        stored,
        (Some("old@example.test".to_owned()), 0, "manual".to_owned())
    );
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, 3);

    let again = f
        .login_oidc(
            "corp",
            json!({ "sub": "external-subject", "email": "brand-new@example.test", "email_verified": false }),
            &[],
        )
        .await;
    assert_signed_in(&again, "/overview");
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, 3);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_auto_link_requires_an_active_account_and_a_free_slot() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "allowEmailLinking": true })).await;
    let inactive = f
        .stack
        .user(UserSpec {
            active: false,
            hash: None,
            ..UserSpec::local("gone", "gone@example.test", &password_hash())
        })
        .await;
    let linked = f.make_user("linked", "linked@example.test").await;
    f.link("corp", linked, "already-bound", "active").await;

    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "x-1", "email": "gone@example.test", "email_verified": true }),
            &[],
        )
        .await;
    assert_failure(&fetched, "PROVIDER_AUTO_PROVISION_DISABLED");
    assert_eq!(f.links().await.len(), 1);
    let _ = inactive;

    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "x-2", "email": "linked@example.test", "email_verified": true }),
            &[],
        )
        .await;
    assert_failure(&fetched, "PROVIDER_IDENTITY_ALREADY_LINKED");
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.session_count().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_refusals_do_not_reveal_whether_an_account_exists() {
    let f = Federation::start().await;
    f.oidc(
        "closed",
        json!({ "allowEmailLinking": false, "autoProvision": false }),
    )
    .await;
    f.make_user("known", "known@example.test").await;

    let mut seen = Vec::new();
    for email in ["known@example.test", "unknown@example.test"] {
        let fetched = f
            .login_oidc(
                "closed",
                json!({ "sub": format!("s-{email}"), "email": email, "email_verified": true }),
                &[],
            )
            .await;
        assert_failure(&fetched, "PROVIDER_AUTO_PROVISION_DISABLED");
        let rendered = format!("{:?}{}", fetched.headers, fetched.text());
        assert!(!rendered.contains("known@"));
        assert!(!rendered.contains("example.test/") || rendered.contains("files.example.test"));
        seen.push(location(&fetched));
    }
    assert_eq!(seen[0], seen[1]);
    for email in ["known@example.test", "unknown@example.test"] {
        let fetched = f
            .login_oidc(
                "closed",
                json!({ "sub": format!("u-{email}"), "email": email, "email_verified": false }),
                &[],
            )
            .await;
        assert_failure(&fetched, "PROVIDER_EMAIL_UNVERIFIED");
    }
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_auto_provision_creates_one_sso_only_user_with_its_link() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "new-1", "email": "Grace.Hopper@Example.test", "email_verified": true, "name": "Grace Brewster Hopper" }),
            &[],
        )
        .await;
    assert_signed_in(&fetched, "/overview");

    let row: (
        String,
        String,
        String,
        String,
        Option<String>,
        i64,
        String,
        i64,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT email, username, first_name, last_name, password_hash, must_change_password,
                role, is_active, quota_override_mode, id
           FROM users WHERE email_normalized = 'grace.hopper@example.test'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(row.0, "Grace.Hopper@Example.test");
    assert_eq!(row.1, "grace.hopper");
    assert_eq!(
        (row.2.as_str(), row.3.as_str()),
        ("Grace", "Brewster Hopper")
    );
    assert_eq!(row.4, None);
    assert_eq!(row.5, 0);
    assert_eq!(row.6, "user");
    assert_eq!(row.7, 1);
    assert_eq!(row.8, "inherit");
    let preferences: i64 = f
        .count(&format!(
            "SELECT COUNT(*) FROM user_preferences WHERE user_id = '{}'",
            row.9
        ))
        .await;
    assert_eq!(preferences, 1);

    let links = f.links().await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].0, row.9);
    assert_eq!(links[0].2, "auto_provision");
    assert!(links[0].3.is_some());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_auto_provisioned_user_is_never_admin() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    for (index, extra) in [
        json!({ "role": "admin", "roles": ["admin"], "groups": ["admins"], "is_admin": true }),
        json!({ "email": "admin@corp.example", "email_verified": true }),
        json!({ "email": "root@corp.example", "email_verified": true, "admin": "true" }),
    ]
    .into_iter()
    .enumerate()
    {
        let mut set = json!({ "sub": format!("adm-{index}"), "email": format!("candidate{index}@example.test"), "email_verified": true });
        merge(&mut set, extra);
        let fetched = f.login_oidc("corp", set, &[]).await;
        assert_signed_in(&fetched, "/overview");
    }

    let rows: Vec<(String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT u.role, l.link_method, u.password_hash, u.must_change_password, u.is_active
           FROM users u JOIN identity_links l ON l.user_id = u.id",
    )
    .fetch_all(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(
            row,
            &("user".to_owned(), "auto_provision".to_owned(), None, 0, 1)
        );
    }
    assert_eq!(
        f.count("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .await,
        1
    );
    f.stack.stop().await;
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R074")]
#[tokio::test]
async fn regression_R074_external_username_collision_resolution() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    f.make_user("ALICE", "someone@example.test").await;
    f.make_user(
        "\u{ff21}\u{ff4c}\u{ff49}\u{ff43}\u{ff45}-2",
        "other@example.test",
    )
    .await;
    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "a-1", "email": "alice@corp.example", "email_verified": true }),
            &[],
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    let username: String = sqlx::query_scalar(
        "SELECT username FROM users WHERE email_normalized = 'alice@corp.example'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        username, "alice-3",
        "NFKC and case-insensitive collisions advance the suffix"
    );

    for suffix in 4..=50 {
        f.make_user(
            &format!("alice-{suffix}"),
            &format!("filler{suffix}@example.test"),
        )
        .await;
    }
    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "a-2", "email": "ALICE@elsewhere.example", "email_verified": true }),
            &[],
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    let username: String = sqlx::query_scalar(
        "SELECT username FROM users WHERE email_normalized = 'alice@elsewhere.example'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    let (prefix, suffix) = username.rsplit_once('-').unwrap();
    assert_eq!(prefix, "alice");
    assert_eq!(suffix.len(), 8);
    assert!(suffix
        .bytes()
        .all(|byte| matches!(byte, b'a'..=b'z' | b'2'..=b'7')));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_username_exhaustion_fails_with_the_named_code_and_creates_nothing() {
    let f = Federation::start().await;
    let provider = f.oidc("corp", json!({ "autoProvision": true })).await;
    let _ = provider;

    for candidate in deterministic_candidates("alice") {
        f.make_user(&candidate, &format!("{candidate}@taken.test"))
            .await;
    }
    let entropy = [7_u8; RANDOM_SUFFIX_BYTES];
    f.make_user(&random_candidate("alice", &entropy), "random@taken.test")
        .await;
    let users_before = f.count("SELECT COUNT(*) FROM users").await;

    let record = repo::find_by_slug(f.stack.pools.reader(), "corp")
        .await
        .unwrap()
        .unwrap();
    let identity = verified_identity("exhausted-1", "alice@corp.example");
    let outcome = f
        .stack
        .pools
        .write_tx(&f.stack.clock, "test.exhaust", async |tx| {
            let input = ResolveInput {
                provider: &record.provider,
                identity: &identity,
                clock: &f.stack.clock,
                audit: f.stack.providers.audit(),
                client: &crate::features::audit::model::ClientMetadata::none(),
                locale: f.stack.settings.handle().load().default_locale(),
            };
            let email = identity.verified_email().unwrap();
            provision_with(tx, &input, &email, &|| Ok(entropy)).await
        })
        .await;
    let error = outcome.unwrap_err();
    assert_eq!(
        error.code(),
        crate::domain::error_code::ErrorCode::AuthExternalUsernameUnavailable
    );
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, users_before);
    assert_eq!(f.count("SELECT COUNT(*) FROM identity_links").await, 0);
    f.stack.stop().await;
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R039")]
#[tokio::test]
async fn regression_R039_external_login_enforces_account_state() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let hash = password_hash();

    let inactive = f
        .stack
        .user(UserSpec {
            active: false,
            hash: None,
            ..UserSpec::local("inert", "inert@example.test", &hash)
        })
        .await;
    f.link("corp", inactive, "inactive-sub", "active").await;
    let fetched = f
        .login_oidc("corp", json!({ "sub": "inactive-sub" }), &[])
        .await;
    assert_failure(&fetched, "AUTH_ACCOUNT_INACTIVE");

    let suspended = f.make_user("parked", "parked@example.test").await;
    f.link("corp", suspended, "suspended-sub", "suspended")
        .await;
    assert_failure(
        &f.login_oidc("corp", json!({ "sub": "suspended-sub", "email": "parked@example.test", "email_verified": true }), &[]).await,
        "AUTH_ACCOUNT_INACTIVE",
    );
    assert_eq!(
        f.links().await.len(),
        2,
        "a suspended subject is never rebound or re-created"
    );

    let locked = f.make_user("locked", "locked@example.test").await;
    f.link("corp", locked, "locked-sub", "active").await;
    f.stack
        .execute(&format!(
            "INSERT INTO account_lockouts (user_id, failed_count, first_failed_at, last_failed_at,
                                           locked_until, lock_count, updated_at)
             VALUES ('{locked}', 5, '2026-09-25T11:59:00.000Z', '2026-09-25T12:00:00.000Z',
                     '2099-01-01T00:00:00.000Z', 1, '2026-09-25T12:00:00.000Z')"
        ))
        .await;
    assert_failure(
        &f.login_oidc("corp", json!({ "sub": "locked-sub" }), &[])
            .await,
        "AUTH_LOCKED",
    );

    for (_, _, _, last) in f.links().await {
        assert!(
            last.is_none(),
            "a refused gate never marks the link as used"
        );
    }
    assert_eq!(f.session_count().await, 1);

    let forced = f
        .stack
        .user(UserSpec {
            must_change_password: true,
            ..UserSpec::local("forced", "forced@example.test", &hash)
        })
        .await;
    f.link("corp", forced, "forced-sub", "active").await;
    let fetched = f
        .login_oidc("corp", json!({ "sub": "forced-sub" }), &[])
        .await;
    let creds = assert_signed_in(&fetched, "/overview");
    let me = f
        .stack
        .get("/api/v1/auth/me", Some(&creds.session), 91)
        .await;
    assert_eq!(me.json()["restriction"], "must_change_password");

    f.stack
        .setting("two_factor_required", "boolean", "true")
        .await;
    let hybrid = f
        .stack
        .user(UserSpec::local("hybrid", "hybrid@example.test", &hash))
        .await;
    f.link("corp", hybrid, "hybrid-sub", "active").await;
    let fetched = f
        .login_oidc("corp", json!({ "sub": "hybrid-sub" }), &[])
        .await;
    let creds = assert_signed_in(&fetched, "/overview");
    let me = f
        .stack
        .get("/api/v1/auth/me", Some(&creds.session), 92)
        .await;
    assert_eq!(me.json()["restriction"], "mfa_enrollment_required");

    let sso = f.make_user("sso", "sso@example.test").await;
    f.link("corp", sso, "sso-sub", "active").await;
    let fetched = f.login_oidc("corp", json!({ "sub": "sso-sub" }), &[]).await;
    let creds = assert_signed_in(&fetched, "/overview");
    let me = f
        .stack
        .get("/api/v1/auth/me", Some(&creds.session), 93)
        .await;
    assert_eq!(me.json()["restriction"], Value::Null);

    f.stack
        .setting("two_factor_required", "boolean", "false")
        .await;
    let totp = f
        .stack
        .user(UserSpec::local("totp", "totp@example.test", &hash))
        .await;
    f.stack
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{totp}'"
        ))
        .await;
    f.link("corp", totp, "totp-sub", "active").await;
    let fetched = f
        .login_oidc("corp", json!({ "sub": "totp-sub" }), &[])
        .await;
    assert_signed_in(&fetched, "/overview");
    let state: (String, String) =
        sqlx::query_as("SELECT state, auth_method FROM sessions WHERE user_id = ?1")
            .bind(totp.to_string())
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(state, ("active".to_owned(), "external".to_owned()));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_concurrent_callbacks_for_one_subject_bind_it_once() {
    let f = Federation::start().await;
    f.oauth2("social", json!({ "allowEmailLinking": true }))
        .await;
    f.make_user("shared", "shared@example.test").await;

    let first = f.begin("social", None).await;
    let second = f.begin("social", None).await;
    f.idp
        .set_token_response(json!({ "access_token": "oauth-access-token" }));
    f.idp.set_userinfo(json!({
        "sub": "raced", "email": "shared@example.test", "email_verified": true
    }));
    let results =
        futures_util::future::join(f.finish("social", &first), f.finish("social", &second)).await;
    assert_signed_in(&results.0, "/overview");
    assert_signed_in(&results.1, "/overview");
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM audit_events WHERE action = 'IDENTITY_LINK_CREATED'")
            .await,
        1
    );
    assert_eq!(f.session_count().await, 3);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_link_insert_conflicts_resolve_deterministically() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let one = f.make_user("one", "one@example.test").await;
    let two = f.make_user("two", "two@example.test").await;
    let record = repo::find_by_slug(f.stack.pools.reader(), "corp")
        .await
        .unwrap()
        .unwrap();
    let identity = verified_identity("subject-x", "one@example.test");
    let client = crate::features::audit::model::ClientMetadata::none();

    let outcomes = f
        .stack
        .pools
        .write_tx(&f.stack.clock, "test.conflicts", async |tx| {
            let input = ResolveInput {
                provider: &record.provider,
                identity: &identity,
                clock: &f.stack.clock,
                audit: f.stack.providers.audit(),
                client: &client,
                locale: f.stack.settings.handle().load().default_locale(),
            };
            let users = [one, two];
            let mut out = Vec::new();
            for user_id in users {
                let user = crate::features::users::repo::find_by_id_in_tx(tx, user_id)
                    .await
                    .unwrap()
                    .unwrap();
                let email = identity.verified_email().unwrap();
                out.push(
                    create_link(tx, &input, &user, &email, LinkMethod::AutoVerifiedEmail).await?,
                );
            }
            Ok::<_, crate::features::identity_providers::error::ExternalLoginError>(out)
        })
        .await
        .unwrap();
    assert!(matches!(outcomes[0], LinkInsert::Created(_)));
    match &outcomes[1] {
        LinkInsert::SubjectTaken(link) => assert_eq!(link.user_id, one),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.links().await.len(), 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_audit_failure_rolls_back_the_link_and_the_user() {
    let f = Federation::start().await;
    f.oidc(
        "corp",
        json!({ "allowEmailLinking": true, "autoProvision": true }),
    )
    .await;
    f.make_user("target", "target@example.test").await;
    f.stack
        .execute(
            "CREATE TRIGGER fail_link_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'IDENTITY_LINK_CREATED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let users_before = f.count("SELECT COUNT(*) FROM users").await;
    let sessions_before = f.session_count().await;

    let linked = f
        .login_oidc(
            "corp",
            json!({ "sub": "t-1", "email": "target@example.test", "email_verified": true }),
            &[],
        )
        .await;
    assert_failure(&linked, "INTERNAL_ERROR");
    let provisioned = f
        .login_oidc(
            "corp",
            json!({ "sub": "t-2", "email": "fresh@example.test", "email_verified": true }),
            &[],
        )
        .await;
    assert_failure(&provisioned, "INTERNAL_ERROR");

    assert_eq!(f.links().await.len(), 0);
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, users_before);
    assert_eq!(f.session_count().await, sessions_before);
    assert_eq!(f.count("SELECT COUNT(*) FROM audit_events WHERE action IN ('USER_CREATED') AND actor_label = 'fresh'").await, 0);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_audit_records_link_user_and_login_without_secrets() {
    let mut f = Federation::start().await;
    f.oidc(
        "corp",
        json!({ "allowEmailLinking": true, "autoProvision": true }),
    )
    .await;
    f.make_user("target", "target@example.test").await;

    for (sub, email) in [
        ("a-1", "target@example.test"),
        ("a-2", "fresh@example.test"),
    ] {
        let fetched = f
            .login_oidc(
                "corp",
                json!({ "sub": sub, "email": email, "email_verified": true }),
                &[],
            )
            .await;
        assert_signed_in(&fetched, "/overview");
    }
    f.stack.flush_audit().await;
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT action, actor_type, metadata_json FROM audit_events
          WHERE action IN ('IDENTITY_LINK_CREATED', 'USER_CREATED', 'LOGIN_SUCCEEDED')
            AND (actor_label IN ('target', 'fresh') OR actor_label LIKE 'fresh%')
          ORDER BY id",
    )
    .fetch_all(f.stack.pools.reader().executor())
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
    assert!(actions.contains(&"IDENTITY_LINK_CREATED"));
    assert!(actions.contains(&"USER_CREATED"));
    assert!(actions.contains(&"LOGIN_SUCCEEDED"));
    let provider = f.provider_id("corp").await;
    let via: Vec<String> = rows
        .iter()
        .filter(|row| row.0 == "IDENTITY_LINK_CREATED")
        .map(|row| {
            let metadata: Value = serde_json::from_str(row.2.as_deref().unwrap()).unwrap();
            assert_eq!(metadata["provider_id"], provider);
            metadata["via"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(via, ["verified_email", "auto_provision"]);
    let login: Value = serde_json::from_str(
        rows.iter()
            .find(|row| row.0 == "LOGIN_SUCCEEDED")
            .unwrap()
            .2
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(login["method"], "external");

    let all: Vec<String> =
        sqlx::query_scalar("SELECT COALESCE(metadata_json, '') FROM audit_events")
            .fetch_all(f.stack.pools.reader().executor())
            .await
            .unwrap();
    for text in all {
        for secret in ["access-token-value", "auth-code", SECRET, "palmr-client"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_enqueues_a_durable_avatar_job_without_fetching_inline() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;

    let fetched = f
        .login_oidc(
            "corp",
            json!({ "sub": "av-1", "email": "av@example.test", "email_verified": true,
                    "picture": "https://cdn.example/avatar.png" }),
            &[],
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    let jobs: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT kind, payload_json, dedup_key FROM jobs WHERE kind = 'avatar.fetch_external'",
    )
    .fetch_all(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(jobs.len(), 1);
    let payload: Value = serde_json::from_str(&jobs[0].1).unwrap();
    let object = payload.as_object().unwrap();
    assert_eq!(object["url"], "https://cdn.example/avatar.png");
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort();
    assert_eq!(keys, ["identityLinkId", "providerId", "url", "userId"]);
    for forbidden in ["token", "secret", "code", "verifier", "nonce"] {
        assert!(!jobs[0].1.to_lowercase().contains(forbidden), "{forbidden}");
    }
    assert!(jobs[0]
        .2
        .as_deref()
        .unwrap()
        .starts_with("avatar.fetch_external:"));
    let avatar: Option<String> = sqlx::query_scalar(
        "SELECT avatar_storage_object_id FROM users WHERE email_normalized = 'av@example.test'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(
        avatar.is_none(),
        "no URL or object is persisted by the callback"
    );

    let again = f
        .login_oidc(
            "corp",
            json!({ "sub": "av-1", "picture": "https://cdn.example/avatar.png" }),
            &[],
        )
        .await;
    assert_signed_in(&again, "/overview");
    assert_eq!(
        f.count("SELECT COUNT(*) FROM jobs WHERE kind = 'avatar.fetch_external'")
            .await,
        1
    );

    let bare = f
        .login_oidc(
            "corp",
            json!({ "sub": "av-2", "email": "bare@example.test", "email_verified": true }),
            &["picture"],
        )
        .await;
    assert_signed_in(&bare, "/overview");
    for hostile in [
        "javascript:alert(1)",
        "http://insecure.example/a.png",
        "data:image/png;base64,AAAA",
    ] {
        let fetched = f
            .login_oidc("corp", json!({ "sub": "av-3", "email": "hostile@example.test", "email_verified": true, "picture": hostile }), &[])
            .await;
        assert_signed_in(&fetched, "/overview");
    }
    assert_eq!(
        f.count("SELECT COUNT(*) FROM jobs WHERE kind = 'avatar.fetch_external'")
            .await,
        1
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_frontegg_avatar_claim_is_mapped_and_optional() {
    let f = Federation::start().await;
    f.oidc(
        "frontegg",
        json!({ "preset": "frontegg", "autoProvision": true }),
    )
    .await;

    let with = f
        .login_oidc(
            "frontegg",
            json!({ "sub": "fe-1", "email": "fe1@example.test", "email_verified": true,
                    "profilePictureUrl": "https://corp.frontegg.example/avatar.png" }),
            &["picture"],
        )
        .await;
    assert_signed_in(&with, "/overview");
    let payload: String =
        sqlx::query_scalar("SELECT payload_json FROM jobs WHERE kind = 'avatar.fetch_external'")
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert!(payload.contains("https://corp.frontegg.example/avatar.png"));

    let without = f
        .login_oidc(
            "frontegg",
            json!({ "sub": "fe-2", "email": "fe2@example.test", "email_verified": true }),
            &["picture", "profilePictureUrl"],
        )
        .await;
    assert_signed_in(&without, "/overview");
    assert_eq!(
        f.count("SELECT COUNT(*) FROM jobs WHERE kind = 'avatar.fetch_external'")
            .await,
        1
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_discord_avatar_hash_becomes_a_cdn_url_job() {
    let f = Federation::start().await;
    f.oauth2(
        "discord",
        json!({ "preset": "discord", "autoProvision": true, "allowEmailLinking": true }),
    )
    .await;
    let fetched = f
        .login_oauth2(
            "discord",
            json!({ "id": "80351110224678912", "username": "nelly", "global_name": "Nelly",
                    "avatar": "8342729096ea3675442027381ff50dfe",
                    "email": "nelly@example.test", "verified": true }),
        )
        .await;
    assert_signed_in(&fetched, "/overview");
    let payload: String =
        sqlx::query_scalar("SELECT payload_json FROM jobs WHERE kind = 'avatar.fetch_external'")
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert!(payload.contains(
        "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png"
    ));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_does_not_hold_the_sqlite_writer_during_network_calls() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({}), &[]);
    f.idp.set_token_delay(Some(StdDuration::from_millis(1500)));

    let started = Instant::now();
    let (callback, write_finished) = tokio::join!(f.finish("corp", &begun), async {
        tokio::time::sleep(StdDuration::from_millis(300)).await;
        f.stack
            .execute("UPDATE app_settings SET updated_at = updated_at WHERE key = 'auth_providers_enabled'")
            .await;
        started.elapsed()
    });
    assert_signed_in(&callback, "/overview");
    assert!(
        write_finished < StdDuration::from_millis(1200),
        "an unrelated write waited {write_finished:?} for the token exchange"
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_global_toggle_and_disabled_provider_are_refused_before_exchange() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({}), &[]);
    f.stack
        .execute("UPDATE identity_providers SET is_enabled = 0")
        .await;
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_DISABLED");
    assert!(f.idp.token_requests().await.is_empty());

    f.stack
        .execute("UPDATE identity_providers SET is_enabled = 1")
        .await;
    let begun = f.begin("corp", None).await;
    f.stack
        .setting("auth_providers_enabled", "boolean", "false")
        .await;
    assert_failure(&f.finish("corp", &begun).await, "PROVIDER_DISABLED");
    assert!(f.idp.token_requests().await.is_empty());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_replaces_a_presented_session_and_clears_the_binding_cookie() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let begun = f.begin("corp", None).await;
    f.arm_oidc(&begun, json!({ "email": "swap@example.test" }), &[]);
    let fetched = f
        .stack
        .send(with_peer(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "{PROVIDERS}/corp/callback?code=c&state={}",
                    begun.state
                ))
                .header(
                    COOKIE,
                    format!(
                        "palmr_oauth={}; palmr_session={}",
                        begun.binding, f.admin.session
                    ),
                )
                .body(Body::empty())
                .unwrap(),
            200,
        ))
        .await;
    assert_signed_in(&fetched, "/overview");
    let (state, reason) = session_state(&f.stack, &f.admin.session).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("revoked", Some("rotated"))
    );
    f.stack.stop().await;
}

#[test]
fn it_callback_route_policy_is_public_with_the_login_rate_limit() {
    use crate::features::identity_providers::routes::CALLBACK_ROUTE;

    assert_eq!(
        CALLBACK_ROUTE.auth(),
        crate::app::auth_class::AuthClass::Public
    );
    assert_eq!(
        CALLBACK_ROUTE.rate_limit(),
        crate::app::router::RateLimitClass::AuthLogin
    );
}

#[tokio::test]
async fn it_callback_post_and_preflight_methods_are_not_the_callback() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let fetched = f
        .stack
        .post_json(&format!("{PROVIDERS}/corp/callback"), "{}", 98, None)
        .await;
    assert!(matches!(
        fetched.status,
        StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_FOUND
    ));
    let _ = Call::new(Method::GET, "/", &f.admin);
    f.stack.stop().await;
}
