use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::domain::alias::Alias;
use crate::domain::email::{Email, InvalidEmail};
use crate::domain::id::Id;
use crate::domain::locale::LocaleCode;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::jobs::DedupKey;

pub const MAX_DISPLAY_CHARS: usize = 200;
pub const MAX_RECIPIENT_NAME_CHARS: usize = 100;
pub const MAX_FILE_NAMES: usize = 20;
pub const MAX_SUBJECT_CHARS: usize = 200;
pub const MAX_PARAMS_BYTES: usize = 16 * 1024;
pub const MAX_BATCH_KEY_CHARS: usize = 256;

pub enum Outbox {}

pub type OutboxId = Id<Outbox>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MailKind {
    PasswordReset,
    EmailVerification,
    Invite,
    ShareRecipientNotify,
    ReverseShareOwnerNotify,
    SecurityNotification,
}

impl MailKind {
    pub const ALL: [Self; 6] = [
        Self::PasswordReset,
        Self::EmailVerification,
        Self::Invite,
        Self::ShareRecipientNotify,
        Self::ReverseShareOwnerNotify,
        Self::SecurityNotification,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PasswordReset => "password_reset",
            Self::EmailVerification => "email_verification",
            Self::Invite => "invite",
            Self::ShareRecipientNotify => "share_recipient_notify",
            Self::ReverseShareOwnerNotify => "reverse_share_owner_notify",
            Self::SecurityNotification => "security_notification",
        }
    }

    pub const fn carries_sealed_token(self) -> bool {
        matches!(
            self,
            Self::PasswordReset | Self::EmailVerification | Self::Invite
        )
    }
}

impl fmt::Display for MailKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownMailKind;

impl fmt::Display for UnknownMailKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("mail kind is not one of the transactional e-mail kinds")
    }
}

impl std::error::Error for UnknownMailKind {}

impl FromStr for MailKind {
    type Err = UnknownMailKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or(UnknownMailKind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxState {
    Pending,
    Sending,
    Sent,
    Failed,
    Canceled,
}

impl OutboxState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Sent | Self::Failed | Self::Canceled)
    }

    pub const fn is_deliverable(self) -> bool {
        matches!(self, Self::Pending | Self::Sending)
    }
}

impl FromStr for OutboxState {
    type Err = UnknownMailKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "pending" => Ok(Self::Pending),
            "sending" => Ok(Self::Sending),
            "sent" => Ok(Self::Sent),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            _ => Err(UnknownMailKind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityEvent {
    PasswordChanged,
    TwoFactorEnabled,
    TwoFactorDisabled,
    SessionsRevoked,
}

impl SecurityEvent {
    pub const ALL: [Self; 4] = [
        Self::PasswordChanged,
        Self::TwoFactorEnabled,
        Self::TwoFactorDisabled,
        Self::SessionsRevoked,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PasswordChanged => "password_changed",
            Self::TwoFactorEnabled => "two_factor_enabled",
            Self::TwoFactorDisabled => "two_factor_disabled",
            Self::SessionsRevoked => "sessions_revoked",
        }
    }
}

impl fmt::Display for SecurityEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum ParamsError {
    Json(serde_json::Error),
    KindMismatch,
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(_) => f.write_str("mail parameters are not valid JSON"),
            Self::KindMismatch => f.write_str("mail parameters do not match the outbox kind"),
        }
    }
}

impl std::error::Error for ParamsError {}

impl From<serde_json::Error> for ParamsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl<'de> Deserialize<'de> for SecurityEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::ALL
            .into_iter()
            .find(|event| event.as_str() == text)
            .ok_or_else(|| serde::de::Error::custom("unknown security notification event"))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DisplayText(String);

impl DisplayText {
    pub fn new(text: &str) -> Self {
        Self::truncated(text, MAX_DISPLAY_CHARS)
    }

