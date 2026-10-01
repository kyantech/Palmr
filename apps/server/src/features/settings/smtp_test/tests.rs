use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use rustls::pki_types::ServerName;
use serde_json::{json, Value};

use super::*;
use crate::features::email::fake_smtp::{Behavior, FakeSmtp, Wire};
use crate::features::email::transport::{CapturingTransport, ProbeScript, SmtpTransport};
use crate::features::settings::model::AppSettings;

const USER: &str = "mailer";
const PASSWORD: &str = "palmr-smtp-test-password-5d2e";
const WRONG_PASSWORD: &str = "palmr-smtp-wrong-password-9a41";
const SAVED_PASSWORD: &str = "palmr-smtp-saved-password-0c7b";

fn unsaved(port: u16, security: &str, extra: &Value) -> Value {
    let mut body = json!({
        "host": "127.0.0.1",
        "port": port,
        "security": security,
        "fromEmail": "palmr@example.test",
        "fromName": "Palmr",
    });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    body
}

fn request(unsaved: &Value) -> SmtpTestInput {
    parse_request(&json!({ "to": "ada@example.test", "useUnsavedSettings": unsaved })).unwrap()
}

fn service() -> SmtpTestService {
    SmtpTestService::new(
        SettingsHandle::documented_defaults(),
        Arc::new(SmtpTransport),
    )
}

fn credentials() -> Value {
    json!({ "username": USER, "password": PASSWORD })
}

fn stages(report: &SmtpTestReport) -> Vec<&'static str> {
    report.stages.iter().map(|stage| stage.as_str()).collect()
}

async fn failed(service: &SmtpTestService, body: &Value) -> ProbeFailure {
    match service.run(request(body)).await {
        Err(SmtpTestError::Failed(failure)) => failure,
        other => panic!("expected a transport failure, got {other:?}"),
    }
}

#[tokio::test]
async fn it_smtp_test_plaintext_none_reports_only_connect_and_send() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain));
    let body = unsaved(server.port(), "none", &json!({ "noAuth": true }));
    let report = service().run(request(&body)).await.unwrap();
    assert_eq!(stages(&report), ["connect", "send"]);

    let record = server.record();
    assert_eq!(record.messages.len(), 1);
    assert!(record.messages[0].contains("Subject: Palmr SMTP test"));
    assert!(record.messages[0].contains("ada@example.test"));
    assert_eq!(record.encrypted_commands, 0);
    assert!(!record.commands.contains(&"STARTTLS".to_owned()));
    assert!(record.auth_attempts.is_empty());
}

#[tokio::test]
async fn it_smtp_test_starttls_reports_every_stage_and_authenticates_encrypted() {
    let server = FakeSmtp::start(Behavior::new(Wire::StartTls).with_auth(USER, PASSWORD));
    let mut extra = credentials();
    extra["allowSelfSignedCertificate"] = json!(true);
    let body = unsaved(server.port(), "starttls", &extra);
    let report = service().run(request(&body)).await.unwrap();
    assert_eq!(stages(&report), ["connect", "starttls", "auth", "send"]);

    let record = server.record();
    assert_eq!(
        record.auth_attempts,
        [(USER.to_owned(), PASSWORD.to_owned())]
    );
    assert_eq!(record.messages.len(), 1);
    assert_eq!(
        record.plaintext_commands, 2,
        "only the first EHLO and STARTTLS may cross in the clear: {:?}",
        record.commands
    );
    assert!(record.encrypted_commands >= 4);
}

#[tokio::test]
async fn it_smtp_test_implicit_tls_has_no_starttls_stage() {
    let server = FakeSmtp::start(Behavior::new(Wire::Implicit).with_auth(USER, PASSWORD));
    let mut extra = credentials();
    extra["allowSelfSignedCertificate"] = json!(true);
    let body = unsaved(server.port(), "implicit", &extra);
    let report = service().run(request(&body)).await.unwrap();
    assert_eq!(stages(&report), ["connect", "auth", "send"]);

    let record = server.record();
    assert_eq!(record.plaintext_commands, 0);
    assert!(!record.commands.contains(&"STARTTLS".to_owned()));
    assert_eq!(record.auth_attempts.len(), 1);
}

