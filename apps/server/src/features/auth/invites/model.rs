use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::email::Email;
use crate::domain::id::Id;
use crate::domain::locale::LocaleCode;
use crate::domain::role::Role;
use crate::domain::secret::{Secret, REDACTED};
use crate::domain::time::Timestamp;
use crate::domain::username::Username;
use crate::features::users::model::{self as users, UserId};
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::error::InviteError;

pub enum Invite {}

pub type InviteId = Id<Invite>;

pub const MIN_VALIDITY_HOURS: u32 = 1;
pub const MAX_VALIDITY_HOURS: u32 = 720;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateInviteRequest {
    /// The address the invite is bound to; the account is created with it.
    #[schema(example = "new@example.com")]
    pub email: String,
    #[schema(example = "user")]
    pub role: String,
    /// Validity in hours (1–720); the admin-configured invite validity when omitted.
    #[schema(example = 24, minimum = 1, maximum = 720)]
    pub expires_in_hours: Option<i64>,
    /// Queue the invite e-mail. Requires a configured SMTP server.
    #[schema(example = true)]
    pub send_email: bool,
}

impl fmt::Debug for CreateInviteRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateInviteRequest")
            .field("role", &self.role)
            .field("expires_in_hours", &self.expires_in_hours)
            .field("send_email", &self.send_email)
            .finish_non_exhaustive()
    }
}

impl JsonRequest for CreateInviteRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("email", JsonKind::String),
        JsonField::required("role", JsonKind::String),
        JsonField::optional("expiresInHours", JsonKind::Integer),
        JsonField::required("sendEmail", JsonKind::Boolean),
    ];
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteResponse {
    pub id: String,
    /// The one-time invite link, built from the configured public base URL.
    #[schema(example = "https://palmr.example.com/invite/9f3c…")]
    pub invite_url: String,
    #[schema(value_type = String, format = DateTime, example = "2026-09-23T14:20:00.000Z")]
    pub expires_at: String,
}

