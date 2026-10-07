use serde_json::json;
use url::Url;

use super::authorize::authorization_request_aad;
use super::claims;
use super::error::ExternalLoginError;
use super::exchange::{exchange_code, ExchangeRequest, TokenResponse};
use super::link::{LinkCompletion, LINK_RETURN_TO};
use super::model::{
    self, client_secret_aad, AuthorizePurpose, IdentityProvider, Preset, ProviderVariant,
    TokenAuthMethod,
};
use super::oauth2::{fetch_json, fetch_userinfo, profile_from_claims};
use super::oidc::{
    IdTokenRequest, IdTokenValidator, ValidatedIdToken, ValidationPurpose,
    DEFAULT_ALLOWED_ALGORITHMS,
};
use super::reauth::{self, ReauthCompletion};
use super::repo;
use super::resolve::{self, refused, ExternalIdentity, LinkState, ResolveInput};
use super::service::IdentityProviderService;
use crate::domain::error_code::ErrorCode;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::audit::actions;
use crate::features::audit::model::{
    Actor, AuditCode, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::auth::service::{IssueSession, Refusal, SessionIssue};
use crate::features::auth::sessions::{
    AuthMethod, AuthenticatedPrincipal, MintedSession, SessionClient, SessionError,
};
use crate::features::auth::AuthService;
use crate::features::users::model::UserId;
use crate::features::users::repo as users;
use crate::infra::crypto::hash::TokenDigest;
use crate::infra::crypto::hkdf::SealPurpose;
use crate::infra::crypto::token::Token;
use crate::infra::http::cookies::CookiePolicy;
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{DedupKey, JobKind, JobPayload, NewJob};

pub const CONSUME_TRANSACTION: &str = "identity_providers.callback_consume";
pub const LOGIN_TRANSACTION: &str = "identity_providers.external_login";
pub const AVATAR_TRANSACTION: &str = "identity_providers.avatar_enqueue";
pub const MAX_STATE_CHARS: usize = 128;
pub const MAX_CODE_CHARS: usize = 4096;
pub const FAILURE_PATH: &str = "/login";
pub const FAILURE_PARAMETER: &str = "error";
pub const REQUEST_ID_PARAMETER: &str = "requestId";
pub const STATUS_PARAMETER: &str = "status";
pub const REAUTH_COMPLETE_PATH: &str = "/auth/reauth-complete";

#[derive(Default)]
pub struct CallbackParams {
    pub code: Option<Secret<String>>,
    pub state: Option<Secret<String>>,
    pub denied: bool,
    pub malformed: bool,
}

impl CallbackParams {
    pub fn parse(raw_query: Option<&str>) -> Self {
        let mut params = Self::default();
        let mut seen_code = false;
        let mut seen_state = false;
        for (name, value) in url::form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
            match name.as_ref() {
                "error" => params.denied = true,
                "code" => {
                    params.malformed |= std::mem::replace(&mut seen_code, true);
                    params.code = Some(Secret::new(value.into_owned()));
                }
                "state" => {
                    params.malformed |= std::mem::replace(&mut seen_state, true);
                    params.state = Some(Secret::new(value.into_owned()));
                }
                _ => {}
            }
        }
        params
    }
}

pub struct CallbackRequest {
    pub binding: Option<Secret<String>>,
    pub session: SessionClient,
    pub audit: ClientMetadata,
    pub presented_session: Option<TokenDigest>,
    pub session_cookie: Option<Secret<String>>,
}

pub struct CallbackCompletion {
    pub session: Option<MintedSession>,
    pub location: String,
    pub purpose: AuthorizePurpose,
    pub reauth_channel: Option<String>,
}

pub struct CallbackFailure {
    pub error: ExternalLoginError,
    pub purpose: Option<AuthorizePurpose>,
    pub reauth_channel: Option<String>,
}

#[derive(Default)]
struct Known {
    purpose: Option<AuthorizePurpose>,
    reauth_channel: Option<String>,
}

