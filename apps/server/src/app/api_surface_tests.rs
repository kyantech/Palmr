use std::collections::BTreeSet;

use http::Method;
use serde_json::{json, Value};
use utoipa_axum::routes;

use super::auth_class::AuthClass;
use super::openapi::ApiDocs;
use super::router::{
    application_routes, IdempotencyMode, RateLimitClass, RouteInventory, RoutePolicy, Routes,
    Transport,
};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::features::users::admin_service::USER_SORT;

const PUBLIC_ROUTES_GOLDEN: &str = include_str!("../../../../tests/snapshots/public_routes.txt");

const ANONYMOUS_CLASSES: [AuthClass; 3] =
    [AuthClass::Public, AuthClass::PublicGrant, AuthClass::Setup];

const FORBIDDEN_REQUEST_FIELDS: [&str; 4] = ["objectKey", "objectName", "key", "storageKey"];

const FORBIDDEN_STORAGE_INPUTS: [&str; 6] = [
    "objectKey",
    "objectName",
    "key",
    "storageKey",
    "bucket",
    "bucketName",
];

const FORBIDDEN_NAMESPACES: [&str; 2] = ["/s3", "/api/v1/storage"];

const STORAGE_CAPABILITY_SEGMENTS: [&str; 3] = ["s3", "multipart", "presign"];

const TRANSFER_ITEM_SCOPE: [&str; 4] = ["sessions", "{sid}", "files", "{itemId}"];

const CAPABILITY_CLASSES: [AuthClass; 3] = [
    AuthClass::Authenticated,
    AuthClass::AuthenticatedRecentAuth,
    AuthClass::PublicGrant,
];

const ADMIN_STORAGE_NAMESPACE: &str = "/api/v1/admin/storage";

const ADMIN_STORAGE_ROUTES: [&str; 3] = [
    "/api/v1/admin/storage",
    "/api/v1/admin/storage/self-test",
    "/api/v1/admin/storage/cors-template",
];

const HTTP_SURFACE_TOKENS: [&str; 5] = ["axum", "utoipa", "Router", "routes!", "IntoResponse"];

fn public_route_snapshot(inventory: &RouteInventory) -> String {
    inventory
        .entries()
        .iter()
        .filter(|entry| ANONYMOUS_CLASSES.contains(&entry.policy().auth()))
        .map(|entry| {
            format!(
                "{} {} {}\n",
                entry.method(),
                entry.path(),
                entry.policy().auth()
            )
        })
        .collect()
}

fn application_inventory() -> RouteInventory {
    application_routes().build().unwrap().inventory
}

fn application_document() -> Value {
    let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
        &str,
        &str,
    )>()))
    .unwrap()
    .config;
    let docs = ApiDocs::new(
        application_routes().build().unwrap().openapi,
        &config.base_url,
    )
    .unwrap();
    serde_json::from_slice(docs.document()).unwrap()
}

#[test]
fn it_public_route_snapshot_matches() {
    let generated = public_route_snapshot(&application_inventory());
    assert!(
        generated == PUBLIC_ROUTES_GOLDEN,
        "the anonymous route surface changed; review it and replace tests/snapshots/public_routes.txt with:\n{generated}"
    );
}

#[test]
fn unit_public_route_snapshot_is_sorted_and_classified() {
    let lines: Vec<&str> = PUBLIC_ROUTES_GOLDEN.lines().collect();
    let mut sorted = lines.clone();
    sorted.sort_by_key(|line| line.split(' ').nth(1).unwrap_or_default().to_owned());
    assert_eq!(lines, sorted);
    for line in lines {
        let fields: Vec<&str> = line.split(' ').collect();
        assert_eq!(fields.len(), 3, "{line}");
        assert!(fields[1].starts_with('/'), "{line}");
        assert!(
            ANONYMOUS_CLASSES
                .iter()
                .any(|class| class.as_str() == fields[2]),
            "{line}"
        );
    }
}

#[utoipa::path(get, path = "/api/v1/public/unreviewed", responses((status = 200)))]
async fn unreviewed() -> http::StatusCode {
    http::StatusCode::OK
}

#[utoipa::path(get, path = "/api/v1/admin/storage", responses((status = 200)))]
async fn admin_storage_status() -> http::StatusCode {
    http::StatusCode::OK
}

const fn policy(auth: AuthClass) -> RoutePolicy {
    RoutePolicy::new(auth, RateLimitClass::Read, Transport::ControlPlane)
}

#[test]
fn unit_public_route_snapshot_detects_surface_changes() {
    let golden = public_route_snapshot(&application_inventory());

    for class in ANONYMOUS_CLASSES {
        let widened = application_routes()
            .merge(Routes::new().route(policy(class), routes!(unreviewed)))
            .build()
            .unwrap()
            .inventory;
        let snapshot = public_route_snapshot(&widened);
        assert_ne!(snapshot, golden, "{class}");
        assert!(snapshot.contains(&format!("GET /api/v1/public/unreviewed {class}\n")));
    }

    for class in [
        AuthClass::Authenticated,
        AuthClass::AuthenticatedRecentAuth,
        AuthClass::Admin,
        AuthClass::AdminRecentAuth,
    ] {
        let private = application_routes()
            .merge(Routes::new().route(policy(class), routes!(unreviewed)))
            .build()
            .unwrap()
            .inventory;
        assert_eq!(public_route_snapshot(&private), golden, "{class}");
    }

    let narrowed = super::health::routes().build().unwrap().inventory;
    assert_ne!(public_route_snapshot(&narrowed), golden);
}

#[test]
fn unit_public_route_snapshot_detects_class_changes() {
    let snapshots: BTreeSet<String> = ANONYMOUS_CLASSES
        .into_iter()
        .map(|class| {
            public_route_snapshot(
                &application_routes()
                    .merge(Routes::new().route(policy(class), routes!(unreviewed)))
                    .build()
                    .unwrap()
                    .inventory,
            )
        })
        .collect();
    assert_eq!(snapshots.len(), ANONYMOUS_CLASSES.len());
}

fn request_schemas(document: &Value) -> Vec<(String, &Value)> {
    let mut found = Vec::new();
    let Some(paths) = document["paths"].as_object() else {
        return found;
    };
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (method, operation) in item {
            let origin = format!("{} {path}", method.to_ascii_uppercase());
            if let Some(content) = operation["requestBody"]["content"].as_object() {
                for (media, body) in content {
                    found.push((format!("{origin} request body {media}"), &body["schema"]));
                }
            }
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                found.push((format!("{origin} parameter"), &parameter["schema"]));
            }
        }
    }
    found
}

fn storage_key_violations(document: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    for (origin, schema) in request_schemas(document) {
        let mut visited = BTreeSet::new();
        walk_request_schema(document, schema, &origin, &mut visited, &mut violations);
    }
    violations
}

