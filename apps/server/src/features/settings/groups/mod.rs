pub mod general;
pub mod public_links;
pub mod quotas;
pub mod security;

use serde_json::{Map, Value};

use super::model::{AppSettings, ThumbnailSourceLimit};
use crate::domain::locale::LocaleCode;
use crate::features::audit::actions::SettingValue;
use crate::features::users::model::display_text;

pub const MAX_DESCRIPTION_CHARS: usize = 300;
pub const MAX_SAFE_BYTES: i64 = (1 << 53) - 1;
pub const MAX_STORED_COUNT: i64 = u32::MAX as i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsGroup {
    General,
    Security,
    Quotas,
    PublicLinks,
}

impl SettingsGroup {
    pub const ALL: [Self; 4] = [
        Self::General,
        Self::Security,
        Self::Quotas,
        Self::PublicLinks,
    ];

    pub const fn path_name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Security => "security",
            Self::Quotas => "quotas",
            Self::PublicLinks => "public-links",
        }
    }

    pub const fn transaction(self) -> &'static str {
        match self {
            Self::General => "settings.patch.general",
            Self::Security => "settings.patch.security",
            Self::Quotas => "settings.patch.quotas",
            Self::PublicLinks => "settings.patch.public_links",
        }
    }

    pub const fn fields(self) -> &'static [Field] {
        match self {
            Self::General => general::FIELDS,
            Self::Security => security::FIELDS,
            Self::Quotas => quotas::FIELDS,
            Self::PublicLinks => public_links::FIELDS,
        }
    }

    pub fn view(self, settings: &AppSettings) -> Map<String, Value> {
        let value = match self {
            Self::General => serde_json::to_value(general::GeneralSettings::from(settings)),
            Self::Security => serde_json::to_value(security::SecuritySettings::from(settings)),
            Self::Quotas => serde_json::to_value(quotas::QuotaSettings::from(settings)),
            Self::PublicLinks => {
                serde_json::to_value(public_links::PublicLinkSettings::from(settings))
            }
        };
        match value {
            Ok(Value::Object(members)) => members,
            _ => Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Name,
    Description,
    Locale,
    ThumbnailLimit,
    Flag,
    InvertedFlag,
    Integer(Bounds),
    OptionalInteger(Bounds),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub floor: i64,
    pub ceiling: i64,
}

impl Bounds {
    pub const fn new(floor: i64, ceiling: i64) -> Self {
        Self { floor, ceiling }
    }

    pub const fn unbounded_above(floor: i64) -> Self {
        Self::new(floor, MAX_STORED_COUNT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub api: &'static str,
    pub key: &'static str,
    pub kind: Kind,
}

impl Field {
    pub const fn new(api: &'static str, key: &'static str, kind: Kind) -> Self {
        Self { api, key, kind }
    }

    pub fn storage_form(&self, api: &Value) -> Stored {
        match api {
            Value::Bool(flag) => Stored::Flag(*flag != matches!(self.kind, Kind::InvertedFlag)),
            Value::Number(number) => number.as_i64().map_or(Stored::Null, Stored::Integer),
            Value::String(text) => Stored::Text(text.clone()),
            _ => Stored::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    Flag(bool),
    Integer(i64),
    Text(String),
    Null,
}

impl Stored {
    pub fn json(&self) -> Value {
        match self {
            Self::Flag(flag) => Value::from(*flag),
            Self::Integer(number) => Value::from(*number),
            Self::Text(text) => Value::from(text.as_str()),
            Self::Null => Value::Null,
        }
    }

    pub fn audit(&self) -> SettingValue {
        match self {
            Self::Flag(flag) => SettingValue::Bool(*flag),
            Self::Integer(number) => SettingValue::Integer(*number),
            Self::Text(text) => SettingValue::Text(text.clone()),
            Self::Null => SettingValue::Unset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub field: &'static Field,
    pub requested: Stored,
}

impl Change {
    pub fn stored(&self) -> Stored {
        self.field.storage_form(&self.requested.json())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchError {
    BodyNotObject,
    Unknown,
    Invalid { field: &'static str },
    AboveCeiling { field: &'static str, ceiling: i64 },
    BelowFloor { field: &'static str, floor: i64 },
}

pub fn parse_patch(group: SettingsGroup, body: &Value) -> Result<Vec<Change>, PatchError> {
    let Value::Object(members) = body else {
        return Err(PatchError::BodyNotObject);
    };
    let fields = group.fields();
    if members
        .keys()
        .any(|name| !fields.iter().any(|field| field.api == name))
    {
        return Err(PatchError::Unknown);
    }
    let mut changes = Vec::with_capacity(members.len());
    for field in fields {
        if let Some(value) = members.get(field.api) {
            changes.push(Change {
                field,
                requested: validate(field, value)?,
            });
        }
    }
    Ok(changes)
}

fn validate(field: &'static Field, value: &Value) -> Result<Stored, PatchError> {
    let invalid = PatchError::Invalid { field: field.api };
    match field.kind {
        Kind::Name => {
            let text = value.as_str().ok_or(invalid)?;
            display_text(text).map(Stored::Text).ok_or(invalid)
        }
        Kind::Description => {
            let trimmed = value.as_str().ok_or(invalid)?.trim();
            let acceptable = trimmed.chars().count() <= MAX_DESCRIPTION_CHARS
                && !trimmed.chars().any(char::is_control);
            acceptable
                .then(|| Stored::Text(trimmed.to_owned()))
                .ok_or(invalid)
        }
        Kind::Locale => {
            let code = value
                .as_str()
                .and_then(|text| text.parse::<LocaleCode>().ok())
                .ok_or(invalid)?;
            Ok(Stored::Text(code.as_str().to_owned()))
        }
        Kind::ThumbnailLimit => {
            let limit = value
                .as_str()
                .and_then(ThumbnailSourceLimit::parse)
                .ok_or(invalid)?;
            Ok(Stored::Text(limit.as_str().to_owned()))
        }
        Kind::Flag | Kind::InvertedFlag => value.as_bool().map(Stored::Flag).ok_or(invalid),
        Kind::Integer(bounds) => bounded(field, value, bounds),
        Kind::OptionalInteger(bounds) => {
            if value.is_null() {
                Ok(Stored::Null)
            } else {
                bounded(field, value, bounds)
            }
        }
    }
}

fn bounded(field: &'static Field, value: &Value, bounds: Bounds) -> Result<Stored, PatchError> {
    let number = value
        .as_i64()
        .ok_or(PatchError::Invalid { field: field.api })?;
    if number < bounds.floor {
        return Err(PatchError::BelowFloor {
            field: field.api,
            floor: bounds.floor,
        });
    }
    if number > bounds.ceiling {
        return Err(PatchError::AboveCeiling {
            field: field.api,
            ceiling: bounds.ceiling,
        });
    }
    Ok(Stored::Integer(number))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::features::settings::model::{
        apply_value, spec, Group, ValueType, SETTINGS_REGISTRY,
    };

    fn registry_group(group: SettingsGroup) -> Group {
        match group {
            SettingsGroup::General => Group::General,
            SettingsGroup::Security => Group::Security,
            SettingsGroup::Quotas => Group::Quotas,
            SettingsGroup::PublicLinks => Group::PublicLinks,
        }
    }

    fn expected_type(kind: Kind) -> ValueType {
        match kind {
            Kind::Name | Kind::Description | Kind::Locale | Kind::ThumbnailLimit => {
                ValueType::String
            }
            Kind::Flag | Kind::InvertedFlag => ValueType::Boolean,
            Kind::Integer(_) | Kind::OptionalInteger(_) => ValueType::Integer,
        }
    }

    #[test]
    fn unit_group_fields_match_the_settings_registry() {
        for group in SettingsGroup::ALL {
            for field in group.fields() {
                let setting = spec(field.key).unwrap_or_else(|| panic!("{}", field.key));
                assert_eq!(setting.group, registry_group(group), "{}", field.key);
                assert_eq!(
                    setting.value_type,
                    expected_type(field.kind),
                    "{}",
                    field.key
                );
                assert_eq!(
                    setting.slot.is_nullable(),
                    matches!(field.kind, Kind::OptionalInteger(_)),
                    "{}",
                    field.key
                );
            }
            let exposed: Vec<&str> = group.fields().iter().map(|field| field.key).collect();
            for setting in SETTINGS_REGISTRY {
                if setting.group == registry_group(group) && setting.key != "setup_completed" {
                    assert!(
                        exposed.contains(&setting.key),
                        "{} is not exposed",
                        setting.key
                    );
                }
            }
        }
    }

    #[test]
    fn unit_group_views_expose_exactly_the_field_names() {
        let defaults = AppSettings::defaults();
        for group in SettingsGroup::ALL {
            let view = group.view(&defaults);
            let mut names: Vec<&str> = view.keys().map(String::as_str).collect();
            let mut declared: Vec<&str> = group.fields().iter().map(|field| field.api).collect();
            names.sort_unstable();
            declared.sort_unstable();
            assert_eq!(names, declared, "{}", group.path_name());
            for field in group.fields() {
                assert!(
                    validate(field, &view[field.api]).is_ok(),
                    "the default of {} violates its own rule",
                    field.api
                );
            }
        }
    }

    #[test]
    fn unit_no_group_field_names_an_operator_value() {
        for group in SettingsGroup::ALL {
            for field in group.fields() {
                for variable in crate::config::Variable::ALL {
                    let operator = variable.name().to_ascii_lowercase();
                    assert_ne!(field.key, operator);
                    assert!(!field.api.to_ascii_lowercase().contains("palmr"));
                }
                for forbidden in [
                    "port", "bind", "proxy", "s3", "endpoint", "bucket", "base_url",
                ] {
                    assert!(!field.key.contains(forbidden), "{}", field.key);
                }
            }
        }
    }

    #[test]
    fn unit_every_accepted_boundary_value_survives_the_startup_decoder() {
        for group in SettingsGroup::ALL {
            for field in group.fields() {
                let mut candidates: Vec<Value> = Vec::new();
                match field.kind {
                    Kind::Integer(bounds) => {
                        candidates.extend([bounds.floor, bounds.ceiling].map(Value::from));
                    }
                    Kind::OptionalInteger(bounds) => {
                        candidates.extend([bounds.floor, bounds.ceiling].map(Value::from));
                        candidates.push(Value::Null);
                    }
                    Kind::Flag | Kind::InvertedFlag => {
                        candidates.extend([Value::from(true), Value::from(false)]);
                    }
                    Kind::Locale => candidates.extend(
                        LocaleCode::ALL
                            .iter()
                            .map(|code| Value::from(code.as_str())),
                    ),
                    Kind::ThumbnailLimit => candidates.extend(
                        ["64MiB", "128MiB", "256MiB", "512MiB", "unlimited"].map(Value::from),
                    ),
                    Kind::Name => candidates.push(Value::from("n".repeat(100))),
                    Kind::Description => {
                        candidates.extend([Value::from(""), Value::from("d".repeat(300))]);
                    }
                }
                for candidate in candidates {
                    let stored = validate(field, &candidate).unwrap();
                    let change = Change {
                        field,
                        requested: stored,
                    };
                    let mut settings = AppSettings::defaults();
                    apply_value(
                        &mut settings,
                        spec(field.key).unwrap(),
                        &change.stored().json(),
                    )
                    .unwrap_or_else(|_| panic!("{} = {candidate}", field.api));
                    let view = group.view(&settings);
                    assert_eq!(view[field.api], candidate, "{}", field.api);
                }
            }
        }
    }

    #[test]
    fn unit_patch_parsing_distinguishes_absent_null_and_invalid() {
        let quotas = SettingsGroup::Quotas;
        assert_eq!(parse_patch(quotas, &json!({})).unwrap(), vec![]);
        let only_one = parse_patch(quotas, &json!({ "maxFileSizeBytes": 7 })).unwrap();
        assert_eq!(only_one.len(), 1);
        assert_eq!(only_one[0].field.api, "maxFileSizeBytes");
        let nulled = parse_patch(quotas, &json!({ "defaultUserQuotaBytes": null })).unwrap();
        assert_eq!(nulled[0].requested, Stored::Null);
        assert_eq!(
            parse_patch(SettingsGroup::General, &json!({ "appName": null })),
            Err(PatchError::Invalid { field: "appName" })
        );
        assert_eq!(
            parse_patch(SettingsGroup::Security, &json!({ "passwordMinLength": 7 })),
            Err(PatchError::BelowFloor {
                field: "passwordMinLength",
                floor: 8
            })
        );
        assert_eq!(
            parse_patch(SettingsGroup::Security, &json!({ "recentAuthMinutes": 16 })),
            Err(PatchError::AboveCeiling {
                field: "recentAuthMinutes",
                ceiling: 15
            })
        );
        assert_eq!(
            parse_patch(
                SettingsGroup::Security,
                &json!({ "nope": 1, "passwordMinLength": 7 })
            ),
            Err(PatchError::Unknown)
        );
        assert_eq!(
            parse_patch(SettingsGroup::General, &json!([1])),
            Err(PatchError::BodyNotObject)
        );
    }

    #[test]
    fn unit_hide_version_is_stored_inverted() {
        let changes = parse_patch(SettingsGroup::General, &json!({ "hideVersion": true })).unwrap();
        assert_eq!(changes[0].field.key, "show_version");
        assert_eq!(changes[0].requested, Stored::Flag(true));
        assert_eq!(changes[0].stored(), Stored::Flag(false));
    }
}
