use sqlx::sqlite::SqliteRow;
use sqlx::Row;

use super::error::ExternalLoginError;
use super::model::{IdentityLinkId, IdentityProvider, ProviderId};
use super::provision;
use crate::domain::clock::Clock;
use crate::domain::email::Email;
use crate::domain::error_code::ErrorCode;
use crate::domain::locale::LocaleCode;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::users::model::{NormalizedIdentifier, User, UserId};
use crate::features::users::repo as users;
use crate::infra::db::{DbError, WriteTx};

pub const MAX_SUBJECT_CHARS: usize = 255;

const FIND_BY_SUBJECT: &str = "SELECT id, user_id, state, avatar_fetched_at
    FROM identity_links WHERE provider_id = ?1 AND subject = ?2";

const FIND_FOR_USER: &str = "SELECT id, user_id, state, avatar_fetched_at
    FROM identity_links WHERE user_id = ?1 AND provider_id = ?2";

const INSERT_LINK: &str = "INSERT INTO identity_links
    (id, user_id, provider_id, subject, email_at_link, email_verified_at_link, link_method,
     state, created_at)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8)";

const TOUCH_LINK: &str = "UPDATE identity_links SET last_login_at = ?2 WHERE id = ?1";

const BIND_SESSION: &str = "UPDATE sessions SET identity_link_id = ?2 WHERE id = ?1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

impl ExternalIdentity {
    pub fn subject_usable(&self) -> bool {
        let length = self.subject.chars().count();
        (1..=MAX_SUBJECT_CHARS).contains(&length) && !self.subject.chars().any(char::is_control)
    }

    pub fn verified_email(&self) -> Option<Email> {
        if !self.email_verified {
            return None;
        }
        self.usable_email()
    }

    pub fn usable_email(&self) -> Option<Email> {
        Email::parse(self.email.as_deref()?).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Active,
    Suspended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityLink {
    pub id: IdentityLinkId,
    pub user_id: UserId,
    pub state: LinkState,
    pub avatar_fetched: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMethod {
    AutoVerifiedEmail,
    AutoProvision,
    Manual,
}

impl LinkMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutoVerifiedEmail => "auto_verified_email",
            Self::AutoProvision => "auto_provision",
            Self::Manual => "manual",
        }
    }

    pub const fn audit_via(self) -> &'static str {
        match self {
            Self::AutoVerifiedEmail => "verified_email",
            Self::AutoProvision => "auto_provision",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Existing(IdentityLink),
    Linked(IdentityLink),
    Provisioned(IdentityLink),
}

impl Resolution {
    pub const fn link(&self) -> &IdentityLink {
        match self {
            Self::Existing(link) | Self::Linked(link) | Self::Provisioned(link) => link,
        }
    }
}

pub struct ResolveInput<'a> {
    pub provider: &'a IdentityProvider,
    pub identity: &'a ExternalIdentity,
    pub clock: &'a dyn Clock,
    pub audit: &'a AuditService,
    pub client: &'a ClientMetadata,
    pub locale: LocaleCode,
}

pub async fn resolve(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
) -> Result<Resolution, ExternalLoginError> {
    let provider = input.provider;
    let identity = input.identity;
    if let Some(link) = find_by_subject(tx, provider.id, &identity.subject).await? {
        return Ok(Resolution::Existing(link));
    }

    let Some(email) = identity.verified_email() else {
        return Err(refused(ErrorCode::ProviderEmailUnverified));
    };
    let matches =
        users::find_all_by_email_normalized_in_tx(tx, &NormalizedIdentifier::from(&email)).await?;
    match single_match(matches)? {
        Some(user) if provider.allow_email_linking && user.is_active => {
            link_existing_user(tx, input, &user, &email).await
        }
        Some(_) => Err(refused(ErrorCode::ProviderAutoProvisionDisabled)),
        None if provider.auto_provision => provision::provision(tx, input, &email).await,
        None => Err(refused(ErrorCode::ProviderAutoProvisionDisabled)),
    }
}

