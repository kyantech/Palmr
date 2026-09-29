use serde::Serialize;
use time::Duration;
use utoipa::ToSchema;

use crate::domain::id::Id;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::settings::model::AppSettings;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::crypto::CryptoError;

pub enum TrustedDevice {}
pub type TrustedDeviceId = Id<TrustedDevice>;

pub const LABEL_MAX_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevicePolicy {
    pub enabled: bool,
    pub duration_days: u32,
}

impl TrustedDevicePolicy {
    pub fn from_settings(settings: &AppSettings) -> Self {
        let security = &settings.security;
        Self {
            enabled: security.trusted_devices_enabled && security.trusted_device_duration_days > 0,
            duration_days: security.trusted_device_duration_days,
        }
    }

    pub fn lifetime(self) -> Option<Duration> {
        self.enabled
            .then(|| Duration::days(i64::from(self.duration_days)))
    }
}

pub struct PreparedDevice {
    pub(super) token: Secret<String>,
    pub(super) token_hash: TokenDigest,
}

impl PreparedDevice {
    pub fn mint() -> Result<Self, CryptoError> {
        let token = Token::mint()?;
        Ok(Self {
            token_hash: token.digest(),
            token: token.encode(),
        })
    }
}

pub struct IssuedDevice {
    pub token: Secret<String>,
    pub expires_at: Timestamp,
}

pub struct NewTrustedDevice<'a> {
    pub id: TrustedDeviceId,
    pub user_id: UserId,
    pub token_hash: &'a TokenDigest,
    pub label: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub ip_address: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

pub struct TrustedDeviceRecord {
    pub id: TrustedDeviceId,
    pub token_hash: TokenDigest,
    pub label: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDeviceItem {
    pub id: String,
    /// Display metadata derived from the enrolling User-Agent; never part of trust.
    #[schema(required = true)]
    pub label: Option<String>,
    /// Display metadata; recorded only when the client address came through a trusted proxy.
    #[schema(required = true)]
    pub ip_at_enrollment: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    /// True only when this request presented the device's own `palmr_device` token.
    pub is_current: bool,
}

impl TrustedDeviceItem {
    pub fn from_record(record: TrustedDeviceRecord, presented: Option<&TokenDigest>) -> Self {
        Self {
            id: record.id.to_string(),
            is_current: presented.is_some_and(|digest| digest.verify(&record.token_hash)),
            label: record.label,
            ip_at_enrollment: record.ip_address,
            created_at: record.created_at.to_string(),
            last_seen_at: record.last_seen_at.to_string(),
            expires_at: record.expires_at.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDeviceList {
    pub items: Vec<TrustedDeviceItem>,
    /// Opaque cursor for the next page; `null` on the last page.
    #[schema(required = true)]
    pub next_cursor: Option<String>,
    #[schema(required = true, minimum = 0)]
    pub total_count: Option<u64>,
    pub policy: TrustedDevicePolicy,
}

pub fn device_label(user_agent: Option<&str>) -> Option<String> {
    let agent = user_agent?;
    let browser = BROWSERS
        .iter()
        .find(|(markers, _)| markers.iter().any(|marker| agent.contains(marker)))
        .map(|(_, name)| *name);
    let system = SYSTEMS
        .iter()
        .find(|(markers, _)| markers.iter().any(|marker| agent.contains(marker)))
        .map(|(_, name)| *name);
    let label = match (browser, system) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(name), None) | (None, Some(name)) => name.to_owned(),
        (None, None) => return None,
    };
    Some(label.chars().take(LABEL_MAX_CHARS).collect())
}

const BROWSERS: &[(&[&str], &str)] = &[
    (&["Edg/", "EdgA/", "EdgiOS/"], "Edge"),
    (&["OPR/", "Opera"], "Opera"),
    (&["SamsungBrowser/"], "Samsung Internet"),
    (&["Firefox/", "FxiOS/"], "Firefox"),
    (&["Chrome/", "CriOS/", "Chromium/"], "Chrome"),
    (&["Safari/"], "Safari"),
];

const SYSTEMS: &[(&[&str], &str)] = &[
    (&["Windows"], "Windows"),
    (&["iPhone", "iPad", "iPod"], "iOS"),
    (&["Android"], "Android"),
    (&["CrOS"], "ChromeOS"),
    (&["Mac OS X", "Macintosh"], "macOS"),
    (&["Linux"], "Linux"),
];

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::device_label;

    #[rstest]
    #[case(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.6; rv:130.0) Gecko/20100101 Firefox/130.0",
        Some("Firefox on macOS")
    )]
    #[case(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
        Some("Edge on Windows")
    )]
    #[case(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
        Some("Safari on iOS")
    )]
    #[case(
        "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36",
        Some("Chrome on Android")
    )]
    #[case("Mozilla/5.0 (X11; Linux x86_64)", Some("Linux"))]
    #[case("curl/8.9.1", None)]
    #[case("<script>alert(1)</script> Firefox/1.0", Some("Firefox"))]
    fn unit_trusted_device_label_uses_fixed_vocabulary(
        #[case] agent: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(device_label(Some(agent)).as_deref(), expected);
    }

    #[test]
    fn unit_trusted_device_label_absent_without_user_agent() {
        assert_eq!(device_label(None), None);
    }
}
