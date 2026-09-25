use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use http::{HeaderMap, HeaderValue, Method, StatusCode};
use time::macros::datetime;
use time::OffsetDateTime;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use url::Url;

use super::client::S3Clients;
use super::fake_server::{http_date, CorsMode, FakeS3, PresignFault, Seen, BUCKET};
use super::probe_http::{ProbeFailure, ProbeResponse};
use super::selftest::{
    cors_verdict, presigned_failure, Preflights, PROBE_MULTIPART_BYTES, PROBE_PART_BYTES,
};
use super::S3Provider;
use crate::config::{EnvironmentSource, LogFormat, OperatorConfig};
use crate::domain::clock::{Clock, TestClock};
use crate::infra::telemetry::build_dispatch;
use crate::storage::caps::StorageCapabilities;
use crate::storage::health::{
    CheckName, CheckStatus, Diagnosis, Fact, ProbeDepth, SelfTestReport, SelfTestResult,
    StorageHealth, StorageMonitor, PROBE_PREFIX,
};
use crate::storage::provider::{PresignedRequest, StorageProvider};

const NOW: OffsetDateTime = datetime!(2026-09-25 12:00 UTC);
const ACCESS_KEY: &str = "AKIA-selftest-access-sentinel";
const SECRET_KEY: &str = "selftest-secret-sentinel-3f9d";
const BASE_URL: &str = "https://palmr.example.test";
const BROWSER_ORIGIN: &str = "https://palmr.example.test";

const DEEP_CHECKS: [CheckName; 13] = [
    CheckName::HeadBucket,
    CheckName::Write,
    CheckName::Stat,
    CheckName::Read,
    CheckName::Range,
    CheckName::PresignedGet,
    CheckName::PresignedPart,
    CheckName::CorsPreflight,
    CheckName::ListMultipartUploads,
    CheckName::Multipart,
    CheckName::Delete,
    CheckName::Absent,
    CheckName::Cleanup,
];

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap()
    }
}

fn config(fake: &FakeS3, profile: &str) -> OperatorConfig {
    let endpoint = fake.endpoint();
    OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_BASE_URL", BASE_URL),
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", endpoint.as_str()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", BUCKET),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
        ("PALMR_S3_PROFILE", profile),
    ]))
    .unwrap()
    .config
}

fn provider_with(fake: &FakeS3, profile: &str, clock: &TestClock) -> S3Provider {
    let config = config(fake, profile);
    let clients = S3Clients::build(&config.storage).unwrap().unwrap();
    S3Provider::new(clients, 64 * 1024, config.base_url.url())
        .unwrap()
        .with_clock(Arc::new(clock.clone()))
}

fn provider(fake: &FakeS3) -> S3Provider {
    provider_with(fake, "minio", &TestClock::new(NOW))
}

fn status(report: &SelfTestReport, name: CheckName) -> (CheckStatus, Option<Diagnosis>) {
    let check = report
        .check(name)
        .unwrap_or_else(|| panic!("missing {name:?} in {report:?}"));
    (check.status, check.diagnosis)
}

fn presigned_requests(seen: &[Seen], method: &str) -> Vec<Seen> {
    seen.iter()
        .filter(|request| request.presigned() && request.method == method)
        .cloned()
        .collect()
}

fn assert_no_secrets(text: &str) {
    for secret in [
        ACCESS_KEY,
        SECRET_KEY,
        "X-Amz-Signature",
        "X-Amz-Credential",
        "Authorization",
        "fake-upload-",
        PROBE_PREFIX,
    ] {
        assert!(!text.contains(secret), "{secret} leaked:\n{text}");
    }
}

async fn captured_startup(provider: S3Provider) -> (Arc<SelfTestReport>, StorageHealth, String) {
    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("palmr_server=debug"),
        LogFormat::Json,
        output.clone(),
        (),
        false,
    );
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let provider: Arc<dyn StorageProvider> = Arc::new(provider);
    let monitor = StorageMonitor::new(provider, Arc::new(TestClock::new(NOW)), Arc::new(|_| {}));
    let report = monitor.startup(None).await;
    let health = monitor.status().snapshot().health;
    (report, health, output.text())
}