fn walk_request_schema(
    document: &Value,
    schema: &Value,
    origin: &str,
    visited: &mut BTreeSet<String>,
    violations: &mut Vec<String>,
) {
    let Some(schema) = schema.as_object() else {
        return;
    };
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if visited.insert(reference.to_owned()) {
            let target = resolve(document, reference);
            walk_request_schema(
                document,
                target,
                &format!("{origin} → {reference}"),
                visited,
                violations,
            );
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            if FORBIDDEN_REQUEST_FIELDS.contains(&name.as_str()) {
                violations.push(format!("{origin} exposes request field {name:?}"));
            }
            walk_request_schema(
                document,
                property,
                &format!("{origin}.{name}"),
                visited,
                violations,
            );
        }
    }
    for nested in ["items", "additionalProperties", "not", "contains"] {
        if let Some(inner) = schema.get(nested) {
            walk_request_schema(document, inner, origin, visited, violations);
        }
    }
    for composite in ["allOf", "oneOf", "anyOf", "prefixItems"] {
        for inner in schema
            .get(composite)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            walk_request_schema(document, inner, origin, visited, violations);
        }
    }
}

fn resolve<'a>(document: &'a Value, reference: &str) -> &'a Value {
    reference
        .strip_prefix("#/")
        .map(|pointer| {
            document
                .pointer(&format!("/{pointer}"))
                .unwrap_or(&Value::Null)
        })
        .unwrap_or(&Value::Null)
}

#[test]
fn it_openapi_has_no_storage_key_fields() {
    let document = application_document();
    assert!(!document["paths"].as_object().unwrap().is_empty());
    assert_eq!(storage_key_violations(&document), Vec::<String>::new());
}

fn document_with(paths: Value, schemas: Value) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": { "title": "t", "version": "0" },
        "paths": paths,
        "components": { "schemas": schemas },
    })
}

fn json_body(schema: Value) -> Value {
    json!({ "content": { "application/json": { "schema": schema } } })
}

#[test]
fn unit_storage_key_walker_detects_nested_request_fields() {
    let document = document_with(
        json!({ "/x": { "post": {
            "requestBody": json_body(json!({
                "type": "object",
                "properties": { "upload": {
                    "type": "object",
                    "properties": { "parts": {
                        "type": "array",
                        "items": { "type": "object", "properties": { "objectKey": { "type": "string" } } }
                    } }
                } }
            })),
            "responses": {}
        } } }),
        json!({}),
    );
    assert_eq!(
        storage_key_violations(&document),
        ["POST /x request body application/json.upload.parts exposes request field \"objectKey\""]
    );
}

#[test]
fn unit_storage_key_walker_follows_component_references() {
    for field in FORBIDDEN_REQUEST_FIELDS {
        let document = document_with(
            json!({ "/x": { "put": {
                "requestBody": json_body(json!({ "allOf": [
                    { "$ref": "#/components/schemas/Outer" }
                ] })),
                "responses": {}
            } } }),
            json!({
                "Outer": { "type": "object", "properties": {
                    "target": { "oneOf": [ { "$ref": "#/components/schemas/Inner" }, { "type": "null" } ] }
                } },
                "Inner": { "type": "object", "properties": { field: { "type": "string" } } },
            }),
        );
        let violations = storage_key_violations(&document);
        assert_eq!(violations.len(), 1, "{field}: {violations:?}");
        assert!(
            violations[0].contains("#/components/schemas/Inner"),
            "{field}"
        );
        assert!(violations[0].ends_with(&format!("exposes request field {field:?}")));
    }
}

#[test]
fn unit_storage_key_walker_ignores_response_only_and_prose_occurrences() {
    let document = document_with(
        json!({ "/x": { "post": {
            "description": "never send objectKey or storageKey",
            "requestBody": json_body(json!({
                "type": "object",
                "description": "the key is chosen by the server",
                "properties": {
                    "name": { "type": "string", "examples": ["key"] },
                    "keyHint": { "type": "string" }
                }
            })),
            "responses": { "200": json_body(json!({ "$ref": "#/components/schemas/Stored" })) }
        } } }),
        json!({
            "Stored": { "type": "object", "properties": {
                "objectKey": { "type": "string" },
                "key": { "type": "string" }
            } },
        }),
    );
    assert_eq!(storage_key_violations(&document), Vec::<String>::new());
}

#[test]
fn unit_storage_key_walker_terminates_on_recursive_schemas() {
    let document = document_with(
        json!({ "/x": { "post": {
            "requestBody": json_body(json!({ "$ref": "#/components/schemas/Node" })),
            "responses": {}
        } } }),
        json!({
            "Node": { "type": "object", "properties": {
                "children": { "type": "array", "items": { "$ref": "#/components/schemas/Node" } },
                "storageKey": { "type": "string" }
            } },
        }),
    );
    assert_eq!(storage_key_violations(&document).len(), 1);
}

fn template_variables(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| name.trim_start_matches('*'))
        .collect()
}

fn in_namespace(path: &str, namespace: &str) -> bool {
    path.get(..namespace.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(namespace))
        && matches!(path.as_bytes().get(namespace.len()), None | Some(b'/'))
}

fn storage_primitive_violations(inventory: &RouteInventory, document: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    for entry in inventory.entries() {
        let route = format!(
            "{} {} ({})",
            entry.method(),
            entry.path(),
            entry.policy().auth()
        );
        for namespace in FORBIDDEN_NAMESPACES {
            if in_namespace(entry.path(), namespace) {
                violations.push(format!(
                    "{route} is a generic storage primitive under {namespace}"
                ));
            }
        }
        let segments: Vec<&str> = entry.path().split('/').collect();
        if let Some(index) = segments.iter().position(|segment| {
            STORAGE_CAPABILITY_SEGMENTS
                .iter()
                .any(|capability| segment.eq_ignore_ascii_case(capability))
        }) {
            let scoped = segments[index] == "s3"
                && index >= TRANSFER_ITEM_SCOPE.len()
                && segments[index - TRANSFER_ITEM_SCOPE.len()..index] == TRANSFER_ITEM_SCOPE;
            if !scoped {
                violations.push(format!(
                    "{route} exposes a storage capability outside a transfer item"
                ));
            }
            if !CAPABILITY_CLASSES.contains(&entry.policy().auth()) {
                violations.push(format!(
                    "{route} exposes a storage capability without an authorizing grant"
                ));
            }
        }
        if in_namespace(entry.path(), ADMIN_STORAGE_NAMESPACE)
            && !ADMIN_STORAGE_ROUTES.contains(&entry.path())
        {
            violations.push(format!(
                "{route} is an undocumented admin storage primitive"
            ));
        }
        for variable in template_variables(entry.path()) {
            if FORBIDDEN_STORAGE_INPUTS.contains(&variable) {
                violations.push(format!("{route} addresses storage by {variable:?}"));
            }
        }
        let operation =
            &document["paths"][entry.path()][entry.method().as_str().to_ascii_lowercase()];
        for parameter in operation["parameters"].as_array().into_iter().flatten() {
            let name = parameter["name"].as_str().unwrap_or_default();
            if FORBIDDEN_STORAGE_INPUTS.contains(&name) {
                violations.push(format!("{route} accepts storage input {name:?}"));
            }
        }
    }
    violations
}

