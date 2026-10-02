pub mod support;

use std::sync::Arc;
use std::time::Duration;

use palmr_server::identity_providers::http_client::ProviderHttpClient;
use palmr_server::identity_providers::jwks::JwksError;
use palmr_server::identity_providers::model::{ClaimMapping, ProviderId};
use palmr_server::identity_providers::oidc::{
    IdTokenError, IdTokenRequest, IdTokenValidator, JwsAlgorithm, ValidationPurpose,
    DEFAULT_ALLOWED_ALGORITHMS,
};
use palmr_server::{ErrorCode, TestClock};
use serde_json::{json, Value};
use support::mock_idp::{self, MockIdp};
use time::macros::datetime;

/// The fixed wall clock matching [`mock_idp::MOCK_NOW`].
fn fixed_clock() -> TestClock {
    TestClock::new(datetime!(2027-01-15 08:00:00 UTC))
}

struct Fixture {
    idp: MockIdp,
    clock: TestClock,
    validator: IdTokenValidator,
    provider_id: ProviderId,
    mapping: ClaimMapping,
    issuer: String,
    jwks_uri: String,
}

impl Fixture {
    async fn start() -> Self {
        let idp = MockIdp::start().await;
        let clock = fixed_clock();
        let validator = IdTokenValidator::new(Arc::new(clock.clone()), ProviderHttpClient::new());
        let provider_id = ProviderId::generate(&clock);
        let issuer = idp.issuer().to_owned();
        let jwks_uri = idp.jwks_uri();
        Self {
            idp,
            clock,
            validator,
            provider_id,
            mapping: ClaimMapping::standard(),
            issuer,
            jwks_uri,
        }
    }

    fn request<'a>(
        &'a self,
        token: &'a str,
        purpose: ValidationPurpose,
        access_token: Option<&'a str>,
    ) -> IdTokenRequest<'a> {
        IdTokenRequest {
            provider_id: self.provider_id,
            issuer: &self.issuer,
            client_id: mock_idp::CLIENT_ID,
            jwks_uri: &self.jwks_uri,
            allowed_algorithms: &DEFAULT_ALLOWED_ALGORITHMS,
            expected_nonce: mock_idp::NONCE,
            purpose,
            access_token,
            claims: &self.mapping,
            token,
        }
    }

    fn claims(&self) -> Value {
        self.idp.valid_claims(mock_idp::MOCK_NOW, mock_idp::NONCE)
    }

    async fn validate(&self, token: &str) -> Result<(), IdTokenError> {
        self.validator
            .validate(&self.request(token, ValidationPurpose::Login, None))
            .await
            .map(|_| ())
    }
}

#[tokio::test]
async fn it_id_token_rejects_none_alg() {
    let fixture = Fixture::start().await;
    let token = fixture.idp.sign_none(mock_idp::RSA1_KID, &fixture.claims());

    let error = fixture
        .validator
        .validate(&fixture.request(&token, ValidationPurpose::Login, None))
        .await
        .unwrap_err();

    assert_eq!(error, IdTokenError::AlgorithmRejected);
    assert_eq!(error.api_code(), ErrorCode::ProviderIdTokenInvalid);
    // `none` is rejected before any key handling, so nothing was fetched.
    assert_eq!(fixture.idp.jwks_request_count().await, 0);
}

#[tokio::test]
async fn it_id_token_rejects_wrong_iss_aud() {
    let fixture = Fixture::start().await;

    let mut wrong_issuer = fixture.claims();
    wrong_issuer["iss"] = json!("https://evil.example/");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &wrong_issuer);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::IssuerMismatch)
    );

    let mut wrong_scalar = fixture.claims();
    wrong_scalar["aud"] = json!("some-other-client");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &wrong_scalar);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::AudienceMismatch)
    );

    let mut wrong_array = fixture.claims();
    wrong_array["aud"] = json!(["another-client", "some-other-client"]);
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &wrong_array);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::AudienceMismatch)
    );

    let mut wrong_azp = fixture.claims();
    wrong_azp["aud"] = json!(["another-client", mock_idp::CLIENT_ID]);
    wrong_azp["azp"] = json!("not-the-client");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &wrong_azp);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::AzpMismatch)
    );
}