#[tokio::test]
async fn it_s3_full_self_test_executes_deep_checks() {
    let fake = FakeS3::start().await;
    let report = provider(&fake).self_test(ProbeDepth::Full).await;

    assert_eq!(report.result(), SelfTestResult::Passed, "{report:#?}");
    for name in DEEP_CHECKS {
        assert_eq!(
            status(&report, name),
            (CheckStatus::Passed, None),
            "{name:?}"
        );
    }
    let seen = fake.seen();
    assert!(seen
        .iter()
        .any(|request| request.method == "HEAD" && request.path == format!("/{BUCKET}/")));
    let preflights: Vec<&Seen> = seen
        .iter()
        .filter(|request| request.method == "OPTIONS")
        .collect();
    assert_eq!(preflights.len(), 2);
    for preflight in &preflights {
        assert_eq!(preflight.header("origin"), Some(BROWSER_ORIGIN));
        assert!(preflight.presigned());
    }
    let methods: Vec<&str> = preflights
        .iter()
        .filter_map(|preflight| preflight.header("access-control-request-method"))
        .collect();
    assert_eq!(methods, ["PUT", "GET"]);

    let direct = presigned_requests(&seen, "PUT");
    assert_eq!(direct.len(), 1);
    let part = &direct[0];
    assert_eq!(part.query_value("partNumber").as_deref(), Some("1"));
    assert_eq!(
        part.query_value("X-Amz-SignedHeaders").as_deref(),
        Some("host")
    );
    let mut names: Vec<&str> = part.headers.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["content-length", "host", "origin"]);
    assert_eq!(
        part.header("content-length"),
        Some(PROBE_PART_BYTES.to_string().as_str())
    );

    let proxied: Vec<&Seen> = seen
        .iter()
        .filter(|request| request.is("PUT", "partNumber") && !request.presigned())
        .collect();
    assert_eq!(proxied.len(), 2);
    assert!(proxied
        .iter()
        .all(|request| request.header("authorization").is_some()));
    assert!(seen.iter().any(|request| request.is("POST", "uploads")));
    assert!(seen.iter().any(|request| request.is("POST", "uploadId")));
    assert!(seen.iter().any(|request| request.is("GET", "uploads")));
    assert_eq!(presigned_requests(&seen, "GET").len(), 1);
    assert!(
        seen.iter()
            .filter(|request| request.path.contains(PROBE_PREFIX.trim_end_matches('/')))
            .all(|request| !request.path.contains("/objects/")
                && !request.path.contains("/branding/"))
    );

    let completed = seen
        .iter()
        .find(|request| request.is("POST", "uploadId"))
        .unwrap();
    assert!(completed
        .path
        .starts_with(&format!("/{BUCKET}/{PROBE_PREFIX}")));
    assert_eq!(fake.open_uploads(), 0);
    assert!(fake.keys().is_empty(), "{:?}", fake.keys());
    assert_eq!(
        report.fact(Fact::MultipartCompletion).unwrap().verified,
        Some(true)
    );
    assert_eq!(
        report.fact(Fact::ListMultipartUploads).unwrap().verified,
        Some(true)
    );
    assert_eq!(report.fact(Fact::BucketCors).unwrap().verified, Some(true));
    assert_eq!(
        report.fact(Fact::AddressingStyle).unwrap().verified,
        Some(true)
    );
    assert_eq!(PROBE_MULTIPART_BYTES, 2 * 5 * 1024 * 1024 + 1024);
}