    pub fn truncated(text: &str, max_chars: usize) -> Self {
        let mut bounded = String::with_capacity(text.len().min(max_chars));
        let mut count = 0;
        for ch in text.chars() {
            if ch.is_control() {
                continue;
            }
            if count == max_chars {
                break;
            }
            bounded.push(ch);
            count += 1;
        }
        Self(bounded)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for DisplayText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DisplayText").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for DisplayText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Ok(Self::new(&text))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FileNames(Vec<DisplayText>);

impl FileNames {
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self(
            names
                .into_iter()
                .take(MAX_FILE_NAMES)
                .map(|name| DisplayText::new(name.as_ref()))
                .collect(),
        )
    }

    pub fn joined(&self) -> String {
        self.0
            .iter()
            .map(DisplayText::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for FileNames {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let names = Vec::<String>::deserialize(deserializer)?;
        Ok(Self::new(names))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum MailParams {
    PasswordReset {
        expiry_minutes: u32,
    },
    EmailVerification {
        expiry_minutes: u32,
    },
    Invite {
        inviter_name: DisplayText,
        expiry_hours: u32,
    },
    ShareRecipientNotify {
        sender_name: DisplayText,
        share_name: DisplayText,
        share_alias: DisplayText,
        file_names: FileNames,
    },
    ReverseShareOwnerNotify {
        uploader_name: DisplayText,
        link_name: DisplayText,
        file_names: FileNames,
    },
    SecurityNotification {
        event: SecurityEvent,
    },
}

impl MailParams {
    pub const fn kind(&self) -> MailKind {
        match self {
            Self::PasswordReset { .. } => MailKind::PasswordReset,
            Self::EmailVerification { .. } => MailKind::EmailVerification,
            Self::Invite { .. } => MailKind::Invite,
            Self::ShareRecipientNotify { .. } => MailKind::ShareRecipientNotify,
            Self::ReverseShareOwnerNotify { .. } => MailKind::ReverseShareOwnerNotify,
            Self::SecurityNotification { .. } => MailKind::SecurityNotification,
        }
    }

    pub fn from_json(kind: MailKind, json: &str) -> Result<Self, ParamsError> {
        let params: Self = serde_json::from_str(json)?;
        if params.kind() == kind {
            Ok(params)
        } else {
            Err(ParamsError::KindMismatch)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recipient {
    email: Email,
    name: Option<DisplayText>,
}

impl Recipient {
    pub fn new(email: Email, name: Option<&str>) -> Self {
        Self {
            email,
            name: name.map(|name| DisplayText::truncated(name, MAX_RECIPIENT_NAME_CHARS)),
        }
    }

    pub fn email(&self) -> &Email {
        &self.email
    }

    pub fn name(&self) -> Option<&DisplayText> {
        self.name.as_ref()
    }
}

impl TryFrom<(&str, Option<&str>)> for Recipient {
    type Error = InvalidEmail;

    fn try_from((email, name): (&str, Option<&str>)) -> Result<Self, Self::Error> {
        Ok(Self::new(Email::parse(email)?, name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalePreference {
    Account(LocaleCode),
    Explicit(LocaleCode),
    InstanceDefault,
}

pub fn resolve_locale(
    preference: LocalePreference,
    instance_default: Option<LocaleCode>,
) -> LocaleCode {
    match preference {
        LocalePreference::Account(locale) | LocalePreference::Explicit(locale) => locale,
        LocalePreference::InstanceDefault => instance_default.unwrap_or(LocaleCode::EnUs),
    }
}

pub fn outbox_token_aad(id: OutboxId, kind: MailKind) -> Vec<u8> {
    let mut aad = Vec::with_capacity(12 + 1 + 36 + 1 + kind.as_str().len());
    aad.extend_from_slice(b"email_outbox");
    aad.push(0);
    aad.extend_from_slice(id.to_string().as_bytes());
    aad.push(0);
    aad.extend_from_slice(kind.as_str().as_bytes());
    aad
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    KindMismatch,
    InvalidAlias,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KindMismatch => f.write_str("mail parameters do not match the outbox kind"),
            Self::InvalidAlias => f.write_str("mail link alias is invalid"),
        }
    }
}

impl std::error::Error for LinkError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailLink {
    PasswordReset,
    EmailVerification,
    Invite,
    Share(Alias),
    ReverseShares,
    Security,
}

impl MailLink {
    pub const fn carries_token(&self) -> bool {
        matches!(
            self,
            Self::PasswordReset | Self::EmailVerification | Self::Invite
        )
    }

    pub fn path(&self) -> String {
        match self {
            Self::PasswordReset => "/reset-password".to_owned(),
            Self::EmailVerification => "/verify-email".to_owned(),
            Self::Invite => "/invite".to_owned(),
            Self::Share(alias) => format!("/s/{}", alias.as_str()),
            Self::ReverseShares => "/reverse-shares".to_owned(),
            Self::Security => "/settings/security".to_owned(),
        }
    }
}

pub fn link_from(kind: MailKind, params: &MailParams) -> Result<MailLink, LinkError> {
    match (kind, params) {
        (MailKind::PasswordReset, MailParams::PasswordReset { .. }) => Ok(MailLink::PasswordReset),
        (MailKind::EmailVerification, MailParams::EmailVerification { .. }) => {
            Ok(MailLink::EmailVerification)
        }
        (MailKind::Invite, MailParams::Invite { .. }) => Ok(MailLink::Invite),
        (MailKind::ShareRecipientNotify, MailParams::ShareRecipientNotify { share_alias, .. }) => {
            Alias::parse(share_alias.as_str())
                .map(MailLink::Share)
                .map_err(|_| LinkError::InvalidAlias)
        }
        (MailKind::ReverseShareOwnerNotify, MailParams::ReverseShareOwnerNotify { .. }) => {
            Ok(MailLink::ReverseShares)
        }
        (MailKind::SecurityNotification, MailParams::SecurityNotification { .. }) => {
            Ok(MailLink::Security)
        }
        _ => Err(LinkError::KindMismatch),
    }
}

pub fn public_link(base_url: &url::Url, link: &MailLink, token: Option<&str>) -> String {
    let mut url = base_url.as_str().trim_end_matches('/').to_owned();
    url.push_str(&link.path());
    if link.carries_token() {
        if let Some(token) = token {
            url.push('/');
            url.push_str(&percent_encode_segment(token));
        }
    }
    url
}

fn percent_encode_segment(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[derive(Debug)]
pub struct NewMail {
    pub kind: MailKind,
    pub recipient: Recipient,
    pub locale_preference: LocalePreference,
    pub params: MailParams,
    pub token: Option<Secret<String>>,
    pub dedup_key: Option<DedupKey>,
    pub batch_key: Option<String>,
    pub scheduled_at: Option<Timestamp>,
}

impl NewMail {
    pub fn new(
        kind: MailKind,
        recipient: Recipient,
        locale_preference: LocalePreference,
        params: MailParams,
    ) -> Self {
        Self {
            kind,
            recipient,
            locale_preference,
            params,
            token: None,
            dedup_key: None,
            batch_key: None,
            scheduled_at: None,
        }
    }

    #[must_use]
    pub fn with_token(mut self, token: Secret<String>) -> Self {
        self.token = Some(token);
        self
    }

    #[must_use]
    pub fn with_dedup_key(mut self, dedup_key: DedupKey) -> Self {
        self.dedup_key = Some(dedup_key);
        self
    }

    #[must_use]
    pub fn with_batch_key(mut self, batch_key: impl Into<String>) -> Self {
        self.batch_key = Some(batch_key.into());
        self
    }

    #[must_use]
    pub const fn with_scheduled_at(mut self, scheduled_at: Timestamp) -> Self {
        self.scheduled_at = Some(scheduled_at);
        self
    }
}

#[derive(Debug)]
pub struct OutboxRow {
    pub id: OutboxId,
    pub kind: MailKind,
    pub to_email: Email,
    pub to_name: Option<DisplayText>,
    pub locale: LocaleCode,
    pub params: MailParams,
    pub state: OutboxState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub sealed_token: Option<SealedSecret>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_mail_kind_closed_set() {
        let names: Vec<&str> = MailKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            names,
            [
                "password_reset",
                "email_verification",
                "invite",
                "share_recipient_notify",
                "reverse_share_owner_notify",
                "security_notification",
            ]
        );
        for kind in MailKind::ALL {
            assert_eq!(kind.as_str().parse(), Ok(kind));
            assert!(kind.as_str().len() <= 64);
        }
        assert_eq!("arbitrary".parse::<MailKind>(), Err(UnknownMailKind));
        assert!(MailKind::PasswordReset.carries_sealed_token());
        assert!(!MailKind::ShareRecipientNotify.carries_sealed_token());
    }

    #[test]
    fn unit_outbox_state_terminal() {
        assert!(OutboxState::Sent.is_terminal());
        assert!(OutboxState::Failed.is_terminal());
        assert!(OutboxState::Canceled.is_terminal());
        assert!(OutboxState::Pending.is_deliverable());
        assert!(OutboxState::Sending.is_deliverable());
        for state in [
            OutboxState::Pending,
            OutboxState::Sending,
            OutboxState::Sent,
            OutboxState::Failed,
            OutboxState::Canceled,
        ] {
            assert_eq!(state.as_str().parse(), Ok(state));
        }
        assert_eq!("unknown".parse::<OutboxState>(), Err(UnknownMailKind));
    }

    #[test]
    fn unit_display_text_bounds_and_strips_controls() {
        let long = "a".repeat(10_000);
        let bounded = DisplayText::new(&long);
        assert_eq!(bounded.as_str().chars().count(), MAX_DISPLAY_CHARS);
        assert!(!bounded.as_str().contains('\u{0}'));

        let hostile = "line\r\nbreak\u{0}\u{202e}";
        let stripped = DisplayText::new(hostile);
        assert!(!stripped.as_str().contains('\r'));
        assert!(!stripped.as_str().contains('\n'));
        assert!(stripped.as_str().contains('\u{202e}'));
    }

    #[test]
    fn unit_file_names_bounded() {
        let names = FileNames::new((0..100).map(|index| format!("file-{index}").repeat(200)));
        assert_eq!(names.len(), MAX_FILE_NAMES);
        assert!(names.joined().chars().count() > 0);
        assert!(!names.is_empty());
        assert!(FileNames::new(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn unit_mail_params_round_trip_and_kind_check() {
        let params = MailParams::ShareRecipientNotify {
            sender_name: DisplayText::new("Ada"),
            share_name: DisplayText::new("Docs"),
            share_alias: DisplayText::new("abc123"),
            file_names: FileNames::new(["a.txt", "b.txt"]),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"template\":\"share_recipient_notify\""));
        let parsed = MailParams::from_json(MailKind::ShareRecipientNotify, &json).unwrap();
        assert_eq!(parsed, params);
        assert!(MailParams::from_json(MailKind::Invite, &json).is_err());

        let event = MailParams::SecurityNotification {
            event: SecurityEvent::TwoFactorEnabled,
        };
        let event_json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            MailParams::from_json(MailKind::SecurityNotification, &event_json).unwrap(),
            event
        );
        assert!(serde_json::from_str::<SecurityEvent>("\"unknown\"").is_err());
    }

    #[test]
    fn unit_email_locale_resolution() {
        assert_eq!(
            resolve_locale(
                LocalePreference::Account(LocaleCode::FrFr),
                Some(LocaleCode::DeDe)
            ),
            LocaleCode::FrFr
        );
        assert_eq!(
            resolve_locale(
                LocalePreference::Explicit(LocaleCode::JaJp),
                Some(LocaleCode::DeDe)
            ),
            LocaleCode::JaJp
        );
        assert_eq!(
            resolve_locale(LocalePreference::InstanceDefault, Some(LocaleCode::PtBr)),
            LocaleCode::PtBr
        );
        assert_eq!(
            resolve_locale(LocalePreference::InstanceDefault, None),
            LocaleCode::EnUs
        );
        for locale in LocaleCode::ALL {
            assert_eq!(
                resolve_locale(LocalePreference::Explicit(*locale), None),
                *locale
            );
        }
    }

    #[test]
    fn unit_outbox_token_aad_is_row_bound() {
        use crate::domain::clock::TestClock;
        use time::macros::datetime;

        let clock = TestClock::new(datetime!(2026-09-24 12:00 UTC));
        let first = OutboxId::generate(&clock);
        let second = OutboxId::generate(&clock);

        assert_ne!(
            outbox_token_aad(first, MailKind::PasswordReset),
            outbox_token_aad(second, MailKind::PasswordReset)
        );
        assert_ne!(
            outbox_token_aad(first, MailKind::PasswordReset),
            outbox_token_aad(first, MailKind::Invite)
        );
        assert_eq!(
            outbox_token_aad(first, MailKind::PasswordReset),
            outbox_token_aad(first, MailKind::PasswordReset)
        );
    }
}