impl Known {
    fn of(purpose: AuthorizePurpose, post_auth_path: Option<&str>) -> Self {
        let reauth_channel = (purpose == AuthorizePurpose::Reauth)
            .then(|| post_auth_path.and_then(reauth::channel_of_target))
            .flatten()
            .map(str::to_owned);
        Self {
            purpose: Some(purpose),
            reauth_channel,
        }
    }
}

enum Flow {
    Login,
    Link(AuthenticatedPrincipal),
    Reauth(AuthenticatedPrincipal),
}

impl Flow {
    const fn validation_purpose(&self) -> ValidationPurpose {
        match self {
            Self::Login => ValidationPurpose::Login,
            Self::Link(_) => ValidationPurpose::Link,
            Self::Reauth(_) => ValidationPurpose::Reauth,
        }
    }
}

struct SignedIn {
    user_id: UserId,
    username: String,
    session: MintedSession,
    link: resolve::IdentityLink,
}

struct RefusedActor {
    user_id: UserId,
    username: String,
    reason: &'static str,
    code: AuditCode,
}

#[derive(Clone)]
pub struct ExternalLoginService {
    providers: IdentityProviderService,
    auth: AuthService,
    validator: IdTokenValidator,
}

impl ExternalLoginService {
    pub fn new(providers: IdentityProviderService, auth: AuthService) -> Self {
        let validator = IdTokenValidator::new(providers.clock_handle(), providers.http().clone());
        Self {
            providers,
            auth,
            validator,
        }
    }

    pub fn cookie_policy(&self) -> CookiePolicy {
        self.providers.cookie_policy()
    }

    pub const fn auth(&self) -> &AuthService {
        &self.auth
    }

    pub(super) const fn providers(&self) -> &IdentityProviderService {
        &self.providers
    }

    pub fn success_location(&self, path: &str) -> String {
        success_location(self.providers.base_url().url(), path)
    }

    pub fn failure_location(
        &self,
        purpose: Option<AuthorizePurpose>,
        reauth_channel: Option<&str>,
        code: ErrorCode,
        request_id: Option<&str>,
    ) -> String {
        failure_location(
            self.providers.base_url().url(),
            purpose,
            reauth_channel,
            code,
            request_id,
        )
    }

    pub async fn complete(
        &self,
        slug: &str,
        params: CallbackParams,
        request: CallbackRequest,
    ) -> Result<CallbackCompletion, CallbackFailure> {
        let mut known = Known::default();
        self.run(slug, params, request, &mut known)
            .await
            .map_err(|error| CallbackFailure {
                error,
                purpose: known.purpose,
                reauth_channel: known.reauth_channel,
            })
    }

    async fn recover_known(
        &self,
        slug: &str,
        params: &CallbackParams,
        request: &CallbackRequest,
    ) -> Option<Known> {
        let (Some(binding), Some(state)) = (&request.binding, &params.state) else {
            return None;
        };
        if params.malformed || state.expose_secret().len() > MAX_STATE_CHARS {
            return None;
        }
        let state_digest = Token::decode(state.expose_secret()).ok()?.digest();
        let presented = Token::decode(binding.expose_secret()).ok()?.digest();
        let now = Timestamp::try_from(self.providers.clock().now()).ok()?;
        let reader = self.providers.pools().reader();
        let pending = repo::find_pending_auth_request(reader, &state_digest, now)
            .await
            .ok()??;
        if !presented.verify(&pending.binding_cookie_hash) {
            return None;
        }
        repo::find_by_slug(reader, slug)
            .await
            .ok()??
            .provider
            .id
            .eq(&pending.provider_id)
            .then(|| Known::of(pending.purpose, pending.post_auth_path.as_deref()))
    }

