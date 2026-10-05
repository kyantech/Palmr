use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::{AuthenticatedPrincipal, SessionService};
use crate::features::identity_providers::authorize::AUTH_REQUEST_TTL_SECONDS;
use crate::features::identity_providers::callback::ExternalLoginService;
use crate::features::identity_providers::link_routes::link_error;
use crate::features::identity_providers::reauth::ExternalReauthResponse;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::{Authenticated, SignOutCaller};
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};
use crate::infra::ratelimit::{MfaPendingToken, RateLimitGate};

use super::error::LoginError;
use super::lockout::AttemptClient;
use super::login::{LoginInput, LoginRequest};
use super::mfa::{LoginTotpInput, LoginTotpRequest, MfaChallengeBody, MfaContext};
use super::model::{LoginResponse, MeResponse};
use super::recent_auth::{ReauthContext, ReauthenticateRequest};
use super::service::{AuthService, LoggedIn, LoginContext, LoginResult};
use super::trusted_devices::service::presented_device;

pub const LOGIN_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthLogin,
    Transport::ControlPlane,
);

pub const LOGIN_TOTP_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthTotp,
    Transport::ControlPlane,
);

pub const LOGOUT_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
)
.with_idempotent_sign_out();

pub const REAUTHENTICATE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::AuthTotp,
    Transport::ControlPlane,
);