#[tokio::test]
async fn it_s3_light_self_test_omits_deep_checks() {
    let fake = FakeS3::start().await;
    let report = provider(&fake).self_test(ProbeDepth::Light).await;

    assert_eq!(report.result(), SelfTestResult::Passed, "{report:#?}");
    let names: Vec<CheckName> = report.checks.iter().map(|check| check.name).collect();
    assert_eq!(
        names,
        [
            CheckName::Write,
            CheckName::Stat,
            CheckName::Read,
            CheckName::Range,
            CheckName::Delete,
            CheckName::Absent,
        ]
    );
    let seen = fake.seen();
    assert!(seen.iter().all(|request| !request.presigned()));
    assert!(seen.iter().all(|request| request.method != "OPTIONS"));
    assert!(seen
        .iter()
        .all(|request| !request.query.contains("uploads")));
    assert!(seen
        .iter()
        .all(|request| !request.query.contains("uploadId")));
    assert!(seen
        .iter()
        .all(|request| request.path != format!("/{BUCKET}/")));
    assert_eq!(seen.len(), 7, "{seen:#?}");
    let range = seen
        .iter()
        .find(|request| request.header("range").is_some())
        .unwrap();
    assert_eq!(range.header("range"), Some("bytes=1024-1535"));
    assert!(fake.keys().is_empty());
}

#[tokio::test]
async fn it_selftest_cors_missing_etag_named_failure() {
    let fake = FakeS3::start().await;
    fake.set_cors(CorsMode::MissingEtag);

    let (report, health, logs) = captured_startup(provider(&fake)).await;

    assert_eq!(
        status(&report, CheckName::CorsPreflight),
        (CheckStatus::Failed, Some(Diagnosis::CorsMissingEtag))
    );
    assert_eq!(Diagnosis::CorsMissingEtag.as_str(), "cors_missing_etag");
    assert_eq!(
        status(&report, CheckName::PresignedPart).0,
        CheckStatus::Passed
    );
    assert_eq!(report.core_failure(), None);
    assert_eq!(report.result(), SelfTestResult::Degraded);
    assert_eq!(
        report.degradations().collect::<Vec<_>>(),
        [Diagnosis::CorsMissingEtag]
    );
    assert_eq!(health, StorageHealth::Degraded);
    assert_eq!(report.fact(Fact::BucketCors).unwrap().verified, Some(false));
    assert!(logs.contains("cors_missing_etag"), "{logs}");
    assert!(logs.contains("STARTUP_STORAGE_SELFTEST_FAILED"), "{logs}");
    assert!(logs.contains("ExposeHeaders"), "{logs}");
    assert_no_secrets(&logs);
}