    async fn run(
        &self,
        slug: &str,
        params: CallbackParams,
        request: CallbackRequest,
        known: &mut Known,
    ) -> Result<CallbackCompletion, ExternalLoginError> {
        if params.denied {
            *known = self
                .recover_known(slug, &params, &request)
                .await
                .unwrap_or_default();
            return Err(refused(ErrorCode::ProviderAuthDenied));
        }
        let invalid_state = || refused(ErrorCode::ProviderStateInvalid);
        let (Some(binding), Some(state), Some(code)) =
            (&request.binding, &params.state, &params.code)
        else {
            return Err(invalid_state());
        };
        if params.malformed
            || state.expose_secret().len() > MAX_STATE_CHARS
            || code.expose_secret().len() > MAX_CODE_CHARS
            || code.expose_secret().is_empty()
        {
            return Err(invalid_state());
        }
        let state_digest = Token::decode(state.expose_secret())
            .map_err(|_| invalid_state())?
            .digest();

        let clock = self.providers.clock();
        let now = Timestamp::try_from(clock.now())?;
        let row = self
            .providers
            .pools()
            .write_tx(clock, CONSUME_TRANSACTION, async |tx| {
                repo::consume_auth_request(tx, &state_digest, now).await
            })
            .await?
            .ok_or_else(invalid_state)?;

        let presented = Token::decode(binding.expose_secret())
            .ok()
            .map(|token| token.digest());
        if !presented.is_some_and(|digest| digest.verify(&row.binding_cookie_hash)) {
            return Err(invalid_state());
        }

        let record = repo::find_by_slug(self.providers.pools().reader(), slug)
            .await?
            .filter(|record| record.provider.id == row.provider_id)
            .ok_or_else(invalid_state)?;
        let provider = &record.provider;
        *known = Known::of(row.purpose, row.post_auth_path.as_deref());
        if !self
            .providers
            .settings()
            .load()
            .security
            .auth_providers_enabled
            || !provider.enabled
        {
            return Err(refused(ErrorCode::ProviderDisabled));
        }
        let expected_redirect =
            model::redirect_uri(self.providers.base_url().url(), &provider.slug);
        if row.redirect_uri != expected_redirect {
            return Err(invalid_state());
        }

        let flow = self.bind_flow(&row, &request).await?;
        if matches!(flow, Flow::Reauth(_)) && known.reauth_channel.is_none() {
            return Err(invalid_state());
        }
        if matches!(flow, Flow::Reauth(_)) && provider.protocol() == model::Protocol::OAuth2 {
            let now = Timestamp::try_from(clock.now())?;
            if !self.recent_auth_open(row.created_at, now)? {
                return Err(refused(ErrorCode::AuthRecentAuthRequired));
            }
        }

        let verifier = self
            .providers
            .keys()
            .open(
                SealPurpose::Oidc,
                &authorization_request_aad(row.id),
                &row.verifier,
            )
            .ok()
            .and_then(|opened| String::from_utf8(opened.expose_secret().clone()).ok())
            .map(Secret::new)
            .ok_or_else(invalid_state)?;
        let client_secret = self.client_secret(provider)?;

        let tokens = self
            .exchange(
                provider,
                code,
                &expected_redirect,
                &verifier,
                client_secret.as_ref(),
            )
            .await?;
        let identity = match &provider.kind {
            ProviderVariant::Oidc(_) => {
                self.oidc_identity(provider, &row.nonce, flow.validation_purpose(), tokens)
                    .await?
            }
            ProviderVariant::OAuth2(_) => self.oauth2_identity(provider, tokens).await?,
        };
        if !identity.subject_usable() {
            return Err(refused(ErrorCode::ProviderSubjectMissing));
        }

        let path = match (&flow, known.reauth_channel.as_deref()) {
            (Flow::Login, _) => row
                .post_auth_path
                .as_deref()
                .unwrap_or(super::authorize::DEFAULT_RETURN_TO)
                .to_owned(),
            (Flow::Link(_), _) => LINK_RETURN_TO.to_owned(),
            (Flow::Reauth(_), channel) => reauth_success_path(channel.unwrap_or_default()),
        };
        let session = match flow {
            Flow::Login => {
                let signed_in = self.sign_in(provider, &identity, &request).await?;
                self.after_login(provider, &identity, &signed_in, &request)
                    .await;
                Some(signed_in.session)
            }
            Flow::Link(principal) => {
                self.complete_link(LinkCompletion {
                    provider,
                    identity: &identity,
                    principal: &principal,
                    client: &request.audit,
                })
                .await?;
                None
            }
            Flow::Reauth(principal) => {
                self.complete_reauth(ReauthCompletion {
                    provider,
                    identity: &identity,
                    principal: &principal,
                })
                .await?;
                None
            }
        };
        Ok(CallbackCompletion {
            location: self.success_location(&path),
            session,
            purpose: row.purpose,
            reauth_channel: known.reauth_channel.clone(),
        })
    }

