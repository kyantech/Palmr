use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::time::Instant;
use utoipa::ToSchema;

use super::groups::smtp;
use super::model::{SmtpSecurity, SmtpSettings};
use super::snapshot::SettingsHandle;
use crate::domain::email::Email;
use crate::domain::secret::Secret;
use crate::features::email::transport::{
    EmailTransport, FailureReason, OutboundMailbox, OutboundMessage, ProbeFailure, SmtpConfig,
    SmtpGap, Stage, StageTracker,
};

pub const TEST_DEADLINE: Duration = Duration::from_secs(20);

const TO: &str = "to";
const UNSAVED: &str = "useUnsavedSettings";
const UNSAVED_FIELDS: [&str; 9] = [
    "host",
    "port",
    "security",
    "username",
    "password",
    "fromName",
    "fromEmail",
    "allowSelfSignedCertificate",
    "noAuth",
];

#[derive(Debug)]
pub struct SmtpTestInput {
    pub to: Email,
    pub unsaved: Option<SmtpSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpTestReport {
    pub stages: Vec<Stage>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpTestError {
    Invalid { field: &'static str },
    Failed(ProbeFailure),
}

pub fn parse_request(body: &Value) -> Result<SmtpTestInput, Vec<&'static str>> {
    let Value::Object(members) = body else {
        return Err(vec!["body"]);
    };
    let mut invalid = Vec::new();
    if members.keys().any(|name| name != TO && name != UNSAVED) {
        invalid.push("body");
    }
    let to = members
        .get(TO)
        .and_then(Value::as_str)
        .and_then(|text| Email::parse(text).ok());
    if to.is_none() {
        invalid.push(TO);
    }
    let unsaved = match members.get(UNSAVED) {
        None | Some(Value::Null) => None,
        Some(Value::Object(unsaved)) => match parse_unsaved(unsaved) {
            Ok(settings) => Some(settings),
            Err(fields) => {
                invalid.extend(fields);
                None
            }
        },
        Some(_) => {
            invalid.push(UNSAVED);
            None
        }
    };
    match to {
        Some(to) if invalid.is_empty() => Ok(SmtpTestInput { to, unsaved }),
        _ => Err(invalid),
    }
}

fn parse_unsaved(members: &Map<String, Value>) -> Result<SmtpSettings, Vec<&'static str>> {
    let mut invalid = Vec::new();
    if members
        .keys()
        .any(|name| !UNSAVED_FIELDS.contains(&name.as_str()))
    {
        invalid.push(UNSAVED);
    }
    let mut take = |name: &'static str, ok: bool| {
        if !ok {
            invalid.push(unsaved_field(name));
        }
    };

    let host = members
        .get("host")
        .and_then(Value::as_str)
        .and_then(smtp::host);
    take("host", host.is_some());
    let port = members
        .get("port")
        .and_then(Value::as_i64)
        .and_then(smtp::port);
    take("port", port.is_some());
    let security = members
        .get("security")
        .and_then(Value::as_str)
        .and_then(SmtpSecurity::parse);
    take("security", security.is_some());
    let from_email = members
        .get("fromEmail")
        .and_then(Value::as_str)
        .and_then(smtp::sender_email);
    take("fromEmail", from_email.is_some());

    let username = optional(members, "username", smtp::username);
    take("username", username.is_some());
    let password = optional(members, "password", |text| {
        smtp::password(text).map(|text| Secret::new(text.to_owned()))
    });
    take("password", password.is_some());
    let from_name = optional(members, "fromName", smtp::sender_name);
    take("fromName", from_name.is_some());
    let allow_self_signed = flag(members, "allowSelfSignedCertificate");
    take("allowSelfSignedCertificate", allow_self_signed.is_some());
    let no_auth = flag(members, "noAuth");
    take("noAuth", no_auth.is_some());

    match (
        host,
        port,
        security,
        from_email,
        username,
        password,
        from_name,
        allow_self_signed,
        no_auth,
    ) {
        (
            Some(host),
            Some(port),
            Some(security),
            Some(from_email),
            Some(username),
            Some(password),
            Some(from_name),
            Some(allow_self_signed_certificate),
            Some(no_auth),
        ) if invalid.is_empty() => Ok(SmtpSettings {
            enabled: true,
            host: Some(host),
            port,
            security,
            username,
            password,
            from_name,
            from_email: Some(from_email),
            allow_self_signed_certificate,
            no_auth,
        }),
        _ => Err(invalid),
    }
}

fn optional<T>(
    members: &Map<String, Value>,
    name: &str,
    accept: impl FnOnce(&str) -> Option<T>,
) -> Option<Option<T>> {
    match members.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => accept(text).map(Some),
        Some(_) => None,
    }
}

fn flag(members: &Map<String, Value>, name: &str) -> Option<bool> {
    match members.get(name) {
        None | Some(Value::Null) => Some(false),
        Some(Value::Bool(flag)) => Some(*flag),
        Some(_) => None,
    }
}

