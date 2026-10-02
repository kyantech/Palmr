use std::fmt;

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use utoipa::ToSchema;

use super::discovery::acceptable_issuer;
use super::http_client::acceptable_url;
use super::model::{
    is_valid_slug, Preset, Protocol, TokenAuthMethod, CLAIM_MAX_CHARS, CLIENT_ID_MAX_CHARS,
    CLIENT_SECRET_MAX_BYTES, DISPLAY_NAME_MAX_CHARS, MAX_SORT_ORDER, SCOPES_MAX_CHARS,
};
use crate::domain::secret::{Secret, REDACTED};
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

const MAX_SCOPES: usize = 50;
const ENDPOINT_MEMBERS: [&str; 4] = ["authorization", "token", "userinfo", "jwks"];
const CLAIM_MEMBERS: [&str; 6] = [
    "subject",
    "email",
    "emailVerified",
    "username",
    "name",
    "picture",
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Patch<T> {
    #[default]
    Absent,
    Clear,
    Set(T),
}

impl<T> Patch<T> {
    pub const fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EndpointsInput {
    pub authorization: Patch<String>,
    pub token: Patch<String>,
    pub userinfo: Patch<String>,
    pub jwks: Patch<String>,
}

impl EndpointsInput {
    pub const fn is_empty(&self) -> bool {
        self.authorization.is_absent()
            && self.token.is_absent()
            && self.userinfo.is_absent()
            && self.jwks.is_absent()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaimsInput {
    pub subject: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<String>,
    pub username: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

impl ClaimsInput {
    pub const fn is_empty(&self) -> bool {
        self.subject.is_none()
            && self.email.is_none()
            && self.email_verified.is_none()
            && self.username.is_none()
            && self.name.is_none()
            && self.picture.is_none()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointsRequest {
    /// Absolute https URL (plain http only for a loopback host). An OAuth2 provider requires `authorization`, `token` and `userinfo`; an OIDC provider takes the members it lacks from discovery.
    #[schema(example = "https://sso.example.com/authorize")]
    pub authorization: Option<String>,
    #[schema(example = "https://sso.example.com/token")]
    pub token: Option<String>,
    #[schema(example = "https://sso.example.com/userinfo")]
    pub userinfo: Option<String>,
    /// OIDC only.
    #[schema(example = "https://sso.example.com/jwks")]
    pub jwks: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimMappingRequest {
    #[schema(example = "sub")]
    pub subject: Option<String>,
    #[schema(example = "email")]
    pub email: Option<String>,
    #[schema(example = "email_verified")]
    pub email_verified: Option<String>,
    #[schema(example = "preferred_username")]
    pub username: Option<String>,
    #[schema(example = "name")]
    pub name: Option<String>,
    #[schema(example = "picture")]
    pub picture: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProviderRequest {
    /// Lowercase `[a-z0-9_-]`, 2–40 characters, unique and immutable: it is part of the callback URI registered at the identity provider.
    #[schema(
        min_length = 2,
        max_length = 40,
        pattern = "^[a-z0-9_-]+$",
        example = "authentik"
    )]
    pub slug: String,
    #[schema(min_length = 1, max_length = 64, example = "Company SSO")]
    pub display_name: String,
    #[schema(value_type = Protocol)]
    pub protocol: String,
    /// Defaults the omitted members from the bundled preset. Defaults to `generic`.
    #[schema(value_type = Option<Preset>)]
    pub preset: Option<String>,
    /// OIDC only. Discovery runs from `<issuerUrl>/.well-known/openid-configuration`, and the document's `issuer` must equal this value exactly.
    #[schema(
        max_length = 512,
        example = "https://sso.example.com/application/o/palmr/"
    )]
    pub issuer_url: Option<String>,
    #[schema(min_length = 1, max_length = 512)]
    pub client_id: String,
    /// Write-only. Required unless `tokenAuthMethod` is `none`, which is for public clients only.
    #[schema(write_only, min_length = 1, format = Password)]
    pub client_secret: Option<String>,
    /// An OIDC provider must request `openid`.
    #[schema(value_type = Option<Vec<String>>, example = json!(["openid", "email", "profile"]))]
    pub scopes: Option<Vec<String>>,
    #[schema(value_type = Option<TokenAuthMethod>)]
    pub token_auth_method: Option<String>,
    #[schema(value_type = Option<EndpointsRequest>)]
    pub endpoints: Option<Value>,
    #[schema(value_type = Option<ClaimMappingRequest>)]
    pub claim_mapping: Option<Value>,
    /// Defaults to `false`. Auto-provisioned accounts are always role `user`.
    pub auto_provision: Option<bool>,
    /// Defaults to `true` for `oidc` and `false` for `oauth2`.
    pub allow_email_linking: Option<bool>,
    /// Defaults to `false`.
    pub enabled: Option<bool>,
    /// Defaults to the end of the list.
    #[schema(minimum = 0, maximum = 1_000_000)]
    pub sort_order: Option<i64>,
}

impl fmt::Debug for CreateProviderRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateProviderRequest")
            .field("slug", &self.slug)
            .field("protocol", &self.protocol)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| REDACTED),
            )
            .finish_non_exhaustive()
    }
}

impl JsonRequest for CreateProviderRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("slug", JsonKind::String),
        JsonField::required("displayName", JsonKind::String),
        JsonField::required("protocol", JsonKind::String),
        JsonField::optional("preset", JsonKind::String),
        JsonField::optional("issuerUrl", JsonKind::String),
        JsonField::required("clientId", JsonKind::String),
        JsonField::optional("clientSecret", JsonKind::String),
        JsonField::optional("scopes", JsonKind::Array),
        JsonField::optional("tokenAuthMethod", JsonKind::String),
        JsonField::optional("endpoints", JsonKind::Object),
        JsonField::optional("claimMapping", JsonKind::Object),
        JsonField::optional("autoProvision", JsonKind::Boolean),
        JsonField::optional("allowEmailLinking", JsonKind::Boolean),
        JsonField::optional("enabled", JsonKind::Boolean),
        JsonField::optional("sortOrder", JsonKind::Integer),
    ];
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProviderRequest {
    /// The slug is immutable and is rejected here, like every member not listed.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, nullable = false, min_length = 1, max_length = 64)]
    pub display_name: Option<Option<String>>,
    /// Changing the protocol clears the endpoints discovered or entered for the previous one and invalidates `validatedAt`.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Protocol, nullable = false)]
    pub protocol: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Preset, nullable = false)]
    pub preset: Option<Option<String>>,
    /// Changing the issuer discards the cached endpoints, runs discovery again and invalidates `validatedAt`. `null` is valid only for `oauth2`.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, max_length = 512)]
    pub issuer_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, nullable = false, min_length = 1, max_length = 512)]
    pub client_id: Option<Option<String>>,
    /// Write-only. Absent leaves the stored secret unchanged, a string replaces it and explicit `null` clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable = true, write_only, min_length = 1, format = Password)]
    pub client_secret: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Vec<String>, nullable = false)]
    pub scopes: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = TokenAuthMethod, nullable = false)]
    pub token_auth_method: Option<Option<String>>,
    /// Per member: absent leaves it unchanged, a URL sets it and `null` clears it.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = EndpointsRequest, nullable = false)]
    pub endpoints: Option<Option<Value>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = ClaimMappingRequest, nullable = false)]
    pub claim_mapping: Option<Option<Value>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = bool, nullable = false)]
    pub auto_provision: Option<Option<bool>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = bool, nullable = false)]
    pub allow_email_linking: Option<Option<bool>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = bool, nullable = false)]
    pub enabled: Option<Option<bool>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = i64, nullable = false, minimum = 0, maximum = 1_000_000)]
    pub sort_order: Option<Option<i64>>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