    async fn bind_flow(
        &self,
        row: &repo::ConsumedAuthRequest,
        request: &CallbackRequest,
    ) -> Result<Flow, ExternalLoginError> {
        if row.purpose == AuthorizePurpose::Login {
            return Ok(Flow::Login);
        }
        let invalid = || refused(ErrorCode::ProviderStateInvalid);
        let (Some(user_id), Some(cookie)) = (row.user_id, request.session_cookie.as_ref()) else {
            return Err(invalid());
        };
        let principal =
            self.auth
                .sessions()
                .authenticate(cookie)
                .await
                .map_err(|error| match error {
                    SessionError::Db(error) => error.into(),
                    _ => invalid(),
                })?;
        if principal.user_id != user_id {
            return Err(invalid());
        }
        Ok(match row.purpose {
            AuthorizePurpose::Reauth => Flow::Reauth(principal),
            AuthorizePurpose::Login | AuthorizePurpose::Link => Flow::Link(principal),
        })
    }

    fn client_secret(
        &self,
        provider: &IdentityProvider,
    ) -> Result<Option<Secret<String>>, ExternalLoginError> {
        if provider.token_auth_method == TokenAuthMethod::None {
            return Ok(None);
        }
        let Some(sealed) = provider.client_secret.as_ref() else {
            return Ok(None);
        };
        let opened = self.providers.keys().open(
            SealPurpose::Idp,
            &client_secret_aad(provider.id),
            sealed,
        )?;
        let text = String::from_utf8(opened.expose_secret().clone())
            .map_err(|_| ExternalLoginError::internal("external_login_client_secret"))?;
        Ok(Some(Secret::new(text)))
    }

    async fn exchange(
        &self,
        provider: &IdentityProvider,
        code: &Secret<String>,
        redirect_uri: &str,
        verifier: &Secret<String>,
        client_secret: Option<&Secret<String>>,
    ) -> Result<TokenResponse, ExternalLoginError> {
        let endpoint = provider
            .kind
            .endpoints()
            .token
            .ok_or(ExternalLoginError::internal(
                "external_login_token_endpoint",
            ))?;
        exchange_code(
            self.providers.http(),
            &ExchangeRequest {
                endpoint: &endpoint,
                client_id: &provider.client_id,
                client_secret,
                method: provider.token_auth_method,
                code,
                redirect_uri,
                verifier,
            },
        )
        .await
        .map_err(|failure| {
            tracing::warn!(
                reason = failure.code(),
                "authorization code exchange failed"
            );
            refused(ErrorCode::ProviderCodeExchangeFailed)
        })
    }