#[test]
#[expect(
    non_snake_case,
    reason = "regression test names keep the upper-case catalogue identifier"
)]
fn regression_R032_no_unauthenticated_storage_primitives() {
    let inventory = application_inventory();
    let document = application_document();
    assert_eq!(
        storage_primitive_violations(&inventory, &document),
        Vec::<String>::new()
    );
    let ungranted: Vec<&str> = inventory
        .entries()
        .iter()
        .filter(|entry| matches!(entry.policy().auth(), AuthClass::Public | AuthClass::Setup))
        .map(|entry| entry.path())
        .collect();
    assert!(!ungranted.is_empty());
    for path in ungranted {
        assert!(!path.to_ascii_lowercase().contains("presign"), "{path}");
    }

    let (planted, planted_document) = inventory_and_document(
        Routes::new()
            .route(
                policy(AuthClass::Authenticated),
                routes!(transfer_item_multipart),
            )
            .route(policy(AuthClass::Public), routes!(ungranted_item_presign))
            .route(
                policy(AuthClass::Authenticated),
                routes!(unscoped_multipart_create),
            )
            .route(policy(AuthClass::Admin), routes!(admin_storage_object))
            .route(policy(AuthClass::Admin), routes!(admin_storage_self_test)),
    );
    assert_eq!(
        storage_primitive_violations(&planted, &planted_document),
        [
            "DELETE /api/v1/admin/storage/objects/{objectId} (admin) is an undocumented admin storage primitive",
            "POST /api/v1/public/transfers/sessions/{sid}/files/{itemId}/s3/multipart/presign (public) exposes a storage capability without an authorizing grant",
            "POST /api/v1/uploads/multipart (authenticated) exposes a storage capability outside a transfer item",
        ]
    );

    let storage_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage");
    for (name, source) in [
        ("mod.rs", include_str!("../storage/mod.rs")),
        ("provider.rs", include_str!("../storage/provider.rs")),
        ("caps.rs", include_str!("../storage/caps.rs")),
        ("key.rs", include_str!("../storage/key.rs")),
        ("error.rs", include_str!("../storage/error.rs")),
        ("health.rs", include_str!("../storage/health.rs")),
    ] {
        assert!(storage_dir.join(name).is_file(), "{name}");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for token in HTTP_SURFACE_TOKENS {
            assert!(!production.contains(token), "storage/{name} names {token}");
        }
    }

    let provider = include_str!("../storage/provider.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(provider.contains("\n\npub struct GrantContext {\n    _sealed: (),\n}\n"));
    assert!(!provider.contains("impl GrantContext"));
    assert!(!provider.contains("for GrantContext {\n    fn default"));
    assert_eq!(provider.matches("for GrantContext").count(), 1);
    assert!(provider.contains("impl fmt::Debug for GrantContext"));
    let presign_methods: Vec<&str> = provider
        .split("async fn presign_")
        .skip(1)
        .map(|signature| signature.split(';').next().unwrap())
        .collect();
    assert_eq!(presign_methods.len(), 2);
    for signature in presign_methods {
        assert!(
            signature.contains("authorized: &GrantContext,"),
            "{signature}"
        );
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart",
    params(("sid" = String, Path), ("itemId" = String, Path)),
    responses((status = 201))
)]
async fn transfer_item_multipart() -> http::StatusCode {
    http::StatusCode::CREATED
}

#[utoipa::path(
    post,
    path = "/api/v1/public/transfers/sessions/{sid}/files/{itemId}/s3/multipart/presign",
    params(("sid" = String, Path), ("itemId" = String, Path)),
    responses((status = 200))
)]
async fn ungranted_item_presign() -> http::StatusCode {
    http::StatusCode::OK
}

#[utoipa::path(post, path = "/api/v1/uploads/multipart", responses((status = 201)))]
async fn unscoped_multipart_create() -> http::StatusCode {
    http::StatusCode::CREATED
}

#[utoipa::path(
    delete,
    path = "/api/v1/admin/storage/objects/{objectId}",
    params(("objectId" = String, Path)),
    responses((status = 204))
)]
async fn admin_storage_object() -> http::StatusCode {
    http::StatusCode::NO_CONTENT
}

#[utoipa::path(post, path = "/api/v1/admin/storage/self-test", responses((status = 200)))]
async fn admin_storage_self_test() -> http::StatusCode {
    http::StatusCode::OK
}

#[utoipa::path(put, path = "/s3/{key}", params(("key" = String, Path)), responses((status = 200)))]
async fn v3_style_presign() -> http::StatusCode {
    http::StatusCode::OK
}

#[utoipa::path(get, path = "/api/v1/storage/objects", responses((status = 200)))]
async fn generic_storage() -> http::StatusCode {
    http::StatusCode::OK
}

#[utoipa::path(
    get,
    path = "/api/v1/public/uploads/presign",
    params(("objectName" = String, Query), ("bucket" = String, Query)),
    responses((status = 200))
)]
async fn presign_by_key() -> http::StatusCode {
    http::StatusCode::OK
}

fn inventory_and_document(routes: Routes<()>) -> (RouteInventory, Value) {
    let assembled = routes.build().unwrap();
    let document = serde_json::to_value(&assembled.openapi).unwrap();
    (assembled.inventory, document)
}

#[test]
fn unit_storage_primitive_gate_detects_v3_shapes() {
    let (inventory, document) = inventory_and_document(
        Routes::new()
            .route(policy(AuthClass::Public), routes!(v3_style_presign))
            .route(policy(AuthClass::PublicGrant), routes!(generic_storage))
            .route(policy(AuthClass::Public), routes!(presign_by_key)),
    );
    assert_eq!(
        storage_primitive_violations(&inventory, &document),
        [
            "GET /api/v1/public/uploads/presign (public) exposes a storage capability outside a transfer item",
            "GET /api/v1/public/uploads/presign (public) exposes a storage capability without an authorizing grant",
            "GET /api/v1/public/uploads/presign (public) accepts storage input \"objectName\"",
            "GET /api/v1/public/uploads/presign (public) accepts storage input \"bucket\"",
            "GET /api/v1/storage/objects (public+grant) is a generic storage primitive under /api/v1/storage",
            "PUT /s3/{key} (public) is a generic storage primitive under /s3",
            "PUT /s3/{key} (public) exposes a storage capability outside a transfer item",
            "PUT /s3/{key} (public) exposes a storage capability without an authorizing grant",
            "PUT /s3/{key} (public) addresses storage by \"key\"",
            "PUT /s3/{key} (public) accepts storage input \"key\"",
        ]
    );
}

#[test]
fn unit_storage_primitive_gate_allows_the_admin_storage_status_route() {
    let (inventory, document) = inventory_and_document(
        Routes::new().route(policy(AuthClass::Admin), routes!(admin_storage_status)),
    );
    assert_eq!(
        storage_primitive_violations(&inventory, &document),
        Vec::<String>::new()
    );
    assert!(!in_namespace("/api/v1/storageinfo", "/api/v1/storage"));
    assert!(in_namespace("/S3/anything", "/s3"));
}

const AUTHORIZATION_DIRECTORIES: [&str; 5] = [
    "app",
    "infra/http",
    "infra/ratelimit",
    "features/auth/sessions",
    "features/setup",
];