#[tokio::test]
async fn it_id_token_accepts_valid_algorithms_and_rejects_others() {
    let fixture = Fixture::start().await;

    for (kid, token) in [
        (
            mock_idp::RSA1_KID,
            fixture
                .idp
                .sign_rs256(mock_idp::RSA1_KID, &fixture.claims()),
        ),
        (
            mock_idp::EC1_KID,
            fixture.idp.sign_es256(mock_idp::EC1_KID, &fixture.claims()),
        ),
        (
            mock_idp::RSA1_KID,
            fixture
                .idp
                .sign_ps256(mock_idp::RSA1_KID, &fixture.claims()),
        ),
    ] {
        fixture
            .validator
            .validate(&fixture.request(&token, ValidationPurpose::Login, None))
            .await
            .unwrap_or_else(|error| panic!("{kid} should verify: {error:?}"));
    }

    let token = fixture
        .idp
        .sign_rs256(mock_idp::RSA1_KID, &fixture.claims());
    let restricted = IdTokenRequest {
        allowed_algorithms: &[JwsAlgorithm::Es256],
        ..fixture.request(&token, ValidationPurpose::Login, None)
    };
    assert_eq!(
        fixture.validator.validate(&restricted).await.unwrap_err(),
        IdTokenError::AlgorithmRejected
    );

    let confusion = fixture.idp.sign_hs256(
        mock_idp::RSA1_KID,
        &fixture.claims(),
        fixture.idp.rsa_public_pem().as_bytes(),
    );
    assert_eq!(
        fixture.validate(&confusion).await,
        Err(IdTokenError::AlgorithmRejected)
    );
}

#[tokio::test]
async fn it_id_token_rejects_invalid_signature_and_unknown_kid() {
    let fixture = Fixture::start().await;

    let forged = fixture
        .idp
        .sign_rs256_with_secondary(mock_idp::RSA1_KID, &fixture.claims());
    assert_eq!(
        fixture.validate(&forged).await,
        Err(IdTokenError::SignatureInvalid)
    );

    let unknown = fixture.idp.sign_rs256("ghost-kid", &fixture.claims());
    assert_eq!(
        fixture.validate(&unknown).await,
        Err(IdTokenError::Jwks(JwksError::KeyNotFound))
    );
}

#[tokio::test]
async fn it_jwks_refetch_rate_limited() {
    let fixture = Fixture::start().await;
    let claims = fixture.claims();

    let valid = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &claims);
    fixture.validate(&valid).await.unwrap();
    assert_eq!(fixture.idp.jwks_request_count().await, 1);

    // A cached key never refetches.
    fixture.validate(&valid).await.unwrap();
    assert_eq!(fixture.idp.jwks_request_count().await, 1);

    // An unknown key inside the window is not allowed to refetch.
    let unknown = fixture.idp.sign_rs256("ghost-kid", &claims);
    assert_eq!(
        fixture.validate(&unknown).await,
        Err(IdTokenError::Jwks(JwksError::KeyNotFound))
    );
    assert_eq!(fixture.idp.jwks_request_count().await, 1);

    // The injected clock proves the window without a real sixty-second sleep.
    fixture.clock.advance(Duration::from_secs(61));
    assert_eq!(
        fixture.validate(&unknown).await,
        Err(IdTokenError::Jwks(JwksError::KeyNotFound))
    );
    assert_eq!(fixture.idp.jwks_request_count().await, 2);
}

#[tokio::test]
async fn it_jwks_rotated_key_succeeds_after_refresh() {
    let fixture = Fixture::start().await;

    let first = fixture
        .idp
        .sign_rs256(mock_idp::RSA1_KID, &fixture.claims());
    fixture.validate(&first).await.unwrap();

    fixture.idp.use_rotated_jwks();
    fixture.clock.advance(Duration::from_secs(61));

    let rotated = fixture
        .idp
        .sign_rs256(mock_idp::RSA2_KID, &fixture.claims());
    fixture.validate(&rotated).await.unwrap();
    assert_eq!(fixture.idp.jwks_request_count().await, 2);
}

