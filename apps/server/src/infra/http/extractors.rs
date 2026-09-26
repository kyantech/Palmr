use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::request::Parts;
use http::Method;

use crate::app::auth_class::{AbsentSession, AuthClass, RecentAuthWaiver};
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, SessionError, SessionRestriction, SessionService,
};

use super::cookies::{self, SESSION_COOKIE};
use super::csrf::{is_state_changing, CsrfProof};
use super::error::ApiError;
use super::idempotency::IdempotencyScope;
use super::request_id::{tag_error, RequestId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuthGate {
    class: AuthClass,
    absent_session: AbsentSession,
    recent_auth_waiver: RecentAuthWaiver,
}

pub fn apply<S>(
    class: AuthClass,
    absent_session: AbsentSession,
    recent_auth_waiver: RecentAuthWaiver,
    handler: MethodRouter<S>,
) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    handler.route_layer(from_fn_with_state(
        AuthGate {
            class,
            absent_session,
            recent_auth_waiver,
        },
        enforce_auth_class,
    ))
}

async fn enforce_auth_class(
    State(gate): State<AuthGate>,
    mut request: Request,
    next: Next,
) -> Response {
    let class = gate.class;
    if matches!(
        class,
        AuthClass::Public | AuthClass::PublicGrant | AuthClass::Setup
    ) {
        return next.run(request).await;
    }

    let signed_out_satisfies = gate.absent_session == AbsentSession::AlreadySignedOut;
    if signed_out_satisfies && !cookies::presents(request.headers(), SESSION_COOKIE) {
        request.extensions_mut().insert(SignedOut);
        return next.run(request).await;
    }

    let request_id = RequestId::of(&request);
    let Some(service) = request.extensions().get::<SessionService>().cloned() else {
        return tagged(SessionError::AuthRequired, request_id.as_ref());
    };
    let proof = if is_state_changing(request.method()) {
        match request.extensions().get::<CsrfProof>() {
            Some(proof) => Some(proof.digest().clone()),
            None => return tagged(SessionError::CsrfMissing, request_id.as_ref()),
        }
    } else {
        None
    };
    let principal = match service
        .authenticate_headers(request.headers(), proof.as_ref())
        .await
    {
        Ok(principal) => principal,
        Err(SessionError::AuthRequired) if signed_out_satisfies => {
            request.extensions_mut().insert(SignedOut);
            return next.run(request).await;
        }
        Err(error) => return tagged(error, request_id.as_ref()),
    };
    let waived = gate.recent_auth_waiver.waives(principal.restriction);
    let enforced = if waived {
        AuthClass::Authenticated
    } else {
        class
    };
    if let Err(error) = service.enforce_class(&principal, enforced) {
        return tagged(error, request_id.as_ref());
    }
    if let Err(error) = enforce_restriction(
        principal.restriction,
        request.method(),
        request.uri().path(),
    ) {
        return tagged(error, request_id.as_ref());
    }
    request
        .extensions_mut()
        .insert(IdempotencyScope::user(principal.user_id));
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(AuthorizedClass(class));
    if waived {
        request.extensions_mut().insert(ForcedPasswordChange);
    }
    next.run(request).await
}

fn tagged(error: SessionError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "session authentication failed");
    }
    tag_error(api_error, request_id).into_response()
}

pub fn enforce_restriction(
    restriction: SessionRestriction,
    method: &Method,
    path: &str,
) -> Result<(), SessionError> {
    if restriction == SessionRestriction::None || restriction_allows(restriction, method, path) {
        return Ok(());
    }
    Err(match restriction {
        SessionRestriction::None => unreachable!("handled above"),
        SessionRestriction::MustChangePassword => SessionError::PasswordChangeRequired,
        SessionRestriction::MustEnrollTotp => SessionError::TotpEnrollmentRequired,
    })
}

