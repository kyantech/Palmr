use std::collections::BTreeSet;

use serde_json::{json, Value};
use utoipa_axum::routes;

use super::auth_class::AuthClass;
use super::openapi::ApiDocs;
use super::router::{
    application_routes, RateLimitClass, RouteInventory, RoutePolicy, Routes, Transport,
};
use crate::config::{EnvironmentSource, OperatorConfig};

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
            "GET /api/v1/public/uploads/presign (public) accepts storage input \"objectName\"",
            "GET /api/v1/public/uploads/presign (public) accepts storage input \"bucket\"",
            "GET /api/v1/storage/objects (public+grant) is a generic storage primitive under /api/v1/storage",
            "PUT /s3/{key} (public) is a generic storage primitive under /s3",
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
