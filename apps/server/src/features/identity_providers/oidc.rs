use std::sync::Arc;

use base64ct::{Base64UrlUnpadded, Encoding};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::http_client::ProviderHttpClient;
use super::jwks::{JwksCache, JwksError};
use super::model::{ClaimMapping, ProviderId};
use super::oauth2::ExternalProfile;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;

/// Clock skew tolerated on `exp`, `iat` and `nbf`, and inside the `auth_time`
/// recent-auth window. It applies to those claims only.
pub const CLOCK_SKEW_SECS: i64 = 60;
/// The recent-authentication window for `purpose = reauth`.
pub const REAUTH_WINDOW_SECS: i64 = 300;
/// A defensible upper bound on an ID token; a real one is a few kilobytes.
pub const MAX_ID_TOKEN_BYTES: usize = 64 * 1024;

/// The asymmetric JWS algorithms Palmr accepts on the OIDC path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwsAlgorithm {
    Rs256,
    Es256,
    Ps256,
}

impl JwsAlgorithm {
    pub const ALL: [Self; 3] = [Self::Rs256, Self::Es256, Self::Ps256];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Es256 => "ES256",
            Self::Ps256 => "PS256",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|alg| alg.as_str() == text)
    }

    const fn jsonwebtoken(self) -> Algorithm {
        match self {
            Self::Rs256 => Algorithm::RS256,
            Self::Es256 => Algorithm::ES256,
            Self::Ps256 => Algorithm::PS256,
        }
    }
}

/// The runtime validation policy default: exactly the three accepted
/// algorithms. There is no persisted per-provider allowlist.
pub const DEFAULT_ALLOWED_ALGORITHMS: [JwsAlgorithm; 3] = JwsAlgorithm::ALL;

/// What the authorization request was for. Only `Reauth` requires `auth_time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationPurpose {
    Login,
    Link,
    Reauth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdTokenError {
    TokenTooLarge,
    MalformedToken,
    AlgorithmRejected,
    KidMissing,
    UnusableKey,
    SignatureInvalid,
    IssuerMismatch,
    AudienceMismatch,
    AzpMismatch,
    Expired,
    IssuedInFuture,
    NotYetValid,
    ClaimsMalformed,
    SubjectMissing,
    NonceMismatch,
    AtHashMismatch,
    AuthTimeInvalid,
    Jwks(JwksError),
}

impl IdTokenError {
    /// A stable internal reason for tests and observability. It never contains
    /// any token, key or credential material.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TokenTooLarge => "token_too_large",
            Self::MalformedToken => "malformed_token",
            Self::AlgorithmRejected => "algorithm_rejected",
            Self::KidMissing => "kid_missing",
            Self::UnusableKey => "unusable_key",
            Self::SignatureInvalid => "signature_invalid",
            Self::IssuerMismatch => "issuer_mismatch",
            Self::AudienceMismatch => "audience_mismatch",
            Self::AzpMismatch => "azp_mismatch",
            Self::Expired => "expired",
            Self::IssuedInFuture => "issued_in_future",
            Self::NotYetValid => "not_yet_valid",
            Self::ClaimsMalformed => "claims_malformed",
            Self::SubjectMissing => "subject_missing",
            Self::NonceMismatch => "nonce_mismatch",
            Self::AtHashMismatch => "at_hash_mismatch",
            Self::AuthTimeInvalid => "auth_time_invalid",
            Self::Jwks(error) => error.code(),
        }
    }

    /// The canonical external error code for this failure. The distinctions
    /// are preserved: a missing stable subject and a failed OIDC recent-auth
    /// freshness check are not folded into `PROVIDER_ID_TOKEN_INVALID`, so the
    /// callback layer never has to reverse-map a generic code.
    pub const fn api_code(&self) -> ErrorCode {
        match self {
            Self::SubjectMissing => ErrorCode::ProviderSubjectMissing,
            Self::AuthTimeInvalid => ErrorCode::AuthRecentAuthRequired,
            Self::Jwks(error) => error.api_code(),
            _ => ErrorCode::ProviderIdTokenInvalid,
        }
    }
}

