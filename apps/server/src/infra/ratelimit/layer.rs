use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{FromRequestParts, RawPathParams, Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::request::Parts;

use super::class::{Dimension, RateLimitClass, Stage};
use super::error::RateLimitRejection;
use super::key::{MfaPendingToken, NormalizedAccount, PublicScope, RateLimitPrincipal, Subject};
use super::limiter::{Denial, RateLimiter};
use crate::infra::http::proxy::ResolvedClient;
use crate::infra::http::request_id::RequestId;

const ALIAS_PARAM: &str = "alias";
const CAPABILITY_PARAM: &str = "token";

pub(crate) fn apply<S>(class: RateLimitClass, handler: MethodRouter<S>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    if class.counts_requests() {
        handler.route_layer(from_fn_with_state(class, enforce_rate_limit))
    } else {
        handler
    }
}

fn keys_on_public_scope(class: RateLimitClass) -> bool {
    class
        .buckets()
        .iter()
        .any(|bucket| bucket.dimension() == Dimension::IpAndPublicScope)
}

async fn public_scope(parts: &mut Parts) -> Option<PublicScope> {
    let params = RawPathParams::from_request_parts(parts, &()).await.ok()?;
    params.iter().find_map(|(name, value)| match name {
        ALIAS_PARAM => Some(PublicScope::alias(value)),
        CAPABILITY_PARAM => Some(PublicScope::capability(value)),
        _ => None,
    })
}

fn rejection(
    class: RateLimitClass,
    denial: Denial,
    request_id: Option<&RequestId>,
) -> RateLimitRejection {
    match denial {
        Denial::Throttled(throttled) => RateLimitRejection::throttled(throttled, request_id),
        Denial::MissingIdentity(dimension) => {
            tracing::error!(
                scope = class.as_str(),
                dimension = ?dimension,
                "a rate-limit bucket was evaluated without its identity"
            );
            RateLimitRejection::internal(request_id)
        }
    }
}

async fn enforce_rate_limit(
    State(class): State<RateLimitClass>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = RequestId::of(&request);
    let limiter = request.extensions().get::<Arc<RateLimiter>>().cloned();
    let client = request.extensions().get::<ResolvedClient>().copied();
    let (Some(limiter), Some(client)) = (limiter, client) else {
        tracing::error!(
            scope = class.as_str(),
            "a rate-limited route was reached without the limiter or a resolved client"
        );
        return RateLimitRejection::internal(request_id.as_ref()).into_response();
    };

    let (mut parts, body) = request.into_parts();
    let scope = if keys_on_public_scope(class) {
        public_scope(&mut parts).await
    } else {
        None
    };
    let principal = parts.extensions.get::<RateLimitPrincipal>().copied();
    let subject = Subject::new(client.ip())
        .with_principal(principal)
        .with_scope(scope);

    if let Err(denial) = limiter.admit(class, Stage::Edge, &subject) {
        return rejection(class, denial, request_id.as_ref()).into_response();
    }

    let mut request = Request::from_parts(parts, body);
    if !class.has_deferred_buckets() {
        return next.run(request).await;
    }

    let admitted = Arc::new(AtomicBool::new(false));
    request.extensions_mut().insert(RateLimitGate {
        class,
        limiter,
        subject,
        request_id,
        admitted: Arc::clone(&admitted),
    });
    let response = next.run(request).await;
    if !admitted.load(Ordering::Acquire) {
        tracing::error!(
            scope = class.as_str(),
            "a route completed without admitting its deferred rate-limit buckets"
        );
    }
    response
}

#[derive(Clone)]
pub struct RateLimitGate {
    class: RateLimitClass,
    limiter: Arc<RateLimiter>,
    subject: Subject,
    request_id: Option<RequestId>,
    admitted: Arc<AtomicBool>,
}

impl RateLimitGate {
    #[must_use]
    pub const fn with_account(mut self, account: NormalizedAccount) -> Self {
        self.subject = self.subject.with_account(account);
        self
    }

    #[must_use]
    pub const fn with_mfa_pending(mut self, token: MfaPendingToken) -> Self {
        self.subject = self.subject.with_mfa_pending(token);
        self
    }

    pub fn admit(self) -> Result<(), RateLimitRejection> {
        if self.admitted.swap(true, Ordering::AcqRel) {
            tracing::error!(
                scope = self.class.as_str(),
                "deferred rate-limit buckets were admitted twice for one request"
            );
            return Err(RateLimitRejection::internal(self.request_id.as_ref()));
        }
        self.limiter
            .admit(self.class, Stage::Deferred, &self.subject)
            .map_err(|denial| rejection(self.class, denial, self.request_id.as_ref()))
    }
}

impl<S> FromRequestParts<S> for RateLimitGate
where
    S: Send + Sync,
{
    type Rejection = RateLimitRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.remove::<Self>().ok_or_else(|| {
            tracing::error!("a handler asked for a rate-limit gate its route does not provide");
            RateLimitRejection::internal(parts.extensions.get::<RequestId>())
        })
    }
}
