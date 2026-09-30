use std::fmt;

use serde::Deserialize;
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::email::Email;
use crate::domain::locale::LocaleCode;
use crate::domain::role::Role;
use crate::domain::secret::{Secret, REDACTED};
use crate::domain::username::Username;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::admin_service::AdminUserError;
use super::model::{display_text, QuotaOverride};
use super::profile::MAX_SAFE_JSON_INTEGER;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUserRequest {
    #[schema(example = "Grace")]
    pub first_name: String,
    #[schema(example = "Hopper")]
    pub last_name: String,
    #[schema(example = "grace")]
    pub username: String,
    #[schema(example = "grace@example.com")]
    pub email: String,
    #[schema(example = "user")]
    pub role: String,
    /// Write-only temporary password. Omitted or `null` creates an SSO-only account without a local credential.
    #[schema(format = Password)]
    pub password: Option<String>,
    /// Defaults to `true` when a password is supplied. `true` without a password is rejected.
    pub require_password_change: Option<bool>,
    /// Initial explicit byte quota. Omitted or `null` inherits the instance default.
    #[schema(
        minimum = 0,
        maximum = 9_007_199_254_740_991_u64,
        example = 5_368_709_120_u64
    )]
    pub quota_bytes: Option<i64>,
    /// Initial locale. Defaults to the instance default locale.
    #[schema(example = "en-US")]
    pub locale: Option<String>,
    /// Defaults to `true`.
    pub is_active: Option<bool>,
}

impl fmt::Debug for CreateUserRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateUserRequest")
            .field("username", &self.username)
            .field("role", &self.role)
            .field("password", &self.password.as_ref().map(|_| REDACTED))
            .finish_non_exhaustive()
    }
}

impl JsonRequest for CreateUserRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("firstName", JsonKind::String),
        JsonField::required("lastName", JsonKind::String),
        JsonField::required("username", JsonKind::String),
        JsonField::required("email", JsonKind::String),
        JsonField::required("role", JsonKind::String),
        JsonField::optional("password", JsonKind::String),
        JsonField::optional("requirePasswordChange", JsonKind::Boolean),
        JsonField::optional("quotaBytes", JsonKind::Integer),
        JsonField::optional("locale", JsonKind::String),
        JsonField::optional("isActive", JsonKind::Boolean),
    ];
}

pub enum InitialCredential {
    Local {
        password: Secret<String>,
        must_change_password: bool,
    },
    SsoOnly,
}

impl fmt::Debug for InitialCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local {
                must_change_password,
                ..
            } => f
                .debug_struct("Local")
                .field("password", &REDACTED)
                .field("must_change_password", must_change_password)
                .finish(),
            Self::SsoOnly => f.write_str("SsoOnly"),
        }
    }
}

#[derive(Debug)]
pub struct CreateInput {
    pub first_name: String,
    pub last_name: String,
    pub username: Username,
    pub email: Email,
    pub role: Role,
    pub credential: InitialCredential,
    pub quota: QuotaOverride,
    pub locale: Option<LocaleCode>,
    pub is_active: bool,
}