impl std::fmt::Display for IdTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jwks(error) => write!(f, "ID token key lookup failed: {error}"),
            other => write!(f, "the ID token is invalid: {}", other.code()),
        }
    }
}

impl std::error::Error for IdTokenError {}

/// The signed claims Palmr preserves for the callback layer. Account
/// resolution, linking and role assignment are owned by M12-T04.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedIdToken {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub auth_time: Option<i64>,
    pub algorithm: JwsAlgorithm,
    pub key_id: String,
}

impl ValidatedIdToken {
    /// Supplements absent profile fields from a userinfo document. The signed
    /// ID token stays authoritative: identity and security-critical values
    /// (`subject`, `email`, `email_verified`) are never overwritten, and the
    /// non-overriding merge only fills profile fields that are absent.
    pub fn supplemented_with(mut self, userinfo: &ExternalProfile) -> Self {
        if self.username.is_none() {
            self.username.clone_from(&userinfo.username);
        }
        if self.name.is_none() {
            self.name.clone_from(&userinfo.name);
        }
        if self.picture.is_none() {
            self.picture.clone_from(&userinfo.picture);
        }
        self
    }
}

/// Everything the validator needs that is already resolved by the caller. No
/// HTTP request, route or session state is passed in.
#[derive(Debug, Clone, Copy)]
pub struct IdTokenRequest<'a> {
    pub provider_id: ProviderId,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub jwks_uri: &'a str,
    pub allowed_algorithms: &'a [JwsAlgorithm],
    pub expected_nonce: &'a str,
    pub purpose: ValidationPurpose,
    pub access_token: Option<&'a str>,
    pub claims: &'a ClaimMapping,
    pub token: &'a str,
}

#[derive(Clone)]
pub struct IdTokenValidator {
    jwks: JwksCache,
    clock: Arc<dyn Clock>,
}

impl IdTokenValidator {
    pub fn new(clock: Arc<dyn Clock>, client: ProviderHttpClient) -> Self {
        Self {
            jwks: JwksCache::new(Arc::clone(&clock), client),
            clock,
        }
    }

    /// Verifies an OIDC ID token fail-closed in the documented order:
    /// algorithm, `kid`, signature, `iss`, `aud`/`azp`, time claims, nonce,
    /// `at_hash`, then `auth_time` for recent-auth.
    pub async fn validate(
        &self,
        request: &IdTokenRequest<'_>,
    ) -> Result<ValidatedIdToken, IdTokenError> {
        let (header_segment, _, _) = split(request.token)?;
        let header = header_json(header_segment)?;
        let algorithm = select_algorithm(&header, request.allowed_algorithms)?;
        let key_id = select_kid(&header)?;

        let key = self
            .jwks
            .signing_key(request.provider_id, request.jwks_uri, key_id)
            .await
            .map_err(IdTokenError::Jwks)?;
        if !key_matches(&key, algorithm) {
            return Err(IdTokenError::UnusableKey);
        }
        let decoding = DecodingKey::from_jwk(&key).map_err(|_| IdTokenError::UnusableKey)?;
        let claims = verify_signature(request.token, &decoding, algorithm, key_id)?;

        let now = self.clock.now().unix_timestamp();
        let context = ClaimContext {
            issuer: request.issuer,
            client_id: request.client_id,
            expected_nonce: request.expected_nonce,
            purpose: request.purpose,
            access_token: request.access_token,
            mapping: request.claims,
        };
        check_claims(&claims, &context, algorithm, key_id, now)
    }
}

struct ClaimContext<'a> {
    issuer: &'a str,
    client_id: &'a str,
    expected_nonce: &'a str,
    purpose: ValidationPurpose,
    access_token: Option<&'a str>,
    mapping: &'a ClaimMapping,
}

