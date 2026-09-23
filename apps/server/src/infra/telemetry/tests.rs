use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Map, Value};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::MakeWriter;

use super::{
    build_dispatch, init, parse_filter, LogFormat, RedactedPresignedUrl, TelemetryInitError,
    STARTUP_TRACING_INIT_FAILED,
};
use crate::domain::secret::Secret;

const FIXED_TIMESTAMP: &str = "2026-01-02T03:04:05.678Z";
const SECRET_SENTINEL: &str = "telemetry-secret-sentinel-3b8a";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        String::from_utf8(bytes).unwrap()
    }

    fn json_lines(&self) -> Vec<Map<String, Value>> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

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

struct FixedTime;

impl FormatTime for FixedTime {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        w.write_str(FIXED_TIMESTAMP)
    }
}

fn capture(filter: &str, format: LogFormat, emit: impl FnOnce()) -> Capture {
    let output = Capture::default();
    let dispatch = build_dispatch(
        parse_filter(filter).unwrap(),
        format,
        output.clone(),
        FixedTime,
        false,
    );
    tracing::dispatcher::with_default(&dispatch, emit);
    output
}

#[test]
fn unit_log_fields_canonical() {
    let output = capture("info", LogFormat::Json, || {
        let request = tracing::info_span!(
            "request",
            request_id = "0190a8f2-7c1e-7d3a-9b1f-2a3b4c5d6e7f",
            method = "GET",
            route = "/api/v1/files/{id}",
            client_ip = "203.0.113.7",
            status = tracing::field::Empty,
        );
        let _request = request.enter();
        let _transfer = tracing::info_span!(
            "transfer_session",
            transfer_session_id = "ts-1",
            upload_id = "up-1",
            storage_object_id = "so-1",
            user_id = "u-1",
            session_id = "s-1",
        )
        .entered();
        request.record("status", 201_u16);
        tracing::info!(
            duration_ms = 12_u64,
            error_code = "QUOTA_EXCEEDED",
            job_id = "j-1",
            job_kind = "s3.reconcile_multipart",
            "request.completed"
        );
    });

    let lines = output.json_lines();
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    let mut keys: Vec<&str> = line.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = vec![
        "timestamp",
        "level",
        "request_id",
        "method",
        "route",
        "status",
        "duration_ms",
        "client_ip",
        "user_id",
        "session_id",
        "error_code",
        "transfer_session_id",
        "upload_id",
        "storage_object_id",
        "job_id",
        "job_kind",
        "target",
        "message",
    ];
    expected.sort_unstable();
    assert_eq!(keys, expected);

    assert_eq!(line["timestamp"], FIXED_TIMESTAMP);
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["request_id"], "0190a8f2-7c1e-7d3a-9b1f-2a3b4c5d6e7f");
    assert_eq!(line["route"], "/api/v1/files/{id}");
    assert_eq!(line["status"], 201);
    assert_eq!(line["duration_ms"], 12);
    assert_eq!(line["transfer_session_id"], "ts-1");
    assert_eq!(line["message"], "request.completed");
}

#[test]
fn unit_log_fields_request_id_only_inside_request_span() {
    let output = capture("info", LogFormat::Json, || {
        tracing::info!("startup.completed");
        tracing::info_span!("request", request_id = "req-1")
            .in_scope(|| tracing::warn!(status = 404_u16, "request.completed"));
        tracing::info!("after");
    });

    let lines = output.json_lines();
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].contains_key("request_id"));
    assert_eq!(lines[1]["request_id"], "req-1");
    assert_eq!(lines[1]["level"], "WARN");
    assert!(!lines[2].contains_key("request_id"));
    for line in &lines {
        assert_eq!(line["timestamp"], FIXED_TIMESTAMP);
    }
}

#[test]
fn unit_log_fields_inner_span_and_event_override_outer() {
    let output = capture("info", LogFormat::Json, || {
        let _outer = tracing::info_span!("outer", job_id = "outer", user_id = "u-1").entered();
        let _inner = tracing::info_span!("inner", job_id = "inner").entered();
        tracing::info!(user_id = "u-2", "event");
    });

    let line = &output.json_lines()[0];
    assert_eq!(line["job_id"], "inner");
    assert_eq!(line["user_id"], "u-2");
}

#[test]
fn unit_log_fields_reserved_names_cannot_be_overwritten() {
    let output = capture("info", LogFormat::Json, || {
        tracing::info!(
            timestamp = "forged",
            level = "forged",
            target = "forged",
            "event"
        );
    });

    let text = output.text();
    assert_eq!(text.matches("\"timestamp\"").count(), 1);
    let line = &output.json_lines()[0];
    assert_eq!(line["timestamp"], FIXED_TIMESTAMP);
    assert_eq!(line["level"], "INFO");
    assert_ne!(line["target"], "forged");
}

