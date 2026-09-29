use http::HeaderMap;

use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::repo as audit_repo;
use crate::features::auth::error::LoginError;
use crate::features::auth::service::AuthService;
use crate::features::auth::sessions::{AuthenticatedPrincipal, SessionClient};
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::token::Token;
use crate::infra::db::WriteTx;
use crate::infra::http::cookies::{self, CookieError, DEVICE_COOKIE};
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    CursorKey, PageRequest, QueryParams, SortAllowlist, SortDirection, SortField, SortKeyKind,
    SortValue, TotalCount,
};

use super::error::TrustedDeviceError;
use super::model::{
    device_label, IssuedDevice, NewTrustedDevice, PreparedDevice, TrustedDeviceId,
    TrustedDeviceItem, TrustedDeviceList, TrustedDevicePolicy,
};
use super::repo::{self, LAST_SEEN_COLUMN};

static DEVICE_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "lastSeenAt",
    LAST_SEEN_COLUMN,
    SortKeyKind::Text,
)];
static DEVICE_SORT: SortAllowlist = SortAllowlist::new(&DEVICE_SORT_FIELDS, 0, SortDirection::Desc);

pub fn presented_device(headers: &HeaderMap) -> Option<TokenDigest> {
    let raw = cookies::read(headers, DEVICE_COOKIE).ok()??;
    Token::decode(&raw).ok().map(|token| token.digest())
}

pub struct Revocation {
    pub clears_cookie: bool,
}

impl AuthService {
    pub fn trusted_device_policy(&self) -> TrustedDevicePolicy {
        TrustedDevicePolicy::from_settings(&self.settings.load())
    }

    pub(in crate::features::auth) async fn usable_device_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        presented: Option<&TokenDigest>,
        user_id: UserId,
        now: Timestamp,
    ) -> Result<Option<TrustedDeviceId>, LoginError> {
        let Some(presented) = presented else {
            return Ok(None);
        };
        if !self.trusted_device_policy().enabled {
            return Ok(None);
        }
        let Some(id) = repo::find_usable_in_tx(tx, presented, user_id, now).await? else {
            return Ok(None);
        };
        repo::mark_used_in_tx(tx, id, now).await?;
        Ok(Some(id))
    }

    pub(in crate::features::auth) async fn remember_device_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        prepared: &PreparedDevice,
        user_id: UserId,
        client: &SessionClient,
    ) -> Result<IssuedDevice, LoginError> {
        let lifetime = self
            .trusted_device_policy()
            .lifetime()
            .ok_or(LoginError::TrustedDeviceDisabled)?;
        let now = Timestamp::try_from(self.clock.now())?;
        let expires_at = Timestamp::try_from(now.get() + lifetime)?;
        let ip_address = bounded(client.ip_address.as_deref(), 45);
        let user_agent = bounded(client.user_agent.as_deref(), 512);
        repo::insert_in_tx(
            tx,
            &NewTrustedDevice {
                id: TrustedDeviceId::generate(self.clock.as_ref()),
                user_id,
                token_hash: &prepared.token_hash,
                label: device_label(client.user_agent.as_deref()),
                created_at: now,
                expires_at,
                ip_address: ip_address.as_deref(),
                user_agent: user_agent.as_deref(),
            },
        )
        .await?;
        Ok(IssuedDevice {
            token: prepared.token.clone(),
            expires_at,
        })
    }

    pub fn emit_device_cookie(
        &self,
        headers: &mut HeaderMap,
        device: &IssuedDevice,
    ) -> Result<(), CookieError> {
        let seconds = (device.expires_at.get() - self.clock.now()).whole_seconds();
        self.sessions().cookie_policy().append_device(
            headers,
            &device.token,
            u64::try_from(seconds.max(0)).unwrap_or(0),
        )
    }

    pub fn trusted_device_page(&self, raw_query: Option<&str>) -> Result<PageRequest, ApiError> {
        let params = QueryParams::parse(raw_query);
        PageRequest::from_query(&params, &DEVICE_SORT, self.keys.as_ref())
    }

    pub async fn list_trusted_devices(
        &self,
        principal: &AuthenticatedPrincipal,
        presented: Option<&TokenDigest>,
        page: PageRequest,
    ) -> Result<TrustedDeviceList, TrustedDeviceError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let (rows, count) = repo::list(self.pools.reader(), principal.user_id, now, &page).await?;
        let page = page.into_page(
            rows,
            self.keys.as_ref(),
            |row, _| CursorKey::new(SortValue::Text(row.last_seen_at.to_string()), row.id),
            TotalCount::Exact(count),
        );
        Ok(TrustedDeviceList {
            items: page
                .items
                .into_iter()
                .map(|row| TrustedDeviceItem::from_record(row, presented))
                .collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
            policy: self.trusted_device_policy(),
        })
    }

    pub async fn revoke_trusted_device(
        &self,
        principal: &AuthenticatedPrincipal,
        id: TrustedDeviceId,
        presented: Option<&TokenDigest>,
        client: &ClientMetadata,
    ) -> Result<Revocation, TrustedDeviceError> {
        self.pools
            .write_tx(
                self.clock.as_ref(),
                "auth.trusted_devices.revoke_one",
                async |tx| {
                    let Some(owned) = repo::find_owned_in_tx(tx, id, principal.user_id).await?
                    else {
                        return Err(TrustedDeviceError::NotFound);
                    };
                    let current = presented.is_some_and(|digest| digest.verify(&owned.token_hash));
                    if owned.revoked {
                        return Ok(Revocation {
                            clears_cookie: current,
                        });
                    }
                    let now = Timestamp::try_from(self.clock.now())?;
                    if repo::revoke_owned_in_tx(tx, id, principal.user_id, now).await? {
                        let event = revocation_event(
                            principal,
                            actions::trusted_device_revoked(current),
                            now,
                            client,
                        )
                        .with_target(Target::new(TargetType::TrustedDevice).id(&id.to_string()));
                        audit_repo::insert(tx, self.clock.as_ref(), &event).await?;
                    }
                    Ok(Revocation {
                        clears_cookie: current,
                    })
                },
            )
            .await
    }

    pub async fn revoke_all_trusted_devices(
        &self,
        principal: &AuthenticatedPrincipal,
        client: &ClientMetadata,
    ) -> Result<u64, TrustedDeviceError> {
        self.pools
            .write_tx(
                self.clock.as_ref(),
                "auth.trusted_devices.revoke_all",
                async |tx| {
                    let now = Timestamp::try_from(self.clock.now())?;
                    let revoked = repo::revoke_all_in_tx(tx, principal.user_id, now).await?;
                    if revoked > 0 {
                        let event = revocation_event(
                            principal,
                            actions::all_trusted_devices_revoked(revoked),
                            now,
                            client,
                        )
                        .with_target(
                            Target::new(TargetType::User)
                                .id(&principal.user_id.to_string())
                                .label(&principal.username),
                        );
                        audit_repo::insert(tx, self.clock.as_ref(), &event).await?;
                    }
                    Ok(revoked)
                },
            )
            .await
    }
}

fn revocation_event(
    principal: &AuthenticatedPrincipal,
    spec: actions::ActionSpec,
    now: Timestamp,
    client: &ClientMetadata,
) -> AuditEvent {
    AuditEvent::new(
        spec,
        Actor::user(&principal.user_id.to_string(), &principal.username),
        Outcome::Success,
        now,
    )
    .with_client(client.clone())
}

fn bounded(value: Option<&str>, max_chars: usize) -> Option<String> {
    value.map(|value| value.chars().take(max_chars).collect())
}
