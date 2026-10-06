use std::collections::HashSet;
use std::sync::Arc;

use super::discovery::{self, Discovered};
use super::draft::{Changes, Draft, SecretChange};
use super::error::ProviderError;
use super::http_client::ProviderHttpClient;
use super::input::{CreateInput, UpdateInput};
use super::model::{
    client_secret_aad, IdentityProvider, ProviderId, ProviderItem, ProviderRecord, ProviderVariant,
};
use super::password_login::{assert_safe_sso_after_change, Projection};
use super::provider_test::{self, ProviderTestResult};
use super::repo::{self, ProviderWrite};
use crate::config::PublicBaseUrl;
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{
    self, IdentityProviderCreatedFacts, IdentityProviderUpdatedFacts, Presence,
};
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::AuthenticatedPrincipal;
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    CursorKey, Page, PageRequest, QueryParams, SortAllowlist, SortDirection, SortField,
    SortKeyKind, SortValue, TotalCount,
};

pub const CREATE_TRANSACTION: &str = "identity_providers.create";
pub const UPDATE_TRANSACTION: &str = "identity_providers.update";
pub const DELETE_TRANSACTION: &str = "identity_providers.delete";
pub const ORDER_TRANSACTION: &str = "identity_providers.order";
pub const VALIDATION_TRANSACTION: &str = "identity_providers.validation";

pub(super) static PROVIDER_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "sortOrder",
    "p.sort_order",
    SortKeyKind::Integer,
)];
pub static PROVIDER_SORT: SortAllowlist =
    SortAllowlist::new(&PROVIDER_SORT_FIELDS, 0, SortDirection::Asc).with_id_column("p.id");

#[derive(Clone)]
pub struct IdentityProviderService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
    audit: AuditService,
    http: ProviderHttpClient,
    base_url: PublicBaseUrl,
    settings: SettingsHandle,
}

struct Mutation<'a> {
    admin: &'a AuthenticatedPrincipal,
    id: ProviderId,
    slug: &'a str,
    at: Timestamp,
    client: &'a ClientMetadata,
}

struct Prepared {
    kind: ProviderVariant,
    secret: Option<SealedSecret>,
}

