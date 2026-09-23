use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use http::Method;
use tower::{Service, ServiceBuilder};
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::OpenApi;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};

use super::auth_class::AuthClass;
use super::health;
use super::state::AppState;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::features::branding;
use crate::infra::http::encoding::{
    reject_undecodable_body, request_decompression, response_compression,
};
use crate::infra::http::error::ApiError;
use crate::infra::http::headers::{apply_security_headers, SecurityHeaders, SecurityPolicy};
use crate::infra::http::limits::{enforce_deadline, limit_body, BodyLimit, ControlPlaneLimits};
use crate::infra::http::panic::catch_panic;
use crate::infra::http::path::normalize_path;
use crate::infra::http::proxy::{resolve_client, TrustedProxies};
use crate::infra::http::request_id::{assign_request_id, tag_error, RequestId, RequestIdSource};
use crate::infra::http::static_assets::StaticAssets;
use crate::infra::http::trace::{record_route, trace_request, RequestLog};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RateLimitClass {
    None,
    Read,
    Write,
    AdminWrite,
    AuthLogin,
    AuthTotp,
    AuthReset,
    AuthToken,
    PublicRead,
    PublicPassword,
    PublicSession,
    TransferControl,
    TransferData,
    EmailTest,
    ProviderTest,
}

impl RateLimitClass {
    pub const ALL: [Self; 15] = [
        Self::None,
        Self::Read,
        Self::Write,
        Self::AdminWrite,
        Self::AuthLogin,
        Self::AuthTotp,
        Self::AuthReset,
        Self::AuthToken,
        Self::PublicRead,
        Self::PublicPassword,
        Self::PublicSession,
        Self::TransferControl,
        Self::TransferData,
        Self::EmailTest,
        Self::ProviderTest,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "rl.none",
            Self::Read => "rl.read",
            Self::Write => "rl.write",
            Self::AdminWrite => "rl.admin.write",
            Self::AuthLogin => "rl.auth.login",
            Self::AuthTotp => "rl.auth.totp",
            Self::AuthReset => "rl.auth.reset",
            Self::AuthToken => "rl.auth.token",
            Self::PublicRead => "rl.public.read",
            Self::PublicPassword => "rl.public.password",
            Self::PublicSession => "rl.public.session",
            Self::TransferControl => "rl.transfer.control",
            Self::TransferData => "rl.transfer.data",
            Self::EmailTest => "rl.email.test",
            Self::ProviderTest => "rl.provider.test",
        }
    }
}