#[tokio::test]
async fn it_jwks_refresh_allowance_is_per_provider() {
    let fixture = Fixture::start().await;
    let claims = fixture.claims();

    let valid = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &claims);
    fixture.validate(&valid).await.unwrap();
    assert_eq!(fixture.idp.jwks_request_count().await, 1);

    let unknown = fixture.idp.sign_rs256("ghost-kid", &claims);
    // Provider A is inside its window and may not refetch.
    assert_eq!(
        fixture.validate(&unknown).await,
        Err(IdTokenError::Jwks(JwksError::KeyNotFound))
    );
    assert_eq!(fixture.idp.jwks_request_count().await, 1);

    // Provider B has its own allowance and does fetch.
    let other_clock = fixed_clock();
    let other_id = ProviderId::generate(&other_clock);
    let request = IdTokenRequest {
        provider_id: other_id,
        ..fixture.request(&unknown, ValidationPurpose::Login, None)
    };
    assert_eq!(
        fixture.validator.validate(&request).await.unwrap_err(),
        IdTokenError::Jwks(JwksError::KeyNotFound)
    );
    assert_eq!(fixture.idp.jwks_request_count().await, 2);
}

#[tokio::test]
async fn it_jwks_concurrent_unknown_kid_does_not_stampede() {
    let fixture = Fixture::start().await;
    let unknown = fixture.idp.sign_rs256("ghost-kid", &fixture.claims());

    let requests: Vec<_> = (0..8)
        .map(|_| fixture.request(&unknown, ValidationPurpose::Login, None))
        .collect();
    let outcomes = futures_util::future::join_all(
        requests
            .iter()
            .map(|request| fixture.validator.validate(request)),
    )
    .await;

    for outcome in outcomes {
        assert_eq!(
            outcome.unwrap_err(),
            IdTokenError::Jwks(JwksError::KeyNotFound)
        );
    }
    // Single-flight per provider collapses the concurrent misses into one fetch.
    assert_eq!(fixture.idp.jwks_request_count().await, 1);
}

#[tokio::test]
async fn it_jwks_malformed_document_fails_closed() {
    let fixture = Fixture::start().await;
    fixture.idp.set_jwks_raw("{ this is not a JWKS");

    let token = fixture
        .idp
        .sign_rs256(mock_idp::RSA1_KID, &fixture.claims());
    let error = fixture.validate(&token).await.unwrap_err();
    assert_eq!(error, IdTokenError::Jwks(JwksError::Malformed));
    assert_eq!(error.api_code(), ErrorCode::ProviderDiscoveryFailed);
}

#[tokio::test]
async fn it_id_token_time_claim_boundaries_are_enforced() {
    let fixture = Fixture::start().await;

    let mut expired = fixture.claims();
    expired["exp"] = json!(mock_idp::MOCK_NOW - 60);
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &expired);
    assert_eq!(fixture.validate(&token).await, Err(IdTokenError::Expired));

    let mut future_iat = fixture.claims();
    future_iat["iat"] = json!(mock_idp::MOCK_NOW + 60);
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &future_iat);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::IssuedInFuture)
    );

    let mut future_nbf = fixture.claims();
    future_nbf["nbf"] = json!(mock_idp::MOCK_NOW + 61);
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &future_nbf);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::NotYetValid)
    );
}

#[tokio::test]
async fn it_id_token_nonce_and_at_hash_are_enforced() {
    let fixture = Fixture::start().await;

    let mut missing_nonce = fixture.claims();
    missing_nonce.as_object_mut().unwrap().remove("nonce");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &missing_nonce);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::NonceMismatch)
    );

    let mut wrong_nonce = fixture.claims();
    wrong_nonce["nonce"] = json!("a-different-nonce");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &wrong_nonce);
    assert_eq!(
        fixture.validate(&token).await,
        Err(IdTokenError::NonceMismatch)
    );

    let mut valid_at_hash = fixture.claims();
    valid_at_hash["at_hash"] = json!("Pxa-1wifRlPl7yG_0oJNfw");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &valid_at_hash);
    fixture
        .validator
        .validate(&fixture.request(&token, ValidationPurpose::Login, Some("access-token")))
        .await
        .unwrap();

    let mut invalid_at_hash = fixture.claims();
    invalid_at_hash["at_hash"] = json!("AAAAAAAAAAAAAAAAAAAAAA");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &invalid_at_hash);
    assert_eq!(
        fixture
            .validator
            .validate(&fixture.request(&token, ValidationPurpose::Login, Some("access-token")))
            .await
            .unwrap_err(),
        IdTokenError::AtHashMismatch
    );
}

