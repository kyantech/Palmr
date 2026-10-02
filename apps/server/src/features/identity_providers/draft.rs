use super::discovery::{discovery_url, Discovered};
use super::input::{ClaimsInput, CreateInput, EndpointsInput, Patch, UpdateInput};
use super::model::{
    ClaimMapping, Endpoints, IdentityProvider, OAuth2Provider, OidcProvider, Preset, Protocol,
    ProviderVariant, TokenAuthMethod, DEFAULT_SCOPES, SCOPES_MAX_CHARS,
};
use super::presets::spec_for;
use crate::domain::secret::Secret;
use crate::features::audit::actions::Presence;

#[derive(Debug)]
pub enum SecretChange {
    Keep,
    Replace(Secret<String>),
    Clear,
}

#[derive(Debug)]
pub struct Draft {
    pub slug: String,
    pub display_name: String,
    pub protocol: Protocol,
    pub preset: Preset,
    pub issuer: Option<String>,
    pub endpoints: Endpoints,
    pub client_id: String,
    pub secret: SecretChange,
    pub had_secret: bool,
    pub scopes: Vec<String>,
    pub token_auth_method: TokenAuthMethod,
    pub claims: ClaimMapping,
    pub auto_provision: bool,
    pub allow_email_linking: bool,
    pub enabled: bool,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub fields: Vec<&'static str>,
    pub client_secret: Option<(Presence, Presence)>,
    pub validation_reset: bool,
    pub enabled: Option<bool>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.client_secret.is_none() && self.enabled.is_none()
    }

    pub fn has_configuration_change(&self) -> bool {
        !self.fields.is_empty() || self.client_secret.is_some()
    }

    fn mark(&mut self, field: &'static str, invalidates: bool) {
        if !self.fields.contains(&field) {
            self.fields.push(field);
        }
        self.validation_reset |= invalidates;
    }
}

fn presence(present: bool) -> Presence {
    if present {
        Presence::Set
    } else {
        Presence::Unset
    }
}

fn set_text(slot: &mut String, value: &str) -> bool {
    let changed = slot != value;
    if changed {
        value.clone_into(slot);
    }
    changed
}

fn apply_endpoint(slot: &mut Option<String>, patch: &Patch<String>) -> bool {
    match patch {
        Patch::Absent => false,
        Patch::Clear => slot.take().is_some(),
        Patch::Set(value) => {
            let changed = slot.as_deref() != Some(value.as_str());
            *slot = Some(value.clone());
            changed
        }
    }
}

fn apply_endpoints(endpoints: &mut Endpoints, input: &EndpointsInput) -> bool {
    let results = [
        apply_endpoint(&mut endpoints.authorization, &input.authorization),
        apply_endpoint(&mut endpoints.token, &input.token),
        apply_endpoint(&mut endpoints.userinfo, &input.userinfo),
        apply_endpoint(&mut endpoints.jwks, &input.jwks),
    ];
    results.into_iter().any(|changed| changed)
}

fn apply_claims(claims: &mut ClaimMapping, input: &ClaimsInput) -> bool {
    let slots: [(&mut String, &Option<String>); 6] = [
        (&mut claims.subject, &input.subject),
        (&mut claims.email, &input.email),
        (&mut claims.email_verified, &input.email_verified),
        (&mut claims.username, &input.username),
        (&mut claims.name, &input.name),
        (&mut claims.picture, &input.picture),
    ];
    let mut changed = false;
    for (slot, value) in slots {
        if let Some(value) = value {
            changed |= set_text(slot, value);
        }
    }
    changed
}

