use std::io::Read;
use std::net::TcpListener;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use hyper_util::client::legacy::connect::dns::Name;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde_json::json;
use tower::Service;

use super::http_client::{
    FetchFailure, ProviderHttpClient, PublicOnlyResolver, CONNECT_TIMEOUT, MAX_REDIRECTS,
    REQUEST_TIMEOUT, RESPONSE_LIMIT_BYTES,
};
use super::test_support::{FakeIdp, Reply};

const CERTIFICATE: &[u8] = include_bytes!("../email/testdata/smtp_test_cert.der");
const PRIVATE_KEY: &[u8] = include_bytes!("../email/testdata/smtp_test_key.der");

fn client() -> ProviderHttpClient {
    ProviderHttpClient::new()
}

#[tokio::test]
async fn it_provider_fetch_sends_no_credentials_cookies_or_caller_headers() {
    let idp = FakeIdp::start().await;
    idp.set("/doc", Reply::json(&json!({ "ok": true })));

    let document = client()
        .document(&format!("{}/doc?x=1", idp.base()))
        .await
        .unwrap();

    assert_eq!(document.status.as_u16(), 200);
    let requests = idp.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    for forbidden in [
        "authorization",
        "cookie",
        "proxy-authorization",
        "x-palmr-csrf",
        "origin",
        "referer",
    ] {
        assert_eq!(requests[0].header(forbidden), None, "{forbidden}");
    }
    assert_eq!(requests[0].header("accept"), Some("application/json"));
    assert_eq!(requests[0].header("user-agent"), Some("Palmr"));
}

#[tokio::test]
async fn it_provider_fetch_response_cap_is_256_kib() {
    let idp = FakeIdp::start().await;
    idp.set("/at-cap", Reply::raw(vec![b' '; RESPONSE_LIMIT_BYTES]));
    idp.set("/over", Reply::raw(vec![b' '; RESPONSE_LIMIT_BYTES + 1]));
    idp.set(
        "/over-unsized",
        Reply::raw(vec![b' '; RESPONSE_LIMIT_BYTES + 4096]).unsized_body(),
    );
    idp.set(
        "/at-cap-unsized",
        Reply::raw(vec![b' '; RESPONSE_LIMIT_BYTES]).unsized_body(),
    );
    let client = client();

    assert_eq!(
        client
            .document(&format!("{}/at-cap", idp.base()))
            .await
            .unwrap()
            .body
            .len(),
        RESPONSE_LIMIT_BYTES
    );
    assert_eq!(
        client
            .document(&format!("{}/at-cap-unsized", idp.base()))
            .await
            .unwrap()
            .body
            .len(),
        RESPONSE_LIMIT_BYTES
    );
    for path in ["/over", "/over-unsized"] {
        assert_eq!(
            client
                .document(&format!("{}{path}", idp.base()))
                .await
                .unwrap_err(),
            FetchFailure::TooLarge,
            "{path}"
        );
    }
}

#[test]
fn unit_provider_fetch_budget_is_ten_seconds_total_and_five_to_connect() {
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(10));
    assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(5));
}

#[tokio::test]
async fn it_provider_fetch_is_cut_off_at_its_total_timeout() {
    let idp = FakeIdp::start().await;
    idp.set(
        "/slow",
        Reply::json(&json!({})).delayed(Duration::from_secs(30)),
    );
    let client = ProviderHttpClient::with_timeout(Duration::from_millis(300));

    let started = std::time::Instant::now();
    let failure = client
        .document(&format!("{}/slow", idp.base()))
        .await
        .unwrap_err();

    assert_eq!(failure, FetchFailure::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        client
            .probe(&format!("{}/slow", idp.base()))
            .await
            .unwrap_err(),
        FetchFailure::Timeout
    );
}