#[tokio::test]
async fn it_smtp_test_plaintext_with_credentials_authenticates_without_tls() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, PASSWORD));
    let body = unsaved(server.port(), "none", &credentials());
    let report = service().run(request(&body)).await.unwrap();
    assert_eq!(stages(&report), ["connect", "auth", "send"]);
    assert_eq!(server.record().encrypted_commands, 0);
}

#[tokio::test]
async fn it_smtp_test_no_auth_mode_never_offers_credentials() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, PASSWORD));
    let mut extra = credentials();
    extra["noAuth"] = json!(true);
    let body = unsaved(server.port(), "none", &extra);
    let report = service().run(request(&body)).await.unwrap();
    assert_eq!(stages(&report), ["connect", "send"]);
    assert!(server.record().auth_attempts.is_empty());
}

#[tokio::test]
async fn it_smtp_test_rejected_credentials_fail_at_auth_without_echo() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, PASSWORD));
    let body = unsaved(
        server.port(),
        "none",
        &json!({ "username": USER, "password": WRONG_PASSWORD }),
    );
    let failure = failed(&service(), &body).await;
    assert_eq!(failure.stage, Stage::Auth);
    assert_eq!(failure.reason, FailureReason::CredentialsRejected);
    for text in [failure.reason.message(), &format!("{failure:?}")] {
        assert!(!text.contains(WRONG_PASSWORD), "{text}");
        assert!(!text.contains(USER), "{text}");
    }
    let record = server.record();
    assert_eq!(record.auth_attempts.len(), 1);
    assert!(record.messages.is_empty());
}

#[tokio::test]
async fn it_smtp_test_server_without_auth_fails_at_auth_when_credentials_are_required() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain));
    let body = unsaved(server.port(), "none", &credentials());
    let failure = failed(&service(), &body).await;
    assert_eq!(failure.stage, Stage::Auth);
    assert_eq!(failure.reason, FailureReason::NoAuthMechanism);
}

#[tokio::test]
async fn it_smtp_test_self_signed_is_rejected_by_default_and_accepted_only_with_opt_in() {
    for (wire, security, rejected_at) in [
        (Wire::StartTls, "starttls", Stage::Starttls),
        (Wire::Implicit, "implicit", Stage::Connect),
    ] {
        let server = FakeSmtp::start(Behavior::new(wire).with_auth(USER, PASSWORD));
        let strict = unsaved(server.port(), security, &credentials());
        let failure = failed(&service(), &strict).await;
        assert_eq!(failure.stage, rejected_at, "{security}");
        assert_eq!(failure.reason, FailureReason::Tls, "{security}");
        assert!(server.record().auth_attempts.is_empty(), "{security}");
        assert!(server.record().messages.is_empty(), "{security}");

        let mut extra = credentials();
        extra["allowSelfSignedCertificate"] = json!(true);
        let permissive = unsaved(server.port(), security, &extra);
        service().run(request(&permissive)).await.unwrap();
        assert_eq!(server.record().messages.len(), 1, "{security}");

        let strict_again = failed(&service(), &strict).await;
        assert_eq!(strict_again.reason, FailureReason::Tls, "{security}");
        assert_eq!(server.record().messages.len(), 1, "{security}");
    }
}

#[tokio::test]
async fn it_smtp_test_missing_starttls_fails_at_starttls() {
    let mut behavior = Behavior::new(Wire::StartTls);
    behavior.advertise_starttls = false;
    let server = FakeSmtp::start(behavior);
    let body = unsaved(
        server.port(),
        "starttls",
        &json!({ "noAuth": true, "allowSelfSignedCertificate": true }),
    );
    let failure = failed(&service(), &body).await;
    assert_eq!(failure.stage, Stage::Starttls);
    assert_eq!(failure.reason, FailureReason::StarttlsUnsupported);
    assert!(server.record().messages.is_empty());
}

#[tokio::test]
async fn it_smtp_test_unreachable_server_fails_at_connect() {
    let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let body = unsaved(port, "none", &json!({ "noAuth": true }));
    let failure = failed(&service(), &body).await;
    assert_eq!(failure.stage, Stage::Connect);
    assert_eq!(failure.reason, FailureReason::Unreachable);
}