impl Draft {
    pub fn for_create(input: CreateInput) -> Result<Self, Vec<&'static str>> {
        let preset = input.preset.unwrap_or(Preset::Generic);
        if preset
            .fixed_protocol()
            .is_some_and(|fixed| fixed != input.protocol)
        {
            return Err(vec!["preset"]);
        }
        let spec = spec_for(preset, input.protocol);
        let mut endpoints = spec.endpoint_defaults();
        apply_endpoints(&mut endpoints, &input.endpoints);
        let mut claims = spec.claim_mapping();
        apply_claims(&mut claims, &input.claims);
        let scopes = input
            .scopes
            .unwrap_or_else(|| match (spec.scopes, input.protocol) {
                ([], Protocol::Oidc) => DEFAULT_SCOPES.map(str::to_owned).to_vec(),
                _ => spec.scope_list(),
            });
        let issuer = input.issuer.or_else(|| {
            spec.issuer_url
                .map(str::to_owned)
                .filter(|_| input.protocol == Protocol::Oidc)
        });
        let secret = input
            .client_secret
            .map_or(SecretChange::Keep, SecretChange::Replace);
        Ok(Self {
            slug: input.slug,
            display_name: input.display_name,
            protocol: input.protocol,
            preset,
            issuer,
            endpoints,
            client_id: input.client_id,
            secret,
            had_secret: false,
            scopes,
            token_auth_method: input.token_auth_method.unwrap_or(spec.token_auth_method),
            claims,
            auto_provision: input.auto_provision.unwrap_or(false),
            allow_email_linking: input
                .allow_email_linking
                .unwrap_or_else(|| input.protocol.default_allow_email_linking()),
            enabled: input.enabled.unwrap_or(false),
            sort_order: input.sort_order,
        })
    }

    pub fn from_existing(provider: &IdentityProvider) -> Self {
        Self {
            slug: provider.slug.clone(),
            display_name: provider.display_name.clone(),
            protocol: provider.protocol(),
            preset: provider.preset,
            issuer: provider.kind.issuer().map(str::to_owned),
            endpoints: provider.kind.endpoints(),
            client_id: provider.client_id.clone(),
            secret: SecretChange::Keep,
            had_secret: provider.client_secret.is_some(),
            scopes: provider.scopes.clone(),
            token_auth_method: provider.token_auth_method,
            claims: provider.claims.clone(),
            auto_provision: provider.auto_provision,
            allow_email_linking: provider.allow_email_linking,
            enabled: provider.enabled,
            sort_order: Some(provider.sort_order),
        }
    }

    pub fn secret_after(&self) -> bool {
        match self.secret {
            SecretChange::Keep => self.had_secret,
            SecretChange::Replace(_) => true,
            SecretChange::Clear => false,
        }
    }

    pub fn apply(&mut self, input: UpdateInput) -> Changes {
        let mut changes = Changes::default();
        if let Some(name) = &input.display_name {
            if set_text(&mut self.display_name, name) {
                changes.mark("display_name", false);
            }
        }
        if let Some(protocol) = input.protocol {
            if protocol != self.protocol {
                self.protocol = protocol;
                self.endpoints = Endpoints::default();
                if protocol == Protocol::OAuth2 {
                    self.issuer = None;
                }
                changes.mark("protocol", true);
            }
        }
        if let Some(preset) = input.preset {
            if preset != self.preset {
                self.preset = preset;
                changes.mark("preset", false);
            }
        }
        match &input.issuer {
            Patch::Absent => {}
            Patch::Clear => {
                if self.issuer.take().is_some() {
                    self.endpoints = Endpoints::default();
                    changes.mark("issuer", true);
                }
            }
            Patch::Set(issuer) => {
                if self.issuer.as_deref() != Some(issuer.as_str()) {
                    self.issuer = Some(issuer.clone());
                    self.endpoints = Endpoints::default();
                    changes.mark("issuer", true);
                }
            }
        }
        if let Some(client_id) = &input.client_id {
            if set_text(&mut self.client_id, client_id) {
                changes.mark("client_id", true);
            }
        }
        match input.client_secret {
            Patch::Absent => {}
            Patch::Clear => {
                if self.secret_after() {
                    changes.client_secret = Some((Presence::Set, Presence::Unset));
                    changes.validation_reset = true;
                }
                self.secret = SecretChange::Clear;
            }
            Patch::Set(secret) => {
                changes.client_secret = Some((presence(self.secret_after()), Presence::Set));
                changes.validation_reset = true;
                self.secret = SecretChange::Replace(secret);
            }
        }
        if let Some(scopes) = input.scopes {
            if scopes != self.scopes {
                self.scopes = scopes;
                changes.mark("scopes", false);
            }
        }
        if let Some(method) = input.token_auth_method {
            if method != self.token_auth_method {
                self.token_auth_method = method;
                changes.mark("token_auth_method", true);
            }
        }
        if !input.endpoints.is_empty() && apply_endpoints(&mut self.endpoints, &input.endpoints) {
            changes.mark("endpoints", true);
        }
        if !input.claims.is_empty() && apply_claims(&mut self.claims, &input.claims) {
            changes.mark("claim_mapping", false);
        }
        if let Some(flag) = input.auto_provision {
            if flag != self.auto_provision {
                self.auto_provision = flag;
                changes.mark("auto_provision", false);
            }
        }
        if let Some(flag) = input.allow_email_linking {
            if flag != self.allow_email_linking {
                self.allow_email_linking = flag;
                changes.mark("allow_email_linking", false);
            }
        }
        if let Some(order) = input.sort_order {
            if self.sort_order != Some(order) {
                self.sort_order = Some(order);
                changes.mark("sort_order", false);
            }
        }
        if let Some(enabled) = input.enabled {
            if enabled != self.enabled {
                self.enabled = enabled;
                changes.enabled = Some(enabled);
            }
        }
        changes
    }

    pub fn needs_discovery(&self) -> bool {
        self.protocol == Protocol::Oidc
            && self.issuer.is_some()
            && (self.endpoints.authorization.is_none()
                || self.endpoints.token.is_none()
                || self.endpoints.jwks.is_none())
    }

    pub fn fill_from(&mut self, discovered: &Discovered) {
        let fill = |slot: &mut Option<String>, value: &Option<String>| {
            if slot.is_none() {
                slot.clone_from(value);
            }
        };
        fill(
            &mut self.endpoints.authorization,
            &discovered.endpoints.authorization,
        );
        fill(&mut self.endpoints.token, &discovered.endpoints.token);
        fill(&mut self.endpoints.userinfo, &discovered.endpoints.userinfo);
        fill(&mut self.endpoints.jwks, &discovered.endpoints.jwks);
    }

    pub fn validate(&self) -> Result<ProviderVariant, Vec<&'static str>> {
        let mut invalid = Vec::new();
        if self
            .preset
            .fixed_protocol()
            .is_some_and(|fixed| fixed != self.protocol)
        {
            invalid.push("preset");
        }
        let secret_ok = match self.token_auth_method {
            TokenAuthMethod::None => !self.secret_after(),
            TokenAuthMethod::ClientSecretBasic | TokenAuthMethod::ClientSecretPost => {
                self.secret_after()
            }
        };
        if !secret_ok {
            invalid.push("clientSecret");
        }
        if self.scopes.join(" ").len() > SCOPES_MAX_CHARS
            || (self.protocol == Protocol::Oidc
                && !self.scopes.iter().any(|scope| scope == "openid"))
        {
            invalid.push("scopes");
        }
        let kind = match self.protocol {
            Protocol::Oidc => self.oidc_kind(&mut invalid),
            Protocol::OAuth2 => self.oauth2_kind(&mut invalid),
        };
        match kind {
            Some(kind) if invalid.is_empty() => Ok(kind),
            _ => Err(invalid),
        }
    }

    fn oidc_kind(&self, invalid: &mut Vec<&'static str>) -> Option<ProviderVariant> {
        let issuer = self.issuer.clone();
        if issuer.is_none() {
            invalid.push("issuerUrl");
        }
        let endpoints = &self.endpoints;
        let (Some(authorization), Some(token), Some(jwks)) = (
            endpoints.authorization.clone(),
            endpoints.token.clone(),
            endpoints.jwks.clone(),
        ) else {
            invalid.push("endpoints");
            return None;
        };
        let issuer = issuer?;
        Some(ProviderVariant::Oidc(OidcProvider {
            discovery_url: discovery_url(&issuer),
            issuer,
            authorization_endpoint: authorization,
            token_endpoint: token,
            userinfo_endpoint: endpoints.userinfo.clone(),
            jwks_uri: jwks,
        }))
    }

    fn oauth2_kind(&self, invalid: &mut Vec<&'static str>) -> Option<ProviderVariant> {
        if self.issuer.is_some() {
            invalid.push("issuerUrl");
        }
        let endpoints = &self.endpoints;
        if endpoints.jwks.is_some() {
            invalid.push("endpoints");
        }
        let (Some(authorization), Some(token), Some(userinfo)) = (
            endpoints.authorization.clone(),
            endpoints.token.clone(),
            endpoints.userinfo.clone(),
        ) else {
            invalid.push("endpoints");
            return None;
        };
        Some(ProviderVariant::OAuth2(OAuth2Provider {
            authorization_endpoint: authorization,
            token_endpoint: token,
            userinfo_endpoint: userinfo,
        }))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::features::identity_providers::input::{
        CreateInput, CreateProviderRequest, UpdateInput, UpdateProviderRequest,
    };
    use crate::infra::http::json::parse;

    fn create(body: serde_json::Value) -> Draft {
        let request = parse::<CreateProviderRequest>(body).unwrap();
        Draft::for_create(CreateInput::parse(request).unwrap()).unwrap()
    }

    fn update(body: serde_json::Value) -> UpdateInput {
        UpdateInput::parse(parse::<UpdateProviderRequest>(body).unwrap()).unwrap()
    }

    fn oidc_body() -> serde_json::Value {
        json!({
            "slug": "authentik", "displayName": "Company SSO", "protocol": "oidc",
            "issuerUrl": "https://sso.example.com/application/o/palmr/",
            "clientId": "palmr", "clientSecret": "s3cret",
        })
    }

    fn discovered() -> Discovered {
        Discovered {
            issuer_url: "https://sso.example.com/application/o/palmr/".to_owned(),
            endpoints: Endpoints {
                authorization: Some("https://sso.example.com/authorize".to_owned()),
                token: Some("https://sso.example.com/token".to_owned()),
                userinfo: Some("https://sso.example.com/userinfo".to_owned()),
                jwks: Some("https://sso.example.com/jwks".to_owned()),
            },
            scopes_supported: Vec::new(),
            token_endpoint_auth_methods_supported: Vec::new(),
        }
    }

    #[test]
    fn unit_provider_create_defaults_follow_the_protocol() {
        let oidc = create(oidc_body());
        assert!(!oidc.auto_provision && !oidc.enabled);
        assert!(oidc.allow_email_linking);
        assert_eq!(oidc.scopes, ["openid", "profile", "email"]);
        assert_eq!(oidc.token_auth_method, TokenAuthMethod::ClientSecretPost);
        assert!(oidc.needs_discovery());

        let oauth2 = create(json!({
            "slug": "custom-oauth", "displayName": "Custom", "protocol": "oauth2",
            "clientId": "c", "clientSecret": "s",
            "endpoints": {"authorization": "https://idp.example.com/a", "token": "https://idp.example.com/t", "userinfo": "https://idp.example.com/u"},
        }));
        assert!(!oauth2.auto_provision && !oauth2.allow_email_linking);
        assert!(!oauth2.needs_discovery());
        assert!(oauth2.validate().is_ok());
    }

    #[test]
    fn unit_provider_create_overrides_persist_in_the_draft() {
        let mut body = oidc_body();
        body["autoProvision"] = json!(true);
        body["allowEmailLinking"] = json!(false);
        body["enabled"] = json!(true);
        let draft = create(body);
        assert!(draft.auto_provision && draft.enabled && !draft.allow_email_linking);
    }

    #[test]
    fn unit_provider_preset_defaults_fill_omitted_members_only() {
        let github = create(json!({
            "slug": "github", "displayName": "GitHub", "protocol": "oauth2", "preset": "github",
            "clientId": "c", "clientSecret": "s",
        }));
        assert_eq!(github.scopes, ["read:user", "user:email"]);
        assert_eq!(github.claims.subject, "id");
        assert_eq!(
            github.endpoints.userinfo.as_deref(),
            Some("https://api.github.com/user")
        );
        assert!(github.validate().is_ok());
        assert!(!github.allow_email_linking);

        let overridden = create(json!({
            "slug": "github", "displayName": "GitHub", "protocol": "oauth2", "preset": "github",
            "clientId": "c", "clientSecret": "s", "scopes": ["user:email"],
            "claimMapping": {"subject": "node_id"},
            "endpoints": {"userinfo": "https://ghe.example.com/api/v3/user"},
        }));
        assert_eq!(overridden.scopes, ["user:email"]);
        assert_eq!(overridden.claims.subject, "node_id");
        assert_eq!(overridden.claims.username, "login");
        assert_eq!(
            overridden.endpoints.userinfo.as_deref(),
            Some("https://ghe.example.com/api/v3/user")
        );

        let google = create(json!({
            "slug": "google", "displayName": "Google", "protocol": "oidc", "preset": "google",
            "clientId": "c", "clientSecret": "s",
        }));
        assert_eq!(
            google.issuer.as_deref(),
            Some("https://accounts.google.com")
        );
    }

    #[test]
    fn unit_provider_preset_must_match_its_fixed_protocol() {
        let request = parse::<CreateProviderRequest>(json!({
            "slug": "github", "displayName": "GitHub", "protocol": "oidc", "preset": "github",
            "clientId": "c", "clientSecret": "s",
        }))
        .unwrap();
        assert_eq!(
            Draft::for_create(CreateInput::parse(request).unwrap()).unwrap_err(),
            ["preset"]
        );
    }

    #[test]
    fn unit_provider_oidc_requires_issuer_openid_scope_and_complete_endpoints() {
        let mut draft = create(oidc_body());
        assert_eq!(draft.validate().unwrap_err(), ["endpoints"]);
        draft.fill_from(&discovered());
        assert!(draft.validate().is_ok());
        assert!(!draft.needs_discovery());

        draft.scopes = vec!["email".to_owned()];
        assert_eq!(draft.validate().unwrap_err(), ["scopes"]);
        draft.scopes = vec!["openid".to_owned()];
        draft.issuer = None;
        assert_eq!(draft.validate().unwrap_err(), ["issuerUrl"]);
    }

    #[test]
    fn unit_provider_oauth2_requires_explicit_endpoints_and_has_no_issuer_or_jwks() {
        let mut body = json!({
            "slug": "custom-oauth", "displayName": "Custom", "protocol": "oauth2",
            "clientId": "c", "clientSecret": "s",
        });
        assert_eq!(create(body.clone()).validate().unwrap_err(), ["endpoints"]);
        body["endpoints"] = json!({"authorization": "https://idp.example.com/a", "token": "https://idp.example.com/t"});
        assert_eq!(create(body.clone()).validate().unwrap_err(), ["endpoints"]);
        body["endpoints"]["userinfo"] = json!("https://idp.example.com/u");
        body["endpoints"]["jwks"] = json!("https://idp.example.com/j");
        assert_eq!(create(body.clone()).validate().unwrap_err(), ["endpoints"]);
        body["endpoints"].as_object_mut().unwrap().remove("jwks");
        body["issuerUrl"] = json!("https://idp.example.com/");
        assert_eq!(create(body).validate().unwrap_err(), ["issuerUrl"]);
    }

    #[test]
    fn unit_provider_public_client_has_no_secret_and_confidential_client_has_one() {
        let mut public = oidc_body();
        public["tokenAuthMethod"] = json!("none");
        public.as_object_mut().unwrap().remove("clientSecret");
        let mut draft = create(public.clone());
        draft.fill_from(&discovered());
        assert!(draft.validate().is_ok());

        public["clientSecret"] = json!("s3cret");
        let mut with_secret = create(public);
        with_secret.fill_from(&discovered());
        assert_eq!(with_secret.validate().unwrap_err(), ["clientSecret"]);

        let mut body = oidc_body();
        body.as_object_mut().unwrap().remove("clientSecret");
        let mut missing = create(body);
        missing.fill_from(&discovered());
        assert_eq!(missing.validate().unwrap_err(), ["clientSecret"]);
    }

    fn existing() -> Draft {
        let mut draft = create(oidc_body());
        draft.fill_from(&discovered());
        draft.had_secret = true;
        draft.secret = SecretChange::Keep;
        draft.sort_order = Some(3);
        draft
    }

    #[test]
    fn unit_provider_patch_invalidates_validation_for_connection_critical_fields() {
        for body in [
            json!({"issuerUrl": "https://other.example.com/"}),
            json!({"clientId": "other"}),
            json!({"clientSecret": "rotated"}),
            json!({"endpoints": {"token": "https://sso.example.com/token2"}}),
            json!({"protocol": "oauth2"}),
            json!({"tokenAuthMethod": "client_secret_basic"}),
        ] {
            let mut draft = existing();
            let changes = draft.apply(update(body.clone()));
            assert!(changes.validation_reset, "{body}");
        }
        for body in [
            json!({"displayName": "Renamed"}),
            json!({"scopes": ["openid"]}),
            json!({"claimMapping": {"email": "mail"}}),
            json!({"autoProvision": true}),
            json!({"allowEmailLinking": false}),
            json!({"sortOrder": 9}),
            json!({"enabled": true}),
            json!({"preset": "authentik"}),
        ] {
            let mut draft = existing();
            let changes = draft.apply(update(body.clone()));
            assert!(!changes.validation_reset, "{body}");
            assert!(!changes.is_empty(), "{body}");
        }
    }

    #[test]
    fn unit_provider_patch_with_unchanged_values_is_a_no_op() {
        let mut draft = existing();
        let changes = draft.apply(update(json!({
            "displayName": "Company SSO",
            "clientId": "palmr",
            "issuerUrl": "https://sso.example.com/application/o/palmr/",
            "enabled": false,
            "autoProvision": false,
            "sortOrder": 3,
            "endpoints": {"token": "https://sso.example.com/token"},
        })));
        assert!(changes.is_empty(), "{changes:?}");
        assert!(!changes.validation_reset);
    }

    #[test]
    fn unit_provider_patch_secret_semantics() {
        let mut draft = existing();
        let changes = draft.apply(update(json!({"displayName": "x"})));
        assert!(matches!(draft.secret, SecretChange::Keep));
        assert_eq!(changes.client_secret, None);

        let mut draft = existing();
        let changes = draft.apply(update(json!({"clientSecret": "rotated"})));
        assert!(matches!(draft.secret, SecretChange::Replace(_)));
        assert_eq!(changes.client_secret, Some((Presence::Set, Presence::Set)));

        let mut draft = existing();
        let changes = draft.apply(update(
            json!({"clientSecret": null, "tokenAuthMethod": "none"}),
        ));
        assert!(matches!(draft.secret, SecretChange::Clear));
        assert_eq!(
            changes.client_secret,
            Some((Presence::Set, Presence::Unset))
        );
        assert!(draft.validate().is_ok());

        let mut draft = existing();
        draft.apply(update(json!({"clientSecret": null})));
        assert_eq!(draft.validate().unwrap_err(), ["clientSecret"]);
    }

    #[test]
    fn unit_provider_issuer_change_discards_cached_endpoints_and_rediscovers() {
        let mut draft = existing();
        let changes = draft.apply(update(json!({"issuerUrl": "https://other.example.com/"})));
        assert_eq!(changes.fields, ["issuer"]);
        assert_eq!(draft.endpoints, Endpoints::default());
        assert!(draft.needs_discovery());

        let mut draft = existing();
        draft.apply(update(json!({
            "issuerUrl": "https://other.example.com/",
            "endpoints": {"authorization": "https://other.example.com/a", "token": "https://other.example.com/t", "jwks": "https://other.example.com/j"},
        })));
        assert!(!draft.needs_discovery());
    }

    #[test]
    fn unit_provider_protocol_switch_resets_endpoints_and_issuer() {
        let mut draft = existing();
        draft.apply(update(json!({"protocol": "oauth2"})));
        assert_eq!(draft.protocol, Protocol::OAuth2);
        assert_eq!(draft.issuer, None);
        assert_eq!(draft.endpoints, Endpoints::default());
        assert_eq!(draft.validate().unwrap_err(), ["endpoints"]);
    }

    #[test]
    fn unit_provider_enabled_transition_is_reported_separately() {
        let mut draft = existing();
        let changes = draft.apply(update(json!({"enabled": true})));
        assert_eq!(changes.enabled, Some(true));
        assert!(changes.fields.is_empty());
        let changes = draft.apply(update(json!({"enabled": true})));
        assert_eq!(changes.enabled, None);
        let changes = draft.apply(update(json!({"enabled": false, "displayName": "Z"})));
        assert_eq!(changes.enabled, Some(false));
        assert_eq!(changes.fields, ["display_name"]);
    }
}