const fn unsaved_field(name: &str) -> &'static str {
    match name.as_bytes() {
        b"host" => "useUnsavedSettings.host",
        b"port" => "useUnsavedSettings.port",
        b"security" => "useUnsavedSettings.security",
        b"username" => "useUnsavedSettings.username",
        b"password" => "useUnsavedSettings.password",
        b"fromName" => "useUnsavedSettings.fromName",
        b"fromEmail" => "useUnsavedSettings.fromEmail",
        b"allowSelfSignedCertificate" => "useUnsavedSettings.allowSelfSignedCertificate",
        b"noAuth" => "useUnsavedSettings.noAuth",
        _ => UNSAVED,
    }
}

const fn gap_field(gap: SmtpGap, unsaved: bool) -> &'static str {
    if unsaved {
        match gap {
            SmtpGap::Host => "useUnsavedSettings.host",
            SmtpGap::FromEmail => "useUnsavedSettings.fromEmail",
            SmtpGap::Username => "useUnsavedSettings.username",
            SmtpGap::Password => "useUnsavedSettings.password",
        }
    } else {
        gap.field()
    }
}

#[derive(Clone)]
pub struct SmtpTestService {
    settings: SettingsHandle,
    transport: Arc<dyn EmailTransport>,
}

impl std::fmt::Debug for SmtpTestService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpTestService").finish_non_exhaustive()
    }
}

impl SmtpTestService {
    pub fn new(settings: SettingsHandle, transport: Arc<dyn EmailTransport>) -> Self {
        Self {
            settings,
            transport,
        }
    }

    pub async fn run(&self, input: SmtpTestInput) -> Result<SmtpTestReport, SmtpTestError> {
        let current = self.settings.load();
        let from_unsaved = input.unsaved.is_some();
        let config = match &input.unsaved {
            Some(unsaved) => SmtpConfig::resolve(unsaved),
            None => SmtpConfig::resolve(&current.smtp),
        }
        .map_err(|gap| SmtpTestError::Invalid {
            field: gap_field(gap, from_unsaved),
        })?;
        let message = test_message(&config, input.to, current.app_name());
        drop(current);

        let progress = StageTracker::default();
        let started = Instant::now();
        let outcome = tokio::time::timeout(
            TEST_DEADLINE,
            self.transport.probe(&config, &message, &progress),
        )
        .await;
        let failure = match outcome {
            Ok(Ok(())) => {
                return Ok(SmtpTestReport {
                    stages: progress.completed(),
                    duration_ms: millis(started.elapsed()),
                });
            }
            Ok(Err(failure)) => failure,
            Err(_elapsed) => ProbeFailure::new(progress.current(), FailureReason::TimedOut),
        };
        tracing::warn!(
            stage = failure.stage.as_str(),
            reason = ?failure.reason,
            "the SMTP test failed"
        );
        Err(SmtpTestError::Failed(failure))
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

fn test_message(config: &SmtpConfig, to: Email, app_name: &str) -> OutboundMessage {
    let subject = format!("{app_name} SMTP test");
    let text =
        format!("This is a test message from {app_name}. Your SMTP settings can deliver e-mail.");
    let html = format!("<p>{}</p>", escape(&text));
    OutboundMessage {
        from: OutboundMailbox {
            name: config.from_name.clone(),
            email: config.from_email.clone(),
        },
        to: OutboundMailbox {
            name: None,
            email: to,
        },
        subject,
        html,
        text,
    }
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SmtpTestRequest {
    /// The recipient of the test message.
    #[schema(example = "ada@example.com")]
    pub to: String,
    /// When present the test runs against these values without saving them. They are
    /// self-contained: nothing is borrowed from the saved configuration, including the
    /// saved password.
    pub use_unsaved_settings: Option<SmtpUnsavedSettings>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SmtpUnsavedSettings {
    pub host: String,
    #[schema(minimum = 1, maximum = 65535)]
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    /// Write-only; used for this test only and never stored or echoed.
    #[schema(write_only)]
    pub password: Option<String>,
    pub from_name: Option<String>,
    pub from_email: String,
    #[serde(default)]
    pub allow_self_signed_certificate: bool,
    #[serde(default)]
    pub no_auth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTestStageName {
    Connect,
    Starttls,
    Auth,
    Send,
}

impl From<Stage> for SmtpTestStageName {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Connect => Self::Connect,
            Stage::Starttls => Self::Starttls,
            Stage::Auth => Self::Auth,
            Stage::Send => Self::Send,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct SmtpTestStage {
    pub name: SmtpTestStageName,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SmtpTestResult {
    pub ok: bool,
    /// Only the stages the chosen security mode actually executes, in order.
    pub stages: Vec<SmtpTestStage>,
    pub duration_ms: u64,
}

impl From<SmtpTestReport> for SmtpTestResult {
    fn from(report: SmtpTestReport) -> Self {
        Self {
            ok: true,
            stages: report
                .stages
                .into_iter()
                .map(|stage| SmtpTestStage {
                    name: stage.into(),
                    ok: true,
                })
                .collect(),
            duration_ms: report.duration_ms,
        }
    }
}

#[cfg(test)]
mod tests;