impl fmt::Display for RateLimitClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBody {
    GlobalLimit,
    Streamed,
    Capped { max_bytes: NonZeroU64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseEncoding {
    Compressible,
    Identity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    ControlPlane,
    IdleOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytePath {
    request_body: RequestBody,
    response_encoding: ResponseEncoding,
    deadline: Deadline,
}

impl BytePath {
    pub const fn new(
        request_body: RequestBody,
        response_encoding: ResponseEncoding,
        deadline: Deadline,
    ) -> Self {
        Self {
            request_body,
            response_encoding,
            deadline,
        }
    }

    pub const fn declares_opt_out(&self) -> bool {
        !matches!(
            (self.request_body, self.response_encoding, self.deadline),
            (
                RequestBody::GlobalLimit,
                ResponseEncoding::Compressible,
                Deadline::ControlPlane
            )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    ControlPlane,
    BytePath(BytePath),
}

impl Transport {
    pub const fn request_body(&self) -> RequestBody {
        match self {
            Self::ControlPlane => RequestBody::GlobalLimit,
            Self::BytePath(path) => path.request_body,
        }
    }

    pub const fn response_encoding(&self) -> ResponseEncoding {
        match self {
            Self::ControlPlane => ResponseEncoding::Compressible,
            Self::BytePath(path) => path.response_encoding,
        }
    }

    pub const fn deadline(&self) -> Deadline {
        match self {
            Self::ControlPlane => Deadline::ControlPlane,
            Self::BytePath(path) => path.deadline,
        }
    }

    pub const fn is_byte_path(&self) -> bool {
        matches!(self, Self::BytePath(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportLayers {
    deadline: Option<Duration>,
    body_limit: Option<BodyLimit>,
    compress_response: bool,
    decompress_request: bool,
}

impl TransportLayers {
    pub fn for_transport(transport: Transport, limits: ControlPlaneLimits) -> Self {
        let bounded_body = matches!(transport.request_body(), RequestBody::GlobalLimit);
        Self {
            deadline: matches!(transport.deadline(), Deadline::ControlPlane)
                .then_some(limits.deadline),
            body_limit: bounded_body.then_some(limits.body),
            compress_response: matches!(
                transport.response_encoding(),
                ResponseEncoding::Compressible
            ),
            decompress_request: bounded_body,
        }
    }

    pub const fn deadline(&self) -> Option<Duration> {
        self.deadline
    }

    pub const fn body_limit(&self) -> Option<BodyLimit> {
        self.body_limit
    }

    pub const fn compresses_response(&self) -> bool {
        self.compress_response
    }

    pub const fn decompresses_request(&self) -> bool {
        self.decompress_request
    }

    // `route_layer` wraps outward, so layers are added innermost first: the
    // resulting order is route recording, deadline, body limit, compression,
    // decompression.
    fn apply<S>(self, mut handler: MethodRouter<S>) -> MethodRouter<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        if let (true, Some(limit)) = (self.decompress_request, self.body_limit) {
            handler = handler
                .route_layer(from_fn_with_state(limit, limit_body))
                .route_layer(request_decompression())
                .route_layer(from_fn(reject_undecodable_body));
        }
        if self.compress_response {
            handler = handler.route_layer(response_compression());
        }
        if let Some(limit) = self.body_limit {
            handler = handler.route_layer(from_fn_with_state(limit, limit_body));
        }
        if let Some(deadline) = self.deadline {
            handler = handler.route_layer(from_fn_with_state(deadline, enforce_deadline));
        }
        handler.route_layer(from_fn(record_route))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutePolicy {
    auth: AuthClass,
    rate_limit: RateLimitClass,
    transport: Transport,
    security: SecurityPolicy,
    request_log: RequestLog,
}

impl RoutePolicy {
    pub const fn new(auth: AuthClass, rate_limit: RateLimitClass, transport: Transport) -> Self {
        Self {
            auth,
            rate_limit,
            transport,
            security: SecurityPolicy::Default,
            request_log: RequestLog::Standard,
        }
    }

    pub const fn with_security(mut self, security: SecurityPolicy) -> Self {
        self.security = security;
        self
    }

    pub const fn with_request_log(mut self, request_log: RequestLog) -> Self {
        self.request_log = request_log;
        self
    }

    pub const fn auth(&self) -> AuthClass {
        self.auth
    }

    pub const fn rate_limit(&self) -> RateLimitClass {
        self.rate_limit
    }

    pub const fn transport(&self) -> Transport {
        self.transport
    }

    pub const fn security(&self) -> SecurityPolicy {
        self.security
    }

    pub const fn request_log(&self) -> RequestLog {
        self.request_log
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    path: String,
    method: Method,
    policy: RoutePolicy,
    layers: TransportLayers,
}

impl RouteEntry {
    pub const fn layers(&self) -> TransportLayers {
        self.layers
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub const fn policy(&self) -> RoutePolicy {
        self.policy
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteInventory {
    entries: Vec<RouteEntry>,
}

impl RouteInventory {
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }

    pub fn get(&self, method: &Method, path: &str) -> Option<&RouteEntry> {
        self.entries
            .iter()
            .find(|entry| entry.method == method && entry.path == path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    MissingOperation,
    InvalidPath { path: String },
    Duplicate { method: Method, path: String },
    BytePathWithoutOptOut { method: Method, path: String },
    EmbedPolicyOutsideEmbedRoutes { method: Method, path: String },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOperation => f.write_str("route registered without an OpenAPI operation"),
            Self::InvalidPath { path } => write!(f, "route path {path:?} must start with '/'"),
            Self::Duplicate { method, path } => {
                write!(f, "{method} {path} is registered more than once")
            }
            Self::BytePathWithoutOptOut { method, path } => write!(
                f,
                "{method} {path} is a byte path but declares no request-body, compression or timeout opt-out"
            ),
            Self::EmbedPolicyOutsideEmbedRoutes { method, path } => write!(
                f,
                "{method} {path} declares the embed security policy outside the embed routes"
            ),
        }
    }
}

impl std::error::Error for RouteError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteBuildError {
    errors: Vec<RouteError>,
}

impl RouteBuildError {
    pub fn errors(&self) -> &[RouteError] {
        &self.errors
    }
}

impl fmt::Display for RouteBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("router has invalid route declarations")?;
        for error in &self.errors {
            write!(f, "; {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RouteBuildError {}

type RouteKey = (String, u8);

pub struct AssembledRouter<S = AppState> {
    pub router: axum::Router<S>,
    pub openapi: OpenApi,
    pub inventory: RouteInventory,
}

// The inner router is private so a route can only enter through `route`,
// which takes a complete `RoutePolicy`; the inventory is recorded at that
// same site instead of being reconstructed from the assembled Axum router.
pub struct Routes<S = AppState> {
    router: OpenApiRouter<S>,
    entries: BTreeMap<RouteKey, RouteEntry>,
    errors: Vec<RouteError>,
    limits: ControlPlaneLimits,
}

impl<S> Default for Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Routes<S>
where
    S: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self::with_limits(ControlPlaneLimits::default())
    }

    pub fn with_limits(limits: ControlPlaneLimits) -> Self {
        Self {
            router: OpenApiRouter::new(),
            entries: BTreeMap::new(),
            errors: Vec::new(),
            limits,
        }
    }

    pub fn route(mut self, policy: RoutePolicy, method_router: UtoipaMethodRouter<S>) -> Self {
        let (schemas, paths, handler) = method_router;
        let layers = TransportLayers::for_transport(policy.transport, self.limits);
        let mut declared = Vec::new();
        let mut errors = Vec::new();

        for (path, item) in &paths.paths {
            if !path.starts_with('/') {
                errors.push(RouteError::InvalidPath { path: path.clone() });
                continue;
            }
            for method in operation_methods(item) {
                let entry = RouteEntry {
                    path: path.clone(),
                    method,
                    policy,
                    layers,
                };
                if matches!(policy.transport, Transport::BytePath(path) if !path.declares_opt_out())
                {
                    errors.push(RouteError::BytePathWithoutOptOut {
                        method: entry.method.clone(),
                        path: entry.path.clone(),
                    });
                }
                if !policy.security.permits_path(&entry.path) {
                    errors.push(RouteError::EmbedPolicyOutsideEmbedRoutes {
                        method: entry.method.clone(),
                        path: entry.path.clone(),
                    });
                }
                if self.entries.contains_key(&entry.key())
                    || declared
                        .iter()
                        .any(|seen: &RouteEntry| seen.key() == entry.key())
                {
                    errors.push(RouteError::Duplicate {
                        method: entry.method.clone(),
                        path: entry.path.clone(),
                    });
                }
                declared.push(entry);
            }
        }

        if declared.is_empty() && errors.is_empty() {
            errors.push(RouteError::MissingOperation);
        }

        if errors.is_empty() {
            let handler = policy
                .request_log
                .apply(policy.security.apply(layers.apply(handler)));
            self.router = self.router.routes((schemas, paths, handler));
            self.entries
                .extend(declared.into_iter().map(|entry| (entry.key(), entry)));
        } else {
            self.errors.extend(errors);
        }
        self
    }

    pub fn merge(mut self, other: Routes<S>) -> Self {
        self.errors.extend(other.errors);

        let duplicates: Vec<RouteError> = other
            .entries
            .values()
            .filter(|entry| self.entries.contains_key(&entry.key()))
            .map(|entry| RouteError::Duplicate {
                method: entry.method.clone(),
                path: entry.path.clone(),
            })
            .collect();

        if duplicates.is_empty() {
            self.router = self.router.merge(other.router);
            self.entries.extend(other.entries);
        } else {
            self.errors.extend(duplicates);
        }
        self
    }

    pub fn build(self) -> Result<AssembledRouter<S>, RouteBuildError> {
        if !self.errors.is_empty() {
            return Err(RouteBuildError {
                errors: self.errors,
            });
        }
        let (router, openapi) = self.router.split_for_parts();
        Ok(AssembledRouter {
            router,
            openapi,
            inventory: RouteInventory {
                entries: self.entries.into_values().collect(),
            },
        })
    }
}

pub fn application_routes() -> Routes<AppState> {
    Routes::new()
        .merge(health::routes())
        .merge(branding::routes::routes())
}

// Call only after every route is merged: axum attaches the 405 fallback to
// the method routers that exist at this point, and later ones would keep
// axum's empty-bodied 405.
pub fn serve_unmatched<S>(router: axum::Router<S>, assets: StaticAssets) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .method_not_allowed_fallback(method_not_allowed)
        .fallback_service(assets.fallback())
}

async fn method_not_allowed(request: Request) -> Response {
    tag_error(
        ApiError::new(ErrorCode::MethodNotAllowed),
        RequestId::of(&request).as_ref(),
    )
    .into_response()
}

#[derive(Clone)]
pub struct HttpEdge {
    clock: Arc<dyn Clock>,
    proxies: Arc<TrustedProxies>,
    security: Arc<SecurityHeaders>,
}

impl HttpEdge {
    pub fn new(clock: Arc<dyn Clock>, proxies: TrustedProxies, security: SecurityHeaders) -> Self {
        Self {
            clock,
            proxies: Arc::new(proxies),
            security: Arc::new(security),
        }
    }
}

// Path normalization must wrap the router from outside, because axum matches
// the route before any `Router::layer` middleware runs. Security headers sit
// outside panic catch so every response, including a caught panic, carries
// them. CORS is deliberately absent because the SPA is served same-origin.
pub fn with_middleware(
    router: axum::Router,
    edge: &HttpEdge,
) -> impl Service<Request, Response = Response, Error = Infallible, Future: Send + 'static>
       + Clone
       + Send
       + Sync
       + 'static {
    let request_ids = RequestIdSource::new(Arc::clone(&edge.proxies), Arc::clone(&edge.clock));
    ServiceBuilder::new()
        .layer(from_fn_with_state(request_ids, assign_request_id))
        .layer(from_fn_with_state(Arc::clone(&edge.clock), trace_request))
        .layer(from_fn_with_state(
            Arc::clone(&edge.proxies),
            resolve_client,
        ))
        .layer(from_fn_with_state(
            Arc::clone(&edge.security),
            apply_security_headers,
        ))
        .layer(from_fn(catch_panic))
        .layer(from_fn(normalize_path))
        .service(router)
}

impl RouteEntry {
    fn key(&self) -> RouteKey {
        (self.path.clone(), method_rank(&self.method))
    }
}

fn operation_methods(item: &PathItem) -> Vec<Method> {
    let operations: [(&Option<Operation>, Method); 8] = [
        (&item.get, Method::GET),
        (&item.head, Method::HEAD),
        (&item.post, Method::POST),
        (&item.put, Method::PUT),
        (&item.patch, Method::PATCH),
        (&item.delete, Method::DELETE),
        (&item.options, Method::OPTIONS),
        (&item.trace, Method::TRACE),
    ];
    operations
        .into_iter()
        .filter_map(|(operation, method)| operation.as_ref().map(|_| method))
        .collect()
}

fn method_rank(method: &Method) -> u8 {
    match *method {
        Method::GET => 0,
        Method::HEAD => 1,
        Method::POST => 2,
        Method::PUT => 3,
        Method::PATCH => 4,
        Method::DELETE => 5,
        Method::OPTIONS => 6,
        Method::TRACE => 7,
        _ => u8::MAX,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::num::NonZeroU64;
    use std::time::Duration;

    use axum::body::Body;
    use http::{Method, Request, StatusCode};
    use rstest::rstest;
    use tower::ServiceExt;
    use utoipa::openapi::path::Paths;
    use utoipa_axum::routes;

    use super::{
        application_routes, operation_methods, BytePath, Deadline, RateLimitClass, RequestBody,
        ResponseEncoding, RouteError, RouteInventory, RoutePolicy, Routes, Transport,
        TransportLayers,
    };
    use crate::app::auth_class::AuthClass;
    use crate::infra::http::limits::{BodyLimit, ControlPlaneLimits, CONTROL_PLANE_BODY_LIMIT};
    use crate::infra::http::trace::RequestLog;

    #[utoipa::path(get, path = "/test/items", responses((status = 200)))]
    async fn list_items() -> StatusCode {
        StatusCode::OK
    }

    #[utoipa::path(post, path = "/test/items", responses((status = 201)))]
    async fn create_item() -> StatusCode {
        StatusCode::CREATED
    }

    #[utoipa::path(get, path = "/test/items/{id}/content", params(("id" = String, Path)), responses((status = 200)))]
    async fn item_content() -> StatusCode {
        StatusCode::OK
    }

    #[utoipa::path(patch, path = "/test/uploads/{id}", params(("id" = String, Path)), responses((status = 204)))]
    async fn upload_chunk() -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[utoipa::path(get, path = "test/relative", responses((status = 200)))]
    async fn relative_path() -> StatusCode {
        StatusCode::OK
    }

    const PUBLIC_READ: RoutePolicy = RoutePolicy::new(
        AuthClass::Public,
        RateLimitClass::PublicRead,
        Transport::ControlPlane,
    );

    const DOWNLOAD: RoutePolicy = RoutePolicy::new(
        AuthClass::PublicGrant,
        RateLimitClass::TransferData,
        Transport::BytePath(BytePath::new(
            RequestBody::GlobalLimit,
            ResponseEncoding::Identity,
            Deadline::IdleOnly,
        )),
    );

    const UPLOAD: RoutePolicy = RoutePolicy::new(
        AuthClass::Authenticated,
        RateLimitClass::TransferData,
        Transport::BytePath(BytePath::new(
            RequestBody::Streamed,
            ResponseEncoding::Identity,
            Deadline::IdleOnly,
        )),
    );

    fn sample_routes() -> Routes<()> {
        Routes::new()
            .route(UPLOAD, routes!(upload_chunk))
            .route(DOWNLOAD, routes!(item_content))
            .route(PUBLIC_READ, routes!(list_items))
            .route(
                RoutePolicy::new(
                    AuthClass::Admin,
                    RateLimitClass::AdminWrite,
                    Transport::ControlPlane,
                ),
                routes!(create_item),
            )
    }

    fn sample_inventory() -> RouteInventory {
        sample_routes().build().unwrap().inventory
    }

    fn openapi_operations(openapi: &utoipa::openapi::OpenApi) -> BTreeSet<(String, Method)> {
        openapi
            .paths
            .paths
            .iter()
            .flat_map(|(path, item)| {
                operation_methods(item)
                    .into_iter()
                    .map(move |method| (path.clone(), method))
            })
            .collect()
    }

    fn assert_every_route_declares_one_auth_class<S>(routes: Routes<S>)
    where
        S: Clone + Send + Sync + 'static,
    {
        let assembled = routes.build().unwrap();
        let inventoried: Vec<(String, Method)> = assembled
            .inventory
            .entries()
            .iter()
            .map(|entry| (entry.path().to_owned(), entry.method().clone()))
            .collect();
        let unique: BTreeSet<(String, Method)> = inventoried.iter().cloned().collect();

        assert_eq!(unique.len(), inventoried.len());
        assert_eq!(unique, openapi_operations(&assembled.openapi));
        for entry in assembled.inventory.entries() {
            assert!(AuthClass::ALL.contains(&entry.policy().auth()));
            assert!(RateLimitClass::ALL.contains(&entry.policy().rate_limit()));
        }
    }

    fn assert_every_byte_route_declares_its_opt_outs(inventory: &RouteInventory) {
        for entry in inventory.entries() {
            let route = format!("{} {}", entry.method(), entry.path());
            let transport = entry.policy().transport();
            let layers = entry.layers();
            if let Transport::BytePath(path) = transport {
                assert!(
                    path.declares_opt_out(),
                    "{route} is a byte path without opt-outs"
                );
            }
            if transport.response_encoding() == ResponseEncoding::Identity {
                assert!(!layers.compresses_response(), "{route} is compressed");
            }
            if transport.deadline() == Deadline::IdleOnly {
                assert_eq!(
                    layers.deadline(),
                    None,
                    "{route} has a control-plane deadline"
                );
            }
            if transport.request_body() != RequestBody::GlobalLimit {
                assert_eq!(
                    layers.body_limit(),
                    None,
                    "{route} has the global body limit"
                );
                assert!(!layers.decompresses_request(), "{route} decodes its body");
            }
        }
    }

    #[test]
    fn svc_every_route_declares_one_auth_class() {
        assert_every_route_declares_one_auth_class(application_routes());
        assert_every_route_declares_one_auth_class(sample_routes());
    }

    #[test]
    fn svc_every_byte_route_declares_its_opt_outs() {
        assert_every_byte_route_declares_its_opt_outs(
            &application_routes().build().unwrap().inventory,
        );
        assert_every_byte_route_declares_its_opt_outs(&sample_inventory());
        assert_every_byte_route_declares_its_opt_outs(
            &Routes::<()>::new()
                .route(
                    RoutePolicy::new(
                        AuthClass::Admin,
                        RateLimitClass::AdminWrite,
                        Transport::BytePath(BytePath::new(
                            RequestBody::Capped {
                                max_bytes: NonZeroU64::new(5 * 1024 * 1024).unwrap(),
                            },
                            ResponseEncoding::Compressible,
                            Deadline::ControlPlane,
                        )),
                    ),
                    routes!(create_item),
                )
                .build()
                .unwrap()
                .inventory,
        );
    }

    #[rstest]
    #[case::control_plane(Transport::ControlPlane, true, true, true)]
    #[case::download(
        Transport::BytePath(BytePath::new(
            RequestBody::GlobalLimit,
            ResponseEncoding::Identity,
            Deadline::IdleOnly
        )),
        false,
        true,
        false
    )]
    #[case::upload(
        Transport::BytePath(BytePath::new(
            RequestBody::Streamed,
            ResponseEncoding::Identity,
            Deadline::IdleOnly
        )),
        false,
        false,
        false
    )]
    #[case::capped_upload(
        Transport::BytePath(BytePath::new(
            RequestBody::Capped { max_bytes: NonZeroU64::new(5 * 1024 * 1024).unwrap() },
            ResponseEncoding::Compressible,
            Deadline::ControlPlane,
        )),
        true,
        false,
        true
    )]
    fn unit_transport_layers_follow_policy(
        #[case] transport: Transport,
        #[case] deadline: bool,
        #[case] global_body_limit: bool,
        #[case] compresses: bool,
    ) {
        let limits = ControlPlaneLimits::default();
        let layers = TransportLayers::for_transport(transport, limits);
        assert_eq!(
            layers.deadline(),
            deadline.then_some(Duration::from_secs(30))
        );
        assert_eq!(
            layers.body_limit(),
            global_body_limit.then_some(CONTROL_PLANE_BODY_LIMIT)
        );
        assert_eq!(layers.decompresses_request(), global_body_limit);
        assert_eq!(layers.compresses_response(), compresses);
    }

    #[test]
    fn unit_control_plane_limits_default() {
        let limits = ControlPlaneLimits::default();
        assert_eq!(limits.deadline, Duration::from_secs(30));
        assert_eq!(limits.body.max_bytes(), 2 * 1024 * 1024);
    }

    #[test]
    fn unit_inventory_records_applied_layers() {
        let limits = ControlPlaneLimits {
            deadline: Duration::from_secs(5),
            body: BodyLimit::new(64),
        };
        let inventory = Routes::<()>::with_limits(limits)
            .route(PUBLIC_READ, routes!(list_items))
            .route(UPLOAD, routes!(upload_chunk))
            .build()
            .unwrap()
            .inventory;

        let list = inventory.get(&Method::GET, "/test/items").unwrap();
        assert_eq!(
            list.layers(),
            TransportLayers::for_transport(Transport::ControlPlane, limits)
        );
        assert_eq!(list.layers().deadline(), Some(Duration::from_secs(5)));
        assert_eq!(list.layers().body_limit(), Some(BodyLimit::new(64)));

        let upload = inventory.get(&Method::PATCH, "/test/uploads/{id}").unwrap();
        assert_eq!(upload.layers().deadline(), None);
        assert_eq!(upload.layers().body_limit(), None);
        assert!(!upload.layers().compresses_response());
    }

    #[test]
    fn unit_application_routes_build() {
        let assembled = application_routes().build().unwrap();
        assert_eq!(
            assembled.inventory.entries().len(),
            openapi_operations(&assembled.openapi).len()
        );
    }

    #[test]
    fn unit_inventory_records_declared_policy() {
        let inventory = sample_inventory();

        let upload = inventory.get(&Method::PATCH, "/test/uploads/{id}").unwrap();
        assert_eq!(upload.policy(), UPLOAD);
        assert_eq!(upload.policy().auth(), AuthClass::Authenticated);
        assert_eq!(upload.policy().rate_limit(), RateLimitClass::TransferData);
        assert_eq!(
            upload.policy().transport().request_body(),
            RequestBody::Streamed
        );

        let create = inventory.get(&Method::POST, "/test/items").unwrap();
        assert_eq!(create.policy().auth(), AuthClass::Admin);
        assert_eq!(create.policy().rate_limit(), RateLimitClass::AdminWrite);
        assert!(!create.policy().transport().is_byte_path());

        assert!(inventory.get(&Method::DELETE, "/test/items").is_none());
    }

    #[test]
    fn unit_inventory_is_deterministic_and_sorted() {
        let first = sample_inventory();
        let reordered = Routes::<()>::new()
            .route(PUBLIC_READ, routes!(list_items))
            .route(
                RoutePolicy::new(
                    AuthClass::Admin,
                    RateLimitClass::AdminWrite,
                    Transport::ControlPlane,
                ),
                routes!(create_item),
            )
            .route(DOWNLOAD, routes!(item_content))
            .route(UPLOAD, routes!(upload_chunk))
            .build()
            .unwrap()
            .inventory;

        assert_eq!(first, reordered);
        let order: Vec<(&str, &Method)> = first
            .entries()
            .iter()
            .map(|entry| (entry.path(), entry.method()))
            .collect();
        assert_eq!(
            order,
            [
                ("/test/items", &Method::GET),
                ("/test/items", &Method::POST),
                ("/test/items/{id}/content", &Method::GET),
                ("/test/uploads/{id}", &Method::PATCH),
            ]
        );
    }

    #[test]
    fn unit_every_auth_class_can_be_declared() {
        for class in AuthClass::ALL {
            let inventory = Routes::<()>::new()
                .route(
                    RoutePolicy::new(class, RateLimitClass::Read, Transport::ControlPlane),
                    routes!(list_items),
                )
                .build()
                .unwrap()
                .inventory;
            assert_eq!(inventory.entries()[0].policy().auth(), class);
        }
    }

    #[test]
    fn unit_rate_limit_labels_are_the_accepted_set() {
        let labels: Vec<&str> = RateLimitClass::ALL
            .iter()
            .map(|class| class.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "rl.none",
                "rl.read",
                "rl.write",
                "rl.admin.write",
                "rl.auth.login",
                "rl.auth.totp",
                "rl.auth.reset",
                "rl.auth.token",
                "rl.public.read",
                "rl.public.password",
                "rl.public.session",
                "rl.transfer.control",
                "rl.transfer.data",
                "rl.email.test",
                "rl.provider.test",
            ]
        );
    }

    #[test]
    fn unit_control_plane_transport_has_no_opt_outs() {
        let transport = Transport::ControlPlane;
        assert_eq!(transport.request_body(), RequestBody::GlobalLimit);
        assert_eq!(
            transport.response_encoding(),
            ResponseEncoding::Compressible
        );
        assert_eq!(transport.deadline(), Deadline::ControlPlane);
        assert!(!transport.is_byte_path());
    }

    #[test]
    fn unit_request_log_is_standard_unless_declared() {
        let policy = RoutePolicy::new(
            AuthClass::Public,
            RateLimitClass::Read,
            Transport::ControlPlane,
        );
        assert_eq!(policy.request_log(), RequestLog::Standard);
        assert_eq!(
            policy.with_request_log(RequestLog::Polled).request_log(),
            RequestLog::Polled
        );
    }

    #[test]
    fn unit_byte_path_opt_outs_are_explicit() {
        let capped = BytePath::new(
            RequestBody::Capped {
                max_bytes: NonZeroU64::new(5 * 1024 * 1024).unwrap(),
            },
            ResponseEncoding::Compressible,
            Deadline::ControlPlane,
        );
        assert!(capped.declares_opt_out());
        assert!(BytePath::new(
            RequestBody::GlobalLimit,
            ResponseEncoding::Identity,
            Deadline::ControlPlane
        )
        .declares_opt_out());
        assert!(BytePath::new(
            RequestBody::GlobalLimit,
            ResponseEncoding::Compressible,
            Deadline::IdleOnly
        )
        .declares_opt_out());
        assert!(!BytePath::new(
            RequestBody::GlobalLimit,
            ResponseEncoding::Compressible,
            Deadline::ControlPlane
        )
        .declares_opt_out());
    }

    #[test]
    fn unit_byte_path_without_opt_out_fails_build() {
        let policy = RoutePolicy::new(
            AuthClass::Authenticated,
            RateLimitClass::TransferData,
            Transport::BytePath(BytePath::new(
                RequestBody::GlobalLimit,
                ResponseEncoding::Compressible,
                Deadline::ControlPlane,
            )),
        );
        let error = Routes::<()>::new()
            .route(policy, routes!(item_content))
            .build()
            .err()
            .unwrap();
        assert_eq!(
            error.errors(),
            [RouteError::BytePathWithoutOptOut {
                method: Method::GET,
                path: "/test/items/{id}/content".to_owned(),
            }]
        );
    }

    #[test]
    fn unit_duplicate_route_fails_build() {
        let error = Routes::<()>::new()
            .route(PUBLIC_READ, routes!(list_items))
            .route(
                RoutePolicy::new(
                    AuthClass::Admin,
                    RateLimitClass::Read,
                    Transport::ControlPlane,
                ),
                routes!(list_items),
            )
            .build()
            .err()
            .unwrap();
        assert_eq!(
            error.errors(),
            [RouteError::Duplicate {
                method: Method::GET,
                path: "/test/items".to_owned(),
            }]
        );
    }

    #[test]
    fn unit_duplicate_route_across_merge_fails_build() {
        let left = Routes::<()>::new().route(PUBLIC_READ, routes!(list_items));
        let right = Routes::<()>::new().route(PUBLIC_READ, routes!(list_items));
        let error = left.merge(right).build().err().unwrap();
        assert_eq!(
            error.errors(),
            [RouteError::Duplicate {
                method: Method::GET,
                path: "/test/items".to_owned(),
            }]
        );
    }

    #[test]
    fn unit_merge_combines_inventories_and_openapi() {
        let left = Routes::<()>::new().route(PUBLIC_READ, routes!(list_items));
        let right = Routes::<()>::new().route(
            RoutePolicy::new(
                AuthClass::Admin,
                RateLimitClass::AdminWrite,
                Transport::ControlPlane,
            ),
            routes!(create_item),
        );
        let assembled = left.merge(right).build().unwrap();

        assert_eq!(assembled.inventory.entries().len(), 2);
        assert_eq!(
            openapi_operations(&assembled.openapi),
            BTreeSet::from([
                ("/test/items".to_owned(), Method::GET),
                ("/test/items".to_owned(), Method::POST),
            ])
        );
    }

    #[test]
    fn unit_merge_carries_errors_forward() {
        let broken = Routes::<()>::new().route(PUBLIC_READ, routes!(relative_path));
        let error = Routes::<()>::new()
            .route(PUBLIC_READ, routes!(list_items))
            .merge(broken)
            .build()
            .err()
            .unwrap();
        assert_eq!(
            error.errors(),
            [RouteError::InvalidPath {
                path: "test/relative".to_owned(),
            }]
        );
    }

    #[test]
    fn unit_route_without_openapi_operation_fails_build() {
        let bare = (Vec::new(), Paths::new(), axum::routing::get(list_items));
        let error = Routes::<()>::new()
            .route(PUBLIC_READ, bare)
            .build()
            .err()
            .unwrap();
        assert_eq!(error.errors(), [RouteError::MissingOperation]);
    }

    #[test]
    fn unit_build_error_lists_every_problem() {
        let error = Routes::<()>::new()
            .route(PUBLIC_READ, routes!(list_items))
            .route(PUBLIC_READ, routes!(list_items))
            .route(PUBLIC_READ, routes!(relative_path))
            .build()
            .err()
            .unwrap();
        assert_eq!(error.errors().len(), 2);
        assert_eq!(
            error.to_string(),
            "router has invalid route declarations; GET /test/items is registered more than once; route path \"test/relative\" must start with '/'"
        );
    }

    #[tokio::test]
    async fn svc_registered_route_is_served() {
        let router = sample_routes().build().unwrap().router;

        let created = router
            .clone()
            .oneshot(Request::post("/test/items").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let unregistered = router
            .oneshot(Request::delete("/test/items").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unregistered.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