fn split(token: &str) -> Result<(&str, &str, &str), IdTokenError> {
    if token.len() > MAX_ID_TOKEN_BYTES {
        return Err(IdTokenError::TokenTooLarge);
    }
    let mut parts = token.split('.');
    let header = parts.next().ok_or(IdTokenError::MalformedToken)?;
    let payload = parts.next().ok_or(IdTokenError::MalformedToken)?;
    let signature = parts.next().ok_or(IdTokenError::MalformedToken)?;
    if parts.next().is_some() || header.is_empty() || payload.is_empty() {
        return Err(IdTokenError::MalformedToken);
    }
    Ok((header, payload, signature))
}

fn header_json(segment: &str) -> Result<Map<String, Value>, IdTokenError> {
    let bytes = Base64UrlUnpadded::decode_vec(segment).map_err(|_| IdTokenError::MalformedToken)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| IdTokenError::MalformedToken)?;
    value
        .as_object()
        .cloned()
        .ok_or(IdTokenError::MalformedToken)
}

fn select_algorithm(
    header: &Map<String, Value>,
    allowed: &[JwsAlgorithm],
) -> Result<JwsAlgorithm, IdTokenError> {
    let text = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or(IdTokenError::MalformedToken)?;
    // `none` is rejected before any signature handling, and every symmetric
    // HMAC algorithm is rejected on the asymmetric path so a JWKS public key
    // can never be repurposed as an HMAC secret.
    if text == "none" || text.starts_with("HS") {
        return Err(IdTokenError::AlgorithmRejected);
    }
    let algorithm = JwsAlgorithm::parse(text).ok_or(IdTokenError::AlgorithmRejected)?;
    if allowed.contains(&algorithm) {
        Ok(algorithm)
    } else {
        Err(IdTokenError::AlgorithmRejected)
    }
}

fn select_kid(header: &Map<String, Value>) -> Result<&str, IdTokenError> {
    header
        .get("kid")
        .and_then(Value::as_str)
        .filter(|kid| !kid.is_empty())
        .ok_or(IdTokenError::KidMissing)
}

fn key_matches(key: &Jwk, algorithm: JwsAlgorithm) -> bool {
    match (algorithm, &key.algorithm) {
        (JwsAlgorithm::Rs256 | JwsAlgorithm::Ps256, AlgorithmParameters::RSA(_)) => true,
        (JwsAlgorithm::Es256, AlgorithmParameters::EllipticCurve(params)) => {
            params.curve == EllipticCurve::P256
        }
        _ => false,
    }
}

fn verify_signature(
    token: &str,
    key: &DecodingKey,
    algorithm: JwsAlgorithm,
    _key_id: &str,
) -> Result<Value, IdTokenError> {
    let mut validation = Validation::new(algorithm.jsonwebtoken());
    validation.algorithms = vec![algorithm.jsonwebtoken()];
    validation.leeway = 0;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    match decode::<Value>(token, key, &validation) {
        Ok(data) => Ok(data.claims),
        Err(error) => Err(classify_jwt_error(error.kind())),
    }
}

fn classify_jwt_error(kind: &ErrorKind) -> IdTokenError {
    match kind {
        ErrorKind::InvalidToken | ErrorKind::Base64(_) | ErrorKind::Json(_) => {
            IdTokenError::MalformedToken
        }
        ErrorKind::InvalidAlgorithm
        | ErrorKind::InvalidSignature
        | ErrorKind::InvalidEcdsaKey
        | ErrorKind::InvalidRsaKey(_)
        | ErrorKind::InvalidKeyFormat
        | ErrorKind::RsaFailedSigning => IdTokenError::SignatureInvalid,
        _ => IdTokenError::SignatureInvalid,
    }
}

