//! A deterministic, reusable mock identity provider for M12 integration tests.
//!
//! It is built on `wiremock` and never contacts a real provider. It serves the
//! OIDC discovery document, a JWKS, a token endpoint and a userinfo endpoint,
//! owns local signing keys, mints tokens with configurable claims, and makes
//! every hostile variant a test needs straightforward to produce.
#![allow(
    dead_code,
    reason = "the mock IdP is a reusable M12 test harness; not every helper is used by every test binary"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the mock IdP harness is test-only and may panic on impossible fixture failures"
)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64ct::{Base64UrlUnpadded, Encoding};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const RSA1_PRIVATE_PEM: &str = include_str!("../fixtures/idp/rsa1.pem");
const RSA2_PRIVATE_PEM: &str = include_str!("../fixtures/idp/rsa2.pem");
const RSA1_PUBLIC_PEM: &str = include_str!("../fixtures/idp/rsa1.pub.pem");
const EC1_PRIVATE_PEM: &str = include_str!("../fixtures/idp/ec1.pem");

pub const RSA1_KID: &str = "test-rsa-1";
pub const RSA2_KID: &str = "test-rsa-2";
pub const EC1_KID: &str = "test-ec-1";

/// The mock client identifier; any test client may use it as its `client_id`.
pub const CLIENT_ID: &str = "palmr-client";
/// The nonce the default claim set carries and the validator expects.
pub const NONCE: &str = "expected-nonce";
/// The fixed wall-clock instant the default claim set is built around.
pub const MOCK_NOW: i64 = 1_800_000_000;

const RSA1_N: &str = "2dMhruAEYNPJdYyA0y95kLMVsh55Zr_UqYBLcWpJDLnjLyM2b33HFAk5MtBenNbr8gODMWqDQNdHFzWfmo1m-VqpL39gPuvzWzs-J9DABxydJLAlBxrvqeUayMIp16cILkDOXjkpcuOFe-d97wNd7J8bV0KxtJD3F18W0WDI1O_1KIAltkZ6_Je3OrnCltzNe2lHKj0W62ffuMlFchqZPGFWjXudj890D4xCRUWCzRHPv_gpUQgU1-RcuEq3j1r9zOdBOVO0M1c7MTn2Lp7qTzix5yZD_rDnnS_qRzPYB0aqGXUowa0cjzX3t4GhwK4sMRgCoKSH93WK0rrP-H6xQQ";
const RSA2_N: &str = "rGnuKCi6o2Dj55C5B3wbgLvtnR7LPJ1ITzhmSePh5zVTlUVS206xWG-qpXMuNUu5ISbo2VFl4Q19XKKG41nDik60sX7vSHGFFKQmGW_vPRiM2iIKlJinVnRyJLE6CMBmRtF6DHA0wpugJZLTCmMrVoohftERt15S0OJ8wWmPnMKrMvZ9HUAdOYr9DE9TF3cW1UMWZRuVoft2sKU3rJuoKNL9rOQiNNe_30CQMIcixf1tB_yDENcFd978it3tDikAIUq74GvgpEhFHs4AUN4vUD2bSGdxXLpjrv3ZKt3ftKs2KEY7zG-jjF3fSbARwvFzGa4fJTs7AHQZXbU8FHAXgQ";
const EC1_X: &str = "T3Wzvm9icNgDFo0bC7AhyPXXYQiYVDp71kh6LheffHw";
const EC1_Y: &str = "8-LXWBzxVgHhw2UYpBtuskFDCGhavdQdIme7z5FDfQo";

struct JwksStub {
    status: u16,
    body: Vec<u8>,
}

