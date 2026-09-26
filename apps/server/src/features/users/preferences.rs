use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::clock::Clock;
use crate::domain::locale::LocaleCode;
use crate::domain::time::Timestamp;
use crate::features::auth::model::AccountView;
use crate::infra::db::WriteTx;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

use super::error::UserError;
use super::model::UserId;

const UPSERT: &str =
    "INSERT INTO user_preferences (user_id, locale, theme, accent, created_at, updated_at)
    VALUES (?1, COALESCE(?2, 'en-US'), COALESCE(?3, 'system'), COALESCE(?4, 'default'), ?5, ?5)
    ON CONFLICT (user_id) DO UPDATE SET
        locale = COALESCE(?2, locale),
        theme = COALESCE(?3, theme),
        accent = COALESCE(?4, accent),
        updated_at = ?5";

macro_rules! presets {
    ($name:ident { $($variant:ident = $key:literal,)+ }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $key,)+
                }
            }
        }

        impl FromStr for $name {
            type Err = UnknownPreset;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $($key => Ok(Self::$variant),)+
                    _ => Err(UnknownPreset),
                }
            }
        }
    };
}

presets!(Theme {
    Light = "light",
    Dark = "dark",
    System = "system",
});

presets!(Accent {
    Default = "default",
    Blue = "blue",
    Violet = "violet",
    Emerald = "emerald",
    Amber = "amber",
    Rose = "rose",
    Slate = "slate",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownPreset;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    #[schema(example = "pt-BR")]
    pub locale: String,
    #[schema(example = "system")]
    pub theme: String,
    #[schema(example = "blue")]
    pub accent: String,
}

impl From<AccountView> for Preferences {
    fn from(account: AccountView) -> Self {
        Self {
            locale: account.locale,
            theme: account.theme,
            accent: account.accent,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesRequest {
    #[schema(example = "pt-BR")]
    pub locale: Option<String>,
    #[schema(example = "system")]
    pub theme: Option<String>,
    #[schema(example = "blue")]
    pub accent: Option<String>,
}

impl JsonRequest for PreferencesRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("locale", JsonKind::String),
        JsonField::optional("theme", JsonKind::String),
        JsonField::optional("accent", JsonKind::String),
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferencesChange {
    pub locale: Option<LocaleCode>,
    pub theme: Option<Theme>,
    pub accent: Option<Accent>,
}

impl PreferencesChange {
    pub fn parse(request: PreferencesRequest) -> Result<Self, Vec<&'static str>> {
        let mut invalid = Vec::new();
        let locale = parsed(request.locale, "locale", &mut invalid);
        let theme = parsed(request.theme, "theme", &mut invalid);
        let accent = parsed(request.accent, "accent", &mut invalid);
        if invalid.is_empty() {
            Ok(Self {
                locale,
                theme,
                accent,
            })
        } else {
            Err(invalid)
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.locale.is_none() && self.theme.is_none() && self.accent.is_none()
    }
}

fn parsed<T: FromStr>(
    value: Option<String>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<T> {
    let value = value?;
    let parsed = value.parse().ok();
    if parsed.is_none() {
        invalid.push(field);
    }
    parsed
}

pub async fn apply(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    user_id: UserId,
    change: PreferencesChange,
) -> Result<(), UserError> {
    let now = Timestamp::try_from(clock.now())?;
    sqlx::query(UPSERT)
        .bind(user_id.to_string())
        .bind(change.locale.map(LocaleCode::as_str))
        .bind(change.theme.map(Theme::as_str))
        .bind(change.accent.map(Accent::as_str))
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Accent, PreferencesChange, PreferencesRequest, Theme};
    use crate::domain::locale::LocaleCode;

    fn request(
        locale: Option<&str>,
        theme: Option<&str>,
        accent: Option<&str>,
    ) -> PreferencesRequest {
        PreferencesRequest {
            locale: locale.map(ToOwned::to_owned),
            theme: theme.map(ToOwned::to_owned),
            accent: accent.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn unit_preference_presets_are_the_closed_sets() {
        let themes: Vec<&str> = Theme::ALL.iter().map(|theme| theme.as_str()).collect();
        assert_eq!(themes, ["light", "dark", "system"]);
        let accents: Vec<&str> = Accent::ALL.iter().map(|accent| accent.as_str()).collect();
        assert_eq!(
            accents,
            ["default", "blue", "violet", "emerald", "amber", "rose", "slate"]
        );
        for &accent in Accent::ALL {
            assert_eq!(accent.as_str().parse(), Ok(accent));
        }
        for input in [
            "#1668dc",
            "1668dc",
            "rgb(22, 104, 220)",
            "hsl(215, 82%, 48%)",
            "Blue",
            "BLUE",
            " blue",
            "blue ",
            "red",
            "custom",
            "",
        ] {
            assert!(input.parse::<Accent>().is_err(), "{input:?}");
        }
        for input in ["auto", "Light", "DARK", "high-contrast", ""] {
            assert!(input.parse::<Theme>().is_err(), "{input:?}");
        }
    }

    #[test]
    fn unit_preferences_change_parses_and_names_invalid_fields() {
        let change =
            PreferencesChange::parse(request(Some("pt-BR"), Some("dark"), Some("rose"))).unwrap();
        assert_eq!(change.locale, Some(LocaleCode::PtBr));
        assert_eq!(change.theme, Some(Theme::Dark));
        assert_eq!(change.accent, Some(Accent::Rose));
        assert!(PreferencesChange::parse(request(None, None, None))
            .unwrap()
            .is_empty());
        assert_eq!(
            PreferencesChange::parse(request(Some("pt-PT"), Some("dim"), Some("#ff0000"))),
            Err(vec!["locale", "theme", "accent"])
        );
        assert_eq!(
            PreferencesChange::parse(request(None, Some("light"), Some("teal"))),
            Err(vec!["accent"])
        );
    }
}