pub fn restriction_allows(restriction: SessionRestriction, method: &Method, path: &str) -> bool {
    let shared = matches!(
        (method, path),
        (&Method::GET, "/api/v1/auth/me")
            | (&Method::POST, "/api/v1/auth/logout")
            | (&Method::GET, "/api/v1/bootstrap")
            | (&Method::GET, "/api/v1/settings/effective")
    );
    shared
        || match restriction {
            SessionRestriction::None => true,
            SessionRestriction::MustChangePassword => {
                method == Method::POST && path == "/api/v1/profile/password"
            }
            SessionRestriction::MustEnrollTotp => matches!(
                (method, path),
                (&Method::GET, "/api/v1/auth/2fa")
                    | (&Method::POST, "/api/v1/auth/2fa/enroll")
                    | (&Method::POST, "/api/v1/auth/2fa/enroll/verify")
            ),
        }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuthorizedClass(AuthClass);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignedOut;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForcedPasswordChange;

#[derive(Debug, Clone)]
pub enum SignOutCaller {
    Session(AuthenticatedPrincipal),
    SignedOut,
}

impl<S> FromRequestParts<S> for SignOutCaller
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let principal = parts.extensions.get::<AuthenticatedPrincipal>().cloned();
        let class = parts.extensions.get::<AuthorizedClass>().copied();
        match (principal, class) {
            (Some(principal), Some(_)) => Ok(Self::Session(principal)),
            _ if parts.extensions.get::<SignedOut>().is_some() => Ok(Self::SignedOut),
            _ => Err(tagged_api(SessionError::AuthRequired, parts)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Authenticated(pub AuthenticatedPrincipal);

#[derive(Debug, Clone)]
pub struct AuthenticatedRecentAuth(pub AuthenticatedPrincipal);

#[derive(Debug, Clone)]
pub struct PasswordChangeCaller(pub AuthenticatedPrincipal);

#[derive(Debug, Clone)]
pub struct Admin(pub AuthenticatedPrincipal);

#[derive(Debug, Clone)]
pub struct AdminRecentAuth(pub AuthenticatedPrincipal);

macro_rules! extractor {
    ($name:ident, $accept:expr, $fallback:expr) => {
        impl<S> FromRequestParts<S> for $name
        where
            S: Send + Sync,
        {
            type Rejection = ApiError;

            async fn from_request_parts(
                parts: &mut Parts,
                _state: &S,
            ) -> Result<Self, Self::Rejection> {
                let principal = parts
                    .extensions
                    .get::<AuthenticatedPrincipal>()
                    .cloned()
                    .ok_or_else(|| tagged_api(SessionError::AuthRequired, parts))?;
                let class = parts.extensions.get::<AuthorizedClass>().copied();
                if ($accept)(&principal, class) {
                    Ok(Self(principal))
                } else {
                    Err(tagged_api(($fallback)(&principal), parts))
                }
            }
        }
    };
}

extractor!(
    Authenticated,
    |_principal: &AuthenticatedPrincipal, class: Option<AuthorizedClass>| { class.is_some() },
    |_principal: &AuthenticatedPrincipal| SessionError::AuthRequired
);

extractor!(
    AuthenticatedRecentAuth,
    |principal: &AuthenticatedPrincipal, class: Option<AuthorizedClass>| {
        principal.recent_auth
            && matches!(
                class,
                Some(AuthorizedClass(
                    AuthClass::AuthenticatedRecentAuth | AuthClass::AdminRecentAuth
                ))
            )
    },
    |principal: &AuthenticatedPrincipal| SessionError::RecentAuthRequired {
        method: principal.auth_method.recent_auth_hint()
    }
);

impl<S> FromRequestParts<S> for PasswordChangeCaller
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let principal = parts
            .extensions
            .get::<AuthenticatedPrincipal>()
            .cloned()
            .ok_or_else(|| tagged_api(SessionError::AuthRequired, parts))?;
        let class = parts.extensions.get::<AuthorizedClass>().copied();
        let recent = principal.recent_auth
            && class == Some(AuthorizedClass(AuthClass::AuthenticatedRecentAuth));
        let forced = principal.restriction == SessionRestriction::MustChangePassword
            && parts.extensions.get::<ForcedPasswordChange>().is_some();
        if recent || forced {
            Ok(Self(principal))
        } else {
            Err(tagged_api(
                SessionError::RecentAuthRequired {
                    method: principal.auth_method.recent_auth_hint(),
                },
                parts,
            ))
        }
    }
}

extractor!(
    Admin,
    |principal: &AuthenticatedPrincipal, class: Option<AuthorizedClass>| {
        principal.role == crate::domain::role::Role::Admin
            && matches!(
                class,
                Some(AuthorizedClass(
                    AuthClass::Admin | AuthClass::AdminRecentAuth
                ))
            )
    },
    |_principal: &AuthenticatedPrincipal| SessionError::Forbidden
);

extractor!(
    AdminRecentAuth,
    |principal: &AuthenticatedPrincipal, class: Option<AuthorizedClass>| {
        principal.role == crate::domain::role::Role::Admin
            && principal.recent_auth
            && matches!(class, Some(AuthorizedClass(AuthClass::AdminRecentAuth)))
    },
    |principal: &AuthenticatedPrincipal| {
        if principal.role != crate::domain::role::Role::Admin {
            SessionError::Forbidden
        } else {
            SessionError::RecentAuthRequired {
                method: principal.auth_method.recent_auth_hint(),
            }
        }
    }
);

fn tagged_api(error: SessionError, parts: &Parts) -> ApiError {
    tag_error(error.api_error(), parts.extensions.get::<RequestId>())
}
