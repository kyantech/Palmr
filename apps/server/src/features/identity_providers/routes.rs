use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::path::Parameter;
use utoipa_axum::routes;

use super::authorize::{
    AuthorizeContext, AuthorizeRequest, AuthorizeResponse, AUTH_REQUEST_TTL_SECONDS,
};
use super::callback::{
    CallbackCompletion, CallbackFailure, CallbackParams, CallbackRequest, ExternalLoginService,
};
use super::discovery::Discovered;
use super::error::ProviderError;
use super::input::{
    parse_order, CreateInput, CreateProviderRequest, DiscoverRequest, OrderRequest, UpdateInput,
    UpdateProviderRequest,
};
use super::model::{ProviderId, ProviderItem, PublicProviderList};
use super::presets::PresetCatalogue;
use super::provider_test::ProviderTestResult;
use super::service::{IdentityProviderService, PROVIDER_SORT};
use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::error_code::ErrorCode;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::auth::sessions::SessionService;
use crate::infra::http::cookies::{self, OAUTH_COOKIE, SESSION_COOKIE};
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Admin;
use crate::infra::http::json;
use crate::infra::http::pagination::{cursor_parameter, limit_parameter, Page};
use crate::infra::http::request_id::{tag_error, RequestId};

pub const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

pub const SENSITIVE_WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::AdminRecentAuth,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const ORDER_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::AdminWrite,
    Transport::ControlPlane,
);

pub const PROBE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Admin,
    RateLimitClass::ProviderTest,
    Transport::ControlPlane,
);

pub const PUBLIC_LIST_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::PublicRead,
    Transport::ControlPlane,
);

pub const AUTHORIZE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthLogin,
    Transport::ControlPlane,
);

pub const CALLBACK_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthLogin,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(PUBLIC_LIST_ROUTE, routes!(list_public_providers))
        .route(AUTHORIZE_ROUTE, routes!(authorize_provider))
        .route(CALLBACK_ROUTE, routes!(provider_callback))
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_providers), &list_parameters()),
        )
        .route(SENSITIVE_WRITE_ROUTE, routes!(create_provider))
        .route(SENSITIVE_WRITE_ROUTE, routes!(update_provider))
        .route(SENSITIVE_WRITE_ROUTE, routes!(delete_provider))
        .route(ORDER_ROUTE, routes!(reorder_providers))
        .route(PROBE_ROUTE, routes!(discover_provider))
        .route(PROBE_ROUTE, routes!(test_provider))
        .route(READ_ROUTE, routes!(list_presets))
}