pub fn single_match(mut matches: Vec<User>) -> Result<Option<User>, ExternalLoginError> {
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(refused(ErrorCode::AuthExternalAmbiguousIdentity)),
    }
}

async fn link_existing_user(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
    user: &User,
    email: &Email,
) -> Result<Resolution, ExternalLoginError> {
    let provider = input.provider;
    if find_for_user(tx, user.id, provider.id).await?.is_some() {
        return Err(refused(ErrorCode::ProviderIdentityAlreadyLinked));
    }
    match create_link(tx, input, user, email, LinkMethod::AutoVerifiedEmail).await? {
        LinkInsert::Created(link) => Ok(Resolution::Linked(link)),
        LinkInsert::SubjectTaken(link) => Ok(Resolution::Existing(link)),
        LinkInsert::UserAlreadyLinked => Err(refused(ErrorCode::ProviderIdentityAlreadyLinked)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkInsert {
    Created(IdentityLink),
    SubjectTaken(IdentityLink),
    UserAlreadyLinked,
}

pub async fn create_link(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
    user: &User,
    email: &Email,
    method: LinkMethod,
) -> Result<LinkInsert, ExternalLoginError> {
    insert_link(tx, input, user, Some(email), true, method).await
}

pub async fn insert_link(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
    user: &User,
    email: Option<&Email>,
    email_verified: bool,
    method: LinkMethod,
) -> Result<LinkInsert, ExternalLoginError> {
    let provider = input.provider;
    let id = IdentityLinkId::generate(input.clock);
    let now = Timestamp::try_from(input.clock.now())?;
    let inserted = sqlx::query(INSERT_LINK)
        .bind(id.to_string())
        .bind(user.id.to_string())
        .bind(provider.id.to_string())
        .bind(&input.identity.subject)
        .bind(email.map(Email::as_str))
        .bind(email.is_some() && email_verified)
        .bind(method.as_str())
        .bind(now.to_string())
        .execute(tx.executor())
        .await
        .map_err(DbError::from);
    match inserted {
        Ok(_) => {}
        Err(DbError::UniqueViolation(_)) => {
            if let Some(link) = find_by_subject(tx, provider.id, &input.identity.subject).await? {
                return Ok(LinkInsert::SubjectTaken(link));
            }
            return Ok(LinkInsert::UserAlreadyLinked);
        }
        Err(error) => return Err(error.into()),
    }

    let event = AuditEvent::new(
        actions::identity_link_created(method.audit_via(), &provider.id.to_string()),
        Actor::user(&user.id.to_string(), &user.username),
        Outcome::Success,
        now,
    )
    .with_target(
        Target::new(TargetType::IdentityLink)
            .id(&id.to_string())
            .label(&provider.slug),
    )
    .with_client(input.client.clone());
    input.audit.record_in_tx(tx, &event).await?;

    Ok(LinkInsert::Created(IdentityLink {
        id,
        user_id: user.id,
        state: LinkState::Active,
        avatar_fetched: false,
    }))
}

pub async fn find_by_subject(
    tx: &mut WriteTx<'_>,
    provider_id: ProviderId,
    subject: &str,
) -> Result<Option<IdentityLink>, ExternalLoginError> {
    let row = sqlx::query(FIND_BY_SUBJECT)
        .bind(provider_id.to_string())
        .bind(subject)
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(link_from).transpose()
}

async fn find_for_user(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    provider_id: ProviderId,
) -> Result<Option<IdentityLink>, ExternalLoginError> {
    let row = sqlx::query(FIND_FOR_USER)
        .bind(user_id.to_string())
        .bind(provider_id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(link_from).transpose()
}

pub async fn record_login(
    tx: &mut WriteTx<'_>,
    link: IdentityLinkId,
    session: &str,
    at: Timestamp,
) -> Result<(), ExternalLoginError> {
    sqlx::query(TOUCH_LINK)
        .bind(link.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    sqlx::query(BIND_SESSION)
        .bind(session)
        .bind(link.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

fn link_from(row: &SqliteRow) -> Result<IdentityLink, ExternalLoginError> {
    let id: String = row.try_get("id").map_err(invalid_row)?;
    let user_id: String = row.try_get("user_id").map_err(invalid_row)?;
    let state: String = row.try_get("state").map_err(invalid_row)?;
    let fetched: Option<String> = row.try_get("avatar_fetched_at").map_err(invalid_row)?;
    Ok(IdentityLink {
        id: id.parse().map_err(invalid_row)?,
        user_id: user_id.parse().map_err(invalid_row)?,
        state: match state.as_str() {
            "active" => LinkState::Active,
            "suspended" => LinkState::Suspended,
            _ => return Err(invalid_row(())),
        },
        avatar_fetched: fetched.is_some(),
    })
}

fn invalid_row<E>(_: E) -> ExternalLoginError {
    ExternalLoginError::internal("identity_link_row_invalid")
}

pub const fn refused(code: ErrorCode) -> ExternalLoginError {
    ExternalLoginError::refused(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(email: Option<&str>, verified: bool) -> ExternalIdentity {
        ExternalIdentity {
            subject: "subject-1".to_owned(),
            email: email.map(str::to_owned),
            email_verified: verified,
            username: None,
            name: None,
            picture: None,
        }
    }

    #[test]
    fn unit_only_a_verified_well_formed_email_is_eligible() {
        assert!(identity(Some("a@example.com"), true)
            .verified_email()
            .is_some());
        assert!(identity(Some("a@example.com"), false)
            .verified_email()
            .is_none());
        assert!(identity(None, true).verified_email().is_none());
        for malformed in [
            "",
            "no-at-sign",
            "a@",
            "@example.com",
            "a b@example.com",
            "a@b@c",
        ] {
            assert!(
                identity(Some(malformed), true).verified_email().is_none(),
                "{malformed:?}"
            );
        }
    }

    #[test]
    fn unit_subject_must_be_a_bounded_printable_identifier() {
        let subject = |text: &str| ExternalIdentity {
            subject: text.to_owned(),
            ..identity(None, false)
        };
        assert!(subject("4242").subject_usable());
        assert!(subject(&"s".repeat(MAX_SUBJECT_CHARS)).subject_usable());
        assert!(!subject("").subject_usable());
        assert!(!subject(&"s".repeat(MAX_SUBJECT_CHARS + 1)).subject_usable());
        assert!(!subject("a\nb").subject_usable());
    }

    #[test]
    fn unit_more_than_one_matching_account_fails_closed() {
        assert!(single_match(Vec::new()).unwrap().is_none());
        let error = single_match(vec![sample_user("one"), sample_user("two")]).unwrap_err();
        assert_eq!(error.code(), ErrorCode::AuthExternalAmbiguousIdentity);
        assert_eq!(
            single_match(vec![sample_user("one")])
                .unwrap()
                .unwrap()
                .username,
            "one"
        );
    }

    fn sample_user(username: &str) -> User {
        use crate::domain::bytes::ByteSize;
        use crate::domain::clock::TestClock;
        use crate::domain::role::Role;
        use crate::features::users::model::QuotaOverride;

        let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
        let now = Timestamp::try_from(clock.now()).unwrap();
        User {
            id: UserId::generate(&clock),
            email: format!("{username}@example.com"),
            email_normalized: format!("{username}@example.com"),
            username: username.to_owned(),
            username_normalized: username.to_owned(),
            first_name: String::new(),
            last_name: String::new(),
            password_hash: None,
            password_updated_at: None,
            must_change_password: false,
            role: Role::User,
            is_active: true,
            deactivated_at: None,
            totp_enabled: false,
            quota: QuotaOverride::Inherit,
            used_bytes: ByteSize::ZERO,
            created_at: now,
            updated_at: now,
            created_by: None,
        }
    }
}
