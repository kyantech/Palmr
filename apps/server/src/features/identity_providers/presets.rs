use serde::Serialize;
use utoipa::ToSchema;

use super::model::{ClaimMapping, Endpoints, Preset, Protocol, TokenAuthMethod, DEFAULT_SCOPES};

#[derive(Debug, Clone, Copy)]
pub struct FixedEndpoints {
    pub authorization: Option<&'static str>,
    pub token: Option<&'static str>,
    pub userinfo: Option<&'static str>,
    pub jwks: Option<&'static str>,
}

const NO_ENDPOINTS: FixedEndpoints = FixedEndpoints {
    authorization: None,
    token: None,
    userinfo: None,
    jwks: None,
};

#[derive(Debug, Clone, Copy)]
pub struct ClaimNames {
    pub subject: &'static str,
    pub email: &'static str,
    pub email_verified: &'static str,
    pub username: &'static str,
    pub name: &'static str,
    pub picture: &'static str,
}

const STANDARD_CLAIMS: ClaimNames = ClaimNames {
    subject: "sub",
    email: "email",
    email_verified: "email_verified",
    username: "preferred_username",
    name: "name",
    picture: "picture",
};

#[derive(Debug, Clone, Copy)]
pub struct PresetSpec {
    pub preset: Preset,
    pub protocol: Protocol,
    pub display_name: &'static str,
    pub issuer_url: Option<&'static str>,
    pub endpoints: FixedEndpoints,
    pub scopes: &'static [&'static str],
    pub token_auth_method: TokenAuthMethod,
    pub claims: ClaimNames,
}

impl PresetSpec {
    pub const fn allow_email_linking(&self) -> bool {
        self.protocol.default_allow_email_linking()
    }

    pub fn claim_mapping(&self) -> ClaimMapping {
        ClaimMapping {
            subject: self.claims.subject.to_owned(),
            email: self.claims.email.to_owned(),
            email_verified: self.claims.email_verified.to_owned(),
            username: self.claims.username.to_owned(),
            name: self.claims.name.to_owned(),
            picture: self.claims.picture.to_owned(),
        }
    }

    pub fn scope_list(&self) -> Vec<String> {
        self.scopes
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect()
    }

    pub fn endpoint_defaults(&self) -> Endpoints {
        Endpoints {
            authorization: self.endpoints.authorization.map(str::to_owned),
            token: self.endpoints.token.map(str::to_owned),
            userinfo: self.endpoints.userinfo.map(str::to_owned),
            jwks: self.endpoints.jwks.map(str::to_owned),
        }
    }
}

const fn oidc(preset: Preset, display_name: &'static str) -> PresetSpec {
    PresetSpec {
        preset,
        protocol: Protocol::Oidc,
        display_name,
        issuer_url: None,
        endpoints: NO_ENDPOINTS,
        scopes: &DEFAULT_SCOPES,
        token_auth_method: TokenAuthMethod::ClientSecretPost,
        claims: STANDARD_CLAIMS,
    }
}

pub static CATALOGUE: [PresetSpec; 11] = [
    PresetSpec {
        issuer_url: Some("https://accounts.google.com"),
        ..oidc(Preset::Google, "Google")
    },
    PresetSpec {
        preset: Preset::Github,
        protocol: Protocol::OAuth2,
        display_name: "GitHub",
        issuer_url: None,
        endpoints: FixedEndpoints {
            authorization: Some("https://github.com/login/oauth/authorize"),
            token: Some("https://github.com/login/oauth/access_token"),
            userinfo: Some("https://api.github.com/user"),
            jwks: None,
        },
        scopes: &["read:user", "user:email"],
        token_auth_method: TokenAuthMethod::ClientSecretPost,
        claims: ClaimNames {
            subject: "id",
            username: "login",
            picture: "avatar_url",
            ..STANDARD_CLAIMS
        },
    },
    PresetSpec {
        preset: Preset::Discord,
        protocol: Protocol::OAuth2,
        display_name: "Discord",
        issuer_url: None,
        endpoints: FixedEndpoints {
            authorization: Some("https://discord.com/oauth2/authorize"),
            token: Some("https://discord.com/api/oauth2/token"),
            userinfo: Some("https://discord.com/api/users/@me"),
            jwks: None,
        },
        scopes: &["identify", "email"],
        token_auth_method: TokenAuthMethod::ClientSecretPost,
        claims: ClaimNames {
            subject: "id",
            email_verified: "verified",
            username: "username",
            name: "global_name",
            picture: "avatar",
            ..STANDARD_CLAIMS
        },
    },
    oidc(Preset::PocketId, "Pocket ID"),
    oidc(Preset::Authentik, "Authentik"),
    PresetSpec {
        token_auth_method: TokenAuthMethod::ClientSecretBasic,
        ..oidc(Preset::Zitadel, "Zitadel")
    },
    oidc(Preset::Auth0, "Auth0"),
    oidc(Preset::Kinde, "Kinde"),
    PresetSpec {
        claims: ClaimNames {
            picture: "profilePictureUrl",
            ..STANDARD_CLAIMS
        },
        ..oidc(Preset::Frontegg, "Frontegg")
    },
    oidc(Preset::Generic, "Custom OIDC"),
    PresetSpec {
        preset: Preset::Generic,
        protocol: Protocol::OAuth2,
        display_name: "Custom OAuth2",
        issuer_url: None,
        endpoints: NO_ENDPOINTS,
        scopes: &[],
        token_auth_method: TokenAuthMethod::ClientSecretPost,
        claims: STANDARD_CLAIMS,
    },
];

