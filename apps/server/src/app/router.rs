use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

use http::Method;
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::OpenApi;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};

use super::auth_class::AuthClass;
use super::state::AppState;

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
pub struct RoutePolicy {
    auth: AuthClass,
    rate_limit: RateLimitClass,
    transport: Transport,
}

impl RoutePolicy {
    pub const fn new(auth: AuthClass, rate_limit: RateLimitClass, transport: Transport) -> Self {
        Self {
            auth,
            rate_limit,
            transport,
        }
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    path: String,
    method: Method,
    policy: RoutePolicy,
}

impl RouteEntry {
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
        Self {
            router: OpenApiRouter::new(),
            entries: BTreeMap::new(),
            errors: Vec::new(),
        }
    }

    pub fn route(mut self, policy: RoutePolicy, method_router: UtoipaMethodRouter<S>) -> Self {
        let (_, paths, _) = &method_router;
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
                };
                if matches!(policy.transport, Transport::BytePath(path) if !path.declares_opt_out())
                {
                    errors.push(RouteError::BytePathWithoutOptOut {
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
            self.router = self.router.routes(method_router);
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

    use axum::body::Body;
    use http::{Method, Request, StatusCode};
    use tower::ServiceExt;
    use utoipa::openapi::path::Paths;
    use utoipa_axum::routes;

    use super::{
        application_routes, operation_methods, BytePath, Deadline, RateLimitClass, RequestBody,
        ResponseEncoding, RouteError, RouteInventory, RoutePolicy, Routes, Transport,
    };
    use crate::app::auth_class::AuthClass;

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
            if let Transport::BytePath(path) = entry.policy().transport() {
                assert!(
                    path.declares_opt_out(),
                    "{} {} is a byte path without opt-outs",
                    entry.method(),
                    entry.path()
                );
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