impl fmt::Debug for UpdateProviderRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateProviderRequest")
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| REDACTED),
            )
            .finish_non_exhaustive()
    }
}

impl JsonRequest for UpdateProviderRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("displayName", JsonKind::String),
        JsonField::optional("protocol", JsonKind::String),
        JsonField::optional("preset", JsonKind::String),
        JsonField::optional("issuerUrl", JsonKind::String),
        JsonField::optional("clientId", JsonKind::String),
        JsonField::optional("clientSecret", JsonKind::String),
        JsonField::optional("scopes", JsonKind::Array),
        JsonField::optional("tokenAuthMethod", JsonKind::String),
        JsonField::optional("endpoints", JsonKind::Object),
        JsonField::optional("claimMapping", JsonKind::Object),
        JsonField::optional("autoProvision", JsonKind::Boolean),
        JsonField::optional("allowEmailLinking", JsonKind::Boolean),
        JsonField::optional("enabled", JsonKind::Boolean),
        JsonField::optional("sortOrder", JsonKind::Integer),
    ];
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverRequest {
    #[schema(
        max_length = 512,
        example = "https://sso.example.com/application/o/palmr/"
    )]
    pub issuer_url: String,
}

impl JsonRequest for DiscoverRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("issuerUrl", JsonKind::String)];
}