#[tokio::test]
async fn it_selftest_clock_skew_named_failure() {
    let fake = FakeS3::start().await;
    fake.set_fault(PresignFault::ClockSkew);
    let clock = TestClock::new(NOW);
    let s3 = provider_with(&fake, "minio", &clock);

    let output = Capture::default();
    let dispatch = build_dispatch(
        EnvFilter::new("palmr_server=debug"),
        LogFormat::Json,
        output.clone(),
        (),
        false,
    );
    let report = {
        let _guard = tracing::dispatcher::set_default(&dispatch);
        s3.self_test(ProbeDepth::Full).await
    };

    assert_eq!(
        status(&report, CheckName::PresignedGet),
        (CheckStatus::Failed, Some(Diagnosis::ClockSkew))
    );
    assert_eq!(
        status(&report, CheckName::PresignedPart),
        (CheckStatus::Failed, Some(Diagnosis::ClockSkew))
    );
    assert_eq!(Diagnosis::ClockSkew.as_str(), "clock_skew");
    assert_eq!(report.core_failure(), None);
    assert_eq!(report.result(), SelfTestResult::Degraded);
    assert_eq!(status(&report, CheckName::Multipart).0, CheckStatus::Passed);

    let seen = fake.seen();
    let presigned_puts = presigned_requests(&seen, "PUT");
    let presigned_gets = presigned_requests(&seen, "GET");
    assert_eq!(
        presigned_puts.len(),
        1,
        "a skewed signature is never retried"
    );
    assert_eq!(
        presigned_gets.len(),
        1,
        "a skewed signature is never retried"
    );
    let signed_at = NOW
        .format(time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .unwrap();
    for request in presigned_puts.iter().chain(&presigned_gets) {
        assert_eq!(request.query_value("X-Amz-Date"), Some(signed_at.clone()));
    }
    assert_eq!(clock.now(), NOW);
    assert!(Diagnosis::ClockSkew.remediation().contains("NTP"));
    assert_no_secrets(&output.text());

    let source = include_str!("selftest.rs");
    for forbidden in ["set_offset", "clock_offset", "skew_offset", "backdate"] {
        assert!(!source.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn it_selftest_addressing_style_failure() {
    let fake = FakeS3::start().await;
    fake.set_fault(PresignFault::SignatureMismatch);
    let s3 = provider(&fake);
    let path_style_before = s3.clients.shared().force_path_style();
    let endpoint_before = s3.clients.public_signer().endpoint().clone();

    let report = s3.self_test(ProbeDepth::Full).await;

    assert_eq!(
        status(&report, CheckName::PresignedGet),
        (
            CheckStatus::Failed,
            Some(Diagnosis::AddressingStyleMismatch)
        )
    );
    assert_eq!(
        status(&report, CheckName::PresignedPart),
        (
            CheckStatus::Failed,
            Some(Diagnosis::AddressingStyleMismatch)
        )
    );
    assert_eq!(
        Diagnosis::AddressingStyleMismatch.as_str(),
        "addressing_style_mismatch"
    );
    assert!(Diagnosis::AddressingStyleMismatch
        .remediation()
        .contains("PALMR_S3_FORCE_PATH_STYLE"));
    assert_eq!(report.core_failure(), None);
    assert_eq!(report.result(), SelfTestResult::Degraded);
    assert_eq!(
        report.fact(Fact::AddressingStyle).unwrap().verified,
        Some(false)
    );
    assert_eq!(s3.clients.shared().force_path_style(), path_style_before);
    assert_eq!(s3.clients.public_signer().endpoint(), &endpoint_before);
    assert_eq!(presigned_requests(&fake.seen(), "PUT").len(), 1);

    let report = s3.self_test(ProbeDepth::Full).await;
    assert_eq!(
        status(&report, CheckName::PresignedGet).1,
        Some(Diagnosis::AddressingStyleMismatch)
    );
    assert_eq!(s3.clients.shared().force_path_style(), path_style_before);

    for source in [
        include_str!("selftest.rs"),
        include_str!("probe_http.rs"),
        include_str!("../health/routine.rs"),
        include_str!("../health/monitor.rs"),
    ] {
        for forbidden in [
            "host_str",
            "amazonaws",
            "minio",
            "force_path_style(true",
            "force_path_style(false",
            "set_force_path_style",
            "S3Clients::build",
            "S3Provider::new",
            ".domain(",
        ] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }
}

#[tokio::test]
async fn it_selftest_relaxes_only_verified_checksum_requirement() {
    let fake = FakeS3::start().await;
    let s3 = provider_with(&fake, "r2", &TestClock::new(NOW));
    let assumed = *s3.caps();
    assert!(assumed.requires_checksum_headers);
    assert!(!assumed.supports_presigned_put);

    let light = s3.self_test(ProbeDepth::Light).await;
    assert_eq!(light.result(), SelfTestResult::Passed);
    assert_eq!(*s3.caps(), assumed, "a light probe verifies no capability");

    let report = s3.self_test(ProbeDepth::Full).await;
    assert_eq!(
        status(&report, CheckName::PresignedPart).0,
        CheckStatus::Passed
    );
    let fact = report.fact(Fact::RequiresChecksumHeaders).unwrap();
    assert_eq!(
        (fact.assumed, fact.verified, fact.applied),
        (Some(true), Some(false), true)
    );
    let verified = *s3.caps();
    assert_eq!(
        verified,
        StorageCapabilities {
            requires_checksum_headers: false,
            supports_presigned_put: true,
            ..assumed
        }
    );
    assert_eq!(s3.clients.shared().profile().as_str(), "r2");

    let generic = FakeS3::start().await;
    generic.set_fault(PresignFault::SignatureMismatch);
    let s3 = provider_with(&generic, "r2", &TestClock::new(NOW));
    let report = s3.self_test(ProbeDepth::Full).await;
    let fact = report.fact(Fact::RequiresChecksumHeaders).unwrap();
    assert_eq!((fact.verified, fact.applied), (None, false));
    assert_eq!(*s3.caps(), assumed);
}

#[tokio::test]
async fn it_s3_stale_probe_cleanup_touches_only_probe_namespace() {
    let fake = FakeS3::start().await;
    let old = NOW - time::Duration::hours(2);
    let recent = NOW - time::Duration::minutes(10);
    let stale = "_palmr/probe/0192f3c8d7e97a1b8f0c2d5e6a7b8c9d";
    let fresh = "_palmr/probe/0192f3c8d7e97a1b8f0c2d5e6a7b8c9e";
    let user = "objects/01/92/0192f3c8d7e97a1b8f0c2d5e6a7b8c9f";
    let unparseable = [
        "_palmr/probe/notes.txt",
        "_palmr/probe/0192F3C8D7E97A1B8F0C2D5E6A7B8C9D",
        "_palmr/probe/0192f3c8d7e94a1b8f0c2d5e6a7b8c9d",
        "_palmr/probe/x/0192f3c8d7e97a1b8f0c2d5e6a7b8c9d",
    ];
    fake.insert(stale, b"stale", old);
    fake.insert(fresh, b"fresh", recent);
    fake.insert(user, b"user", old);
    for key in unparseable {
        fake.insert(key, b"unknown", old);
    }

    let report = provider(&fake).self_test(ProbeDepth::Full).await;

    assert_eq!(
        status(&report, CheckName::Cleanup),
        (CheckStatus::Passed, None)
    );
    let mut remaining = fake.keys();
    remaining.sort();
    let mut expected: Vec<String> = [fresh, user]
        .into_iter()
        .chain(unparseable)
        .map(str::to_owned)
        .collect();
    expected.sort();
    assert_eq!(remaining, expected);
    let listed = fake
        .seen()
        .into_iter()
        .find(|request| request.query.contains("list-type=2"))
        .unwrap();
    assert_eq!(listed.query_value("prefix").as_deref(), Some(PROBE_PREFIX));
}

#[tokio::test]
async fn unit_public_probe_refuses_foreign_urls() {
    let fake = FakeS3::start().await;
    let s3 = provider(&fake);
    let foreign = PresignedRequest::new(
        Method::GET,
        Url::parse("http://169.254.169.254/latest/meta-data").unwrap(),
        HeaderMap::new(),
        NOW,
    );
    assert_eq!(
        s3.public_probe
            .execute(&foreign, BROWSER_ORIGIN, None)
            .await
            .unwrap_err(),
        ProbeFailure::Refused
    );
    assert_eq!(
        s3.public_probe
            .preflight(&foreign, BROWSER_ORIGIN, &Method::PUT)
            .await
            .unwrap_err(),
        ProbeFailure::Refused
    );
    let mut same_host_other_port = Url::parse(&fake.endpoint()).unwrap();
    same_host_other_port.set_port(Some(1)).unwrap();
    let other_port =
        PresignedRequest::new(Method::GET, same_host_other_port, HeaderMap::new(), NOW);
    assert_eq!(
        s3.public_probe
            .execute(&other_port, BROWSER_ORIGIN, None)
            .await
            .unwrap_err(),
        ProbeFailure::Refused
    );
    assert!(fake.seen().is_empty());

    let source = include_str!("probe_http.rs");
    let public_items: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub ") || line.starts_with("pub(crate)"))
        .collect();
    assert!(public_items.is_empty(), "{public_items:?}");
    assert!(source.contains("clients.public_tls()"));
    assert!(!source.contains("internal_tls"));
    assert!(!source.contains("Url::parse"));
}

fn response(status: u16, headers: &[(&'static str, &str)], body: &str) -> ProbeResponse {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.append(*name, HeaderValue::from_str(value).unwrap());
    }
    ProbeResponse {
        status: StatusCode::from_u16(status).unwrap(),
        headers: map,
        body: body.to_owned().into(),
    }
}

fn preflights(origin: &str, put: &str, get: &str, expose: Option<&str>) -> Preflights {
    let respond = |method: &str| {
        let mut headers = vec![
            ("access-control-allow-origin", origin),
            ("access-control-allow-methods", method),
        ];
        if let Some(expose) = expose {
            headers.push(("access-control-expose-headers", expose));
        }
        response(200, &headers, "")
    };
    Preflights {
        put: respond(put),
        get: respond(get),
    }
}

#[test]
fn unit_cors_verdict_contract() {
    let recommended = preflights(BROWSER_ORIGIN, "PUT", "GET", Some("ETag, Content-Length"));
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &recommended, None),
        (CheckStatus::Passed, None)
    );
    let listed_methods = preflights(
        BROWSER_ORIGIN,
        "GET, PUT, HEAD",
        "GET, PUT, HEAD",
        Some("etag"),
    );
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &listed_methods, None).0,
        CheckStatus::Passed
    );

    let exposed_on_response = preflights(BROWSER_ORIGIN, "PUT", "GET", None);
    let mut actual = HeaderMap::new();
    actual.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("ETag"),
    );
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &exposed_on_response, Some(&actual)),
        (CheckStatus::Passed, None)
    );
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &exposed_on_response, None),
        (CheckStatus::Failed, Some(Diagnosis::CorsMissingEtag))
    );

    let missing_etag = preflights(BROWSER_ORIGIN, "PUT", "GET", Some("Content-Length"));
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &missing_etag, None),
        (CheckStatus::Failed, Some(Diagnosis::CorsMissingEtag))
    );
    let wildcard = preflights("*", "PUT", "GET", Some("ETag"));
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &wildcard, None),
        (CheckStatus::Warning, Some(Diagnosis::CorsWildcardOrigin))
    );
    let wildcard_without_etag = preflights("*", "PUT", "GET", None);
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &wildcard_without_etag, None).1,
        Some(Diagnosis::CorsMissingEtag)
    );
    let other_origin = preflights("https://evil.example", "PUT", "GET", Some("ETag"));
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &other_origin, None),
        (CheckStatus::Failed, Some(Diagnosis::CorsOriginMismatch))
    );
    let no_get = preflights(BROWSER_ORIGIN, "PUT", "PUT", Some("ETag"));
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &no_get, None),
        (CheckStatus::Failed, Some(Diagnosis::CorsMethodsMissing))
    );
    let rejected = Preflights {
        put: response(403, &[], ""),
        get: response(200, &[], ""),
    };
    assert_eq!(
        cors_verdict(BROWSER_ORIGIN, &rejected, None),
        (CheckStatus::Failed, Some(Diagnosis::CorsPreflightRejected))
    );
}