fn check_claims(
    claims: &Value,
    context: &ClaimContext<'_>,
    algorithm: JwsAlgorithm,
    key_id: &str,
    now: i64,
) -> Result<ValidatedIdToken, IdTokenError> {
    if claims.get("iss").and_then(Value::as_str) != Some(context.issuer) {
        return Err(IdTokenError::IssuerMismatch);
    }
    if !audience_contains(claims.get("aud"), context.client_id) {
        return Err(IdTokenError::AudienceMismatch);
    }
    if let Some(azp) = claims.get("azp") {
        if azp.as_str() != Some(context.client_id) {
            return Err(IdTokenError::AzpMismatch);
        }
    }

    let exp = claims
        .get("exp")
        .and_then(Value::as_i64)
        .ok_or(IdTokenError::ClaimsMalformed)?;
    if exp <= now - CLOCK_SKEW_SECS {
        return Err(IdTokenError::Expired);
    }
    if let Some(iat) = optional_time(claims, "iat")? {
        if iat >= now + CLOCK_SKEW_SECS {
            return Err(IdTokenError::IssuedInFuture);
        }
    }
    if let Some(nbf) = optional_time(claims, "nbf")? {
        if nbf > now + CLOCK_SKEW_SECS {
            return Err(IdTokenError::NotYetValid);
        }
    }

    check_nonce(claims, context.expected_nonce)?;
    check_at_hash(claims, context.access_token, algorithm)?;
    let auth_time = check_auth_time(claims, context.purpose, now)?;

    let mapping = context.mapping;
    let subject = text_claim(claims, &mapping.subject).ok_or(IdTokenError::SubjectMissing)?;
    Ok(ValidatedIdToken {
        subject,
        email: text_claim(claims, &mapping.email),
        email_verified: claims
            .get(&mapping.email_verified)
            .and_then(Value::as_bool)
            .unwrap_or(false),
        username: text_claim(claims, &mapping.username),
        name: text_claim(claims, &mapping.name),
        picture: text_claim(claims, &mapping.picture),
        auth_time,
        algorithm,
        key_id: key_id.to_owned(),
    })
}

fn optional_time(claims: &Value, key: &str) -> Result<Option<i64>, IdTokenError> {
    match claims.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or(IdTokenError::ClaimsMalformed),
    }
}

fn audience_contains(audience: Option<&Value>, client_id: &str) -> bool {
    match audience {
        Some(Value::String(value)) => value == client_id,
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(client_id)),
        _ => false,
    }
}

fn check_nonce(claims: &Value, expected: &str) -> Result<(), IdTokenError> {
    if expected.is_empty() {
        return Err(IdTokenError::NonceMismatch);
    }
    let nonce = claims
        .get("nonce")
        .and_then(Value::as_str)
        .filter(|nonce| !nonce.is_empty())
        .ok_or(IdTokenError::NonceMismatch)?;
    if bool::from(expected.as_bytes().ct_eq(nonce.as_bytes())) {
        Ok(())
    } else {
        Err(IdTokenError::NonceMismatch)
    }
}

fn check_at_hash(
    claims: &Value,
    access_token: Option<&str>,
    algorithm: JwsAlgorithm,
) -> Result<(), IdTokenError> {
    let expected = match claims.get("at_hash") {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::String(value)) if !value.is_empty() => value,
        Some(_) => return Err(IdTokenError::AtHashMismatch),
    };
    let access_token = access_token
        .filter(|token| !token.is_empty())
        .ok_or(IdTokenError::AtHashMismatch)?;
    let actual = at_hash(algorithm, access_token);
    if bool::from(expected.as_bytes().ct_eq(actual.as_bytes())) {
        Ok(())
    } else {
        Err(IdTokenError::AtHashMismatch)
    }
}

/// OpenID Connect `at_hash`: the base64url encoding of the left-most half of
/// the hash of the ASCII access token, using the hash of the signing
/// algorithm (SHA-256 for the accepted RS256/ES256/PS256 family).
fn at_hash(algorithm: JwsAlgorithm, access_token: &str) -> String {
    let digest = match algorithm {
        JwsAlgorithm::Rs256 | JwsAlgorithm::Es256 | JwsAlgorithm::Ps256 => {
            Sha256::digest(access_token.as_bytes())
        }
    };
    Base64UrlUnpadded::encode_string(&digest[..digest.len() / 2])
}