    async fn oidc_identity(
        &self,
        provider: &IdentityProvider,
        nonce: &str,
        purpose: ValidationPurpose,
        tokens: TokenResponse,
    ) -> Result<ExternalIdentity, ExternalLoginError> {
        let ProviderVariant::Oidc(oidc) = &provider.kind else {
            return Err(ExternalLoginError::internal("external_login_protocol"));
        };
        let id_token = tokens
            .id_token
            .ok_or_else(|| refused(ErrorCode::ProviderIdTokenInvalid))?;
        let access_token = tokens.access_token;
        let validated = self
            .validator
            .validate(&IdTokenRequest {
                provider_id: provider.id,
                issuer: &oidc.issuer,
                client_id: &provider.client_id,
                jwks_uri: &oidc.jwks_uri,
                allowed_algorithms: &DEFAULT_ALLOWED_ALGORITHMS,
                expected_nonce: nonce,
                purpose,
                access_token: access_token
                    .as_ref()
                    .map(|token| token.expose_secret().as_str()),
                claims: &provider.claims,
                token: id_token.expose_secret(),
            })
            .await
            .map_err(|failure| {
                tracing::warn!(reason = failure.code(), "ID token rejected");
                refused(failure.api_code())
            })?;
        let validated = self
            .supplement_from_userinfo(
                provider,
                oidc.userinfo_endpoint.as_deref(),
                access_token.as_ref(),
                validated,
            )
            .await;
        Ok(ExternalIdentity {
            subject: validated.subject,
            email: validated.email,
            email_verified: validated.email_verified,
            username: validated.username,
            name: validated.name,
            picture: validated.picture,
        })
    }

    async fn supplement_from_userinfo(
        &self,
        provider: &IdentityProvider,
        endpoint: Option<&str>,
        access_token: Option<&Secret<String>>,
        validated: ValidatedIdToken,
    ) -> ValidatedIdToken {
        let complete =
            validated.username.is_some() && validated.name.is_some() && validated.picture.is_some();
        let (Some(endpoint), Some(access_token), false) = (endpoint, access_token, complete) else {
            return validated;
        };
        match fetch_userinfo(
            self.providers.http(),
            endpoint,
            access_token.expose_secret(),
        )
        .await
        {
            Ok(document) => {
                let profile = profile_from_claims(&document, &provider.claims);
                if profile.subject.as_deref() == Some(validated.subject.as_str()) {
                    validated.supplemented_with(&profile)
                } else {
                    validated
                }
            }
            Err(failure) => {
                tracing::debug!(
                    reason = failure.code(),
                    "optional userinfo supplement skipped"
                );
                validated
            }
        }
    }

    async fn oauth2_identity(
        &self,
        provider: &IdentityProvider,
        tokens: TokenResponse,
    ) -> Result<ExternalIdentity, ExternalLoginError> {
        let ProviderVariant::OAuth2(oauth2) = &provider.kind else {
            return Err(ExternalLoginError::internal("external_login_protocol"));
        };
        let access_token = tokens
            .access_token
            .ok_or_else(|| refused(ErrorCode::ProviderCodeExchangeFailed))?;
        let document = fetch_userinfo(
            self.providers.http(),
            &oauth2.userinfo_endpoint,
            access_token.expose_secret(),
        )
        .await
        .map_err(|failure| {
            tracing::warn!(reason = failure.code(), "userinfo request failed");
            refused(failure.api_code())
        })?;
        let profile = profile_from_claims(&document, &provider.claims);
        let mut identity = ExternalIdentity {
            subject: profile.subject.unwrap_or_default(),
            email: profile.email,
            email_verified: profile.email_verified,
            username: profile.username,
            name: profile.name,
            picture: profile.picture,
        };
        if provider.preset == Preset::Github {
            let endpoint = claims::github_emails_endpoint(&oauth2.userinfo_endpoint)
                .ok_or_else(|| refused(ErrorCode::ProviderUserinfoFailed))?;
            let emails = fetch_json(
                self.providers.http(),
                &endpoint,
                access_token.expose_secret(),
            )
            .await
            .map_err(|failure| {
                tracing::warn!(reason = failure.code(), "secondary e-mail request failed");
                refused(failure.api_code())
            })?;
            identity.email = claims::github_verified_primary_email(&emails);
            identity.email_verified = identity.email.is_some();
        }
        Ok(identity)
    }