struct UserinfoStub {
    status: u16,
    body: Vec<u8>,
    delay: Option<Duration>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct MockIdp {
    server: MockServer,
    jwks: Arc<Mutex<JwksStub>>,
    userinfo: Arc<Mutex<UserinfoStub>>,
    token: Arc<Mutex<Value>>,
    issuer: String,
}

impl MockIdp {
    /// Starts a server with a valid OIDC discovery document, a default JWKS
    /// (the primary RSA key and the P-256 key), a verified-email userinfo
    /// profile and a benign token response.
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let issuer = server.uri();
        let jwks = Arc::new(Mutex::new(JwksStub {
            status: 200,
            body: serde_json::to_vec(&default_jwks()).expect("serialize default JWKS"),
        }));
        let userinfo = Arc::new(Mutex::new(UserinfoStub {
            status: 200,
            body: serde_json::to_vec(&verified_userinfo(NONCE)).expect("serialize userinfo"),
            delay: None,
        }));
        let token = Arc::new(Mutex::new(json!({
            "access_token": "access-token",
            "token_type": "Bearer",
            "expires_in": 3600
        })));

        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "userinfo_endpoint": format!("{issuer}/userinfo"),
            "jwks_uri": format!("{issuer}/jwks"),
            "scopes_supported": ["openid", "profile", "email"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
            "response_types_supported": ["code"],
            "id_token_signing_alg_values_supported": ["RS256", "ES256", "PS256"],
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
            .mount(&server)
            .await;

        let jwks_state = Arc::clone(&jwks);
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(move |_request: &Request| {
                let stub = lock(&jwks_state);
                ResponseTemplate::new(stub.status)
                    .set_body_raw(stub.body.clone(), "application/json")
            })
            .mount(&server)
            .await;

        let userinfo_state = Arc::clone(&userinfo);
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(move |_request: &Request| {
                let stub = lock(&userinfo_state);
                let mut response = ResponseTemplate::new(stub.status)
                    .set_body_raw(stub.body.clone(), "application/json");
                if let Some(delay) = stub.delay {
                    response = response.set_delay(delay);
                }
                response
            })
            .mount(&server)
            .await;

        let token_state = Arc::clone(&token);
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(move |_request: &Request| {
                ResponseTemplate::new(200).set_body_json(lock(&token_state).clone())
            })
            .mount(&server)
            .await;

        Self {
            server,
            jwks,
            userinfo,
            token,
            issuer,
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn jwks_uri(&self) -> String {
        format!("{}/jwks", self.issuer)
    }

    pub fn userinfo_endpoint(&self) -> String {
        format!("{}/userinfo", self.issuer)
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}/token", self.issuer)
    }

    pub fn rsa_public_pem(&self) -> &'static str {
        RSA1_PUBLIC_PEM
    }

    /// The standard, otherwise-valid claim set. Tests override single members
    /// to build hostile variants.
    pub fn valid_claims(&self, now: i64, nonce: &str) -> Value {
        json!({
            "iss": self.issuer,
            "aud": CLIENT_ID,
            "sub": "external-subject",
            "exp": now + 300,
            "iat": now,
            "nbf": now,
            "nonce": nonce,
            "email": "user@example.com",
            "email_verified": true,
            "preferred_username": "externaluser",
            "name": "External User",
            "picture": "https://example.com/avatar.png",
            "auth_time": now
        })
    }

    pub fn sign_rs256(&self, kid: &str, claims: &Value) -> String {
        self.sign(Algorithm::RS256, kid, self.rsa_pem(kid), claims)
    }

    pub fn sign_ps256(&self, kid: &str, claims: &Value) -> String {
        self.sign(Algorithm::PS256, kid, self.rsa_pem(kid), claims)
    }

    /// Signs RS256 with the rotated (secondary) key while naming `kid`; used to
    /// prove signature validation rejects a token whose key does not match.
    pub fn sign_rs256_with_secondary(&self, kid: &str, claims: &Value) -> String {
        self.sign(Algorithm::RS256, kid, RSA2_PRIVATE_PEM, claims)
    }

    pub fn sign_es256(&self, kid: &str, claims: &Value) -> String {
        self.sign(Algorithm::ES256, kid, EC1_PRIVATE_PEM, claims)
    }

    /// `alg = none`: header and payload are encoded, the signature is empty.
    pub fn sign_none(&self, kid: &str, claims: &Value) -> String {
        let header = json!({ "alg": "none", "typ": "JWT", "kid": kid });
        let header = Base64UrlUnpadded::encode_string(&serde_json::to_vec(&header).unwrap());
        let payload = Base64UrlUnpadded::encode_string(&serde_json::to_vec(claims).unwrap());
        format!("{header}.{payload}.")
    }

    /// An HMAC token whose secret is caller-supplied, for algorithm-confusion
    /// coverage (typically the RSA public key PEM as the HMAC secret).
    pub fn sign_hs256(&self, kid: &str, claims: &Value, secret: &[u8]) -> String {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(kid.to_owned());
        encode(&header, claims, &EncodingKey::from_secret(secret)).unwrap()
    }

    fn sign(&self, algorithm: Algorithm, kid: &str, pem: &str, claims: &Value) -> String {
        let mut header = Header::new(algorithm);
        header.kid = Some(kid.to_owned());
        let key = match algorithm {
            Algorithm::ES256 => EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
            _ => EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
        };
        encode(&header, claims, &key).unwrap()
    }