impl fmt::Debug for CreateInviteResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateInviteResponse")
            .field("id", &self.id)
            .field("invite_url", &REDACTED)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteCreator {
    pub id: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteItem {
    pub id: String,
    #[schema(required = true, example = "new@example.com")]
    pub email: Option<String>,
    #[schema(example = "user")]
    pub role: &'static str,
    pub status: InviteStatus,
    pub created_by: InviteCreator,
    pub created_at: String,
    pub expires_at: String,
    #[schema(required = true)]
    pub accepted_at: Option<String>,
    #[schema(required = true)]
    pub accepted_user_id: Option<String>,
    /// When the invite e-mail was last queued for delivery; `null` if it never was.
    #[schema(required = true)]
    pub last_sent_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InviteLookupResponse {
    #[schema(example = true)]
    pub valid: bool,
    #[schema(example = "new@example.com")]
    pub email: String,
    #[schema(example = 8)]
    pub password_min_length: u32,
    #[schema(value_type = String, format = DateTime, example = "2026-09-23T14:20:00.000Z")]
    pub expires_at: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptInviteRequest {
    #[schema(example = "Alan")]
    pub first_name: String,
    #[schema(example = "Turing")]
    pub last_name: String,
    #[schema(example = "alan")]
    pub username: String,
    #[schema(format = Password)]
    pub password: String,
    #[schema(example = "en-US")]
    pub locale: String,
}

impl fmt::Debug for AcceptInviteRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptInviteRequest")
            .field("username", &self.username)
            .field("password", &REDACTED)
            .field("locale", &self.locale)
            .finish_non_exhaustive()
    }
}

impl JsonRequest for AcceptInviteRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("firstName", JsonKind::String),
        JsonField::required("lastName", JsonKind::String),
        JsonField::required("username", JsonKind::String),
        JsonField::required("password", JsonKind::String),
        JsonField::required("locale", JsonKind::String),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InviteStatus {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

impl InviteStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    // A `pending` row past `expires_at` is expired even before `tokens.prune`
    // moves it, so every reader reports one status for it.
    pub fn of(state: &str, expires_at: Timestamp, now: Timestamp) -> Option<Self> {
        match state {
            "pending" if expires_at > now => Some(Self::Pending),
            "pending" | "expired" => Some(Self::Expired),
            "accepted" => Some(Self::Accepted),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    pub fn into_live(self) -> Result<(), InviteError> {
        match self {
            Self::Pending => Ok(()),
            Self::Accepted => Err(InviteError::AlreadyUsed),
            Self::Revoked => Err(InviteError::Revoked),
            Self::Expired => Err(InviteError::Expired),
        }
    }
}

pub struct CreateInput {
    pub email: Email,
    pub role: Role,
    pub validity_hours: Option<u32>,
    pub send_email: bool,
}

impl CreateInput {
    pub fn parse(request: CreateInviteRequest) -> Result<Self, InviteError> {
        let mut invalid = Vec::new();
        let email = Email::parse(&request.email).ok();
        if email.is_none() {
            invalid.push("email");
        }
        let role = request.role.parse::<Role>().ok();
        if role.is_none() {
            invalid.push("role");
        }
        let validity_hours = match request.expires_in_hours {
            None => Some(None),
            Some(hours) => u32::try_from(hours)
                .ok()
                .filter(|hours| (MIN_VALIDITY_HOURS..=MAX_VALIDITY_HOURS).contains(hours))
                .map(Some),
        };
        if validity_hours.is_none() {
            invalid.push("expiresInHours");
        }
        match (email, role, validity_hours) {
            (Some(email), Some(role), Some(validity_hours)) => Ok(Self {
                email,
                role,
                validity_hours,
                send_email: request.send_email,
            }),
            _ => Err(InviteError::Invalid { fields: invalid }),
        }
    }
}

pub struct AcceptInput {
    pub first_name: String,
    pub last_name: String,
    pub username: Username,
    pub password: Secret<String>,
    pub locale: LocaleCode,
}

impl AcceptInput {
    pub fn parse(request: AcceptInviteRequest) -> Result<Self, InviteError> {
        let AcceptInviteRequest {
            first_name,
            last_name,
            username,
            password,
            locale,
        } = request;
        let password = Secret::new(password);
        let mut invalid = Vec::new();
        let first_name = checked(users::display_text(&first_name), "firstName", &mut invalid);
        let last_name = checked(users::display_text(&last_name), "lastName", &mut invalid);
        let username = checked(Username::parse(&username).ok(), "username", &mut invalid);
        let locale = checked(locale.parse().ok(), "locale", &mut invalid);
        match (first_name, last_name, username, locale) {
            (Some(first_name), Some(last_name), Some(username), Some(locale)) => Ok(Self {
                first_name,
                last_name,
                username,
                password,
                locale,
            }),
            _ => Err(InviteError::Invalid { fields: invalid }),
        }
    }
}

fn checked<T>(
    parsed: Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<T> {
    if parsed.is_none() {
        invalid.push(field);
    }
    parsed
}

pub struct PresentedInvite(TokenDigest);

impl PresentedInvite {
    pub fn parse(encoded: &str) -> Result<Self, InviteError> {
        Token::decode(encoded)
            .map(|token| Self(token.digest()))
            .map_err(|_| InviteError::NotFound)
    }

    pub const fn digest(&self) -> &TokenDigest {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct StoredInvite {
    pub id: InviteId,
    pub email: Option<String>,
    pub role: Role,
    pub state: String,
    pub expires_at: Timestamp,
    pub created_by: UserId,
}

impl StoredInvite {
    pub fn status(&self, now: Timestamp) -> Result<InviteStatus, InviteError> {
        InviteStatus::of(&self.state, self.expires_at, now)
            .ok_or(InviteError::RepositoryInvariant { column: "state" })
    }
}

pub fn invite_aad(id: InviteId) -> Vec<u8> {
    let id = id.to_string();
    let mut aad = Vec::with_capacity(7 + id.len());
    aad.extend_from_slice(b"invite");
    aad.push(0);
    aad.extend_from_slice(id.as_bytes());
    aad
}

pub fn batch_key(id: InviteId) -> String {
    format!("invite:{id}")
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn at(offset_minutes: i64) -> Timestamp {
        Timestamp::try_from(
            datetime!(2026-09-25 12:00 UTC) + time::Duration::minutes(offset_minutes),
        )
        .unwrap()
    }

    #[test]
    fn unit_invite_status_treats_lapsed_pending_as_expired() {
        let now = at(0);
        assert_eq!(
            InviteStatus::of("pending", at(1), now),
            Some(InviteStatus::Pending)
        );
        assert_eq!(
            InviteStatus::of("pending", now, now),
            Some(InviteStatus::Expired)
        );
        assert_eq!(
            InviteStatus::of("expired", at(60), now),
            Some(InviteStatus::Expired)
        );
        assert_eq!(
            InviteStatus::of("accepted", at(-60), now),
            Some(InviteStatus::Accepted)
        );
        assert_eq!(
            InviteStatus::of("revoked", at(60), now),
            Some(InviteStatus::Revoked)
        );
        assert_eq!(InviteStatus::of("other", at(60), now), None);
        assert!(matches!(
            InviteStatus::Accepted.into_live(),
            Err(InviteError::AlreadyUsed)
        ));
        assert!(matches!(
            InviteStatus::Revoked.into_live(),
            Err(InviteError::Revoked)
        ));
        assert!(matches!(
            InviteStatus::Expired.into_live(),
            Err(InviteError::Expired)
        ));
    }

    #[test]
    fn unit_presented_invite_rejects_malformed_values_as_not_found() {
        for malformed in ["", "short", &"A".repeat(44), &"+".repeat(43)] {
            assert!(matches!(
                PresentedInvite::parse(malformed),
                Err(InviteError::NotFound)
            ));
        }
        let token = Token::mint().unwrap();
        let parsed = PresentedInvite::parse(token.encode().expose_secret()).unwrap();
        assert_eq!(parsed.digest().as_str(), token.digest().as_str());
    }

    #[test]
    fn unit_create_input_validates_every_field() {
        let request = CreateInviteRequest {
            email: "not an address".to_owned(),
            role: "owner".to_owned(),
            expires_in_hours: Some(721),
            send_email: false,
        };
        let Err(InviteError::Invalid { fields }) = CreateInput::parse(request) else {
            panic!("an invalid create request parsed");
        };
        assert_eq!(fields, ["email", "role", "expiresInHours"]);
        for hours in [0, -1, i64::from(u32::MAX) + 1] {
            let request = CreateInviteRequest {
                email: "new@example.test".to_owned(),
                role: "user".to_owned(),
                expires_in_hours: Some(hours),
                send_email: false,
            };
            assert!(CreateInput::parse(request).is_err());
        }
        let parsed = CreateInput::parse(CreateInviteRequest {
            email: "New@Example.test".to_owned(),
            role: "admin".to_owned(),
            expires_in_hours: None,
            send_email: true,
        })
        .unwrap();
        assert_eq!(parsed.email.normalized(), "new@example.test");
        assert_eq!(parsed.role, Role::Admin);
        assert_eq!(parsed.validity_hours, None);
    }

    #[test]
    fn unit_invite_requests_never_debug_secrets() {
        let accept = AcceptInviteRequest {
            first_name: "Alan".to_owned(),
            last_name: "Turing".to_owned(),
            username: "alan".to_owned(),
            password: "sentinel-password".to_owned(),
            locale: "en-US".to_owned(),
        };
        let created = CreateInviteResponse {
            id: "id".to_owned(),
            invite_url: "https://files.example.test/invite/sentinel-token".to_owned(),
            expires_at: "2026-09-25T12:00:00.000Z".to_owned(),
        };
        for rendered in [format!("{accept:?}"), format!("{created:?}")] {
            assert!(!rendered.contains("sentinel"), "{rendered}");
        }
    }
}