    async fn sign_in(
        &self,
        provider: &IdentityProvider,
        identity: &ExternalIdentity,
        request: &CallbackRequest,
    ) -> Result<SignedIn, ExternalLoginError> {
        let clock = self.providers.clock();
        let prepared = self.auth.sessions().prepare_credentials()?;
        let locale = self.providers.settings().load().default_locale();
        let mut refusal: Option<RefusedActor> = None;

        let outcome = self
            .providers
            .pools()
            .write_tx(clock, LOGIN_TRANSACTION, async |tx| {
                let resolution = resolve::resolve(
                    tx,
                    &ResolveInput {
                        provider,
                        identity,
                        clock,
                        audit: self.providers.audit(),
                        client: &request.audit,
                        locale,
                    },
                )
                .await?;
                let link = resolution.link().clone();

                if link.state == LinkState::Suspended {
                    refusal = self
                        .refused_actor(
                            tx,
                            link.user_id,
                            "link_suspended",
                            AuditCode::AuthAccountInactive,
                        )
                        .await;
                    return Err(refused(ErrorCode::AuthAccountInactive));
                }

                let issue = self
                    .auth
                    .issue_session_in_tx(
                        tx,
                        IssueSession {
                            user_id: link.user_id,
                            method: AuthMethod::External,
                            client: request.session.clone(),
                            credentials: &prepared,
                            verified_password_hash: None,
                            replaces: request.presented_session.as_ref(),
                            promotes: None,
                            trusted_device: None,
                        },
                    )
                    .await?;
                match issue {
                    SessionIssue::Issued { user, session, .. } => {
                        let at = Timestamp::try_from(clock.now())?;
                        resolve::record_login(tx, link.id, &session.id.to_string(), at).await?;
                        Ok(SignedIn {
                            user_id: user.id,
                            username: user.username,
                            session,
                            link,
                        })
                    }
                    SessionIssue::Refused(Refusal::Inactive) => {
                        refusal = self
                            .refused_actor(
                                tx,
                                link.user_id,
                                "inactive",
                                AuditCode::AuthAccountInactive,
                            )
                            .await;
                        Err(refused(ErrorCode::AuthAccountInactive))
                    }
                    SessionIssue::Refused(Refusal::Locked { .. }) => {
                        refusal = self
                            .refused_actor(tx, link.user_id, "locked", AuditCode::AuthLocked)
                            .await;
                        Err(refused(ErrorCode::AuthLocked))
                    }
                    SessionIssue::Refused(Refusal::CredentialChanged)
                    | SessionIssue::SecondFactorRequired(_) => {
                        Err(ExternalLoginError::internal("external_login_session_gate"))
                    }
                }
            })
            .await;

        if let (Err(_), Some(actor)) = (&outcome, refusal) {
            self.audit_refusal(provider, &actor, request);
        }
        outcome
    }

    async fn refused_actor(
        &self,
        tx: &mut crate::infra::db::WriteTx<'_>,
        user_id: UserId,
        reason: &'static str,
        code: AuditCode,
    ) -> Option<RefusedActor> {
        let user = users::find_by_id_in_tx(tx, user_id).await.ok().flatten()?;
        Some(RefusedActor {
            user_id: user.id,
            username: user.username,
            reason,
            code,
        })
    }

    fn audit_refusal(
        &self,
        provider: &IdentityProvider,
        actor: &RefusedActor,
        request: &CallbackRequest,
    ) {
        let Ok(now) = Timestamp::try_from(self.providers.clock().now()) else {
            return;
        };
        let event = AuditEvent::new(
            actions::login_failed_external(&provider.id.to_string(), actor.reason),
            Actor::user(&actor.user_id.to_string(), &actor.username),
            Outcome::Denied(actor.code),
            now,
        )
        .with_target(
            Target::new(TargetType::User)
                .id(&actor.user_id.to_string())
                .label(&actor.username),
        )
        .with_client(request.audit.clone());
        self.providers.audit().record_async(event);
    }