#[tokio::test]
async fn it_provider_fetch_follows_same_origin_redirects_only_up_to_the_limit() {
    let idp = FakeIdp::start().await;
    idp.set("/start", Reply::redirect("/middle"));
    idp.set("/middle", Reply::redirect(&format!("{}/final", idp.base())));
    idp.set("/final", Reply::json(&json!({ "arrived": true })));
    let client = client();

    let document = client
        .document(&format!("{}/start", idp.base()))
        .await
        .unwrap();
    assert_eq!(document.body.as_ref(), br#"{"arrived":true}"#);

    for hop in 0..=MAX_REDIRECTS + 1 {
        idp.set(
            &format!("/hop{hop}"),
            Reply::redirect(&format!("/hop{}", hop + 1)),
        );
    }
    assert_eq!(
        client
            .document(&format!("{}/hop0", idp.base()))
            .await
            .unwrap_err(),
        FetchFailure::TooManyRedirects
    );
    idp.set("/no-location", Reply::status(302));
    assert_eq!(
        client
            .document(&format!("{}/no-location", idp.base()))
            .await
            .unwrap_err(),
        FetchFailure::InvalidUrl
    );
}

#[tokio::test]
async fn it_provider_fetch_never_follows_a_redirect_into_a_private_range() {
    let idp = FakeIdp::start().await;
    let target = FakeIdp::start().await;
    target.set("/secret", Reply::json(&json!({ "secret": true })));
    let client = client();

    for location in [
        format!("{}/secret", target.base()),
        "https://169.254.169.254/latest/meta-data/".to_owned(),
        "https://10.0.0.7/admin".to_owned(),
        "https://[::1]/".to_owned(),
        "https://[fd00:ec2::254]/".to_owned(),
        "https://localhost/".to_owned(),
    ] {
        idp.set("/redirect", Reply::redirect(&location));
        let failure = client
            .document(&format!("{}/redirect", idp.base()))
            .await
            .unwrap_err();
        assert_eq!(failure, FetchFailure::Blocked, "{location}");
    }
    assert!(
        target.requests().is_empty(),
        "the redirect target was contacted"
    );

    for location in [
        "http://example.com/",
        "ftp://example.com/",
        "file:///etc/passwd",
        "javascript:alert(1)",
    ] {
        idp.set("/redirect", Reply::redirect(location));
        assert_eq!(
            client
                .document(&format!("{}/redirect", idp.base()))
                .await
                .unwrap_err(),
            FetchFailure::InvalidUrl,
            "{location}"
        );
    }
}

#[tokio::test]
async fn it_provider_probe_reports_any_status_without_following_redirects() {
    let idp = FakeIdp::start().await;
    let target = FakeIdp::start().await;
    idp.set("/bounce", Reply::redirect(&format!("{}/x", target.base())));
    idp.set("/missing-method", Reply::status(405));
    idp.set("/down", Reply::status(503));
    let client = client();

    assert_eq!(
        client
            .probe(&format!("{}/bounce", idp.base()))
            .await
            .unwrap()
            .as_u16(),
        302
    );
    assert_eq!(
        client
            .probe(&format!("{}/missing-method", idp.base()))
            .await
            .unwrap()
            .as_u16(),
        405
    );
    assert_eq!(
        client
            .probe(&format!("{}/down", idp.base()))
            .await
            .unwrap()
            .as_u16(),
        503
    );
    assert!(target.requests().is_empty());
}

#[tokio::test]
async fn it_provider_fetch_classifies_http_errors_and_rejects_unsafe_urls() {
    let idp = FakeIdp::start().await;
    idp.set("/gone", Reply::status(404));
    idp.set("/boom", Reply::status(500));
    let client = client();

    assert_eq!(
        client
            .document(&format!("{}/gone", idp.base()))
            .await
            .unwrap_err(),
        FetchFailure::Status(404)
    );
    assert_eq!(
        client
            .document(&format!("{}/boom", idp.base()))
            .await
            .unwrap_err(),
        FetchFailure::Status(500)
    );
    for url in [
        "http://example.com/",
        "ftp://example.com/",
        "https://user:pw@example.com/",
        "https://example.com/#frag",
        "",
    ] {
        assert_eq!(
            client.document(url).await.unwrap_err(),
            FetchFailure::InvalidUrl,
            "{url}"
        );
        assert_eq!(
            client.probe(url).await.unwrap_err(),
            FetchFailure::InvalidUrl,
            "{url}"
        );
    }
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = unused.local_addr().unwrap().port();
    drop(unused);
    assert_eq!(
        client
            .document(&format!("http://127.0.0.1:{port}/"))
            .await
            .unwrap_err(),
        FetchFailure::Unreachable
    );
}

#[tokio::test]
async fn it_provider_fetch_verifies_tls_against_its_own_root_store() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(CERTIFICATE.to_vec())],
                PrivatePkcs8KeyDer::from(PRIVATE_KEY.to_vec()).into(),
            )
            .unwrap(),
    );
    let server = std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            if let Ok(connection) = rustls::ServerConnection::new(config) {
                let mut tls = rustls::StreamOwned::new(connection, stream);
                let mut sink = [0_u8; 64];
                let _ = tls.read(&mut sink);
            }
        }
    });

    let failure = client()
        .document(&format!(
            "https://127.0.0.1:{port}/.well-known/openid-configuration"
        ))
        .await
        .unwrap_err();

    assert_eq!(failure, FetchFailure::Tls);
    server.join().unwrap();
}

#[tokio::test]
async fn it_provider_guarded_resolver_refuses_non_public_names() {
    let mut resolver = PublicOnlyResolver;
    let refused = resolver
        .call(Name::from_str("localhost").unwrap())
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn unit_provider_http_client_is_isolated_from_the_other_outbound_clients() {
    let provider = client();
    let s3 = crate::storage::s3::tls::build(None, true).unwrap();
    assert!(!Arc::ptr_eq(provider.tls_config().unwrap(), s3.config()));
    assert_eq!(
        provider.root_count(),
        Some(webpki_roots::TLS_SERVER_ROOTS.len())
    );
    let insecure_s3 = crate::storage::s3::tls::build(None, false).unwrap();
    assert!(!insecure_s3.verifies_certificates());
    assert!(!Arc::ptr_eq(
        provider.tls_config().unwrap(),
        insecure_s3.config()
    ));
}