#[tokio::test]
async fn it_smtp_test_message_refusal_fails_at_send() {
    for (reply, reason) in [
        ("554 5.7.1 refused", FailureReason::MessageRejected),
        ("451 4.3.0 try again later", FailureReason::MessageDeferred),
    ] {
        let mut behavior = Behavior::new(Wire::Plain);
        behavior.data_reply = reply;
        let server = FakeSmtp::start(behavior);
        let body = unsaved(server.port(), "none", &json!({ "noAuth": true }));
        let failure = failed(&service(), &body).await;
        assert_eq!(failure.stage, Stage::Send, "{reply}");
        assert_eq!(failure.reason, reason, "{reply}");
        assert!(!failure.reason.message().contains("refused"), "{reply}");
    }
}

#[tokio::test(start_paused = true)]
async fn it_smtp_test_hard_deadline_is_twenty_seconds_and_names_the_stage() {
    assert_eq!(TEST_DEADLINE, Duration::from_secs(20));
    let transport = Arc::new(CapturingTransport::new());
    transport.script_probe(ProbeScript::Hang);
    let service = SmtpTestService::new(SettingsHandle::documented_defaults(), transport.clone());
    let body = unsaved(587, "starttls", &credentials());

    let started = Instant::now();
    let outcome = service.run(request(&body)).await;
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    assert_eq!(
        outcome,
        Err(SmtpTestError::Failed(ProbeFailure::new(
            Stage::Send,
            FailureReason::TimedOut
        )))
    );
    assert_eq!(transport.count(), 1);
}

#[tokio::test]
async fn it_smtp_test_unsaved_settings_do_not_touch_the_saved_snapshot() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, PASSWORD));
    let handle = SettingsHandle::documented_defaults();
    let before = handle.load();
    let service = SmtpTestService::new(handle.clone(), Arc::new(SmtpTransport));
    let body = unsaved(server.port(), "none", &credentials());
    service.run(request(&body)).await.unwrap();
    assert!(Arc::ptr_eq(&handle.load(), &before));
    assert!(handle.load().smtp.password.is_none());
    assert!(handle.load().smtp.host.is_none());
}

#[tokio::test]
async fn it_smtp_test_unsaved_values_never_borrow_the_saved_password() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, SAVED_PASSWORD));
    let mut settings = AppSettings::defaults();
    settings.smtp.password = Some(Secret::new(SAVED_PASSWORD.to_owned()));
    settings.smtp.username = Some(USER.to_owned());
    let service = SmtpTestService::new(SettingsHandle::new(settings), Arc::new(SmtpTransport));

    let borrowing = unsaved(server.port(), "none", &json!({ "username": USER }));
    let error = service.run(request(&borrowing)).await.unwrap_err();
    assert_eq!(
        error,
        SmtpTestError::Invalid {
            field: "useUnsavedSettings.password"
        }
    );
    let record = server.record();
    assert_eq!(record.connections, 0);
    assert!(record.auth_attempts.is_empty());
}

#[tokio::test]
async fn it_smtp_test_saved_configuration_is_tested_even_while_disabled() {
    let server = FakeSmtp::start(Behavior::new(Wire::Plain).with_auth(USER, SAVED_PASSWORD));
    let mut settings = AppSettings::defaults();
    settings.smtp.enabled = false;
    settings.smtp.host = Some("127.0.0.1".to_owned());
    settings.smtp.port = server.port();
    settings.smtp.security = SmtpSecurity::None;
    settings.smtp.username = Some(USER.to_owned());
    settings.smtp.password = Some(Secret::new(SAVED_PASSWORD.to_owned()));
    settings.smtp.from_email = Some("palmr@example.test".to_owned());
    let service = SmtpTestService::new(SettingsHandle::new(settings), Arc::new(SmtpTransport));

    let input = parse_request(&json!({ "to": "ada@example.test" })).unwrap();
    let report = service.run(input).await.unwrap();
    assert_eq!(stages(&report), ["connect", "auth", "send"]);
    assert_eq!(
        server.record().auth_attempts,
        [(USER.to_owned(), SAVED_PASSWORD.to_owned())]
    );
}

#[tokio::test]
async fn it_smtp_test_incomplete_saved_configuration_is_a_validation_error() {
    let service = service();
    let input = parse_request(&json!({ "to": "ada@example.test" })).unwrap();
    assert_eq!(
        service.run(input).await.unwrap_err(),
        SmtpTestError::Invalid { field: "host" }
    );
}