const AUTHORIZATION_MARKERS: [&str; 4] = [
    "AuthClass",
    "enforce_class",
    "AuthenticatedPrincipal",
    "extractors::",
];

const REQUIRED_AUTHORIZATION_FILES: [&str; 8] = [
    "app/auth_class.rs",
    "app/router.rs",
    "infra/http/extractors.rs",
    "infra/http/csrf.rs",
    "features/auth/sessions/service.rs",
    "features/users/admin_routes.rs",
    "features/users/admin_service.rs",
    "features/identity_providers/routes.rs",
];

const SOURCE_SCAN_CAP: u64 = 1024 * 1024;

fn is_test_source(path: &std::path::Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || path
            .components()
            .any(|part| matches!(part.as_os_str().to_str(), Some("tests" | "flow_tests")))
}

fn authorization_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root.clone()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") || is_test_source(&path) {
                continue;
            }
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mut source = String::new();
            std::io::Read::read_to_string(
                &mut std::io::Read::take(std::fs::File::open(&path).unwrap(), SOURCE_SCAN_CAP),
                &mut source,
            )
            .unwrap();
            let in_scope = AUTHORIZATION_DIRECTORIES
                .iter()
                .any(|directory| relative.starts_with(&format!("{directory}/")))
                || AUTHORIZATION_MARKERS
                    .iter()
                    .any(|marker| source.contains(marker));
            if in_scope {
                sources.push((relative, source));
            }
        }
    }
    sources.sort();
    sources
}

const USER_COUNT_TOKENS: [&str; 14] = [
    "count(*) from users",
    "count(1) from users",
    "count(id) from users",
    "from users limit",
    "count_active_admins",
    "count_users",
    "users_count",
    "user_count",
    "usercount",
    "first_user",
    "only_user",
    "single_user",
    "zero_users",
    "users.len()",
];

fn user_count_violations<N: AsRef<str>, S: AsRef<str>>(sources: &[(N, S)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (name, source) in sources {
        let (name, source) = (name.as_ref(), source.as_ref());
        let production = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for token in USER_COUNT_TOKENS {
            if production.contains(token) {
                violations.push(format!("{name} reads a user count via {token:?}"));
            }
        }
    }
    violations
}

#[test]
fn svc_no_user_count_in_authorization() {
    let sources = authorization_sources();
    let names: Vec<&str> = sources.iter().map(|(name, _)| name.as_str()).collect();
    for required in REQUIRED_AUTHORIZATION_FILES {
        assert!(names.contains(&required), "{required} is not scanned");
    }
    for (name, source) in &sources {
        if AUTHORIZATION_MARKERS
            .iter()
            .any(|marker| source.contains(marker))
        {
            assert!(names.contains(&name.as_str()));
        }
    }
    assert!(sources.len() >= 30, "{}", sources.len());
    assert_eq!(user_count_violations(&sources), Vec::<String>::new());
}

#[test]
fn unit_user_count_gate_detects_count_based_relaxation() {
    let planted = [
        (
            "planted/admin_pre_validation.rs",
            "let users = query_scalar(\"SELECT COUNT(*)  FROM users\");\nif users <= 1 { return next.run(request).await; }",
        ),
        ("planted/first.rs", "if is_first_user { skip_auth(); }"),
        ("planted/clean.rs", "let role = principal.role;"),
    ];
    assert_eq!(
        user_count_violations(&planted),
        [
            "planted/admin_pre_validation.rs reads a user count via \"count(*) from users\"",
            "planted/first.rs reads a user count via \"first_user\"",
        ]
    );
}

#[test]
fn unit_admin_namespace_is_admin_classed_and_never_public() {
    let inventory = application_inventory();
    let mut admin_routes = 0;
    for entry in inventory.entries() {
        if entry.path().starts_with("/api/v1/admin/") || entry.path() == "/api/v1/admin" {
            assert!(
                matches!(
                    entry.policy().auth(),
                    AuthClass::Admin | AuthClass::AdminRecentAuth
                ),
                "{} {} is {}",
                entry.method(),
                entry.path(),
                entry.policy().auth()
            );
            admin_routes += 1;
        }
    }
    assert!(admin_routes >= 26);
    assert!(!PUBLIC_ROUTES_GOLDEN.contains("/api/v1/admin"));
}

#[test]
fn unit_admin_user_read_routes_are_declared_admin_and_read_limited() {
    let inventory = application_inventory();
    for path in [
        "/api/v1/admin/users",
        "/api/v1/admin/users/{id}",
        "/api/v1/admin/users/{userId}/sessions",
    ] {
        let matching: Vec<_> = inventory
            .entries()
            .iter()
            .filter(|entry| entry.path() == path && *entry.method() == Method::GET)
            .collect();
        assert_eq!(matching.len(), 1, "{path}");
        let entry = matching[0];
        assert_eq!(entry.policy().auth(), AuthClass::Admin, "{path}");
        assert_eq!(entry.policy().rate_limit(), RateLimitClass::Read, "{path}");
        assert_eq!(entry.policy().transport(), Transport::ControlPlane);
        assert_eq!(
            entry.policy().idempotency(),
            IdempotencyMode::None,
            "{path}"
        );
    }
}

#[test]
fn unit_admin_user_write_routes_are_declared_admin_write_limited() {
    let inventory = application_inventory();
    for (method, path, auth, idempotency) in [
        (
            Method::POST,
            "/api/v1/admin/users",
            AuthClass::Admin,
            IdempotencyMode::Plaintext,
        ),
        (
            Method::PATCH,
            "/api/v1/admin/users/{id}",
            AuthClass::Admin,
            IdempotencyMode::None,
        ),
        (
            Method::PUT,
            "/api/v1/admin/users/{id}/role",
            AuthClass::AdminRecentAuth,
            IdempotencyMode::None,
        ),
        (
            Method::POST,
            "/api/v1/admin/users/{id}/activate",
            AuthClass::Admin,
            IdempotencyMode::None,
        ),
        (
            Method::POST,
            "/api/v1/admin/users/{id}/deactivate",
            AuthClass::AdminRecentAuth,
            IdempotencyMode::None,
        ),
        (
            Method::POST,
            "/api/v1/admin/users/{id}/password-reset",
            AuthClass::AdminRecentAuth,
            IdempotencyMode::None,
        ),
        (
            Method::POST,
            "/api/v1/admin/users/{id}/unlock",
            AuthClass::Admin,
            IdempotencyMode::None,
        ),
        (
            Method::DELETE,
            "/api/v1/admin/users/{userId}/sessions",
            AuthClass::AdminRecentAuth,
            IdempotencyMode::None,
        ),
        (
            Method::PUT,
            "/api/v1/admin/users/{id}/quota",
            AuthClass::Admin,
            IdempotencyMode::None,
        ),
    ] {
        let matching: Vec<_> = inventory
            .entries()
            .iter()
            .filter(|entry| entry.path() == path && *entry.method() == method)
            .collect();
        assert_eq!(matching.len(), 1, "{method} {path}");
        let entry = matching[0];
        assert_eq!(entry.policy().auth(), auth, "{method} {path}");
        assert_eq!(
            entry.policy().rate_limit(),
            RateLimitClass::AdminWrite,
            "{method} {path}"
        );
        assert_eq!(entry.policy().transport(), Transport::ControlPlane);
        assert_eq!(entry.policy().idempotency(), idempotency, "{method} {path}");
    }
}

#[test]
fn unit_admin_provider_routes_are_declared_with_their_classes_and_limits() {
    let inventory = application_inventory();
    let routes = [
        (
            Method::GET,
            "/api/v1/admin/providers",
            AuthClass::Admin,
            RateLimitClass::Read,
        ),
        (
            Method::POST,
            "/api/v1/admin/providers",
            AuthClass::AdminRecentAuth,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::PATCH,
            "/api/v1/admin/providers/{id}",
            AuthClass::AdminRecentAuth,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::DELETE,
            "/api/v1/admin/providers/{id}",
            AuthClass::AdminRecentAuth,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::PUT,
            "/api/v1/admin/providers/order",
            AuthClass::Admin,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::POST,
            "/api/v1/admin/providers/discover",
            AuthClass::Admin,
            RateLimitClass::ProviderTest,
        ),
        (
            Method::POST,
            "/api/v1/admin/providers/{id}/test",
            AuthClass::Admin,
            RateLimitClass::ProviderTest,
        ),
        (
            Method::GET,
            "/api/v1/admin/providers/presets",
            AuthClass::Admin,
            RateLimitClass::Read,
        ),
    ];
    let declared = inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with("/api/v1/admin/providers"))
        .count();
    assert_eq!(declared, routes.len());
    for (method, path, auth, rate_limit) in routes {
        let matching: Vec<_> = inventory
            .entries()
            .iter()
            .filter(|entry| entry.path() == path && *entry.method() == method)
            .collect();
        assert_eq!(matching.len(), 1, "{method} {path}");
        let policy = matching[0].policy();
        assert_eq!(policy.auth(), auth, "{method} {path}");
        assert_eq!(policy.rate_limit(), rate_limit, "{method} {path}");
        assert_eq!(policy.transport(), Transport::ControlPlane);
        assert_eq!(policy.idempotency(), IdempotencyMode::None);
    }
}