pub fn spec_for(preset: Preset, protocol: Protocol) -> &'static PresetSpec {
    CATALOGUE
        .iter()
        .find(|spec| spec.preset == preset && spec.protocol == protocol)
        .unwrap_or(&CATALOGUE[9])
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresetItem {
    pub preset: Preset,
    pub protocol: Protocol,
    #[schema(example = "Google")]
    pub display_name: String,
    /// The fixed issuer where one is the same for every tenant; `null` when the admin supplies an instance-specific issuer.
    #[schema(required = true)]
    pub issuer_url: Option<String>,
    pub endpoints: Endpoints,
    pub scopes: Vec<String>,
    pub token_auth_method: TokenAuthMethod,
    pub claim_mapping: ClaimMapping,
    pub allow_email_linking: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PresetCatalogue {
    pub items: Vec<PresetItem>,
}

impl PresetCatalogue {
    pub fn bundled() -> Self {
        Self {
            items: CATALOGUE
                .iter()
                .map(|spec| PresetItem {
                    preset: spec.preset,
                    protocol: spec.protocol,
                    display_name: spec.display_name.to_owned(),
                    issuer_url: spec.issuer_url.map(str::to_owned),
                    endpoints: spec.endpoint_defaults(),
                    scopes: spec.scope_list(),
                    token_auth_method: spec.token_auth_method,
                    claim_mapping: spec.claim_mapping(),
                    allow_email_linking: spec.allow_email_linking(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn unit_preset_catalogue_names_every_required_preset() {
        let catalogue = PresetCatalogue::bundled();
        let named: BTreeSet<&str> = catalogue
            .items
            .iter()
            .filter(|item| item.preset != Preset::Generic)
            .map(|item| item.preset.as_str())
            .collect();
        assert_eq!(
            named,
            BTreeSet::from([
                "google",
                "github",
                "discord",
                "pocket_id",
                "authentik",
                "zitadel",
                "auth0",
                "kinde",
                "frontegg",
            ])
        );
        let generic: Vec<(&str, Protocol)> = catalogue
            .items
            .iter()
            .filter(|item| item.preset == Preset::Generic)
            .map(|item| (item.display_name.as_str(), item.protocol))
            .collect();
        assert_eq!(
            generic,
            [
                ("Custom OIDC", Protocol::Oidc),
                ("Custom OAuth2", Protocol::OAuth2)
            ]
        );
        assert_eq!(catalogue.items.len(), 11);
        let unique: BTreeSet<(&str, &str)> = catalogue
            .items
            .iter()
            .map(|item| (item.preset.as_str(), item.protocol.as_str()))
            .collect();
        assert_eq!(unique.len(), catalogue.items.len());
    }

    #[test]
    fn unit_preset_protocols_follow_the_accepted_model() {
        for spec in &CATALOGUE {
            match spec.preset.fixed_protocol() {
                Some(protocol) => assert_eq!(spec.protocol, protocol, "{}", spec.display_name),
                None => assert_eq!(spec.preset, Preset::Generic),
            }
            if spec.protocol == Protocol::OAuth2 {
                assert!(spec.issuer_url.is_none());
                assert!(spec.endpoints.jwks.is_none());
            }
            assert_eq!(spec.allow_email_linking(), spec.protocol == Protocol::Oidc);
        }
        assert_eq!(
            spec_for(Preset::Github, Protocol::OAuth2).display_name,
            "GitHub"
        );
        assert_eq!(
            spec_for(Preset::Zitadel, Protocol::Oidc).token_auth_method,
            TokenAuthMethod::ClientSecretBasic
        );
    }

    #[test]
    fn unit_frontegg_preset_maps_its_actual_avatar_claim() {
        let spec = spec_for(Preset::Frontegg, Protocol::Oidc);
        assert_eq!(spec.claim_mapping().picture, "profilePictureUrl");
        let catalogue = PresetCatalogue::bundled();
        let item = catalogue
            .items
            .iter()
            .find(|item| item.preset == Preset::Frontegg)
            .unwrap();
        assert_eq!(item.claim_mapping.picture, "profilePictureUrl");
        assert_eq!(item.claim_mapping.subject, "sub");
        assert_eq!(item.claim_mapping.email, "email");
    }

    fn keys(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(members) => {
                for (key, member) in members {
                    found.push(key.to_ascii_lowercase());
                    keys(member, found);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| keys(item, found)),
            _ => {}
        }
    }

    #[test]
    fn unit_preset_values_are_safe_and_secret_free() {
        let wire = serde_json::to_value(PresetCatalogue::bundled()).unwrap();
        let mut found = Vec::new();
        keys(&wire, &mut found);
        for key in &found {
            for forbidden in ["secret", "password", "ciphertext", "nonce", "credential"] {
                assert!(!key.contains(forbidden), "{key}");
            }
        }
        for spec in &CATALOGUE {
            for url in [
                spec.issuer_url,
                spec.endpoints.authorization,
                spec.endpoints.token,
                spec.endpoints.userinfo,
                spec.endpoints.jwks,
            ]
            .into_iter()
            .flatten()
            {
                assert!(
                    super::super::http_client::acceptable_url(url).is_some(),
                    "{url}"
                );
            }
        }
    }
}