impl DiscoverRequest {
    pub fn parse(self) -> Result<String, Vec<&'static str>> {
        acceptable_issuer(&self.issuer_url).ok_or_else(|| vec!["issuerUrl"])
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderRequest {
    /// Every provider id exactly once, in the new display order.
    pub order: Vec<String>,
}

impl JsonRequest for OrderRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("order", JsonKind::Array)];
}

#[derive(Debug)]
pub struct CreateInput {
    pub slug: String,
    pub display_name: String,
    pub protocol: Protocol,
    pub preset: Option<Preset>,
    pub issuer: Option<String>,
    pub client_id: String,
    pub client_secret: Option<Secret<String>>,
    pub scopes: Option<Vec<String>>,
    pub token_auth_method: Option<TokenAuthMethod>,
    pub endpoints: EndpointsInput,
    pub claims: ClaimsInput,
    pub auto_provision: Option<bool>,
    pub allow_email_linking: Option<bool>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
}

impl CreateInput {
    pub fn parse(request: CreateProviderRequest) -> Result<Self, Vec<&'static str>> {
        let CreateProviderRequest {
            slug,
            display_name,
            protocol,
            preset,
            issuer_url,
            client_id,
            client_secret,
            scopes,
            token_auth_method,
            endpoints,
            claim_mapping,
            auto_provision,
            allow_email_linking,
            enabled,
            sort_order,
        } = request;
        let mut invalid = Vec::new();
        let slug = checked(is_valid_slug(&slug).then_some(slug), "slug", &mut invalid);
        let display_name = checked(
            valid_display_name(&display_name),
            "displayName",
            &mut invalid,
        );
        let protocol = checked(Protocol::parse(&protocol), "protocol", &mut invalid);
        let preset = optional(preset, |text| Preset::parse(&text), "preset", &mut invalid);
        let issuer = optional(
            issuer_url,
            |text| acceptable_issuer(&text),
            "issuerUrl",
            &mut invalid,
        );
        let client_id = checked(valid_client_id(&client_id), "clientId", &mut invalid);
        let client_secret = optional(client_secret, valid_secret, "clientSecret", &mut invalid);
        let scopes = optional(scopes, valid_scopes, "scopes", &mut invalid);
        let token_auth_method = optional(
            token_auth_method,
            |text| TokenAuthMethod::parse(&text),
            "tokenAuthMethod",
            &mut invalid,
        );
        let endpoints = checked(
            endpoints.map_or(Some(EndpointsInput::default()), |value| {
                parse_endpoints(&value)
            }),
            "endpoints",
            &mut invalid,
        );
        let claims = checked(
            claim_mapping.map_or(Some(ClaimsInput::default()), |value| parse_claims(&value)),
            "claimMapping",
            &mut invalid,
        );
        let sort_order = optional(sort_order, valid_sort_order, "sortOrder", &mut invalid);
        match (slug, display_name, protocol, client_id, endpoints, claims) {
            (
                Some(slug),
                Some(display_name),
                Some(protocol),
                Some(client_id),
                Some(endpoints),
                Some(claims),
            ) if invalid.is_empty() => Ok(Self {
                slug,
                display_name,
                protocol,
                preset: preset.flatten(),
                issuer: issuer.flatten(),
                client_id,
                client_secret: client_secret.flatten(),
                scopes: scopes.flatten(),
                token_auth_method: token_auth_method.flatten(),
                endpoints,
                claims,
                auto_provision,
                allow_email_linking,
                enabled,
                sort_order: sort_order.flatten(),
            }),
            _ => Err(invalid),
        }
    }
}