#[test]
fn it_openapi_admin_provider_routes_never_expose_secrets_or_a_writable_redirect() {
    let document = application_document();
    let schemas = &document["components"]["schemas"];
    let item = &schemas["ProviderItem"]["properties"];
    for member in [
        "clientSecretConfigured",
        "allowEmailLinking",
        "autoProvision",
        "redirectUri",
        "linkedUserCount",
        "validatedAt",
        "preset",
        "tokenAuthMethod",
    ] {
        assert!(item.get(member).is_some(), "ProviderItem.{member}");
    }
    for forbidden in [
        "clientSecret",
        "clientSecretCiphertext",
        "clientSecretNonce",
        "keyVersion",
        "adminEmailDomains",
        "autoProvisionRole",
        "defaultRole",
    ] {
        for schema in [
            "ProviderItem",
            "PresetItem",
            "Discovered",
            "ProviderTestResult",
        ] {
            assert!(
                schemas[schema]["properties"].get(forbidden).is_none(),
                "{schema}.{forbidden}"
            );
        }
        for schema in ["CreateProviderRequest", "UpdateProviderRequest"] {
            if forbidden != "clientSecret" {
                assert!(
                    schemas[schema]["properties"].get(forbidden).is_none(),
                    "{schema}.{forbidden}"
                );
            }
        }
    }
    for schema in ["CreateProviderRequest", "UpdateProviderRequest"] {
        let properties = &schemas[schema]["properties"];
        assert!(
            properties.get("redirectUri").is_none(),
            "{schema}.redirectUri"
        );
        assert_eq!(
            properties["clientSecret"]["writeOnly"],
            json!(true),
            "{schema}"
        );
    }
    assert!(schemas["UpdateProviderRequest"]["properties"]
        .get("slug")
        .is_none());
    assert!(schemas["CreateProviderRequest"]["properties"]
        .get("slug")
        .is_some());
    assert_eq!(
        schemas["ProviderItem"]["properties"]["redirectUri"]["readOnly"],
        json!(true)
    );
    let delete = &document["paths"]["/api/v1/admin/providers/{id}"]["delete"]["responses"];
    assert!(delete["409"]["description"]
        .as_str()
        .unwrap()
        .contains("PROVIDER_HAS_LINKS"));
    assert!(delete["404"]["description"]
        .as_str()
        .unwrap()
        .contains("PROVIDER_NOT_FOUND"));
}

#[test]
fn unit_admin_email_change_routes_are_declared_with_their_classes() {
    let inventory = application_inventory();
    for (method, path, auth, rate_limit) in [
        (
            Method::POST,
            "/api/v1/admin/users/{id}/email",
            AuthClass::AdminRecentAuth,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::POST,
            "/api/v1/admin/users/{id}/email/resend",
            AuthClass::AdminRecentAuth,
            RateLimitClass::EmailTest,
        ),
        (
            Method::DELETE,
            "/api/v1/admin/users/{id}/email",
            AuthClass::AdminRecentAuth,
            RateLimitClass::AdminWrite,
        ),
        (
            Method::POST,
            "/api/v1/auth/email/verify",
            AuthClass::Public,
            RateLimitClass::AuthToken,
        ),
    ] {
        let matching: Vec<_> = inventory
            .entries()
            .iter()
            .filter(|entry| entry.path() == path && *entry.method() == method)
            .collect();
        assert_eq!(matching.len(), 1, "{method} {path}");
        let entry = matching[0];
        assert_eq!(entry.policy().auth(), auth, "{method} {path}");
        assert_eq!(entry.policy().rate_limit(), rate_limit, "{method} {path}");
        assert_eq!(entry.policy().transport(), Transport::ControlPlane);
        assert_eq!(
            entry.policy().idempotency(),
            IdempotencyMode::None,
            "{method} {path}"
        );
    }
}