impl IdentityProviderService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        keys: Arc<KeyRing>,
        audit: AuditService,
        http: ProviderHttpClient,
        base_url: PublicBaseUrl,
        settings: SettingsHandle,
    ) -> Self {
        Self {
            pools,
            clock,
            keys,
            audit,
            http,
            base_url,
            settings,
        }
    }

    pub(crate) fn audit(&self) -> &AuditService {
        &self.audit
    }

    pub(crate) fn http(&self) -> &ProviderHttpClient {
        &self.http
    }

    pub(crate) fn clock_handle(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    pub(crate) fn settings(&self) -> &SettingsHandle {
        &self.settings
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub(crate) fn keys(&self) -> &KeyRing {
        self.keys.as_ref()
    }

    pub(crate) fn pools(&self) -> &DbPools {
        &self.pools
    }

    pub(crate) fn base_url(&self) -> &PublicBaseUrl {
        &self.base_url
    }

    pub fn query(&self, raw_query: Option<&str>) -> Result<PageRequest, ApiError> {
        let params = QueryParams::parse(raw_query);
        PageRequest::from_query(&params, &PROVIDER_SORT, self.keys.as_ref())
    }

    pub async fn list(&self, page: PageRequest) -> Result<Page<ProviderItem>, ProviderError> {
        let (records, total) = repo::list(self.pools.reader(), &page).await?;
        let page = page.into_page(
            records,
            self.keys.as_ref(),
            cursor_key,
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page
                .items
                .iter()
                .map(|record| self.item_of(record))
                .collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn discover(&self, issuer: &str) -> Result<Discovered, ProviderError> {
        discovery::discover(&self.http, issuer)
            .await
            .map_err(ProviderError::DiscoveryFailed)
    }

    pub async fn create(
        &self,
        admin: &AuthenticatedPrincipal,
        input: CreateInput,
        client: &ClientMetadata,
    ) -> Result<ProviderItem, ProviderError> {
        let mut draft = Draft::for_create(input).map_err(invalid)?;
        if repo::slug_taken(self.pools.reader(), &draft.slug).await? {
            return Err(ProviderError::SlugTaken);
        }
        self.complete_discovery(&mut draft).await?;
        let id = ProviderId::generate(self.clock.as_ref());
        let prepared = self.prepare(id, &draft)?;
        let secret_presence = presence(prepared.secret.is_some());
        self.pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let sort_order = match draft.sort_order {
                    Some(order) => order,
                    None => repo::next_sort_order(tx).await?,
                };
                repo::insert(
                    tx,
                    &ProviderWrite {
                        id,
                        slug: &draft.slug,
                        display_name: &draft.display_name,
                        kind: &prepared.kind,
                        preset: draft.preset,
                        scopes: &draft.scopes,
                        client_id: &draft.client_id,
                        secret: prepared.secret.as_ref(),
                        token_auth_method: draft.token_auth_method,
                        claims: &draft.claims,
                        enabled: draft.enabled,
                        auto_provision: draft.auto_provision,
                        allow_email_linking: draft.allow_email_linking,
                        sort_order,
                        validated_at: None,
                        validation_error: None,
                        at,
                        updated_by: Some(admin.user_id),
                    },
                )
                .await?;
                let spec = actions::identity_provider_created(IdentityProviderCreatedFacts {
                    protocol: draft.protocol.as_str(),
                    preset: draft.preset.as_str(),
                    enabled: draft.enabled,
                    auto_provision: draft.auto_provision,
                    allow_email_linking: draft.allow_email_linking,
                    client_secret: secret_presence,
                });
                let mutation = Mutation {
                    admin,
                    id,
                    slug: &draft.slug,
                    at,
                    client,
                };
                self.record(tx, spec, &mutation).await
            })
            .await?;
        self.item(id).await
    }

    pub async fn update(
        &self,
        admin: &AuthenticatedPrincipal,
        id: ProviderId,
        input: UpdateInput,
        client: &ClientMetadata,
    ) -> Result<ProviderItem, ProviderError> {
        let record = repo::find(self.pools.reader(), id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let existing = &record.provider;
        let mut draft = Draft::from_existing(existing);
        let changes = draft.apply(input);
        if changes.is_empty() {
            return Ok(self.item_of(&record));
        }
        self.complete_discovery(&mut draft).await?;
        let prepared = self.prepare(id, &draft)?;
        let (validated_at, validation_error) = if changes.validation_reset {
            (None, None)
        } else {
            (existing.validated_at, existing.validation_error.as_deref())
        };
        let withdraws_login_path = changes.enabled == Some(false) || changes.validation_reset;
        self.pools
            .write_tx(self.clock.as_ref(), UPDATE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                if withdraws_login_path {
                    assert_safe_sso_after_change(tx, Projection::removing_provider(id)).await?;
                }
                let stored = prepared_secret(&prepared, existing, &draft);
                let written = repo::update(
                    tx,
                    &ProviderWrite {
                        id,
                        slug: &draft.slug,
                        display_name: &draft.display_name,
                        kind: &prepared.kind,
                        preset: draft.preset,
                        scopes: &draft.scopes,
                        client_id: &draft.client_id,
                        secret: stored,
                        token_auth_method: draft.token_auth_method,
                        claims: &draft.claims,
                        enabled: draft.enabled,
                        auto_provision: draft.auto_provision,
                        allow_email_linking: draft.allow_email_linking,
                        sort_order: draft.sort_order.unwrap_or(existing.sort_order),
                        validated_at,
                        validation_error,
                        at,
                        updated_by: Some(admin.user_id),
                    },
                    existing.updated_at,
                )
                .await?;
                if !written {
                    return Err(match repo::find_in_tx(tx, id).await? {
                        Some(_) => ProviderError::Stale,
                        None => ProviderError::NotFound,
                    });
                }
                let mutation = Mutation {
                    admin,
                    id,
                    slug: &draft.slug,
                    at,
                    client,
                };
                self.record_update(tx, &changes, &mutation).await
            })
            .await?;
        self.item(id).await
    }

    pub async fn delete(
        &self,
        admin: &AuthenticatedPrincipal,
        id: ProviderId,
        client: &ClientMetadata,
    ) -> Result<(), ProviderError> {
        self.pools
            .write_tx(self.clock.as_ref(), DELETE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let record = repo::find_in_tx(tx, id)
                    .await?
                    .ok_or(ProviderError::NotFound)?;
                let provider = &record.provider;
                assert_safe_sso_after_change(tx, Projection::removing_provider(id)).await?;
                if !repo::delete(tx, id).await? {
                    return Err(ProviderError::NotFound);
                }
                let spec = actions::identity_provider_deleted(
                    provider.protocol().as_str(),
                    provider.preset.as_str(),
                );
                let mutation = Mutation {
                    admin,
                    id,
                    slug: &provider.slug,
                    at,
                    client,
                };
                self.record(tx, spec, &mutation).await
            })
            .await
    }

    pub async fn reorder(
        &self,
        admin: &AuthenticatedPrincipal,
        order: Vec<ProviderId>,
        client: &ClientMetadata,
    ) -> Result<(), ProviderError> {
        self.pools
            .write_tx(self.clock.as_ref(), ORDER_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let current = repo::ids_in_order(tx).await?;
                let requested: HashSet<String> = order.iter().map(ToString::to_string).collect();
                let coherent = requested.len() == current.len()
                    && current.iter().all(|(id, _)| requested.contains(id));
                if !coherent {
                    return Err(invalid(vec!["order"]));
                }
                for (position, id) in order.iter().enumerate() {
                    let position = i64::try_from(position).map_err(|_| {
                        ProviderError::RepositoryInvariant {
                            column: "sort_order",
                        }
                    })?;
                    if repo::set_sort_order(tx, *id, position).await? {
                        let key = id.to_string();
                        let slug = current
                            .iter()
                            .find(|(candidate, _)| *candidate == key)
                            .map_or("", |(_, slug)| slug.as_str());
                        let mutation = Mutation {
                            admin,
                            id: *id,
                            slug,
                            at,
                            client,
                        };
                        let spec = actions::identity_provider_fields_updated(&["sortOrder"]);
                        self.record(tx, spec, &mutation).await?;
                    }
                }
                Ok(())
            })
            .await
    }

    pub async fn test(
        &self,
        admin: &AuthenticatedPrincipal,
        id: ProviderId,
        client: &ClientMetadata,
    ) -> Result<ProviderTestResult, ProviderError> {
        let record = repo::find(self.pools.reader(), id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let provider = &record.provider;
        let checks = provider_test::run(&self.http, provider, self.base_url.url()).await;
        let failure = provider_test::summarize_failures(&checks);
        let at = Timestamp::try_from(self.clock.now())?;
        let validated_at = failure.is_none().then_some(at);
        let mut changed = Vec::new();
        if provider.validated_at != validated_at {
            changed.push("validatedAt");
        }
        if provider.validation_error != failure {
            changed.push("validationError");
        }
        let stamped = self
            .pools
            .write_tx(self.clock.as_ref(), VALIDATION_TRANSACTION, async |tx| {
                let written = repo::stamp_validation(
                    tx,
                    id,
                    provider.updated_at,
                    validated_at,
                    failure.as_deref(),
                )
                .await?;
                if written && !changed.is_empty() {
                    let mutation = Mutation {
                        admin,
                        id,
                        slug: &provider.slug,
                        at,
                        client,
                    };
                    let spec = actions::identity_provider_fields_updated(&changed);
                    self.record(tx, spec, &mutation).await?;
                }
                Ok::<_, ProviderError>(written)
            })
            .await?;
        if !stamped {
            return Err(ProviderError::Stale);
        }
        match validated_at {
            Some(validated_at) => Ok(ProviderTestResult {
                ok: true,
                checks,
                validated_at: validated_at.to_string(),
            }),
            None => Err(ProviderError::ValidationFailed { checks }),
        }
    }

    async fn complete_discovery(&self, draft: &mut Draft) -> Result<(), ProviderError> {
        if !draft.needs_discovery() {
            return Ok(());
        }
        if let Err(fields) = draft.validate() {
            let rejected: Vec<&'static str> = fields
                .into_iter()
                .filter(|field| *field != "endpoints")
                .collect();
            if !rejected.is_empty() {
                return Err(invalid(rejected));
            }
        }
        let issuer = draft.issuer.clone().unwrap_or_default();
        let discovered = self.discover(&issuer).await?;
        draft.fill_from(&discovered);
        Ok(())
    }

    fn prepare(&self, id: ProviderId, draft: &Draft) -> Result<Prepared, ProviderError> {
        let kind = draft.validate().map_err(invalid)?;
        let secret = match &draft.secret {
            SecretChange::Replace(secret) => Some(self.keys.seal(
                SealPurpose::Idp,
                &client_secret_aad(id),
                secret.expose_secret().as_bytes(),
            )?),
            SecretChange::Keep | SecretChange::Clear => None,
        };
        Ok(Prepared { kind, secret })
    }

    async fn item(&self, id: ProviderId) -> Result<ProviderItem, ProviderError> {
        let record = repo::find(self.pools.reader(), id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        Ok(self.item_of(&record))
    }

    fn item_of(&self, record: &ProviderRecord) -> ProviderItem {
        ProviderItem::new(record, self.base_url.url())
    }

    async fn record(
        &self,
        tx: &mut WriteTx<'_>,
        spec: actions::ActionSpec,
        mutation: &Mutation<'_>,
    ) -> Result<(), ProviderError> {
        let event = AuditEvent::new(
            spec,
            Actor::user(
                &mutation.admin.user_id.to_string(),
                &mutation.admin.username,
            ),
            Outcome::Success,
            mutation.at,
        )
        .with_target(
            Target::new(TargetType::Provider)
                .id(&mutation.id.to_string())
                .label(mutation.slug),
        )
        .with_client(mutation.client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    async fn record_update(
        &self,
        tx: &mut WriteTx<'_>,
        changes: &Changes,
        mutation: &Mutation<'_>,
    ) -> Result<(), ProviderError> {
        if changes.has_configuration_change() {
            let spec = actions::identity_provider_updated(&IdentityProviderUpdatedFacts {
                changed: changes.fields.clone(),
                client_secret: changes.client_secret,
                validation_reset: changes.validation_reset,
            });
            self.record(tx, spec, mutation).await?;
        }
        match changes.enabled {
            Some(true) => {
                self.record(tx, actions::identity_provider_enabled(), mutation)
                    .await?;
            }
            Some(false) => {
                self.record(tx, actions::identity_provider_disabled(), mutation)
                    .await?;
            }
            None => {}
        }
        Ok(())
    }
}

fn prepared_secret<'a>(
    prepared: &'a Prepared,
    existing: &'a IdentityProvider,
    draft: &Draft,
) -> Option<&'a SealedSecret> {
    match draft.secret {
        SecretChange::Replace(_) => prepared.secret.as_ref(),
        SecretChange::Keep => existing.client_secret.as_ref(),
        SecretChange::Clear => None,
    }
}

fn invalid(fields: Vec<&'static str>) -> ProviderError {
    ProviderError::Invalid { fields }
}

fn presence(present: bool) -> Presence {
    if present {
        Presence::Set
    } else {
        Presence::Unset
    }
}

fn cursor_key(record: &ProviderRecord, _field: &'static SortField) -> CursorKey {
    CursorKey::new(
        SortValue::Integer(record.provider.sort_order),
        record.provider.id,
    )
}