#[derive(Debug, Default)]
pub struct UpdateInput {
    pub display_name: Option<String>,
    pub protocol: Option<Protocol>,
    pub preset: Option<Preset>,
    pub issuer: Patch<String>,
    pub client_id: Option<String>,
    pub client_secret: Patch<Secret<String>>,
    pub scopes: Option<Vec<String>>,
    pub token_auth_method: Option<TokenAuthMethod>,
    pub endpoints: EndpointsInput,
    pub claims: ClaimsInput,
    pub auto_provision: Option<bool>,
    pub allow_email_linking: Option<bool>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
}

impl UpdateInput {
    pub fn parse(request: UpdateProviderRequest) -> Result<Self, Vec<&'static str>> {
        let UpdateProviderRequest {
            display_name,
            protocol,
            preset,
            issuer_url,
            client_id,
            client_secret,
            scopes,
            token_auth_method,
            endpoints,
            claim_mapping,
            auto_provision,
            allow_email_linking,
            enabled,
            sort_order,
        } = request;
        let mut invalid = Vec::new();
        let display_name = required(
            display_name,
            |text| valid_display_name(text),
            "displayName",
            &mut invalid,
        );
        let protocol = required(
            protocol,
            |text| Protocol::parse(text),
            "protocol",
            &mut invalid,
        );
        let preset = required(preset, |text| Preset::parse(text), "preset", &mut invalid);
        let issuer = patch(
            issuer_url,
            |text| acceptable_issuer(&text),
            "issuerUrl",
            &mut invalid,
        );
        let client_id = required(
            client_id,
            |text| valid_client_id(text),
            "clientId",
            &mut invalid,
        );
        let client_secret = patch(client_secret, valid_secret, "clientSecret", &mut invalid);
        let scopes = match scopes {
            None => None,
            Some(None) => {
                invalid.push("scopes");
                None
            }
            Some(Some(items)) => valid_scopes(items).or_else(|| {
                invalid.push("scopes");
                None
            }),
        };
        let token_auth_method = required(
            token_auth_method,
            |text| TokenAuthMethod::parse(text),
            "tokenAuthMethod",
            &mut invalid,
        );
        let endpoints = match endpoints {
            None => EndpointsInput::default(),
            Some(value) => value.as_ref().and_then(parse_endpoints).unwrap_or_else(|| {
                invalid.push("endpoints");
                EndpointsInput::default()
            }),
        };
        let claims = match claim_mapping {
            None => ClaimsInput::default(),
            Some(value) => value.as_ref().and_then(parse_claims).unwrap_or_else(|| {
                invalid.push("claimMapping");
                ClaimsInput::default()
            }),
        };
        let auto_provision = flag(auto_provision, "autoProvision", &mut invalid);
        let allow_email_linking = flag(allow_email_linking, "allowEmailLinking", &mut invalid);
        let enabled = flag(enabled, "enabled", &mut invalid);
        let sort_order = required(
            sort_order,
            |value| valid_sort_order(*value),
            "sortOrder",
            &mut invalid,
        );
        if !invalid.is_empty() {
            return Err(invalid);
        }
        let input = Self {
            display_name,
            protocol,
            preset,
            issuer,
            client_id,
            client_secret,
            scopes,
            token_auth_method,
            endpoints,
            claims,
            auto_provision,
            allow_email_linking,
            enabled,
            sort_order,
        };
        if input.is_empty() {
            return Err(vec!["body"]);
        }
        Ok(input)
    }

    pub const fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.protocol.is_none()
            && self.preset.is_none()
            && self.issuer.is_absent()
            && self.client_id.is_none()
            && self.client_secret.is_absent()
            && self.scopes.is_none()
            && self.token_auth_method.is_none()
            && self.endpoints.is_empty()
            && self.claims.is_empty()
            && self.auto_provision.is_none()
            && self.allow_email_linking.is_none()
            && self.enabled.is_none()
            && self.sort_order.is_none()
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