#[test]
fn it_openapi_admin_email_change_routes_declare_typed_contracts() {
    let document = application_document();
    let paths = &document["paths"];
    let start = &paths["/api/v1/admin/users/{id}/email"]["post"];
    let cancel = &paths["/api/v1/admin/users/{id}/email"]["delete"];
    let resend = &paths["/api/v1/admin/users/{id}/email/resend"]["post"];
    let verify = &paths["/api/v1/auth/email/verify"]["post"];

    for (operation, class, success) in [
        (start, "admin+recent-auth", "202"),
        (resend, "admin+recent-auth", "202"),
        (cancel, "admin+recent-auth", "204"),
    ] {
        assert!(operation["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == class));
        assert_eq!(
            operation["security"],
            json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }])
        );
        for status in [success, "401", "403", "404", "429"] {
            assert!(operation["responses"][status].is_object(), "{status}");
        }
        assert_eq!(parameter(operation, "id")["in"], "path");
        assert!(operation["responses"]["404"]["description"]
            .as_str()
            .unwrap()
            .contains("USER_NOT_FOUND"));
        assert!(operation["responses"][success]["content"].is_null());
    }
    assert_eq!(
        start["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ChangeEmailRequest"
    );
    assert!(resend["requestBody"].is_null());
    assert!(cancel["requestBody"].is_null());
    let conflict = start["responses"]["409"]["description"].as_str().unwrap();
    assert!(conflict.contains("USER_EMAIL_TAKEN") && conflict.contains("FEATURE_UNAVAILABLE_SMTP"));
    assert!(resend["responses"]["409"]["description"]
        .as_str()
        .unwrap()
        .contains("EMAIL_VERIFICATION_NOT_PENDING"));

    assert_eq!(
        verify["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/VerifyEmailRequest"
    );
    assert!(verify["security"].is_null() || verify["security"] == json!([]));
    assert!(verify["responses"]["204"]["content"].is_null());
    assert!(verify["responses"]["400"]["description"]
        .as_str()
        .unwrap()
        .contains("EMAIL_VERIFICATION_TOKEN_INVALID"));
    assert!(verify["responses"]["410"]["description"]
        .as_str()
        .unwrap()
        .contains("EMAIL_VERIFICATION_TOKEN_EXPIRED"));
    assert!(verify["responses"]["409"]["description"]
        .as_str()
        .unwrap()
        .contains("USER_EMAIL_TAKEN"));

    let schemas = &document["components"]["schemas"];
    for name in ["ChangeEmailRequest", "VerifyEmailRequest"] {
        assert_eq!(schemas[name]["additionalProperties"], false, "{name}");
    }
    assert_eq!(schemas["ChangeEmailRequest"]["required"], json!(["email"]));
    assert_eq!(schemas["VerifyEmailRequest"]["required"], json!(["token"]));
}

#[test]
fn unit_admin_settings_routes_are_declared_with_their_classes() {
    let inventory = application_inventory();
    for group in ["general", "security", "quotas", "public-links", "smtp"] {
        let path = format!("/api/v1/admin/settings/{group}");
        let write_class = if matches!(group, "security" | "smtp") {
            AuthClass::AdminRecentAuth
        } else {
            AuthClass::Admin
        };
        for (method, auth, rate_limit) in [
            (Method::GET, AuthClass::Admin, RateLimitClass::Read),
            (Method::PATCH, write_class, RateLimitClass::AdminWrite),
        ] {
            let matching: Vec<_> = inventory
                .entries()
                .iter()
                .filter(|entry| entry.path() == path && *entry.method() == method)
                .collect();
            assert_eq!(matching.len(), 1, "{method} {path}");
            let policy = matching[0].policy();
            assert_eq!(policy.auth(), auth, "{method} {path}");
            assert_eq!(policy.rate_limit(), rate_limit, "{method} {path}");
            assert_eq!(policy.transport(), Transport::ControlPlane);
            assert_eq!(policy.idempotency(), IdempotencyMode::None);
        }
    }
    let aggregate: Vec<_> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.path() == "/api/v1/admin/settings")
        .collect();
    assert_eq!(aggregate.len(), 1);
    assert_eq!(*aggregate[0].method(), Method::GET);
    assert_eq!(aggregate[0].policy().auth(), AuthClass::Admin);
    assert_eq!(aggregate[0].policy().rate_limit(), RateLimitClass::Read);

    let test: Vec<_> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.path() == "/api/v1/admin/settings/smtp/test")
        .collect();
    assert_eq!(test.len(), 1);
    assert_eq!(*test[0].method(), Method::POST);
    assert_eq!(test[0].policy().auth(), AuthClass::Admin);
    assert_eq!(test[0].policy().rate_limit(), RateLimitClass::EmailTest);
    assert_eq!(test[0].policy().transport(), Transport::ControlPlane);
    assert_eq!(test[0].policy().idempotency(), IdempotencyMode::None);
}