impl CreateInput {
    pub fn parse(request: CreateUserRequest) -> Result<Self, AdminUserError> {
        let CreateUserRequest {
            first_name,
            last_name,
            username,
            email,
            role,
            password,
            require_password_change,
            quota_bytes,
            locale,
            is_active,
        } = request;
        let mut invalid = Vec::new();
        let first_name = checked(display_text(&first_name), "firstName", &mut invalid);
        let last_name = checked(display_text(&last_name), "lastName", &mut invalid);
        let username = checked(Username::parse(&username).ok(), "username", &mut invalid);
        let email = checked(Email::parse(&email).ok(), "email", &mut invalid);
        let role = checked(role.parse::<Role>().ok(), "role", &mut invalid);
        let quota = checked(quota_override(quota_bytes), "quotaBytes", &mut invalid);
        let locale = match locale {
            None => Some(None),
            Some(code) => code.parse::<LocaleCode>().ok().map(Some),
        };
        let locale = checked(locale, "locale", &mut invalid);
        let credential = match (password, require_password_change) {
            (Some(password), require) => Some(InitialCredential::Local {
                password: Secret::new(password),
                must_change_password: require.unwrap_or(true),
            }),
            (None, Some(true)) => {
                invalid.push("requirePasswordChange");
                None
            }
            (None, Some(false) | None) => Some(InitialCredential::SsoOnly),
        };
        match (
            first_name, last_name, username, email, role, quota, locale, credential,
        ) {
            (
                Some(first_name),
                Some(last_name),
                Some(username),
                Some(email),
                Some(role),
                Some(quota),
                Some(locale),
                Some(credential),
            ) => Ok(Self {
                first_name,
                last_name,
                username,
                email,
                role,
                credential,
                quota,
                locale,
                is_active: is_active.unwrap_or(true),
            }),
            _ => Err(AdminUserError::Invalid { fields: invalid }),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateUserRequest {
    #[schema(example = "Grace")]
    pub first_name: Option<String>,
    #[schema(example = "Hopper")]
    pub last_name: Option<String>,
    #[schema(example = "ghopper")]
    pub username: Option<String>,
}

impl JsonRequest for UpdateUserRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("firstName", JsonKind::String),
        JsonField::optional("lastName", JsonKind::String),
        JsonField::optional("username", JsonKind::String),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInput {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub username: Option<Username>,
}

impl UpdateInput {
    pub fn parse(request: UpdateUserRequest) -> Result<Self, AdminUserError> {
        let UpdateUserRequest {
            first_name,
            last_name,
            username,
        } = request;
        if first_name.is_none() && last_name.is_none() && username.is_none() {
            return Err(AdminUserError::Invalid {
                fields: vec![crate::infra::http::json::BODY_FIELD],
            });
        }
        let mut invalid = Vec::new();
        let first_name = optional(first_name, display_text, "firstName", &mut invalid);
        let last_name = optional(last_name, display_text, "lastName", &mut invalid);
        let username = optional(
            username,
            |text| Username::parse(text).ok(),
            "username",
            &mut invalid,
        );
        if invalid.is_empty() {
            Ok(Self {
                first_name,
                last_name,
                username,
            })
        } else {
            Err(AdminUserError::Invalid { fields: invalid })
        }
    }
}

fn quota_override(quota_bytes: Option<i64>) -> Option<QuotaOverride> {
    match quota_bytes {
        None => Some(QuotaOverride::Inherit),
        Some(bytes) => u64::try_from(bytes)
            .ok()
            .filter(|bytes| *bytes <= MAX_SAFE_JSON_INTEGER)
            .and_then(|bytes| ByteSize::try_from(bytes).ok())
            .map(QuotaOverride::Bytes),
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

fn optional<T>(
    value: Option<String>,
    parse: impl FnOnce(&str) -> Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<T> {
    let value = value?;
    let parsed = parse(&value);
    if parsed.is_none() {
        invalid.push(field);
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CreateUserRequest {
        CreateUserRequest {
            first_name: "Grace".to_owned(),
            last_name: "Hopper".to_owned(),
            username: "grace".to_owned(),
            email: "Grace@Example.test".to_owned(),
            role: "user".to_owned(),
            password: Some("temporary-value".to_owned()),
            require_password_change: None,
            quota_bytes: None,
            locale: None,
            is_active: None,
        }
    }

    fn invalid_fields(request: CreateUserRequest) -> Vec<&'static str> {
        match CreateInput::parse(request) {
            Err(AdminUserError::Invalid { fields }) => fields,
            other => panic!("expected a validation failure, got {other:?}"),
        }
    }

    #[test]
    fn unit_admin_create_input_defaults_force_a_password_change_only_with_a_password() {
        let local = CreateInput::parse(request()).unwrap();
        assert!(matches!(
            local.credential,
            InitialCredential::Local {
                must_change_password: true,
                ..
            }
        ));
        assert!(local.is_active);
        assert_eq!(local.quota, QuotaOverride::Inherit);
        assert_eq!(local.locale, None);
        assert_eq!(local.email.normalized(), "grace@example.test");

        let opted_out = CreateInput::parse(CreateUserRequest {
            require_password_change: Some(false),
            ..request()
        })
        .unwrap();
        assert!(matches!(
            opted_out.credential,
            InitialCredential::Local {
                must_change_password: false,
                ..
            }
        ));

        for require in [None, Some(false)] {
            let sso = CreateInput::parse(CreateUserRequest {
                password: None,
                require_password_change: require,
                ..request()
            })
            .unwrap();
            assert!(matches!(sso.credential, InitialCredential::SsoOnly));
        }
    }

    #[test]
    fn unit_admin_create_input_rejects_an_impossible_sso_only_restriction() {
        let fields = invalid_fields(CreateUserRequest {
            password: None,
            require_password_change: Some(true),
            ..request()
        });
        assert_eq!(fields, ["requirePasswordChange"]);
    }

    #[test]
    fn unit_admin_create_input_validates_every_field() {
        let fields = invalid_fields(CreateUserRequest {
            first_name: " ".to_owned(),
            last_name: "x".repeat(101),
            username: "ab".to_owned(),
            email: "not an address".to_owned(),
            role: "owner".to_owned(),
            quota_bytes: Some(-1),
            locale: Some("xx-XX".to_owned()),
            ..request()
        });
        assert_eq!(
            fields,
            [
                "firstName",
                "lastName",
                "username",
                "email",
                "role",
                "quotaBytes",
                "locale"
            ]
        );
    }

    #[test]
    fn unit_admin_create_input_quota_has_no_sentinel_values() {
        for (quota, expected) in [
            (None, QuotaOverride::Inherit),
            (
                Some(0),
                QuotaOverride::Bytes(ByteSize::try_from(0_u64).unwrap()),
            ),
            (
                Some(5_368_709_120),
                QuotaOverride::Bytes(ByteSize::try_from(5_368_709_120_u64).unwrap()),
            ),
        ] {
            let parsed = CreateInput::parse(CreateUserRequest {
                quota_bytes: quota,
                ..request()
            })
            .unwrap();
            assert_eq!(parsed.quota, expected);
        }
        for quota in [-1_i64, i64::MIN, (1 << 53), i64::MAX] {
            assert_eq!(
                invalid_fields(CreateUserRequest {
                    quota_bytes: Some(quota),
                    ..request()
                }),
                ["quotaBytes"]
            );
        }
    }

    #[test]
    fn unit_admin_create_input_debug_never_prints_the_password() {
        let rendered = format!(
            "{:?} {:?}",
            request(),
            CreateInput::parse(request()).unwrap()
        );
        assert!(!rendered.contains("temporary-value"), "{rendered}");
    }

    #[test]
    fn unit_admin_update_input_requires_an_editable_field() {
        let empty = UpdateUserRequest {
            first_name: None,
            last_name: None,
            username: None,
        };
        let Err(AdminUserError::Invalid { fields }) = UpdateInput::parse(empty) else {
            panic!("an empty update parsed");
        };
        assert_eq!(fields, ["body"]);

        let Err(AdminUserError::Invalid { fields }) = UpdateInput::parse(UpdateUserRequest {
            first_name: Some(String::new()),
            last_name: None,
            username: Some("ab".to_owned()),
        }) else {
            panic!("invalid update fields parsed");
        };
        assert_eq!(fields, ["firstName", "username"]);

        let parsed = UpdateInput::parse(UpdateUserRequest {
            first_name: None,
            last_name: Some(" Hopper ".to_owned()),
            username: Some("GHopper".to_owned()),
        })
        .unwrap();
        assert_eq!(parsed.first_name, None);
        assert_eq!(parsed.last_name.as_deref(), Some("Hopper"));
        let username = parsed.username.unwrap();
        assert_eq!(username.as_str(), "GHopper");
        assert_eq!(username.normalized(), "ghopper");
    }
}
