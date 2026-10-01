use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::groups::general::GeneralSettings;
use super::groups::public_links::PublicLinkSettings;
use super::groups::quotas::QuotaSettings;
use super::groups::security::{SecuritySettings, TWO_FACTOR_REQUIRED_KEY};
use super::groups::smtp::{SmtpSettings, PASSWORD_KEY, USERNAME_KEY};
use super::groups::{parse_patch, Change, Kind, PatchError, SettingsGroup, Stored};
use super::model::{self, AppSettings};
use super::repo;
use super::service::SettingValueInput;
use super::snapshot;
use super::{SettingsError, SettingsService};
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{self, ActionSpec, Presence, SettingKey};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::AuthenticatedPrincipal;
use crate::features::email::transport::SmtpConfig;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::db::{DbError, WriteTx};
use crate::infra::http::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AdminSettings {
    pub general: GeneralSettings,
    pub security: SecuritySettings,
    pub quotas: QuotaSettings,
    #[serde(rename = "public-links")]
    #[schema(rename = "public-links")]
    pub public_links: PublicLinkSettings,
    pub smtp: SmtpSettings,
}

impl From<&AppSettings> for AdminSettings {
    fn from(settings: &AppSettings) -> Self {
        Self {
            general: GeneralSettings::from(settings),
            security: SecuritySettings::from(settings),
            quotas: QuotaSettings::from(settings),
            public_links: PublicLinkSettings::from(settings),
            smtp: SmtpSettings::from(settings),
        }
    }
}

#[derive(Debug)]
pub enum AdminSettingsError {
    Patch(PatchError),
    Settings(SettingsError),
    Audit(AuditError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl AdminSettingsError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Patch(_) => "admin_settings_patch_rejected",
            Self::Settings(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "admin_settings_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Patch(PatchError::BodyNotObject) => ApiError::validation(["body"]),
            Self::Patch(PatchError::Unknown) => ApiError::new(ErrorCode::SettingUnknown),
            Self::Patch(PatchError::Invalid { field }) => {
                ApiError::new(ErrorCode::SettingValueInvalid).with_detail("key", *field)
            }
            Self::Patch(PatchError::Incomplete { field }) => {
                ApiError::new(ErrorCode::SettingValueInvalid)
                    .with_detail("key", *field)
                    .with_detail("requiredWhenEnabled", true)
            }
            Self::Patch(PatchError::AboveCeiling { field, ceiling }) => {
                ApiError::new(ErrorCode::SettingValueInvalid)
                    .with_detail("key", *field)
                    .with_detail("max", *ceiling)
            }
            Self::Patch(PatchError::BelowFloor { field, floor }) => {
                ApiError::new(ErrorCode::SettingBelowFloor)
                    .with_detail("key", *field)
                    .with_detail("floor", *floor)
            }
            Self::Db(error)
            | Self::Settings(SettingsError::Db(error))
            | Self::Audit(AuditError::Db(error)) => ApiError::new(error.api_code()),
            Self::Settings(_) | Self::Audit(_) | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for AdminSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Patch(error) => write!(f, "the settings patch was rejected: {error:?}"),
            Self::Settings(error) => write!(f, "settings operation failed: {error}"),
            Self::Audit(error) => write!(f, "settings audit failed: {error}"),
            Self::Db(error) => write!(f, "settings database operation failed: {error}"),
            Self::Time(error) => write!(
                f,
                "the server clock is outside the supported range: {error}"
            ),
        }
    }
}

impl std::error::Error for AdminSettingsError {}

impl From<DbError> for AdminSettingsError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<SettingsError> for AdminSettingsError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

impl From<AuditError> for AdminSettingsError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<InvalidTimestamp> for AdminSettingsError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<PatchError> for AdminSettingsError {
    fn from(error: PatchError) -> Self {
        Self::Patch(error)
    }
}

#[derive(Clone)]
pub struct AdminSettingsService {
    settings: SettingsService,
    clock: Arc<dyn Clock>,
    audit: AuditService,
}

impl fmt::Debug for AdminSettingsService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdminSettingsService")
            .finish_non_exhaustive()
    }
}

impl AdminSettingsService {
    pub fn new(settings: SettingsService, clock: Arc<dyn Clock>, audit: AuditService) -> Self {
        Self {
            settings,
            clock,
            audit,
        }
    }

    pub fn all(&self) -> AdminSettings {
        AdminSettings::from(self.settings.current().as_ref())
    }

    pub fn general(&self) -> GeneralSettings {
        GeneralSettings::from(self.settings.current().as_ref())
    }

    pub fn security(&self) -> SecuritySettings {
        SecuritySettings::from(self.settings.current().as_ref())
    }

    pub fn quotas(&self) -> QuotaSettings {
        QuotaSettings::from(self.settings.current().as_ref())
    }

    pub fn public_links(&self) -> PublicLinkSettings {
        PublicLinkSettings::from(self.settings.current().as_ref())
    }

    pub fn smtp(&self) -> SmtpSettings {
        SmtpSettings::from(self.settings.current().as_ref())
    }