fn optional<S, T>(
    value: Option<S>,
    parse: impl FnOnce(S) -> Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<Option<T>> {
    match value {
        None => Some(None),
        Some(value) => match parse(value) {
            Some(parsed) => Some(Some(parsed)),
            None => {
                invalid.push(field);
                None
            }
        },
    }
}

fn required<S, T>(
    value: Option<Option<S>>,
    parse: impl FnOnce(&S) -> Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<T> {
    match value {
        None => None,
        Some(None) => {
            invalid.push(field);
            None
        }
        Some(Some(value)) => parse(&value).or_else(|| {
            invalid.push(field);
            None
        }),
    }
}

fn flag(
    value: Option<Option<bool>>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Option<bool> {
    required(value, |flag| Some(*flag), field, invalid)
}

fn patch<T>(
    value: Option<Option<String>>,
    parse: impl FnOnce(String) -> Option<T>,
    field: &'static str,
    invalid: &mut Vec<&'static str>,
) -> Patch<T> {
    match value {
        None => Patch::Absent,
        Some(None) => Patch::Clear,
        Some(Some(text)) => parse(text).map_or_else(
            || {
                invalid.push(field);
                Patch::Absent
            },
            Patch::Set,
        ),
    }
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn valid_display_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let length = trimmed.chars().count();
    ((1..=DISPLAY_NAME_MAX_CHARS).contains(&length) && !has_control(trimmed))
        .then(|| trimmed.to_owned())
}

fn valid_client_id(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let length = trimmed.chars().count();
    ((1..=CLIENT_ID_MAX_CHARS).contains(&length) && !has_control(trimmed))
        .then(|| trimmed.to_owned())
}

fn valid_secret(text: String) -> Option<Secret<String>> {
    (!text.is_empty() && text.len() <= CLIENT_SECRET_MAX_BYTES && !text.contains('\0'))
        .then(|| Secret::new(text))
}

fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}

fn valid_scopes(items: Vec<String>) -> Option<Vec<String>> {
    if items.len() > MAX_SCOPES || !items.iter().all(|scope| valid_scope(scope)) {
        return None;
    }
    let mut unique: Vec<String> = Vec::with_capacity(items.len());
    for scope in items {
        if !unique.contains(&scope) {
            unique.push(scope);
        }
    }
    (unique.join(" ").len() <= SCOPES_MAX_CHARS).then_some(unique)
}

fn valid_claim_name(text: &str) -> Option<String> {
    ((1..=CLAIM_MAX_CHARS).contains(&text.len())
        && text.bytes().all(|byte| byte.is_ascii_graphic()))
    .then(|| text.to_owned())
}

fn valid_sort_order(value: i64) -> Option<i64> {
    (0..=MAX_SORT_ORDER).contains(&value).then_some(value)
}

fn object_members<'a>(value: &'a Value, allowed: &[&str]) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    object
        .keys()
        .all(|key| allowed.contains(&key.as_str()))
        .then_some(object)
}

fn parse_endpoints(value: &Value) -> Option<EndpointsInput> {
    let object = object_members(value, &ENDPOINT_MEMBERS)?;
    let member = |name: &str| -> Option<Patch<String>> {
        match object.get(name) {
            None => Some(Patch::Absent),
            Some(Value::Null) => Some(Patch::Clear),
            Some(Value::String(text)) => acceptable_url(text).map(|_| Patch::Set(text.clone())),
            Some(_) => None,
        }
    };
    Some(EndpointsInput {
        authorization: member("authorization")?,
        token: member("token")?,
        userinfo: member("userinfo")?,
        jwks: member("jwks")?,
    })
}

fn parse_claims(value: &Value) -> Option<ClaimsInput> {
    let object = object_members(value, &CLAIM_MEMBERS)?;
    let member = |name: &str| -> Option<Option<String>> {
        match object.get(name) {
            None => Some(None),
            Some(Value::String(text)) => valid_claim_name(text).map(Some),
            Some(_) => None,
        }
    };
    Some(ClaimsInput {
        subject: member("subject")?,
        email: member("email")?,
        email_verified: member("emailVerified")?,
        username: member("username")?,
        name: member("name")?,
        picture: member("picture")?,
    })
}