#[tokio::test]
async fn it_id_token_reauth_requires_fresh_auth_time() {
    let fixture = Fixture::start().await;

    let mut no_auth_time = fixture.claims();
    no_auth_time.as_object_mut().unwrap().remove("auth_time");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &no_auth_time);
    assert_eq!(
        fixture
            .validator
            .validate(&fixture.request(&token, ValidationPurpose::Reauth, None))
            .await
            .unwrap_err(),
        IdTokenError::AuthTimeInvalid
    );
    // The same token is fine for ordinary login.
    fixture.validate(&token).await.unwrap();

    let mut stale = fixture.claims();
    stale["auth_time"] = json!(mock_idp::MOCK_NOW - 361);
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &stale);
    assert_eq!(
        fixture
            .validator
            .validate(&fixture.request(&token, ValidationPurpose::Reauth, None))
            .await
            .unwrap_err(),
        IdTokenError::AuthTimeInvalid
    );

    let fresh = fixture
        .idp
        .sign_rs256(mock_idp::RSA1_KID, &fixture.claims());
    fixture
        .validator
        .validate(&fixture.request(&fresh, ValidationPurpose::Reauth, None))
        .await
        .unwrap();
}

#[tokio::test]
async fn it_id_token_malformed_tokens_are_rejected() {
    let fixture = Fixture::start().await;
    for token in ["", "not-a-jwt", "a.b", "a.b.c.d", "!!!.???.###"] {
        assert_eq!(
            fixture.validate(token).await,
            Err(IdTokenError::MalformedToken),
            "{token:?}"
        );
    }
}

#[tokio::test]
async fn it_id_token_missing_subject_is_classified_separately() {
    let fixture = Fixture::start().await;

    // Everything cryptographic and claim-level is valid, but the stable
    // subject is absent: it must not collapse into a token-invalid code.
    let mut missing_subject = fixture.claims();
    missing_subject.as_object_mut().unwrap().remove("sub");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &missing_subject);
    let error = fixture.validate(&token).await.unwrap_err();
    assert_eq!(error, IdTokenError::SubjectMissing);
    assert_eq!(error.api_code(), ErrorCode::ProviderSubjectMissing);
    assert_ne!(error.api_code(), ErrorCode::ProviderIdTokenInvalid);

    // A genuine token failure stays token-invalid.
    let mut bad_nonce = fixture.claims();
    bad_nonce["nonce"] = json!("not-the-nonce");
    let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &bad_nonce);
    let error = fixture.validate(&token).await.unwrap_err();
    assert_eq!(error, IdTokenError::NonceMismatch);
    assert_eq!(error.api_code(), ErrorCode::ProviderIdTokenInvalid);
}

#[tokio::test]
async fn it_id_token_reauth_auth_time_failure_is_recent_auth_required() {
    let fixture = Fixture::start().await;

    let mut cases: Vec<(&str, Value)> = Vec::new();

    let mut missing = fixture.claims();
    missing.as_object_mut().unwrap().remove("auth_time");
    cases.push(("missing", missing));

    let mut stale = fixture.claims();
    stale["auth_time"] = json!(mock_idp::MOCK_NOW - 361);
    cases.push(("stale", stale));

    let mut future = fixture.claims();
    future["auth_time"] = json!(mock_idp::MOCK_NOW + 61);
    cases.push(("future", future));

    for (label, claims) in cases {
        let token = fixture.idp.sign_rs256(mock_idp::RSA1_KID, &claims);
        let error = fixture
            .validator
            .validate(&fixture.request(&token, ValidationPurpose::Reauth, None))
            .await
            .unwrap_err();
        assert_eq!(error, IdTokenError::AuthTimeInvalid, "{label}");
        assert_eq!(
            error.api_code(),
            ErrorCode::AuthRecentAuthRequired,
            "{label}"
        );
        assert_ne!(
            error.api_code(),
            ErrorCode::ProviderIdTokenInvalid,
            "{label}"
        );
    }
}