#[test]
fn unit_smtp_test_request_validation_names_the_offending_fields() {
    let good = unsaved(587, "starttls", &credentials());
    for (body, fields) in [
        (json!([]), vec!["body"]),
        (json!({}), vec!["to"]),
        (json!({ "to": "not-an-address" }), vec!["to"]),
        (json!({ "to": 7 }), vec!["to"]),
        (json!({ "to": "a@b.test", "extra": 1 }), vec!["body"]),
        (
            json!({ "to": "a@b.test", "useUnsavedSettings": "x" }),
            vec!["useUnsavedSettings"],
        ),
        (
            json!({ "to": "a@b.test", "useUnsavedSettings": {} }),
            vec![
                "useUnsavedSettings.host",
                "useUnsavedSettings.port",
                "useUnsavedSettings.security",
                "useUnsavedSettings.fromEmail",
            ],
        ),
        (
            json!({ "to": "a@b.test", "useUnsavedSettings": unsaved(0, "tls", &json!({
                "host": "bad host/",
                "fromEmail": "nope",
                "username": 5,
                "password": "",
                "noAuth": "yes",
                "bogus": true,
            })) }),
            vec![
                "useUnsavedSettings",
                "useUnsavedSettings.host",
                "useUnsavedSettings.port",
                "useUnsavedSettings.security",
                "useUnsavedSettings.fromEmail",
                "useUnsavedSettings.username",
                "useUnsavedSettings.password",
                "useUnsavedSettings.noAuth",
            ],
        ),
    ] {
        let rejected = parse_request(&body).unwrap_err();
        assert_eq!(rejected, fields, "{body}");
    }
    let accepted = parse_request(&json!({ "to": "a@b.test", "useUnsavedSettings": good })).unwrap();
    let unsaved = accepted.unsaved.unwrap();
    assert_eq!(unsaved.port, 587);
    assert_eq!(unsaved.security, SmtpSecurity::Starttls);
    assert!(!unsaved.no_auth && !unsaved.allow_self_signed_certificate);
    assert!(unsaved.enabled);
    let rendered = format!("{unsaved:?}");
    assert!(!rendered.contains(PASSWORD), "{rendered}");

    let null_unsaved = parse_request(&json!({ "to": "a@b.test", "useUnsavedSettings": null }));
    assert!(null_unsaved.unwrap().unsaved.is_none());
}

#[tokio::test]
async fn it_smtp_test_uses_own_tls_config() {
    let env_before: Vec<(String, String)> = std::env::vars().collect();
    let provider_before = rustls::crypto::CryptoProvider::get_default()
        .map(|provider| Arc::as_ptr(provider) as usize);

    let server = FakeSmtp::start(Behavior::new(Wire::Implicit).with_auth(USER, PASSWORD));
    let mut extra = credentials();
    extra["allowSelfSignedCertificate"] = json!(true);
    let permissive = unsaved(server.port(), "implicit", &extra);
    service().run(request(&permissive)).await.unwrap();

    let strict = unsaved(server.port(), "implicit", &credentials());
    let failure = failed(&service(), &strict).await;
    assert_eq!(
        failure,
        ProbeFailure::new(Stage::Connect, FailureReason::Tls),
        "the SMTP opt-in must not outlive its own client"
    );

    let s3 = crate::storage::s3::tls::build(None, true).unwrap();
    assert!(s3.verifies_certificates());
    assert_eq!(s3.root_count(), webpki_roots::TLS_SERVER_ROOTS.len());
    let tcp = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    let connection = rustls::ClientConnection::new(
        Arc::clone(s3.config()),
        ServerName::try_from("127.0.0.1").unwrap(),
    )
    .unwrap();
    let mut stream = rustls::StreamOwned::new(connection, tcp);
    let outcome = stream
        .write_all(b"EHLO palmr\r\n")
        .and_then(|()| stream.read(&mut [0_u8; 16]).map(drop));
    assert!(
        outcome.is_err(),
        "the S3 client config must still reject the self-signed certificate"
    );

    let provider_after = rustls::crypto::CryptoProvider::get_default()
        .map(|provider| Arc::as_ptr(provider) as usize);
    assert_eq!(provider_before, provider_after);
    let env_after: Vec<(String, String)> = std::env::vars().collect();
    assert_eq!(env_before, env_after);

    for source in [
        include_str!("../smtp_test.rs"),
        include_str!("../../email/transport.rs"),
    ] {
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "set_var",
            "install_default",
            "set_default",
            "REJECT_UNAUTHORIZED",
            "std::env",
        ] {
            assert!(!production.contains(forbidden), "{forbidden}");
        }
    }
}