#[test]
fn unit_presigned_failure_classification() {
    let error = |code: &str| format!("<Error><Code>{code}</Code><Message>m</Message></Error>");
    let date = http_date(NOW);
    let same_clock = [("date", date.as_str())];
    assert_eq!(
        presigned_failure(
            &response(403, &same_clock, &error("RequestTimeTooSkewed")),
            NOW
        ),
        Diagnosis::ClockSkew
    );
    assert_eq!(
        presigned_failure(
            &response(403, &same_clock, &error("SignatureDoesNotMatch")),
            NOW
        ),
        Diagnosis::AddressingStyleMismatch
    );
    assert_eq!(
        presigned_failure(&response(403, &same_clock, &error("AccessDenied")), NOW),
        Diagnosis::PresignRejected
    );
    let far = http_date(NOW + time::Duration::minutes(40));
    assert_eq!(
        presigned_failure(
            &response(403, &[("date", far.as_str())], &error("AccessDenied")),
            NOW
        ),
        Diagnosis::ClockSkew
    );
    let near = http_date(NOW + time::Duration::minutes(5));
    assert_eq!(
        presigned_failure(
            &response(403, &[("date", near.as_str())], &error("AccessDenied")),
            NOW
        ),
        Diagnosis::PresignRejected
    );
    assert_eq!(
        presigned_failure(&response(502, &[], "<html>bad gateway</html>"), NOW),
        Diagnosis::PresignRejected
    );
}