#[test]
fn it_openapi_admin_settings_routes_declare_typed_contracts() {
    let document = application_document();
    let paths = &document["paths"];
    let schemas = &document["components"]["schemas"];
    let security = json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }]);

    for (group, patch_schema, view_schema, class) in [
        ("general", "GeneralPatch", "GeneralSettings", "admin"),
        (
            "security",
            "SecurityPatch",
            "SecuritySettings",
            "admin+recent-auth",
        ),
        ("quotas", "QuotaPatch", "QuotaSettings", "admin"),
        (
            "public-links",
            "PublicLinkPatch",
            "PublicLinkSettings",
            "admin",
        ),
        ("smtp", "SmtpPatch", "SmtpSettings", "admin+recent-auth"),
    ] {
        let item = &paths[format!("/api/v1/admin/settings/{group}")];
        let read = &item["get"];
        let write = &item["patch"];
        assert_eq!(read["security"], json!([{ "palmrSession": [] }]), "{group}");
        assert_eq!(write["security"], security, "{group}");
        for operation in [read, write] {
            assert!(operation["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tag| tag == "admin-settings"));
        }
        assert!(read["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == "admin"));
        assert!(
            write["tags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tag| tag == class),
            "{group}"
        );
        assert_eq!(
            write["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            format!("#/components/schemas/{patch_schema}")
        );
        for operation in [read, write] {
            assert_eq!(
                operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
                format!("#/components/schemas/{view_schema}"),
                "{group}"
            );
            for status in ["401", "403", "429"] {
                assert!(
                    operation["responses"][status].is_object(),
                    "{group} {status}"
                );
            }
        }
        for status in ["400", "415", "422"] {
            assert!(write["responses"][status].is_object(), "{group} {status}");
        }
        let unprocessable = write["responses"]["422"]["description"].as_str().unwrap();
        assert!(unprocessable.contains("SETTING_UNKNOWN"), "{group}");
        assert!(unprocessable.contains("SETTING_VALUE_INVALID"), "{group}");
        assert_eq!(
            schemas[patch_schema]["additionalProperties"], false,
            "{group}"
        );
    }
    let forbidden = paths["/api/v1/admin/settings/security"]["patch"]["responses"]["403"]
        ["description"]
        .as_str()
        .unwrap();
    assert!(forbidden.contains("AUTH_RECENT_AUTH_REQUIRED"));
    for group in ["security", "quotas", "public-links"] {
        let description = paths[format!("/api/v1/admin/settings/{group}")]["patch"]["responses"]
            ["422"]["description"]
            .as_str()
            .unwrap();
        assert!(
            description.contains("SETTING_BELOW_FLOOR") && description.contains("details.floor")
        );
    }

    let aggregate = &paths["/api/v1/admin/settings"]["get"];
    assert_eq!(
        aggregate["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/AdminSettings"
    );
    assert!(paths["/api/v1/admin/settings"]["patch"].is_null());
    let properties = schemas["AdminSettings"]["properties"].as_object().unwrap();
    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["general", "public-links", "quotas", "security", "smtp"]
    );

    let general: Vec<&str> = schemas["GeneralPatch"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for name in [
        "appName",
        "appDescription",
        "defaultLocale",
        "hideVersion",
        "poweredByVisible",
        "thumbnailSourceLimit",
    ] {
        assert!(general.contains(&name), "{name}");
    }
    assert_eq!(general.len(), 6);
    assert_eq!(
        schemas["SecurityPatch"]["properties"]
            .as_object()
            .unwrap()
            .len(),
        12
    );
    assert_eq!(
        schemas["QuotaPatch"]["properties"]["defaultUserQuotaBytes"]["minimum"],
        0
    );
    assert_eq!(
        schemas["GeneralPatch"]["properties"]["appName"]["type"],
        "string"
    );
    assert_eq!(
        schemas["SecurityPatch"]["properties"]["twoFactorRequired"]["type"],
        "boolean"
    );
    assert_eq!(
        schemas["SecurityPatch"]["properties"]["passwordMinLength"]["type"],
        "integer"
    );
    for (schema, member) in [
        ("QuotaPatch", "defaultUserQuotaBytes"),
        ("QuotaPatch", "maxFileSizeBytes"),
        ("PublicLinkPatch", "maxPublicLinkLifetimeDays"),
    ] {
        let types = schemas[schema]["properties"][member]["type"]
            .as_array()
            .unwrap();
        assert!(types.iter().any(|kind| kind == "null"), "{schema}.{member}");
    }
    for schema in ["GeneralPatch", "SecurityPatch"] {
        assert!(schemas[schema]["required"].is_null(), "{schema}");
    }
    assert!(
        schemas["PublicLinkPatch"]["properties"]["maxPublicLinkLifetimeDays"]["maximum"].is_null()
    );
    for member in [
        "passwordMinLength",
        "publicLinkPasswordMinLength",
        "maxLoginAttempts",
        "loginLockoutMinutes",
    ] {
        assert!(
            schemas["SecurityPatch"]["properties"][member]["maximum"].is_null(),
            "{member}"
        );
    }
    for schema in [
        "GeneralPatch",
        "SecurityPatch",
        "QuotaPatch",
        "PublicLinkPatch",
    ] {
        for name in schemas[schema]["properties"].as_object().unwrap().keys() {
            let lowered = name.to_lowercase();
            for operator in [
                "port",
                "bind",
                "baseurl",
                "proxy",
                "storageprovider",
                "s3",
                "datadir",
            ] {
                assert!(!lowered.contains(operator), "{schema}.{name}");
            }
        }
    }
}

#[test]
fn it_openapi_admin_smtp_group_never_declares_a_readable_password() {
    let document = application_document();
    let schemas = &document["components"]["schemas"];
    let settings = schemas["SmtpSettings"]["properties"].as_object().unwrap();
    assert!(settings.contains_key("passwordConfigured"));
    assert!(!settings.contains_key("password"));
    let mut names: Vec<&str> = settings.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "allowSelfSignedCertificate",
            "enabled",
            "fromEmail",
            "fromName",
            "host",
            "noAuth",
            "passwordConfigured",
            "port",
            "security",
            "username",
        ]
    );
    assert_eq!(
        schemas["SmtpSecurity"]["enum"],
        json!(["starttls", "implicit", "none"])
    );
    let patch = &schemas["SmtpPatch"]["properties"];
    assert_eq!(patch["password"]["writeOnly"], true);
    let types = patch["password"]["type"].as_array().unwrap();
    assert!(types.iter().any(|kind| kind == "null"));
    for member in ["host", "username", "fromName", "fromEmail"] {
        let types = patch[member]["type"].as_array().unwrap();
        assert!(types.iter().any(|kind| kind == "null"), "{member}");
    }
    for member in [
        "enabled",
        "port",
        "security",
        "allowSelfSignedCertificate",
        "noAuth",
    ] {
        let nullable = patch[member]["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|kind| kind == "null"));
        assert!(!nullable, "{member}");
    }
    assert_eq!(patch["port"]["minimum"], 1);
    assert_eq!(patch["port"]["maximum"], 65535);
    assert_eq!(schemas["SmtpPatch"]["additionalProperties"], false);
    assert!(schemas["SmtpPatch"]["required"].is_null());
    for schema in schemas.as_object().unwrap().values() {
        let text = schema.to_string();
        assert!(!text.contains("\"example\":\"palmr-smtp"), "{text}");
    }
}

#[test]
fn it_openapi_admin_smtp_test_route_declares_typed_contract() {
    let document = application_document();
    let schemas = &document["components"]["schemas"];
    let operation = &document["paths"]["/api/v1/admin/settings/smtp/test"]["post"];
    assert_eq!(
        operation["security"],
        json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }])
    );
    let tags: Vec<&str> = operation["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(tags.contains(&"admin") && tags.contains(&"admin-settings"));
    assert!(!tags.contains(&"admin+recent-auth"));
    assert_eq!(
        operation["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/SmtpTestRequest"
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/SmtpTestResult"
    );
    for status in ["400", "401", "403", "415", "422", "429", "502"] {
        assert!(operation["responses"][status].is_object(), "{status}");
    }
    assert!(operation["responses"]["502"]["description"]
        .as_str()
        .unwrap()
        .contains("SMTP_TEST_FAILED"));
    assert_eq!(schemas["SmtpTestRequest"]["required"], json!(["to"]));
    assert_eq!(schemas["SmtpTestRequest"]["additionalProperties"], false);
    assert_eq!(
        schemas["SmtpUnsavedSettings"]["properties"]["password"]["writeOnly"],
        true
    );
    assert_eq!(
        schemas["SmtpTestStageName"]["enum"],
        json!(["connect", "starttls", "auth", "send"])
    );
    assert_eq!(
        schemas["SmtpTestResult"]["required"],
        json!(["ok", "stages", "durationMs"])
    );
}

fn parameter<'a>(operation: &'a Value, name: &str) -> &'a Value {
    operation["parameters"]
        .as_array()
        .and_then(|parameters| {
            parameters
                .iter()
                .find(|parameter| parameter["name"] == name)
        })
        .unwrap_or_else(|| panic!("parameter {name} is not declared"))
}

#[test]
fn it_openapi_admin_user_lifecycle_routes_declare_typed_contracts() {
    let document = application_document();
    let paths = &document["paths"];
    let role = &paths["/api/v1/admin/users/{id}/role"]["put"];
    let activate = &paths["/api/v1/admin/users/{id}/activate"]["post"];
    let deactivate = &paths["/api/v1/admin/users/{id}/deactivate"]["post"];

    for (operation, class) in [
        (role, "admin+recent-auth"),
        (activate, "admin"),
        (deactivate, "admin+recent-auth"),
    ] {
        assert!(operation["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == class));
        assert_eq!(
            operation["security"],
            json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }])
        );
        for status in ["200", "401", "403", "404", "429"] {
            assert!(operation["responses"][status].is_object(), "{status}");
        }
        assert_eq!(parameter(operation, "id")["in"], "path");
        assert!(operation["responses"]["404"]["description"]
            .as_str()
            .unwrap()
            .contains("USER_NOT_FOUND"));
        assert_eq!(
            operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/AdminUserItem"
        );
    }
    for operation in [role, deactivate] {
        assert!(operation["responses"]["409"]["description"]
            .as_str()
            .unwrap()
            .contains("LAST_ADMIN_PROTECTED"));
    }
    assert!(activate["responses"]["409"].is_null());
    assert_eq!(
        role["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ChangeRoleRequest"
    );
    assert!(activate["requestBody"].is_null());
    assert!(deactivate["requestBody"].is_null());

    let request = &document["components"]["schemas"]["ChangeRoleRequest"];
    assert_eq!(request["required"], json!(["role"]));
    assert_eq!(request["additionalProperties"], false);
}

#[test]
fn it_openapi_admin_user_security_routes_declare_typed_contracts() {
    let document = application_document();
    let paths = &document["paths"];
    let reset = &paths["/api/v1/admin/users/{id}/password-reset"]["post"];
    let unlock = &paths["/api/v1/admin/users/{id}/unlock"]["post"];
    let revoke = &paths["/api/v1/admin/users/{userId}/sessions"]["delete"];
    let quota = &paths["/api/v1/admin/users/{id}/quota"]["put"];

    for (operation, class, path_parameter, success) in [
        (reset, "admin+recent-auth", "id", "200"),
        (unlock, "admin", "id", "204"),
        (revoke, "admin+recent-auth", "userId", "204"),
        (quota, "admin", "id", "200"),
    ] {
        assert!(operation["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == class));
        assert_eq!(
            operation["security"],
            json!([{ "palmrSession": [], "palmrCsrfCookie": [], "palmrCsrfHeader": [] }])
        );
        for status in [success, "401", "403", "404", "429"] {
            assert!(operation["responses"][status].is_object(), "{status}");
        }
        assert_eq!(parameter(operation, path_parameter)["in"], "path");
        assert!(operation["responses"]["404"]["description"]
            .as_str()
            .unwrap()
            .contains("USER_NOT_FOUND"));
    }
    assert_eq!(
        reset["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/AdminPasswordReset"
    );
    assert!(reset["responses"]["409"]["description"]
        .as_str()
        .unwrap()
        .contains("USER_HAS_NO_LOCAL_AUTH"));
    assert!(reset["responses"]["403"]["description"]
        .as_str()
        .unwrap()
        .contains("AUTH_PASSWORD_LOGIN_DISABLED"));
    assert!(reset["requestBody"].is_null());
    assert!(unlock["requestBody"].is_null());
    assert!(revoke["requestBody"].is_null());
    assert!(reset["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .all(|parameter| parameter["name"] != "Idempotency-Key"));
    assert_eq!(
        quota["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/QuotaOverrideRequest"
    );
    assert_eq!(
        quota["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/AdminUserQuota"
    );
    assert!(quota["responses"]["422"]["description"]
        .as_str()
        .unwrap()
        .contains("VALIDATION_ERROR"));

    let schemas = &document["components"]["schemas"];
    let request = &schemas["QuotaOverrideRequest"];
    assert_eq!(request["required"], json!(["mode"]));
    assert_eq!(request["additionalProperties"], false);
    let response = &schemas["AdminUserQuota"];
    assert_eq!(
        response["required"],
        json!([
            "mode",
            "quotaBytes",
            "instanceDefaultQuotaBytes",
            "effectiveQuotaBytes",
            "belowCurrentUsage"
        ])
    );
    let reset_response = &schemas["AdminPasswordReset"];
    assert_eq!(
        reset_response["required"],
        json!(["temporaryPassword", "mustChangePassword"])
    );
}

#[test]
fn it_openapi_admin_user_read_routes_declare_closed_typed_contracts() {
    let document = application_document();
    let list = &document["paths"]["/api/v1/admin/users"]["get"];
    let detail = &document["paths"]["/api/v1/admin/users/{id}"]["get"];
    let sessions = &document["paths"]["/api/v1/admin/users/{userId}/sessions"]["get"];

    for operation in [list, detail, sessions] {
        assert!(operation["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tag| tag == "admin"));
        assert_eq!(operation["security"], json!([{ "palmrSession": [] }]));
        for status in ["401", "403", "429"] {
            assert!(operation["responses"][status].is_object(), "{status}");
        }
    }

    assert_eq!(
        parameter(list, "sort")["schema"]["enum"],
        json!(USER_SORT.values())
    );
    assert_eq!(
        parameter(list, "sort")["schema"]["default"],
        "createdAt:desc"
    );
    assert_eq!(
        parameter(list, "role")["schema"]["enum"],
        json!(["admin", "user"])
    );
    assert_eq!(
        parameter(list, "status")["schema"]["enum"],
        json!(["active", "inactive"])
    );
    assert_eq!(parameter(list, "q")["schema"]["minLength"], 2);
    assert_eq!(parameter(list, "q")["schema"]["maxLength"], 128);
    assert_eq!(parameter(list, "limit")["schema"]["default"], 50);
    assert_eq!(parameter(list, "limit")["schema"]["maximum"], 200);
    assert_eq!(parameter(list, "cursor")["schema"]["type"], "string");
    assert_eq!(
        parameter(sessions, "sort")["schema"]["enum"],
        json!(["lastSeenAt:asc", "lastSeenAt:desc"])
    );
    assert_eq!(parameter(detail, "id")["in"], "path");
    assert_eq!(parameter(sessions, "userId")["in"], "path");

    for operation in [detail, sessions] {
        assert!(operation["responses"]["404"]["description"]
            .as_str()
            .unwrap()
            .contains("USER_NOT_FOUND"));
    }
    assert!(list["responses"]["400"]["description"]
        .as_str()
        .unwrap()
        .contains("CURSOR_INVALID"));

    let schemas = &document["components"]["schemas"];
    let item = &schemas["AdminUserItem"];
    let required: Vec<&str> = item["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for field in ["quotaBytes", "effectiveQuotaBytes", "usedBytes", "counts"] {
        assert!(required.contains(&field), "{field}");
    }
    assert_eq!(
        item["properties"]["usedBytes"],
        json!({ "$ref": "#/components/schemas/ByteCount" })
    );
    for field in ["quotaBytes", "effectiveQuotaBytes"] {
        assert_eq!(
            item["properties"][field],
            json!({ "oneOf": [{ "type": "null" }, { "$ref": "#/components/schemas/ByteCount" }] }),
            "{field}"
        );
    }
    assert_eq!(schemas["ByteCount"]["maximum"], 9_007_199_254_740_991_u64);
    assert_eq!(schemas["ByteCount"]["format"], "int64");
    let detail_extras = &schemas["AdminUserDetail"]["allOf"][1];
    assert_eq!(
        detail_extras["required"],
        json!([
            "quotaOverrideMode",
            "overQuota",
            "sessionCount",
            "trustedDeviceCount",
            "lockout",
            "identityLinks"
        ])
    );
    assert_eq!(
        schemas["QuotaOverrideModeName"]["enum"],
        json!(["inherit", "unlimited", "bytes"])
    );
    let serialized = serde_json::to_string(&document).unwrap();
    for forbidden in [
        "passwordHash",
        "password_hash",
        "tokenHash",
        "totpSecret",
        "clientSecret",
        "backupCode",
    ] {
        assert!(
            !schemas["AdminUserItem"].to_string().contains(forbidden)
                && !schemas["AdminUserDetail"].to_string().contains(forbidden)
                && !schemas["AdminIdentityLink"].to_string().contains(forbidden),
            "{forbidden}"
        );
    }
    assert!(serialized.contains("USER_NOT_FOUND"));
}