fn list_parameters() -> Vec<Parameter> {
    vec![
        PROVIDER_SORT.parameter(),
        cursor_parameter(),
        limit_parameter(),
    ]
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/providers",
    tag = "auth-providers",
    responses(
        (status = 200, description = "Enabled providers only, ordered by `sortOrder` with a stable tie-break. Each item carries only `slug`, `displayName` and `iconKey`; no issuer, client id, endpoint, scope, claim mapping, secret state or validation state. When the global provider toggle is off this is an empty list, not an error.", body = PublicProviderList),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_public_providers(
    Extension(service): Extension<IdentityProviderService>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    match service.public_providers().await {
        Ok(list) => json_response(StatusCode::OK, &list, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/providers/{slug}/authorize",
    tag = "auth-providers",
    params(("slug" = String, Path, description = "Provider slug")),
    request_body(
        content = AuthorizeRequest,
        content_type = "application/json",
        description = "Starts an **anonymous login** only. `purpose` must be `login`; `link` and `reauth` enter through dedicated authenticated flows and are rejected here as `VALIDATION_ERROR`. `returnTo` is an optional validated relative SPA path (default `/overview`). There is no `redirectUri`/`redirect`/`next`/`callbackUrl` field: the redirect URI is always derived from `PALMR_BASE_URL`."
    ),
    responses(
        (status = 200, description = "An authorization URL plus the `palmr_oauth` browser-binding cookie. The exact callback URI was derived from `PALMR_BASE_URL`, not from the request.", body = AuthorizeResponse),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 403, description = "`PROVIDER_DISABLED`: the provider is disabled or the global provider toggle is off. Nothing is persisted and no cookie is set.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an undeclared/redirect field, a `purpose` other than `login`, or an unusable provider configuration.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn authorize_provider(
    Extension(service): Extension<IdentityProviderService>,
    Path(slug): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let body = match json::read::<AuthorizeRequest>(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    if body.purpose != "login" {
        return tag_error(ApiError::validation(["purpose"]), request_id.as_ref()).into_response();
    }
    match service
        .authorize(&slug, AuthorizeContext::login(body.return_to))
        .await
    {
        Ok(authorized) => {
            let mut response = json_response(
                StatusCode::OK,
                &AuthorizeResponse {
                    authorization_url: authorized.authorization_url,
                },
                request_id.as_ref(),
            );
            if service
                .cookie_policy()
                .append_oauth_binding(
                    response.headers_mut(),
                    &authorized.binding,
                    AUTH_REQUEST_TTL_SECONDS,
                )
                .is_err()
            {
                return tag_error(ApiError::internal(), request_id.as_ref()).into_response();
            }
            response
        }
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/providers/{slug}/callback",
    tag = "auth-providers",
    params(
        ("slug" = String, Path, description = "Provider slug"),
        ("code" = Option<String>, Query, description = "The authorization code issued by the identity provider."),
        ("state" = Option<String>, Query, description = "The single-use authorization state created by `POST /api/v1/auth/providers/{slug}/authorize`."),
        ("error" = Option<String>, Query, description = "Present when the identity provider denied the request. Its value is never trusted, rendered or echoed."),
        ("error_description" = Option<String>, Query, description = "Provider supplied text. It is ignored and never echoed."),
    ),
    responses(
        (
            status = 303,
            description = "Always a redirect, for all three purposes; the landing is chosen by the purpose stored in the server-side authorization request, never by a client or provider parameter. `login`: on success `Location` is `PALMR_BASE_URL` plus the validated post-authentication path (default `/overview`) and `palmr_session` and `palmr_csrf` are set. `link` (started by `POST /api/v1/auth/providers/{slug}/link`): the callback additionally requires the same live Palmr session that started it, whose recent-authentication window must still be open, and on success `Location` is `PALMR_BASE_URL/settings/security`; no session cookie is set or rotated. `reauth` (started by the SSO branch of `POST /api/v1/auth/reauthenticate`, normally in a popup): the callback requires the same session and proves the same provider and subject that established it; on success `last_auth_at` of that session only is stamped, the session token is not rotated and `Location` is `PALMR_BASE_URL/auth/reauth-complete?status=success`, a SPA completion route, not an API route. On every success `palmr_oauth` is cleared. On failure `Location` is `PALMR_BASE_URL/login?error=<CODE>&requestId=<REQUEST_ID>` for `login`, `PALMR_BASE_URL/settings/security?error=<CODE>&requestId=<REQUEST_ID>` for `link` and `PALMR_BASE_URL/auth/reauth-complete?status=error&error=<CODE>&requestId=<REQUEST_ID>` for `reauth`. The purpose is recovered only from an unexpired authorization request whose provider matches the callback and whose browser-binding cookie matches; when it cannot be established (missing, malformed, unknown, expired, already consumed or foreign state, a binding mismatch or a provider mismatch) the landing is the `login` failure form. `<REQUEST_ID>` is the `X-Request-Id` of the callback request itself and is diagnostic only; `error_description`, provider text, codes, state, tokens, subjects and e-mail addresses are never forwarded. `<CODE>` is one of `PROVIDER_STATE_INVALID`, `PROVIDER_AUTH_DENIED`, `PROVIDER_DISABLED`, `PROVIDER_CODE_EXCHANGE_FAILED`, `PROVIDER_ID_TOKEN_INVALID`, `PROVIDER_USERINFO_FAILED`, `PROVIDER_SUBJECT_MISSING`, `PROVIDER_EMAIL_UNVERIFIED`, `PROVIDER_AUTO_PROVISION_DISABLED`, `PROVIDER_IDENTITY_ALREADY_LINKED`, `PROVIDER_LINK_NOT_FOUND`, `AUTH_RECENT_AUTH_REQUIRED`, `AUTH_EXTERNAL_AMBIGUOUS_IDENTITY`, `AUTH_EXTERNAL_USERNAME_UNAVAILABLE`, `AUTH_ACCOUNT_INACTIVE`, `AUTH_LOCKED` or `INTERNAL_ERROR`; `palmr_oauth` is cleared. No JSON body is returned."
        ),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn provider_callback(
    Extension(service): Extension<ExternalLoginService>,
    Path(slug): Path<String>,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let request_id = request_id.as_ref().map(RequestId::as_str);
    let binding = cookies::read(request.headers(), OAUTH_COOKIE)
        .ok()
        .flatten()
        .map(crate::domain::secret::Secret::new);
    let context = CallbackRequest {
        binding,
        session: SessionService::client(&request),
        audit: client_metadata(&request),
        presented_session: SessionService::presented_token(request.headers()),
        session_cookie: cookies::read(request.headers(), SESSION_COOKIE)
            .ok()
            .flatten()
            .map(crate::domain::secret::Secret::new),
    };
    let params = CallbackParams::parse(raw_query.as_deref());

    let (location, session, purpose) = match service.complete(&slug, params, context).await {
        Ok(CallbackCompletion {
            location,
            session,
            purpose,
        }) => (location, session, Some(purpose)),
        Err(CallbackFailure { error, purpose }) => {
            if error.is_server_fault() {
                tracing::error!(kind = error.kind(), "external login callback failed");
            }
            (
                service.failure_location(purpose, error.code(), request_id),
                None,
                purpose,
            )
        }
    };

    let mut response = redirect_response(&location);
    let mut failed = cookies_failed(&service, response.headers_mut(), session.as_ref());
    if failed {
        response = redirect_response(&service.failure_location(
            purpose,
            ErrorCode::InternalError,
            request_id,
        ));
        failed = service
            .cookie_policy()
            .expire_oauth_binding(response.headers_mut())
            .is_err();
    }
    if failed {
        tracing::error!("external login callback cookies could not be emitted");
    }
    response
}

fn cookies_failed(
    service: &ExternalLoginService,
    headers: &mut http::HeaderMap,
    session: Option<&crate::features::auth::sessions::MintedSession>,
) -> bool {
    if let Some(session) = session {
        if service
            .auth()
            .sessions()
            .emit_cookies(headers, session)
            .is_err()
        {
            return true;
        }
    }
    service
        .cookie_policy()
        .expire_oauth_binding(headers)
        .is_err()
}

fn redirect_response(location: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::SEE_OTHER;
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/providers",
    tag = "admin-providers",
    responses(
        (status = 200, description = "External identity providers in `sortOrder` order with a stable tie-breaker. `totalCount` is exact. No response carries a client secret: `clientSecretConfigured` reports only whether one is stored.", body = Page<ProviderItem>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 422, description = "Invalid query.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_providers(
    Extension(service): Extension<IdentityProviderService>,
    Admin(_admin): Admin,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let page = match service.query(raw_query.as_deref()) {
        Ok(page) => page,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.list(page).await {
        Ok(page) => json_response(StatusCode::OK, &page, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/providers",
    tag = "admin-providers",
    request_body(
        content = CreateProviderRequest,
        content_type = "application/json",
        description = "Creates a provider; nothing is created by choosing a preset. An `oidc` provider runs discovery from `<issuerUrl>/.well-known/openid-configuration` for every endpoint the request does not supply, and the document's `issuer` must equal `issuerUrl` exactly. An `oauth2` provider requires `endpoints.authorization`, `.token` and `.userinfo`. `autoProvision` defaults to `false`; `allowEmailLinking` defaults to `true` for `oidc` and `false` for `oauth2`. The callback URI is derived from `PALMR_BASE_URL` and returned as `redirectUri`; it cannot be supplied."
    ),
    responses(
        (status = 201, description = "The created provider. `clientSecretConfigured` reports only whether a secret is stored.", body = ProviderItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 409, description = "`PROVIDER_SLUG_TAKEN`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid field, a preset that does not match the protocol, a missing client secret (unless `tokenAuthMethod` is `none`) or a client secret on a public client.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`PROVIDER_DISCOVERY_FAILED`: discovery was unreachable, timed out, was too large, malformed or its issuer differed from `issuerUrl`.", body = ApiErrorBody),
    )
)]
async fn create_provider(
    Extension(service): Extension<IdentityProviderService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let parsed = match json::read::<CreateProviderRequest>(request.into_body()).await {
        Ok(body) => match CreateInput::parse(body) {
            Ok(input) => input,
            Err(fields) => return invalid(fields, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.create(&admin, parsed, &client).await {
        Ok(item) => json_response(StatusCode::CREATED, &item, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/providers/{id}",
    tag = "admin-providers",
    params(("id" = String, Path, description = "Provider UUIDv7")),
    request_body(
        content = UpdateProviderRequest,
        content_type = "application/json",
        description = "Any non-empty subset of the mutable members; `slug` and `redirectUri` are immutable and rejected. An absent member is unchanged, and no member except `issuerUrl`, `clientSecret` and the members of `endpoints` is nullable. A change to the issuer, client id, client secret, endpoints, protocol or token authentication method clears `validatedAt`. A change of issuer, or an incomplete `oidc` endpoint set, runs discovery again."
    ),
    responses(
        (status = 200, description = "The updated provider.", body = ProviderItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`PASSWORD_LOGIN_DISABLE_UNSAFE` (`details.blockers[]`) while password login is disabled and the change (`enabled: false`, or an edit that clears `validatedAt`) would remove the last usable Administrator external login path. Nothing changes.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid or immutable member, an empty body or an inconsistent resulting configuration.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`PROVIDER_DISCOVERY_FAILED`.", body = ApiErrorBody),
        (status = 503, description = "`DATABASE_BUSY`, including a provider that changed while discovery was running; retry.", body = ApiErrorBody),
    )
)]
async fn update_provider(
    Extension(service): Extension<IdentityProviderService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<ProviderId>() else {
        return provider_error(&ProviderError::NotFound, request_id.as_ref());
    };
    let input = match json::read::<UpdateProviderRequest>(request.into_body()).await {
        Ok(body) => match UpdateInput::parse(body) {
            Ok(input) => input,
            Err(fields) => return invalid(fields, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update(&admin, id, input, &client).await {
        Ok(item) => json_response(StatusCode::OK, &item, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/providers/{id}",
    tag = "admin-providers",
    params(("id" = String, Path, description = "Provider UUIDv7")),
    responses(
        (status = 204, description = "The provider was deleted and `IDENTITY_PROVIDER_DELETED` was audited in the same transaction."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role or recent authentication required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 409, description = "`PASSWORD_LOGIN_DISABLE_UNSAFE` (`details.blockers[]`) while password login is disabled and the provider carries the last usable Administrator external login path, or `PROVIDER_HAS_LINKS`: identity links still reference the provider. It is never deleted and its links are never removed by this call; unlink those identities or remove the affected accounts first.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn delete_provider(
    Extension(service): Extension<IdentityProviderService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<ProviderId>() else {
        return provider_error(&ProviderError::NotFound, request_id.as_ref());
    };
    match service.delete(&admin, id, &client).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/providers/order",
    tag = "admin-providers",
    request_body(
        content = OrderRequest,
        content_type = "application/json",
        description = "Every provider id exactly once, in the new order. Unknown, duplicate, malformed and missing ids are rejected and nothing is changed; the order is applied in one transaction, which also writes `IDENTITY_PROVIDER_UPDATED` for each provider whose position changed."
    ),
    responses(
        (status = 204, description = "The order was applied."),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` (`order`).", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn reorder_providers(
    Extension(service): Extension<IdentityProviderService>,
    Admin(admin): Admin,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let order = match json::read::<OrderRequest>(request.into_body()).await {
        Ok(body) => match parse_order(body) {
            Ok(order) => order,
            Err(fields) => return invalid(fields, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.reorder(&admin, order, &client).await {
        Ok(()) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/providers/discover",
    tag = "admin-providers",
    request_body(
        content = DiscoverRequest,
        content_type = "application/json",
        description = "Fetches only `<issuerUrl>/.well-known/openid-configuration` (10 s total, 256 KiB, verified TLS, no cookies or credentials, redirects never followed into non-public addresses) and previews it. Nothing is stored and no provider is changed."
    ),
    responses(
        (status = 200, description = "The relevant fields of the discovery document; its `issuer` equals `issuerUrl` exactly.", body = Discovered),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an issuer that is not an https URL (plain http only for loopback) without credentials, query or fragment.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 502, description = "`PROVIDER_DISCOVERY_FAILED` with `details.reason`.", body = ApiErrorBody),
    )
)]
async fn discover_provider(
    Extension(service): Extension<IdentityProviderService>,
    Admin(_admin): Admin,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let issuer = match json::read::<DiscoverRequest>(request.into_body()).await {
        Ok(body) => match body.parse() {
            Ok(issuer) => issuer,
            Err(fields) => return invalid(fields, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.discover(&issuer).await {
        Ok(discovered) => json_response(StatusCode::OK, &discovered, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/providers/{id}/test",
    tag = "admin-providers",
    params(("id" = String, Path, description = "Provider UUIDv7")),
    responses(
        (status = 200, description = "Every check passed and `validatedAt` was stamped; a change of the persisted validation state is audited as `IDENTITY_PROVIDER_UPDATED` in the same transaction. The stored client secret and any token are never sent or returned.", body = ProviderTestResult),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required, or the CSRF proof or origin is missing or not allowed.", body = ApiErrorBody),
        (status = 404, description = "`PROVIDER_NOT_FOUND`.", body = ApiErrorBody),
        (status = 422, description = "`PROVIDER_VALIDATION_FAILED` with the failing `details.checks[]`; the provider is left without a current `validatedAt`. This holds in every instance mode: a test is diagnostic and is never refused by the SSO-only standing invariant.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
        (status = 503, description = "`DATABASE_BUSY`, including a provider that changed while its test was running; retry.", body = ApiErrorBody),
    )
)]
async fn test_provider(
    Extension(service): Extension<IdentityProviderService>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let Ok(id) = id.parse::<ProviderId>() else {
        return provider_error(&ProviderError::NotFound, request_id.as_ref());
    };
    match service.test(&admin, id, &client).await {
        Ok(result) => json_response(StatusCode::OK, &result, request_id.as_ref()),
        Err(error) => provider_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/providers/presets",
    tag = "admin-providers",
    responses(
        (status = 200, description = "The bundled preset catalogue: Google, GitHub, Discord, Pocket ID, Authentik, Zitadel, Auth0, Kinde, Frontegg, Custom OIDC and Custom OAuth2. Static data; no network call is made and no provider is created.", body = PresetCatalogue),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "Administrator role required.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_presets(Admin(_admin): Admin, request: Request) -> Response {
    json_response(
        StatusCode::OK,
        &PresetCatalogue::bundled(),
        RequestId::of(&request).as_ref(),
    )
}

fn invalid(fields: Vec<&'static str>, request_id: Option<&RequestId>) -> Response {
    provider_error(&ProviderError::Invalid { fields }, request_id)
}

fn provider_error(error: &ProviderError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() && !matches!(error, ProviderError::DiscoveryFailed(_)) {
        tracing::error!(kind = error.kind(), "identity provider request failed");
    }
    tag_error(api_error, request_id).into_response()
}

fn json_response<T: serde::Serialize>(
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