#[tokio::test]
async fn it_selftest_wildcard_cors_origin_is_a_warning() {
    let fake = FakeS3::start().await;
    fake.set_cors(CorsMode::WildcardOrigin);

    let (report, health, logs) = captured_startup(provider(&fake)).await;

    assert_eq!(
        status(&report, CheckName::CorsPreflight),
        (CheckStatus::Warning, Some(Diagnosis::CorsWildcardOrigin))
    );
    assert_eq!(report.result(), SelfTestResult::Passed);
    assert_eq!(health, StorageHealth::Ok);
    assert!(logs.contains("cors_wildcard_origin"), "{logs}");
    assert_no_secrets(&logs);
}

async fn minio_self_test(
    server: &super::object_tests::minio::MinioServer,
    force_path_style: bool,
) -> SelfTestReport {
    let bucket = server.create_bucket("selftest").await.unwrap();
    let style = if force_path_style { "true" } else { "false" };
    let config = OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_BASE_URL", BASE_URL),
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", server.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", bucket.as_str()),
        ("PALMR_S3_ACCESS_KEY", server.access_key()),
        ("PALMR_S3_SECRET_KEY", server.secret_key()),
        ("PALMR_S3_PROFILE", "minio"),
        ("PALMR_S3_FORCE_PATH_STYLE", style),
    ]))
    .unwrap()
    .config;
    let clients = S3Clients::build(&config.storage).unwrap().unwrap();
    let s3 = S3Provider::new(clients, 64 * 1024, config.base_url.url())
        .unwrap()
        .with_clock(Arc::new(crate::domain::clock::SystemClock));
    let light = s3.self_test(ProbeDepth::Light).await;
    assert_eq!(light.result(), SelfTestResult::Passed, "{light:#?}");
    let report = s3.self_test(ProbeDepth::Full).await;
    let listed = s3
        .list_objects_page(PROBE_PREFIX, None, 1_000)
        .await
        .unwrap();
    assert!(listed.entries.is_empty(), "{style}: {listed:?}");
    report
}