pub fn parse_order(
    request: OrderRequest,
) -> Result<Vec<super::model::ProviderId>, Vec<&'static str>> {
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::with_capacity(request.order.len());
    for text in &request.order {
        let id = text
            .parse::<super::model::ProviderId>()
            .map_err(|_| vec!["order"])?;
        if !seen.insert(id) {
            return Err(vec!["order"]);
        }
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::infra::http::json::parse;

    fn create(body: Value) -> Result<CreateInput, Vec<&'static str>> {
        match parse::<CreateProviderRequest>(body) {
            Ok(request) => CreateInput::parse(request),
            Err(error) => Err(vec![if error.code()
                == crate::domain::error_code::ErrorCode::ValidationError
            {
                "shape"
            } else {
                "json"
            }]),
        }
    }

    fn minimal() -> Value {
        json!({
            "slug": "authentik",
            "displayName": "Company SSO",
            "protocol": "oidc",
            "issuerUrl": "https://sso.example.com/application/o/palmr/",
            "clientId": "palmr",
            "clientSecret": "s3cret",
        })
    }

    fn update(body: Value) -> Result<UpdateInput, Vec<&'static str>> {
        match parse::<UpdateProviderRequest>(body) {
            Ok(request) => UpdateInput::parse(request),
            Err(_) => Err(vec!["shape"]),
        }
    }

    #[test]
    fn unit_provider_create_accepts_the_documented_body() {
        let input = create(minimal()).unwrap();
        assert_eq!(input.slug, "authentik");
        assert_eq!(input.protocol, Protocol::Oidc);
        assert_eq!(input.preset, None);
        assert!(input.client_secret.is_some());
        assert_eq!(input.auto_provision, None);
        assert_eq!(input.allow_email_linking, None);
        assert!(input.endpoints.is_empty() && input.claims.is_empty());
    }

    #[test]
    fn unit_provider_create_reports_each_invalid_field() {
        let cases = [
            (json!({"slug": "A"}), "slug"),
            (json!({"slug": "x"}), "slug"),
            (json!({"displayName": ""}), "displayName"),
            (json!({"displayName": "x".repeat(65)}), "displayName"),
            (json!({"protocol": "saml"}), "protocol"),
            (json!({"preset": "okta"}), "preset"),
            (json!({"issuerUrl": "http://sso.example.com/"}), "issuerUrl"),
            (json!({"clientId": ""}), "clientId"),
            (json!({"clientSecret": ""}), "clientSecret"),
            (json!({"scopes": ["a b"]}), "scopes"),
            (json!({"scopes": [""]}), "scopes"),
            (
                json!({"tokenAuthMethod": "private_key_jwt"}),
                "tokenAuthMethod",
            ),
            (
                json!({"endpoints": {"token": "http://sso.example.com/t"}}),
                "endpoints",
            ),
            (
                json!({"endpoints": {"other": "https://sso.example.com/t"}}),
                "endpoints",
            ),
            (json!({"claimMapping": {"role": "role"}}), "claimMapping"),
            (
                json!({"claimMapping": {"email": "has space"}}),
                "claimMapping",
            ),
            (json!({"sortOrder": -1}), "sortOrder"),
        ];
        for (patch, field) in cases {
            let mut body = minimal();
            body.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert_eq!(create(body.clone()).unwrap_err(), [field], "{body}");
        }
    }

    #[test]
    fn unit_provider_create_cannot_express_roles_domains_redirects_or_internal_ids() {
        for member in [
            "autoProvisionRole",
            "defaultRole",
            "role",
            "adminEmailDomains",
            "redirectUri",
            "id",
            "clientSecretCiphertext",
            "linkedUserCount",
            "validatedAt",
        ] {
            let mut body = minimal();
            body[member] = json!("x");
            assert_eq!(create(body).unwrap_err(), ["shape"], "{member}");
        }
    }

    #[test]
    fn unit_provider_update_distinguishes_absent_clear_and_set() {
        let input = update(json!({"clientSecret": null, "issuerUrl": null, "endpoints": {"userinfo": null, "jwks": "https://sso.example.com/jwks"}})).unwrap();
        assert_eq!(input.client_secret, Patch::Clear);
        assert_eq!(input.issuer, Patch::Clear);
        assert_eq!(input.endpoints.userinfo, Patch::Clear);
        assert_eq!(
            input.endpoints.jwks,
            Patch::Set("https://sso.example.com/jwks".to_owned())
        );
        assert_eq!(input.endpoints.token, Patch::Absent);

        let input = update(json!({"clientSecret": "next", "displayName": " Renamed "})).unwrap();
        assert!(matches!(input.client_secret, Patch::Set(_)));
        assert_eq!(input.display_name.as_deref(), Some("Renamed"));

        let input = update(json!({"enabled": true})).unwrap();
        assert_eq!(input.client_secret, Patch::Absent);
        assert_eq!(input.enabled, Some(true));
    }

    #[test]
    fn unit_provider_update_rejects_null_for_non_nullable_members_and_empty_bodies() {
        for member in [
            "displayName",
            "protocol",
            "preset",
            "clientId",
            "scopes",
            "tokenAuthMethod",
            "endpoints",
            "claimMapping",
            "autoProvision",
            "allowEmailLinking",
            "enabled",
            "sortOrder",
        ] {
            let invalid = update(json!({ member: null })).unwrap_err();
            assert!(
                invalid.contains(&member) || invalid == ["shape"],
                "{member}: {invalid:?}"
            );
        }
        assert_eq!(update(json!({})).unwrap_err(), ["body"]);
        assert_eq!(update(json!({"slug": "renamed"})).unwrap_err(), ["shape"]);
        assert_eq!(
            update(json!({"redirectUri": "https://x.test/"})).unwrap_err(),
            ["shape"]
        );
        assert_eq!(update(json!({"endpoints": {}})).unwrap_err(), ["body"]);
    }

    #[test]
    fn unit_provider_debug_never_renders_the_client_secret() {
        let body = minimal();
        let request: CreateProviderRequest = serde_json::from_value(body.clone()).unwrap();
        assert!(!format!("{request:?}").contains("s3cret"));
        let input = CreateInput::parse(request).unwrap();
        assert!(!format!("{input:?}").contains("s3cret"));
        let patch: UpdateProviderRequest =
            serde_json::from_value(json!({"clientSecret": "s3cret"})).unwrap();
        assert!(!format!("{patch:?}").contains("s3cret"));
        let input = UpdateInput::parse(patch).unwrap();
        assert!(!format!("{input:?}").contains("s3cret"));
    }

    #[test]
    fn unit_provider_scopes_are_scope_tokens_and_deduplicated() {
        assert_eq!(
            valid_scopes(vec!["openid".into(), "email".into(), "openid".into()]),
            Some(vec!["openid".to_owned(), "email".to_owned()])
        );
        assert_eq!(
            valid_scopes(vec!["read:user".into(), "user:email".into()]).map(|s| s.len()),
            Some(2)
        );
        assert_eq!(valid_scopes(vec!["a\"b".into()]), None);
        assert_eq!(valid_scopes(vec!["a\\b".into()]), None);
        assert_eq!(
            valid_scopes((0..51).map(|n| format!("s{n}")).collect()),
            None
        );
        assert_eq!(valid_scopes(vec!["x".repeat(513)]), None);
    }

    #[test]
    fn unit_provider_order_body_rejects_malformed_and_duplicate_ids() {
        let id = "0192f3a1-0000-7000-8000-000000000001";
        let other = "0192f3a1-0000-7000-8000-000000000002";
        let parse_ids = |ids: &[&str]| {
            parse_order(OrderRequest {
                order: ids.iter().map(|id| (*id).to_owned()).collect(),
            })
        };
        assert_eq!(parse_ids(&[id, other]).unwrap().len(), 2);
        assert!(parse_ids(&[]).unwrap().is_empty());
        assert_eq!(parse_ids(&[id, id]).unwrap_err(), ["order"]);
        assert_eq!(parse_ids(&[id, "nope"]).unwrap_err(), ["order"]);
        assert_eq!(
            parse_ids(&["0192F3A1-0000-7000-8000-000000000001"]).unwrap_err(),
            ["order"]
        );
    }
}
