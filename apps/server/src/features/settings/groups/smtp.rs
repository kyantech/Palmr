use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Field, Kind};
use crate::domain::email::Email;
use crate::features::settings::model::{AppSettings, SmtpSecurity};
use crate::features::users::model::display_text;

pub const PASSWORD_KEY: &str = "smtp_password";
pub const USERNAME_KEY: &str = "smtp_username";

pub const FIELDS: &[Field] = &[
    Field::new("enabled", "smtp_enabled", Kind::Flag),
    Field::new("host", "smtp_host", Kind::SmtpHost),
    Field::new("port", "smtp_port", Kind::SmtpPort),
    Field::new("security", "smtp_security", Kind::SmtpSecurity),
    Field::new("username", USERNAME_KEY, Kind::SmtpUsername),
    Field::new("password", PASSWORD_KEY, Kind::Secret("passwordConfigured")),
    Field::new("fromName", "smtp_from_name", Kind::SmtpFromName),
    Field::new("fromEmail", "smtp_from_email", Kind::SmtpFromEmail),
    Field::new(
        "allowSelfSignedCertificate",
        "smtp_allow_self_signed_certificate",
        Kind::Flag,
    ),
    Field::new("noAuth", "smtp_no_auth", Kind::Flag),
];

pub fn host(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let acceptable = ServerName::try_from(trimmed).is_ok() && !trimmed.contains(['/', '@', '\\']);
    acceptable.then(|| trimmed.to_owned())
}

pub fn port(number: i64) -> Option<u16> {
    u16::try_from(number).ok().filter(|port| *port != 0)
}

pub const MAX_STORED_JSON_CHARS: usize = 65_536;

pub fn username(text: &str) -> Option<String> {
    let stored = serde_json::Value::from(text).to_string();
    (!text.is_empty() && stored.chars().count() <= MAX_STORED_JSON_CHARS).then(|| text.to_owned())
}

pub fn password(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

pub fn sender_name(text: &str) -> Option<String> {
    display_text(text)
}

pub fn sender_email(text: &str) -> Option<String> {
    Email::parse(text)
        .ok()
        .map(|email| email.as_str().to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SmtpSettings {
    pub enabled: bool,
    #[schema(required = true, example = "smtp.example.com")]
    pub host: Option<String>,
    #[schema(minimum = 1, maximum = 65535, example = 587)]
    pub port: u16,
    pub security: SmtpSecurity,
    #[schema(required = true)]
    pub username: Option<String>,
    /// Whether a password is stored. The password itself is write-only and never returned.
    pub password_configured: bool,
    #[schema(required = true, example = "Palmr")]
    pub from_name: Option<String>,
    #[schema(required = true, example = "palmr@example.com")]
    pub from_email: Option<String>,
    pub allow_self_signed_certificate: bool,
    pub no_auth: bool,
}

impl From<&AppSettings> for SmtpSettings {
    fn from(settings: &AppSettings) -> Self {
        let smtp = &settings.smtp;
        Self {
            enabled: smtp.enabled,
            host: smtp.host.clone(),
            port: smtp.port,
            security: smtp.security,
            username: smtp.username.clone(),
            password_configured: smtp.password.is_some(),
            from_name: smtp.from_name.clone(),
            from_email: smtp.from_email.clone(),
            allow_self_signed_certificate: smtp.allow_self_signed_certificate,
            no_auth: smtp.no_auth,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SmtpPatch {
    #[schema(nullable = false)]
    pub enabled: Option<bool>,
    /// A DNS name or IP address. Absent leaves it unchanged; explicit `null` clears it.
    #[schema(value_type = Option<String>, nullable = true, example = "smtp.example.com")]
    pub host: Option<Option<String>>,
    #[schema(nullable = false, minimum = 1, maximum = 65535, example = 587)]
    pub port: Option<u16>,
    #[schema(nullable = false)]
    pub security: Option<SmtpSecurity>,
    /// Absent leaves it unchanged; explicit `null` clears it.
    #[schema(value_type = Option<String>, nullable = true)]
    pub username: Option<Option<String>>,
    /// Write-only. Absent leaves the stored password unchanged, a string replaces it and explicit `null` clears it.
    #[schema(value_type = Option<String>, nullable = true, write_only, min_length = 1)]
    pub password: Option<Option<String>>,
    /// Absent leaves it unchanged; explicit `null` clears it.
    #[schema(value_type = Option<String>, nullable = true, max_length = 100)]
    pub from_name: Option<Option<String>>,
    /// Absent leaves it unchanged; explicit `null` clears it.
    #[schema(value_type = Option<String>, nullable = true, example = "palmr@example.com")]
    pub from_email: Option<Option<String>>,
    #[schema(nullable = false)]
    pub allow_self_signed_certificate: Option<bool>,
    #[schema(nullable = false)]
    pub no_auth: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_smtp_host_accepts_dns_names_and_ip_literals_only() {
        for accepted in [
            "smtp.example.com",
            "SMTP.Example.COM",
            "mailhost",
            "127.0.0.1",
            "::1",
            "2001:db8::25",
        ] {
            assert_eq!(host(accepted).as_deref(), Some(accepted), "{accepted}");
        }
        assert_eq!(
            host("  smtp.example.com \t").as_deref(),
            Some("smtp.example.com")
        );
        let too_long = format!("{}.example.com", "a".repeat(250));
        for rejected in [
            "",
            "   ",
            "smtp host",
            "https://smtp.example.com",
            "smtp.example.com:587",
            "smtp.example.com/path",
            "user@smtp.example.com",
            "smtp\\example.com",
            "-bad.example.com",
            too_long.as_str(),
        ] {
            assert_eq!(host(rejected), None, "{rejected:?}");
        }
    }

    #[test]
    fn unit_smtp_scalar_validators_bound_their_input() {
        assert_eq!(port(1), Some(1));
        assert_eq!(port(587), Some(587));
        assert_eq!(port(65_535), Some(65_535));
        for rejected in [0, -1, 65_536, i64::MAX] {
            assert_eq!(port(rejected), None, "{rejected}");
        }
        assert_eq!(
            username("mailer@example.com").as_deref(),
            Some("mailer@example.com")
        );
        let long = "u".repeat(MAX_STORED_JSON_CHARS - 2);
        assert_eq!(username(&long).as_deref(), Some(long.as_str()));
        assert_eq!(username(&"u".repeat(MAX_STORED_JSON_CHARS - 1)), None);
        assert_eq!(username("line\nbreak").as_deref(), Some("line\nbreak"));
        assert_eq!(username(""), None);
        assert_eq!(password("p"), Some("p"));
        assert_eq!(password(" spaced and \u{e9} "), Some(" spaced and \u{e9} "));
        let long = "p".repeat(1 << 20);
        assert_eq!(password(&long).map(str::len), Some(1 << 20));
        assert_eq!(password(""), None);
        assert_eq!(
            sender_name("  Palmr Files ").as_deref(),
            Some("Palmr Files")
        );
        assert_eq!(sender_name("   "), None);
        assert_eq!(sender_name("bad\nname"), None);
        assert_eq!(
            sender_email("Palmr@Example.COM").as_deref(),
            Some("Palmr@Example.COM")
        );
        for rejected in ["", "nope", "a@@b", "a b@c.test", "a@b@c"] {
            assert_eq!(sender_email(rejected), None, "{rejected:?}");
        }
    }
}