#[test]
fn unit_log_json_one_escaped_line_per_event() {
    let output = capture("info", LogFormat::Json, || {
        tracing::info!(
            route = "/a\n{\"x\":1}",
            ratio = 0.5_f64,
            nan = f64::NAN,
            ok = true,
            "multi\nline"
        );
        tracing::info!(count = -3_i64, "second");
    });

    let text = output.text();
    assert_eq!(text.lines().count(), 2);
    let lines = output.json_lines();
    assert_eq!(lines[0]["message"], "multi\nline");
    assert_eq!(lines[0]["route"], "/a\n{\"x\":1}");
    assert_eq!(lines[0]["ratio"], 0.5);
    assert_eq!(lines[0]["nan"], "NaN");
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[1]["count"], -3);
}

#[test]
fn unit_log_json_never_contains_secret_values() {
    let secret = Secret::new(SECRET_SENTINEL.to_owned());
    let output = capture("trace", LogFormat::Json, || {
        let _span = tracing::info_span!("request", request_id = "req-1", token = ?secret).entered();
        tracing::info!(access_key = ?secret, secret_key = %secret, "s3.configured");
        tracing::debug!(config = ?Some(&secret), "debug");
    });

    let text = output.text();
    assert!(!text.contains(SECRET_SENTINEL));
    let lines = output.json_lines();
    assert_eq!(lines[0]["token"], "<redacted>");
    assert_eq!(lines[0]["access_key"], "<redacted>");
    assert_eq!(lines[0]["secret_key"], "<redacted>");
    assert_eq!(lines[1]["config"], "Some(<redacted>)");
}

#[test]
fn unit_presigned_url_redacted_in_logs() {
    let presigned = "https://storage.example.com/bucket/object?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAEXAMPLE%2F20260101&X-Amz-Expires=900&X-Amz-Signature=0d1e2f3a4b5c6d7e8f9a";
    for format in [LogFormat::Json, LogFormat::Pretty] {
        let output = capture("info", format, || {
            tracing::info!(url = %RedactedPresignedUrl::new(presigned), "presign.issued");
            tracing::info!(url = ?RedactedPresignedUrl::new(presigned), "presign.issued");
        });

        let text = output.text();
        assert_eq!(
            text.matches("<presigned:storage.example.com/bucket/object>")
                .count(),
            2
        );
        for leaked in [
            "X-Amz-Signature",
            "0d1e2f3a4b5c6d7e8f9a",
            "AKIAEXAMPLE",
            "X-Amz-Expires",
            "?",
        ] {
            assert!(!text.contains(leaked), "{leaked}");
        }
    }
}

#[test]
fn unit_log_pretty_is_human_readable_and_redacted() {
    let secret = Secret::new(SECRET_SENTINEL);
    let output = capture("info", LogFormat::Pretty, || {
        let _span = tracing::info_span!("request", request_id = "req-9").entered();
        tracing::info!(secret_key = ?secret, "pretty.event");
    });

    let text = output.text();
    assert!(text.contains("pretty.event"));
    assert!(text.contains("request_id"));
    assert!(text.contains("req-9"));
    assert!(text.contains("<redacted>"));
    assert!(!text.contains(SECRET_SENTINEL));
    assert!(!text.contains('\u{1b}'));
    assert!(serde_json::from_str::<Value>(text.lines().next().unwrap()).is_err());
}

#[test]
fn unit_log_level_filter_applied() {
    let output = capture("warn,palmr_server::infra=debug", LogFormat::Json, || {
        tracing::info!(target: "elsewhere", "dropped");
        tracing::warn!(target: "elsewhere", "kept");
        tracing::debug!(target: "palmr_server::infra::telemetry", "kept");
        tracing::trace!(target: "palmr_server::infra::telemetry", "dropped");
    });

    let messages: Vec<Value> = output
        .json_lines()
        .into_iter()
        .map(|line| line["message"].clone())
        .collect();
    assert_eq!(messages, ["kept", "kept"]);
}

#[test]
fn unit_log_filter_accepts_operator_directives() {
    for directives in [
        "info",
        "debug",
        "palmr=debug,info",
        "warn,tower_http=trace",
        "off",
    ] {
        assert!(parse_filter(directives).is_ok(), "{directives}");
    }
}

#[test]
fn unit_log_filter_invalid_is_tracing_init_failure() {
    let error = parse_filter("info,palmr=loud").unwrap_err();

    assert!(matches!(error, TelemetryInitError::InvalidFilter(_)));
    assert_eq!(error.code(), STARTUP_TRACING_INIT_FAILED);
    let rendered = error.to_string();
    assert!(rendered
        .starts_with("STARTUP_TRACING_INIT_FAILED: PALMR_LOG_LEVEL is not a valid log filter"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn unit_telemetry_init_installs_global_subscriber_once() {
    let invalid = init("info,palmr=loud", LogFormat::Json).unwrap_err();
    assert!(matches!(invalid, TelemetryInitError::InvalidFilter(_)));

    init("off", LogFormat::Json).unwrap();
    let second = init("off", LogFormat::Json).unwrap_err();

    assert!(matches!(
        second,
        TelemetryInitError::SubscriberAlreadyInstalled
    ));
    assert_eq!(second.code(), STARTUP_TRACING_INIT_FAILED);
    assert_eq!(
        second.to_string(),
        "STARTUP_TRACING_INIT_FAILED: a global tracing subscriber is already installed"
    );
}