    pub async fn patch(
        &self,
        group: SettingsGroup,
        body: &Value,
        admin: &AuthenticatedPrincipal,
        client: &ClientMetadata,
    ) -> Result<(), AdminSettingsError> {
        let requested = parse_patch(group, body)?;
        if requested.is_empty() {
            return Ok(());
        }
        let admin_id = admin.user_id.to_string();
        let actor = Actor::user(&admin_id, &admin.username);
        let at = Timestamp::try_from(self.clock.now())?;
        self.settings
            .update_group(group.transaction(), async |tx| {
                let rows = repo::load_all_in_tx(tx).await?;
                let keys = self.settings.keys();
                let current = snapshot::build(&rows, &keys)?;
                let view = group.view(&current);
                let effective: Vec<Transition<'_>> = requested
                    .iter()
                    .filter_map(|change| Transition::of(change, &view, &current))
                    .collect();
                if effective.is_empty() {
                    return Ok(());
                }
                if group == SettingsGroup::Smtp {
                    check_smtp_usable(&rows, &keys, &effective)?;
                }
                let applied = Applied {
                    actor: &actor,
                    admin_id: &admin_id,
                    at,
                    client,
                };
                for transition in &effective {
                    self.apply(tx, group, transition, &applied).await?;
                }
                Ok::<(), AdminSettingsError>(())
            })
            .await
    }

    async fn apply(
        &self,
        tx: &mut WriteTx<'_>,
        group: SettingsGroup,
        transition: &Transition<'_>,
        applied: &Applied<'_>,
    ) -> Result<(), AdminSettingsError> {
        let change = transition.change;
        let to = change.stored();
        let secret = matches!(change.field.kind, Kind::Secret(_));
        match &to {
            Stored::Null if secret => {
                self.settings.clear_setting(tx, change.field.key).await?;
            }
            _ => {
                let input = match &to {
                    Stored::Flag(flag) => SettingValueInput::Boolean(*flag),
                    Stored::Integer(number) => SettingValueInput::Integer(*number),
                    Stored::Text(text) => SettingValueInput::String(text),
                    Stored::Secret(secret) => SettingValueInput::Secret(secret.expose_secret()),
                    Stored::Null => SettingValueInput::Null,
                };
                self.settings
                    .write_setting(tx, change.field.key, input, Some(applied.admin_id))
                    .await?;
            }
        }
        let key = SettingKey::new(change.field.key).ok_or(SettingsError::UnknownKey {
            key: change.field.key.to_owned(),
        })?;
        let spec = audit_spec(group, &key, transition, &to);
        let event = AuditEvent::new(spec, applied.actor.clone(), Outcome::Success, applied.at)
            .with_target(
                Target::new(TargetType::Setting)
                    .id(change.field.key)
                    .label(change.field.api),
            )
            .with_client(applied.client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }
}

struct Transition<'a> {
    change: &'a Change,
    from: Stored,
    secret_was_set: bool,
}

impl<'a> Transition<'a> {
    fn of(
        change: &'a Change,
        view: &serde_json::Map<String, Value>,
        current: &AppSettings,
    ) -> Option<Self> {
        if matches!(change.field.kind, Kind::Secret(_)) {
            let secret_was_set = current.smtp.password.is_some();
            return (secret_was_set || change.requested != Stored::Null).then_some(Self {
                change,
                from: Stored::Null,
                secret_was_set,
            });
        }
        let previous = view.get(change.field.view_name()).unwrap_or(&Value::Null);
        (*previous != change.requested.json()).then(|| Self {
            change,
            from: change.field.storage_form(previous),
            secret_was_set: false,
        })
    }
}

fn check_smtp_usable(
    rows: &[repo::SettingRow],
    keys: &KeyRing,
    effective: &[Transition<'_>],
) -> Result<(), AdminSettingsError> {
    let mut resulting = snapshot::build(rows, keys)?;
    for transition in effective {
        let key = transition.change.field.key;
        let spec = model::spec(key).ok_or_else(|| SettingsError::UnknownKey {
            key: key.to_owned(),
        })?;
        match transition.change.stored() {
            Stored::Secret(secret) => model::apply_secret(&mut resulting, spec, secret)?,
            Stored::Null if matches!(transition.change.field.kind, Kind::Secret(_)) => {
                resulting.smtp.password = None;
            }
            stored => model::apply_value(&mut resulting, spec, &stored.json())?,
        }
    }
    match SmtpConfig::gap(&resulting.smtp) {
        Some(gap) if resulting.smtp.enabled => {
            Err(PatchError::Incomplete { field: gap.field() }.into())
        }
        _ => Ok(()),
    }
}

struct Applied<'a> {
    actor: &'a Actor,
    admin_id: &'a str,
    at: Timestamp,
    client: &'a ClientMetadata,
}

fn audit_spec(
    group: SettingsGroup,
    key: &SettingKey,
    transition: &Transition<'_>,
    to: &Stored,
) -> ActionSpec {
    let from = &transition.from;
    match (group, key.as_str(), from, to) {
        (
            SettingsGroup::Security,
            TWO_FACTOR_REQUIRED_KEY,
            Stored::Flag(from),
            Stored::Flag(to),
        ) => actions::mandatory_2fa_policy_changed(*from, *to),
        (SettingsGroup::Security, ..) => {
            actions::security_policy_changed(key, &from.audit(), &to.audit())
        }
        (SettingsGroup::Smtp, PASSWORD_KEY, ..) => actions::smtp_credential_changed(
            key,
            presence(transition.secret_was_set),
            presence(matches!(to, Stored::Secret(_))),
        ),
        (SettingsGroup::Smtp, USERNAME_KEY, ..) => actions::smtp_credential_changed(
            key,
            presence(*from != Stored::Null),
            presence(*to != Stored::Null),
        ),
        (SettingsGroup::Smtp, ..) => {
            actions::smtp_settings_changed(key, &from.audit(), &to.audit())
        }
        _ => actions::setting_value_changed(key, &from.audit(), &to.audit()),
    }
}

const fn presence(set: bool) -> Presence {
    if set {
        Presence::Set
    } else {
        Presence::Unset
    }
}
