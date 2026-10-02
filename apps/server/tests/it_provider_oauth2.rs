pub mod support;

use std::time::Duration;

use palmr_server::identity_providers::http_client::{FetchFailure, ProviderHttpClient};
use palmr_server::identity_providers::model::ClaimMapping;
use palmr_server::identity_providers::oauth2::{
    fetch_userinfo, profile_from_claims, UserinfoError, USERINFO_SIZE_CAP_BYTES,
};
use palmr_server::ErrorCode;
use serde_json::json;
use support::mock_idp::{self, MockIdp};

fn client() -> ProviderHttpClient {
    ProviderHttpClient::new()
}

/// Builds a valid JSON document of exactly `total` bytes.
fn padded_json(total: usize) -> Vec<u8> {
    const PREFIX: &[u8] = br#"{"sub":"s","pad":""#;
    const SUFFIX: &[u8] = br#""}"#;
    const EMPTY: usize = PREFIX.len() + SUFFIX.len();
    assert!(total >= EMPTY);
    let mut body = Vec::with_capacity(total);
    body.extend_from_slice(PREFIX);
    body.extend(std::iter::repeat_n(b'a', total - EMPTY));
    body.extend_from_slice(SUFFIX);
    assert_eq!(body.len(), total);
    body
}

#[tokio::test]
async fn it_userinfo_size_capped() {
    let idp = MockIdp::start().await;
    let client = client();
    let endpoint = idp.userinfo_endpoint();

    // Below and exactly at the cap succeed.
    idp.set_userinfo_raw(200, padded_json(1024));
    assert!(fetch_userinfo(&client, &endpoint, "access-token")
        .await
        .is_ok());

    idp.set_userinfo_raw(200, padded_json(USERINFO_SIZE_CAP_BYTES));
    let at_cap = fetch_userinfo(&client, &endpoint, "access-token")
        .await
        .unwrap();
    assert_eq!(at_cap["sub"], json!("s"));

    // One byte over the cap is rejected.
    idp.set_userinfo_raw(200, padded_json(USERINFO_SIZE_CAP_BYTES + 1));
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "access-token")
            .await
            .unwrap_err(),
        UserinfoError::TooLarge
    );
}

#[tokio::test]
async fn it_userinfo_failure_classes() {
    let idp = MockIdp::start().await;
    let client = client();
    let endpoint = idp.userinfo_endpoint();

    idp.set_userinfo_raw(200, b"{ not json".to_vec());
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "access-token")
            .await
            .unwrap_err(),
        UserinfoError::Malformed
    );

    idp.set_userinfo_raw(503, b"{}".to_vec());
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "access-token")
            .await
            .unwrap_err(),
        UserinfoError::Fetch(FetchFailure::Status(503))
    );

    idp.set_userinfo_raw(200, Vec::new());
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "access-token")
            .await
            .unwrap_err(),
        UserinfoError::Empty
    );

    idp.set_userinfo_raw(200, b"[1,2,3]".to_vec());
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "access-token")
            .await
            .unwrap_err(),
        UserinfoError::NotObject
    );

    idp.set_userinfo(json!({"sub": "s"}));
    assert_eq!(
        fetch_userinfo(&client, &endpoint, "").await.unwrap_err(),
        UserinfoError::Fetch(FetchFailure::InvalidUrl)
    );
}

#[tokio::test]
async fn it_userinfo_timeout_is_enforced() {
    let idp = MockIdp::start().await;
    idp.set_userinfo(mock_idp::verified_userinfo(mock_idp::NONCE));
    idp.set_userinfo_delay(Duration::from_millis(1_000));

    let client = ProviderHttpClient::with_timeout(Duration::from_millis(50));
    let failure = fetch_userinfo(&client, &idp.userinfo_endpoint(), "access-token")
        .await
        .unwrap_err();
    assert_eq!(failure, UserinfoError::Fetch(FetchFailure::Timeout));
}

#[tokio::test]
async fn it_userinfo_sends_bearer_and_no_caller_context() {
    let idp = MockIdp::start().await;
    idp.set_userinfo(mock_idp::verified_userinfo(mock_idp::NONCE));

    let document = fetch_userinfo(&client(), &idp.userinfo_endpoint(), "secret-token")
        .await
        .unwrap();
    assert_eq!(document["sub"], json!("external-subject"));

    assert_eq!(
        idp.userinfo_authorization().await.as_deref(),
        Some("Bearer secret-token")
    );
    assert_eq!(idp.userinfo_cookie().await, None);
}

#[tokio::test]
async fn it_userinfo_profiles_are_mapped_conservatively() {
    let standard = ClaimMapping::standard();

    let verified = profile_from_claims(&mock_idp::verified_userinfo(mock_idp::NONCE), &standard);
    assert_eq!(verified.subject.as_deref(), Some("external-subject"));
    assert_eq!(verified.email.as_deref(), Some("user@example.com"));
    assert!(verified.email_verified);

    let unverified = profile_from_claims(&mock_idp::unverified_userinfo(), &standard);
    assert_eq!(unverified.email.as_deref(), Some("user@example.com"));
    assert!(!unverified.email_verified);

    let missing = profile_from_claims(&mock_idp::missing_email_userinfo(), &standard);
    assert_eq!(missing.email, None);
    assert!(!missing.email_verified);

    let github_mapping = ClaimMapping {
        subject: "id".to_owned(),
        email: "email".to_owned(),
        email_verified: "email_verified".to_owned(),
        username: "login".to_owned(),
        name: "name".to_owned(),
        picture: "avatar_url".to_owned(),
    };
    let github = profile_from_claims(&mock_idp::github_like_userinfo(), &github_mapping);
    assert_eq!(github.subject.as_deref(), Some("4242"));
    assert_eq!(github.username.as_deref(), Some("octocat"));
    assert_eq!(github.email.as_deref(), Some("octocat@github.example"));
    assert!(github.email_verified);
    assert_eq!(
        github.picture.as_deref(),
        Some("https://avatars.example/octocat.png")
    );
}

#[tokio::test]
async fn it_userinfo_error_classification_is_stable() {
    for error in [
        UserinfoError::TooLarge,
        UserinfoError::Malformed,
        UserinfoError::Empty,
        UserinfoError::NotObject,
        UserinfoError::Fetch(FetchFailure::Timeout),
        UserinfoError::Fetch(FetchFailure::Status(500)),
    ] {
        assert_eq!(
            error.api_code(),
            ErrorCode::ProviderUserinfoFailed,
            "{error:?}"
        );
    }
    assert!(ErrorCode::ProviderUserinfoFailed.retryable());
}