    fn rsa_pem(&self, kid: &str) -> &'static str {
        if kid == RSA2_KID {
            RSA2_PRIVATE_PEM
        } else {
            RSA1_PRIVATE_PEM
        }
    }

    pub fn set_jwks(&self, document: Value) {
        let mut stub = lock(&self.jwks);
        stub.status = 200;
        stub.body = serde_json::to_vec(&document).unwrap();
    }

    pub fn set_jwks_raw(&self, body: impl Into<Vec<u8>>) {
        let mut stub = lock(&self.jwks);
        stub.status = 200;
        stub.body = body.into();
    }

    pub fn set_jwks_status(&self, status: u16) {
        lock(&self.jwks).status = status;
    }

    /// Serves the default key set again (primary RSA key and P-256 key).
    pub fn use_default_jwks(&self) {
        self.set_jwks(default_jwks());
    }

    /// Serves only the rotated RSA key (and the P-256 key): the previous `kid`
    /// is gone until a refetch picks this set up.
    pub fn use_rotated_jwks(&self) {
        self.set_jwks(rotated_jwks());
    }

    pub fn set_userinfo(&self, document: Value) {
        let mut stub = lock(&self.userinfo);
        stub.status = 200;
        stub.delay = None;
        stub.body = serde_json::to_vec(&document).unwrap();
    }

    pub fn set_userinfo_raw(&self, status: u16, body: impl Into<Vec<u8>>) {
        let mut stub = lock(&self.userinfo);
        stub.status = status;
        stub.delay = None;
        stub.body = body.into();
    }

    pub fn set_userinfo_delay(&self, delay: Duration) {
        lock(&self.userinfo).delay = Some(delay);
    }

    pub fn set_token_response(&self, document: Value) {
        *lock(&self.token) = document;
    }

    /// The number of outbound `GET /jwks` requests the server has actually
    /// received; this proves the refetch limiter, not an internal timestamp.
    pub async fn jwks_request_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == "/jwks")
            .count()
    }

    /// The `Authorization` header value of the most recent userinfo request.
    pub async fn userinfo_authorization(&self) -> Option<String> {
        self.last_userinfo_header("authorization").await
    }

    /// The `Cookie` header value of the most recent userinfo request, if any.
    pub async fn userinfo_cookie(&self) -> Option<String> {
        self.last_userinfo_header("cookie").await
    }

    async fn last_userinfo_header(&self, name: &str) -> Option<String> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .rfind(|request| request.url.path() == "/userinfo")
            .and_then(|request| request.headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }
}

pub fn default_jwks() -> Value {
    json!({ "keys": [rsa_jwk(RSA1_KID, RSA1_N), ec_jwk()] })
}

pub fn rotated_jwks() -> Value {
    json!({ "keys": [rsa_jwk(RSA2_KID, RSA2_N), ec_jwk()] })
}

pub fn rsa_jwk(kid: &str, modulus: &str) -> Value {
    json!({
        "kty": "RSA",
        "use": "sig",
        "kid": kid,
        "n": modulus,
        "e": "AQAB"
    })
}

pub fn ec_jwk() -> Value {
    json!({
        "kty": "EC",
        "use": "sig",
        "kid": EC1_KID,
        "crv": "P-256",
        "x": EC1_X,
        "y": EC1_Y
    })
}

pub fn verified_userinfo(nonce: &str) -> Value {
    json!({
        "sub": "external-subject",
        "email": "user@example.com",
        "email_verified": true,
        "preferred_username": "externaluser",
        "name": "External User",
        "picture": "https://example.com/avatar.png",
        "nonce": nonce
    })
}

pub fn unverified_userinfo() -> Value {
    json!({
        "sub": "external-subject",
        "email": "user@example.com",
        "email_verified": false,
        "preferred_username": "externaluser",
        "name": "External User"
    })
}

pub fn missing_email_userinfo() -> Value {
    json!({ "sub": "external-subject", "preferred_username": "externaluser" })
}

/// A GitHub-like document where the primary address is public and verified.
pub fn github_like_userinfo() -> Value {
    json!({
        "id": 4242,
        "login": "octocat",
        "name": "The Octocat",
        "email": "octocat@github.example",
        "avatar_url": "https://avatars.example/octocat.png",
        "email_verified": true
    })
}