    async fn after_login(
        &self,
        provider: &IdentityProvider,
        identity: &ExternalIdentity,
        signed_in: &SignedIn,
        request: &CallbackRequest,
    ) {
        if let Ok(now) = Timestamp::try_from(self.providers.clock().now()) {
            let event = AuditEvent::new(
                actions::login_succeeded_external(&provider.id.to_string()),
                Actor::user(&signed_in.user_id.to_string(), &signed_in.username),
                Outcome::Success,
                now,
            )
            .with_target(
                Target::new(TargetType::User)
                    .id(&signed_in.user_id.to_string())
                    .label(&signed_in.username),
            )
            .with_client(request.audit.clone());
            self.providers.audit().record_async(event);
        }

        if signed_in.link.avatar_fetched {
            return;
        }
        let Some(url) = claims::avatar_url(
            provider.preset,
            &identity.subject,
            identity.picture.as_deref(),
        ) else {
            return;
        };
        if let Err(error) = enqueue_avatar(
            &self.providers,
            &signed_in.user_id.to_string(),
            &signed_in.link.id.to_string(),
            &provider.id.to_string(),
            &url,
        )
        .await
        {
            tracing::warn!(
                kind = error.kind(),
                "external avatar job could not be queued"
            );
        }
    }
}

async fn enqueue_avatar(
    providers: &IdentityProviderService,
    user_id: &str,
    link_id: &str,
    provider_id: &str,
    url: &str,
) -> Result<(), ExternalLoginError> {
    let payload = JobPayload::new(&json!({
        "userId": user_id,
        "identityLinkId": link_id,
        "providerId": provider_id,
        "url": url,
    }))
    .map_err(|_| ExternalLoginError::internal("external_avatar_payload"))?;
    let key = DedupKey::new(format!(
        "{}:{link_id}",
        JobKind::AvatarFetchExternal.as_str()
    ))
    .map_err(|_| ExternalLoginError::internal("external_avatar_dedup"))?;
    let job = NewJob::new(JobKind::AvatarFetchExternal, payload).dedup_key(key);
    let clock = providers.clock();
    providers
        .pools()
        .write_tx(clock, AVATAR_TRANSACTION, async |tx| {
            enqueue(tx, clock, &job).await.map(|_| ())
        })
        .await?;
    Ok(())
}

pub fn success_location(base: &Url, path: &str) -> String {
    format!("{}{path}", base.as_str().trim_end_matches('/'))
}

pub fn reauth_success_path(channel: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair(STATUS_PARAMETER, "success");
    query.append_pair(reauth::REAUTH_CHANNEL_PARAMETER, channel);
    format!("{REAUTH_COMPLETE_PATH}?{}", query.finish())
}

pub fn failure_location(
    base: &Url,
    purpose: Option<AuthorizePurpose>,
    reauth_channel: Option<&str>,
    code: ErrorCode,
    request_id: Option<&str>,
) -> String {
    let path = match purpose {
        Some(AuthorizePurpose::Link) => LINK_RETURN_TO,
        Some(AuthorizePurpose::Reauth) => REAUTH_COMPLETE_PATH,
        Some(AuthorizePurpose::Login) | None => FAILURE_PATH,
    };
    let root = format!("{}{path}", base.as_str().trim_end_matches('/'));
    match Url::parse(&root) {
        Ok(mut url) => {
            {
                let mut query = url.query_pairs_mut();
                if purpose == Some(AuthorizePurpose::Reauth) {
                    query.append_pair(STATUS_PARAMETER, "error");
                }
                query.append_pair(FAILURE_PARAMETER, code.as_str());
                if let Some(request_id) = request_id {
                    query.append_pair(REQUEST_ID_PARAMETER, request_id);
                }
                if let (Some(AuthorizePurpose::Reauth), Some(channel)) = (purpose, reauth_channel) {
                    query.append_pair(reauth::REAUTH_CHANNEL_PARAMETER, channel);
                }
            }
            url.into()
        }
        Err(_) => root,
    }
}