fn check_auth_time(
    claims: &Value,
    purpose: ValidationPurpose,
    now: i64,
) -> Result<Option<i64>, IdTokenError> {
    let raw = claims.get("auth_time");
    if purpose != ValidationPurpose::Reauth {
        return match raw {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_i64()
                .map(Some)
                .ok_or(IdTokenError::ClaimsMalformed),
        };
    }
    // Under reauth, a missing, non-numeric, stale or unacceptably future
    // `auth_time` is one semantic failure: recent authentication is required.
    let auth_time = match raw {
        None | Some(Value::Null) => return Err(IdTokenError::AuthTimeInvalid),
        Some(value) => value.as_i64().ok_or(IdTokenError::AuthTimeInvalid)?,
    };
    if auth_time > now + CLOCK_SKEW_SECS {
        return Err(IdTokenError::AuthTimeInvalid);
    }
    if now - auth_time > REAUTH_WINDOW_SECS + CLOCK_SKEW_SECS {
        return Err(IdTokenError::AuthTimeInvalid);
    }
    Ok(Some(auth_time))
}

fn text_claim(claims: &Value, key: &str) -> Option<String> {
    claims
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use base64ct::{Base64UrlUnpadded, Encoding};
    use serde_json::json;

    use super::*;

    const ISSUER: &str = "https://sso.example.com/";
    const CLIENT_ID: &str = "palmr-client";
    const NOW: i64 = 1_800_000_000;

    fn context<'a>(mapping: &'a ClaimMapping, purpose: ValidationPurpose) -> ClaimContext<'a> {
        ClaimContext {
            issuer: ISSUER,
            client_id: CLIENT_ID,
            expected_nonce: "nonce-value",
            purpose,
            access_token: Some("access-token"),
            mapping,
        }
    }

    fn valid_claims() -> Value {
        json!({
            "iss": ISSUER,
            "aud": CLIENT_ID,
            "sub": "u-1",
            "exp": NOW + 300,
            "iat": NOW,
            "nbf": NOW,
            "nonce": "nonce-value",
            "email": "a@example.com",
            "email_verified": true,
            "preferred_username": "alice",
            "name": "Alice",
            "picture": "https://example.com/a.png",
            "auth_time": NOW
        })
    }

    fn accepted(
        claims: &Value,
        purpose: ValidationPurpose,
    ) -> Result<ValidatedIdToken, IdTokenError> {
        let mapping = ClaimMapping::standard();
        let context = context(&mapping, purpose);
        check_claims(claims, &context, JwsAlgorithm::Rs256, "k1", NOW)
    }

    #[test]
    fn unit_oidc_header_algorithm_policy() {
        let allowed = DEFAULT_ALLOWED_ALGORITHMS;
        for accepted in ["RS256", "ES256", "PS256"] {
            let header = json!({"alg": accepted}).as_object().unwrap().clone();
            assert_eq!(
                select_algorithm(&header, &allowed).unwrap().as_str(),
                accepted
            );
        }
        for rejected in ["none", "HS256", "RS512", "ES384", "EdDSA", "", "RS25"] {
            let header = json!({"alg": rejected}).as_object().unwrap().clone();
            assert_eq!(
                select_algorithm(&header, &allowed),
                Err(IdTokenError::AlgorithmRejected),
                "{rejected}"
            );
        }
        let none = json!({"alg": "none"}).as_object().unwrap().clone();
        let only_rs = [JwsAlgorithm::Rs256];
        assert_eq!(
            select_algorithm(&none, &only_rs),
            Err(IdTokenError::AlgorithmRejected)
        );
        let es = json!({"alg": "ES256"}).as_object().unwrap().clone();
        assert_eq!(
            select_algorithm(&es, &only_rs),
            Err(IdTokenError::AlgorithmRejected)
        );
    }

    #[test]
    fn unit_oidc_accepts_a_valid_claim_set() {
        let validated = accepted(&valid_claims(), ValidationPurpose::Login).unwrap();
        assert_eq!(validated.subject, "u-1");
        assert_eq!(validated.email.as_deref(), Some("a@example.com"));
        assert!(validated.email_verified);
        assert_eq!(validated.username.as_deref(), Some("alice"));
        assert_eq!(validated.auth_time, Some(NOW));
        assert_eq!(validated.algorithm, JwsAlgorithm::Rs256);
        assert_eq!(validated.key_id, "k1");
    }

    #[test]
    fn unit_oidc_issuer_is_exact() {
        for issuer in ["https://sso.example.com", "https://SSO.example.com/", ""] {
            let mut claims = valid_claims();
            claims["iss"] = json!(issuer);
            assert_eq!(
                accepted(&claims, ValidationPurpose::Login),
                Err(IdTokenError::IssuerMismatch),
                "{issuer}"
            );
        }
        let mut missing = valid_claims();
        missing.as_object_mut().unwrap().remove("iss");
        assert_eq!(
            accepted(&missing, ValidationPurpose::Login),
            Err(IdTokenError::IssuerMismatch)
        );
    }

    #[test]
    fn unit_oidc_audience_and_azp() {
        let mut scalar = valid_claims();
        scalar["aud"] = json!(CLIENT_ID);
        assert!(accepted(&scalar, ValidationPurpose::Login).is_ok());

        let mut array = valid_claims();
        array["aud"] = json!(["other", CLIENT_ID]);
        assert!(accepted(&array, ValidationPurpose::Login).is_ok());

        let mut wrong_scalar = valid_claims();
        wrong_scalar["aud"] = json!("other");
        assert_eq!(
            accepted(&wrong_scalar, ValidationPurpose::Login),
            Err(IdTokenError::AudienceMismatch)
        );

        let mut wrong_array = valid_claims();
        wrong_array["aud"] = json!(["other", "another"]);
        assert_eq!(
            accepted(&wrong_array, ValidationPurpose::Login),
            Err(IdTokenError::AudienceMismatch)
        );

        let mut no_aud = valid_claims();
        no_aud.as_object_mut().unwrap().remove("aud");
        assert_eq!(
            accepted(&no_aud, ValidationPurpose::Login),
            Err(IdTokenError::AudienceMismatch)
        );

        let mut good_azp = valid_claims();
        good_azp["aud"] = json!(["other", CLIENT_ID]);
        good_azp["azp"] = json!(CLIENT_ID);
        assert!(accepted(&good_azp, ValidationPurpose::Login).is_ok());

        let mut bad_azp = valid_claims();
        bad_azp["azp"] = json!("someone-else");
        assert_eq!(
            accepted(&bad_azp, ValidationPurpose::Login),
            Err(IdTokenError::AzpMismatch)
        );
    }

    #[test]
    fn unit_oidc_time_boundaries_use_sixty_second_skew() {
        for delta in [-61, -60] {
            let mut claims = valid_claims();
            claims["exp"] = json!(NOW + delta);
            assert_eq!(
                accepted(&claims, ValidationPurpose::Login),
                Err(IdTokenError::Expired),
                "exp {delta}"
            );
        }
        for delta in [-59, 0, 59] {
            let mut claims = valid_claims();
            claims["exp"] = json!(NOW + delta);
            assert!(
                accepted(&claims, ValidationPurpose::Login).is_ok(),
                "exp {delta}"
            );
        }

        for delta in [59, 61, 120] {
            let mut claims = valid_claims();
            claims["iat"] = json!(NOW + delta);
            let expected = if delta >= 60 {
                Err(IdTokenError::IssuedInFuture)
            } else {
                Ok(())
            };
            assert_eq!(
                accepted(&claims, ValidationPurpose::Login).is_err(),
                expected.is_err(),
                "iat {delta}"
            );
        }

        for delta in [60, 61, 120] {
            let mut claims = valid_claims();
            claims["nbf"] = json!(NOW + delta);
            let expected_reject = delta > 60;
            assert_eq!(
                accepted(&claims, ValidationPurpose::Login).is_err(),
                expected_reject,
                "nbf {delta}"
            );
        }
    }

    #[test]
    fn unit_oidc_nonce_is_required_and_compared_safely() {
        for nonce in [json!(""), json!("wrong")] {
            let mut claims = valid_claims();
            claims["nonce"] = nonce;
            assert_eq!(
                accepted(&claims, ValidationPurpose::Login),
                Err(IdTokenError::NonceMismatch)
            );
        }
        let mut missing = valid_claims();
        missing.as_object_mut().unwrap().remove("nonce");
        assert_eq!(
            accepted(&missing, ValidationPurpose::Login),
            Err(IdTokenError::NonceMismatch)
        );
    }

    #[test]
    fn unit_oidc_at_hash_vectors() {
        // A fixed vector for SHA-256 over "access-token".
        assert_eq!(
            at_hash(JwsAlgorithm::Rs256, "access-token"),
            "Pxa-1wifRlPl7yG_0oJNfw"
        );
        for alg in [
            JwsAlgorithm::Rs256,
            JwsAlgorithm::Es256,
            JwsAlgorithm::Ps256,
        ] {
            assert_eq!(at_hash(alg, "access-token"), "Pxa-1wifRlPl7yG_0oJNfw");
        }
        let mut valid = valid_claims();
        valid["at_hash"] = json!("Pxa-1wifRlPl7yG_0oJNfw");
        assert!(accepted(&valid, ValidationPurpose::Login).is_ok());

        let mut invalid = valid_claims();
        invalid["at_hash"] = json!("AAAAAAAAAAAAAAAAAAAAAA");
        assert_eq!(
            accepted(&invalid, ValidationPurpose::Login),
            Err(IdTokenError::AtHashMismatch)
        );

        let mut without_token = valid_claims();
        without_token["at_hash"] = json!("Pxa-1wifRlPl7yG_0oJNfw");
        let mapping = ClaimMapping::standard();
        let mut context = context(&mapping, ValidationPurpose::Login);
        context.access_token = None;
        assert_eq!(
            check_claims(&without_token, &context, JwsAlgorithm::Rs256, "k1", NOW),
            Err(IdTokenError::AtHashMismatch)
        );
    }

    #[test]
    fn unit_oidc_auth_time_is_required_only_for_reauth() {
        let mut missing = valid_claims();
        missing.as_object_mut().unwrap().remove("auth_time");
        assert!(accepted(&missing, ValidationPurpose::Login).is_ok());
        assert_eq!(
            accepted(&missing, ValidationPurpose::Reauth),
            Err(IdTokenError::AuthTimeInvalid)
        );

        for delta in [361, 400] {
            let mut stale = valid_claims();
            stale["auth_time"] = json!(NOW - delta);
            assert_eq!(
                accepted(&stale, ValidationPurpose::Reauth),
                Err(IdTokenError::AuthTimeInvalid),
                "stale {delta}"
            );
        }
        for delta in [0, 300, 360] {
            let mut fresh = valid_claims();
            fresh["auth_time"] = json!(NOW - delta);
            assert!(
                accepted(&fresh, ValidationPurpose::Reauth).is_ok(),
                "fresh {delta}"
            );
        }
        let mut future = valid_claims();
        future["auth_time"] = json!(NOW + 61);
        assert_eq!(
            accepted(&future, ValidationPurpose::Reauth),
            Err(IdTokenError::AuthTimeInvalid)
        );
        let mut near_future = valid_claims();
        near_future["auth_time"] = json!(NOW + 59);
        assert!(accepted(&near_future, ValidationPurpose::Reauth).is_ok());
    }

    #[test]
    fn unit_oidc_malformed_tokens_are_rejected() {
        for token in ["", "a", "a.b", "a.b.c.d"] {
            assert_eq!(split(token), Err(IdTokenError::MalformedToken), "{token:?}");
        }
        assert_eq!(
            split(&"x".repeat(MAX_ID_TOKEN_BYTES + 1)),
            Err(IdTokenError::TokenTooLarge)
        );
        assert_eq!(header_json("!!!"), Err(IdTokenError::MalformedToken));
        let not_object = Base64UrlUnpadded::encode_string(b"[]");
        assert_eq!(header_json(&not_object), Err(IdTokenError::MalformedToken));
    }

    #[test]
    fn unit_oidc_supplement_does_not_override_signed_claims() {
        let signed = accepted(&valid_claims(), ValidationPurpose::Login).unwrap();
        let userinfo = ExternalProfile {
            subject: Some("attacker".to_owned()),
            email: Some("attacker@example.com".to_owned()),
            email_verified: false,
            username: Some("from-userinfo".to_owned()),
            name: None,
            picture: Some("https://example.com/b.png".to_owned()),
        };
        let merged = signed.clone().supplemented_with(&userinfo);
        assert_eq!(merged.subject, signed.subject);
        assert_eq!(merged.email, signed.email);
        assert_eq!(merged.email_verified, signed.email_verified);
        assert_eq!(merged.username.as_deref(), Some("alice"));
        assert_eq!(merged.name.as_deref(), Some("Alice"));
        assert_eq!(merged.picture, signed.picture);

        let partial = ValidatedIdToken {
            username: None,
            name: None,
            picture: None,
            ..signed
        };
        let filled = partial.supplemented_with(&ExternalProfile {
            subject: Some("attacker".to_owned()),
            email: Some("attacker@example.com".to_owned()),
            email_verified: false,
            username: Some("userinfo-user".to_owned()),
            name: Some("Userinfo Name".to_owned()),
            picture: Some("https://example.com/b.png".to_owned()),
        });
        assert_eq!(filled.subject, "u-1");
        assert_eq!(filled.email.as_deref(), Some("a@example.com"));
        assert!(filled.email_verified);
        assert_eq!(filled.username.as_deref(), Some("userinfo-user"));
        assert_eq!(filled.name.as_deref(), Some("Userinfo Name"));
        assert_eq!(filled.picture.as_deref(), Some("https://example.com/b.png"));
    }

    #[test]
    fn unit_oidc_error_classification_is_stable() {
        for reason in [
            IdTokenError::AlgorithmRejected,
            IdTokenError::KidMissing,
            IdTokenError::SignatureInvalid,
            IdTokenError::IssuerMismatch,
            IdTokenError::AudienceMismatch,
            IdTokenError::AzpMismatch,
            IdTokenError::Expired,
            IdTokenError::IssuedInFuture,
            IdTokenError::NotYetValid,
            IdTokenError::ClaimsMalformed,
            IdTokenError::NonceMismatch,
            IdTokenError::AtHashMismatch,
        ] {
            assert_eq!(
                reason.api_code(),
                ErrorCode::ProviderIdTokenInvalid,
                "{reason:?}"
            );
        }
        assert_eq!(
            IdTokenError::Jwks(JwksError::KeyNotFound).api_code(),
            ErrorCode::ProviderIdTokenInvalid
        );
        assert_eq!(
            IdTokenError::Jwks(JwksError::Malformed).api_code(),
            ErrorCode::ProviderDiscoveryFailed
        );
        // The distinctions that must not collapse into a token-invalid code.
        assert_eq!(
            IdTokenError::SubjectMissing.api_code(),
            ErrorCode::ProviderSubjectMissing
        );
        assert_eq!(
            IdTokenError::AuthTimeInvalid.api_code(),
            ErrorCode::AuthRecentAuthRequired
        );
    }
}