pub const ME_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(LOGIN_ROUTE, routes!(login))
        .route(LOGIN_TOTP_ROUTE, routes!(login_totp))
        .route(LOGOUT_ROUTE, routes!(logout))
        .route(ME_ROUTE, routes!(me))
        .route(REAUTHENTICATE_ROUTE, routes!(reauthenticate))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body(content = LoginRequest, content_type = "application/json"),
    responses(
        (
            status = 200,
            description = "Signed in; `palmr_session` and `palmr_csrf` are set and any presented session is revoked. For an account with TOTP, a presented `palmr_device` satisfies the second factor only when it is unrevoked, unexpired, bound to this account and trusted devices are enabled.",
            body = LoginResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (
            status = 401,
            description = "`AUTH_INVALID_CREDENTIALS` when the credentials did not authenticate, or `AUTH_2FA_REQUIRED` when the password was proven and the account requires a second factor: no cookie is set, and `details.mfaToken` is the single-use challenge for `POST /api/v1/auth/login/totp`.",
            body = LoginUnauthorized
        ),
        (status = 403, description = "Password login is disabled, or the origin is not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited, or the account is locked after the password was proven.", body = ApiErrorBody),
    )
)]
async fn login(Extension(service): Extension<AuthService>, request: Request) -> Response {
    let request_id = RequestId::of(&request);
    let session = SessionService::client(&request);
    let context = LoginContext {
        attempt: AttemptClient {
            ip: session.ip_address.clone(),
            user_agent: session.user_agent.clone(),
            request_id: request_id.as_ref().map(|id| id.as_str().to_owned()),
        },
        session,
        audit: client_metadata(&request),
        presented_session: SessionService::presented_token(request.headers()),
        presented_device: presented_device(request.headers()),
    };
    let input = match json::read::<LoginRequest>(request.into_body()).await {
        Ok(body) => match LoginInput::parse(body) {
            Ok(input) => input,
            Err(error) => return login_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };

    match service.login(input, context).await {
        Ok(LoginResult::SignedIn(logged_in)) => {
            signed_in(&service, &logged_in, request_id.as_ref())
        }
        Ok(LoginResult::SecondFactorRequired(challenge)) => json(
            StatusCode::UNAUTHORIZED,
            &MfaChallengeBody::new(
                &challenge,
                service.trusted_device_policy().enabled,
                request_id.as_ref().map(RequestId::as_str),
            ),
            request_id.as_ref(),
        ),
        Err(error) => login_error(&error, request_id.as_ref()),
    }
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(untagged)]
#[expect(
    dead_code,
    reason = "documents the two 401 envelopes of the password step"
)]
enum LoginUnauthorized {
    SecondFactorRequired(MfaChallengeBody),
    Error(ApiErrorBody),
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login/totp",
    tag = "auth",
    request_body(
        content = LoginTotpRequest,
        content_type = "application/json",
        description = "`mfaToken` is the challenge from `AUTH_2FA_REQUIRED`; it is the only identity this step accepts. `code` is a six-digit TOTP or a backup code in `XXXX-XXXX-XXXX-XXXX` form."
    ),
    responses(
        (
            status = 200,
            description = "Signed in; the pending challenge is promoted to a fresh session, `palmr_session` and `palmr_csrf` are set, and any presented session is revoked. With `rememberDevice: true`, `palmr_device` is also set.",
            body = LoginResponse
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "The challenge is unknown, expired, used or burnt, or the code did not verify or was already used.", body = ApiErrorBody),
        (status = 403, description = "`TRUSTED_DEVICE_DISABLED` when `rememberDevice` is true while trusted devices are disabled, or the origin is not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited, or the account is locked.", body = ApiErrorBody),
    )
)]
async fn login_totp(
    Extension(service): Extension<AuthService>,
    gate: RateLimitGate,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let session = SessionService::client(&request);
    let context = MfaContext {
        attempt: AttemptClient {
            ip: session.ip_address.clone(),
            user_agent: session.user_agent.clone(),
            request_id: request_id.as_ref().map(|id| id.as_str().to_owned()),
        },
        session,
        audit: client_metadata(&request),
        presented_session: SessionService::presented_token(request.headers()),
    };
    let body = json::read::<LoginTotpRequest>(request.into_body()).await;
    let gate = match &body {
        Ok(body) if !body.mfa_token.is_empty() => {
            gate.with_mfa_pending(MfaPendingToken::new(body.mfa_token.as_bytes()))
        }
        _ => gate,
    };
    if let Err(rejection) = gate.admit() {
        return rejection.into_response();
    }
    let input = match body.map(LoginTotpInput::parse) {
        Ok(Ok(input)) => input,
        Ok(Err(error)) => return login_error(&error, request_id.as_ref()),
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.complete_second_factor(input, context).await {
        Ok(logged_in) => signed_in(&service, &logged_in, request_id.as_ref()),
        Err(error) => login_error(&error, request_id.as_ref()),
    }
}

fn signed_in(
    service: &AuthService,
    logged_in: &LoggedIn,
    request_id: Option<&RequestId>,
) -> Response {
    let mut response = json(StatusCode::OK, &logged_in.response, request_id);
    if let Err(error) = service
        .sessions()
        .emit_cookies(response.headers_mut(), &logged_in.session)
    {
        tracing::error!(
            kind = error.kind(),
            "login session cookies could not be emitted"
        );
        return tag_error(ApiError::internal(), request_id).into_response();
    }
    if let Some(device) = &logged_in.device {
        if service
            .emit_device_cookie(response.headers_mut(), device)
            .is_err()
        {
            tracing::error!("trusted-device cookie could not be emitted");
            return tag_error(ApiError::internal(), request_id).into_response();
        }
    }
    response
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    responses(
        (status = 204, description = "The current session is revoked, or none remained; `palmr_session` and `palmr_csrf` are cleared."),
        (status = 403, description = "A presented session lacks its CSRF proof, or the origin is not allowed.", body = ApiErrorBody),
    )
)]
async fn logout(
    Extension(service): Extension<AuthService>,
    caller: SignOutCaller,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    if let SignOutCaller::Session(principal) = caller {
        if let Err(error) = service.logout(&principal, &client_metadata(&request)).await {
            return login_error(&error, request_id.as_ref());
        }
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    if service
        .sessions()
        .cookie_policy()
        .expire_session_pair(response.headers_mut())
        .is_err()
    {
        return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
    }
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The caller's current authentication state.", body = MeResponse),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
    )
)]
async fn me(
    Extension(service): Extension<AuthService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.me(&principal).await {
        Ok(me) => json(StatusCode::OK, &me, request_id.as_ref()),
        Err(error) => login_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/reauthenticate",
    tag = "auth",
    request_body(
        content = ReauthenticateRequest,
        content_type = "application/json",
        description = "`password` is required for an account with a local password; `totpCode` is required when the account has TOTP enabled. For an account with no local password the body must be empty (`{}`) and the request starts SSO re-authentication through the identity provider that established the current session; supplying `password` or `totpCode` there is a `VALIDATION_ERROR`."
    ),
    responses(
        (
            status = 202,
            description = "SSO-only account. `externalReauthUrl` is the provider authorization URL generated server-side (`prompt=login`, and `max_age=0` for OIDC) and `palmr_oauth` is set. Send the browser there; the callback stamps `last_auth_at` of this session only after proving the same provider and subject.",
            body = ExternalReauthResponse
        ),
        (
            status = 204,
            description = "The current session's recent-authentication window is open; the session and its cookies are unchanged."
        ),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "The credentials did not authenticate, or no session is present.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, the session is restricted, or `PROVIDER_DISABLED` for the SSO branch when the session's provider or the global provider toggle is off.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_LINK_NOT_FOUND` for the SSO branch when the session has no active identity link to re-authenticate through.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn reauthenticate(
    Extension(service): Extension<AuthService>,
    Extension(external): Extension<ExternalLoginService>,
    Authenticated(principal): Authenticated,
    gate: RateLimitGate,
    request: Request,
) -> Response {
    if let Err(rejection) = gate.admit() {
        return rejection.into_response();
    }
    let request_id = RequestId::of(&request);
    let client = SessionService::client(&request);
    let context = ReauthContext {
        attempt: AttemptClient {
            ip: client.ip_address,
            user_agent: client.user_agent,
            request_id: request_id.as_ref().map(|id| id.as_str().to_owned()),
        },
        audit: client_metadata(&request),
    };
    let body = match json::read::<ReauthenticateRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    if let Err(error) = service.reauthenticate(&principal, body, context).await {
        if matches!(error, LoginError::ExternalReauthRequired) {
            return external_reauthentication(&external, &principal, request_id.as_ref()).await;
        }
        return login_error(&error, request_id.as_ref());
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    response
}

async fn external_reauthentication(
    external: &ExternalLoginService,
    principal: &AuthenticatedPrincipal,
    request_id: Option<&RequestId>,
) -> Response {
    let authorized = match external.start_reauth(principal).await {
        Ok(authorized) => authorized,
        Err(error) => return link_error(&error, request_id),
    };
    let mut response = json(
        StatusCode::ACCEPTED,
        &ExternalReauthResponse {
            accepted: true,
            external_reauth_url: authorized.authorization_url,
        },
        request_id,
    );
    if external
        .cookie_policy()
        .append_oauth_binding(
            response.headers_mut(),
            &authorized.binding,
            AUTH_REQUEST_TTL_SECONDS,
        )
        .is_err()
    {
        return tag_error(ApiError::internal(), request_id).into_response();
    }
    response
}

fn login_error(error: &LoginError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "authentication request failed");
    }
    let mut response = tag_error(api_error, request_id).into_response();
    if let Some(retry_after) = error.retry_after() {
        response
            .headers_mut()
            .insert(RETRY_AFTER, retry_after.header_value());
    }
    response
}

fn json<T: serde::Serialize>(
    status: StatusCode,
    body: &T,
    request_id: Option<&RequestId>,
) -> Response {
    match serde_json::to_vec(body) {
        Ok(body) => (
            status,
            [
                (CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE)),
                (CACHE_CONTROL, NO_STORE),
            ],
            body,
        )
            .into_response(),
        Err(_) => tag_error(ApiError::internal(), request_id).into_response(),
    }
}