#[tokio::test(flavor = "multi_thread")]
async fn it_selftest_full_against_minio() {
    let server = super::object_tests::minio::MinioServer::start()
        .await
        .unwrap();
    for force_path_style in [true, false] {
        let report = minio_self_test(&server, force_path_style).await;
        assert_eq!(report.core_failure(), None, "{report:#?}");
        for name in [
            CheckName::HeadBucket,
            CheckName::PresignedGet,
            CheckName::PresignedPart,
            CheckName::Multipart,
            CheckName::Cleanup,
        ] {
            assert_eq!(
                status(&report, name).0,
                CheckStatus::Passed,
                "{name:?} {report:#?}"
            );
        }
        assert_eq!(
            report.fact(Fact::AddressingStyle).unwrap().verified,
            Some(true)
        );
        assert_ne!(
            status(&report, CheckName::CorsPreflight).0,
            CheckStatus::Skipped,
            "{report:#?}"
        );
        assert!(report.check(CheckName::ListMultipartUploads).unwrap().ok());
    }
}

#[tokio::test]
async fn it_selftest_unreachable_public_endpoint_is_informational() {
    let fake = FakeS3::start().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let public = format!("http://127.0.0.1:{}", closed.local_addr().unwrap().port());
    drop(closed);
    let endpoint = fake.endpoint();
    let config = OperatorConfig::load(&EnvironmentSource::from_vars([
        ("PALMR_BASE_URL", BASE_URL),
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", endpoint.as_str()),
        ("PALMR_S3_PUBLIC_ENDPOINT", public.as_str()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", BUCKET),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
    ]))
    .unwrap()
    .config;
    let clients = S3Clients::build(&config.storage).unwrap().unwrap();
    let s3 = S3Provider::new(clients, 64 * 1024, config.base_url.url())
        .unwrap()
        .with_clock(Arc::new(TestClock::new(NOW)));

    let report = s3.self_test(ProbeDepth::Full).await;

    for name in [
        CheckName::PresignedGet,
        CheckName::PresignedPart,
        CheckName::CorsPreflight,
    ] {
        assert_eq!(
            status(&report, name),
            (
                CheckStatus::Info,
                Some(Diagnosis::PublicEndpointUnreachable)
            ),
            "{name:?}"
        );
    }
    assert_eq!(status(&report, CheckName::Multipart).0, CheckStatus::Passed);
    assert_eq!(report.result(), SelfTestResult::Passed);
    assert!(fake.seen().iter().all(|request| !request.presigned()));
    assert_eq!(
        report.fact(Fact::RequiresChecksumHeaders).unwrap().verified,
        None
    );
}
