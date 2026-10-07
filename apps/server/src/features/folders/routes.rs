use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn};
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::Required;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::with_query_parameters;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::json;
use crate::infra::http::pagination::Page;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::FolderError;
use super::model::{
    CreateFolderRequest, FolderChange, FolderDetail, FolderId, FolderItem, FolderTree, NewFolder,
    UpdateFolderRequest, TREE_DEFAULT_DEPTH, TREE_MAX_DEPTH,
};
use super::service::{FolderService, DEPTH_PARAM, ROOT_PARAM};

const READ_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const WRITE_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Write,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

fn tree_parameters() -> Vec<Parameter> {
    let query = |name: &str, description: &str, schema: ObjectBuilder| {
        ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Query)
            .required(Required::False)
            .description(Some(description))
            .schema(Some(schema))
            .build()
    };
    vec![
        query(
            DEPTH_PARAM,
            "Levels returned, counted from the top of the tree.",
            ObjectBuilder::new()
                .schema_type(Type::Integer)
                .minimum(Some(1))
                .maximum(Some(TREE_MAX_DEPTH))
                .default(Some(serde_json::Value::from(TREE_DEFAULT_DEPTH))),
        ),
        query(
            ROOT_PARAM,
            "Root the tree at this folder, which is level 1. Absent returns the root-level folders as level 1. An unknown or foreign folder id is `FOLDER_NOT_FOUND`.",
            ObjectBuilder::new().schema_type(Type::String),
        ),
    ]
}

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(list_folders), &FolderService::list_parameters()),
        )
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(folder_tree), &tree_parameters()),
        )
        .route(WRITE_ROUTE, routes!(create_folder))
        .route(READ_ROUTE, routes!(get_folder))
        .route(WRITE_ROUTE, routes!(update_folder))
}

#[utoipa::path(
    get,
    path = "/api/v1/folders",
    tag = "folders",
    params(
        ("parentId" = Option<String>, Query, description = "List the direct child folders of this folder. Absent lists the root-level folders. An unknown or foreign folder id is `FOLDER_NOT_FOUND`.")
    ),
    responses(
        (status = 200, description = "Direct child folders with recursive subtree aggregates. `fileCount`, `subfolderCount` and `totalBytes` cover the whole subtree of each folder and are computed with one recursive statement per page. `q` filters the direct children of the selected parent by normalized name; it never searches below them. `totalCount` is the exact filtered count.", body = Page<FolderItem>),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the parent is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid `q`, `sort` or `limit`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_folders(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let query = match service.list_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.list(principal.user_id, query).await {
        Ok(page) => json_response(StatusCode::OK, &page, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/folders/tree",
    tag = "folders",
    responses(
        (status = 200, description = "A flat, breadth-first list of folders, never more than 2 000 nodes. A parent always precedes its children. When the cap cuts the walk, `truncated` is `true` and `truncationPoint` names the last node returned.", body = FolderTree),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the root is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for a `depth` outside 1 to 8.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn folder_tree(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let query = match FolderService::tree_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.tree(principal.user_id, query).await {
        Ok(tree) => json_response(StatusCode::OK, &tree, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/folders",
    tag = "folders",
    request_body(
        content = CreateFolderRequest,
        content_type = "application/json",
        description = "A duplicate name in the same parent is disambiguated deterministically (`Docs`, `Docs (1)`, `Docs (2)`); the response carries the name that was stored. Maximum nesting depth is 64."
    ),
    responses(
        (status = 201, description = "The created folder, with the stored name.", body = FolderItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the parent is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`NAME_INVALID`, `FOLDER_DEPTH_EXCEEDED` or `VALIDATION_ERROR`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn create_folder(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let new = match json::read::<CreateFolderRequest>(request.into_body()).await {
        Ok(body) => match NewFolder::parse(body) {
            Ok(new) => new,
            Err(error) => return folder_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.create(principal.user_id, new).await {
        Ok(folder) => json_response(StatusCode::CREATED, &folder, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/folders/{id}",
    tag = "folders",
    params(("id" = String, Path, description = "Folder UUIDv7")),
    responses(
        (status = 200, description = "The folder with its recursive aggregates and `path[]`, the breadcrumbs from the root-level ancestor down to and including the folder.", body = FolderDetail),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`, including another user's folder.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_folder(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FolderId>() else {
        return folder_error(&FolderError::NotFound, request_id.as_ref());
    };
    match service.detail(principal.user_id, id).await {
        Ok(detail) => json_response(StatusCode::OK, &detail, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/folders/{id}",
    tag = "folders",
    params(("id" = String, Path, description = "Folder UUIDv7")),
    request_body(
        content = UpdateFolderRequest,
        content_type = "application/json",
        description = "Absent members are unchanged. `description: null` clears the description; `name` cannot be `null`. A name that collides with a sibling is disambiguated like a create. Moving a folder is a separate operation."
    ),
    responses(
        (status = 200, description = "The updated folder, with the stored name.", body = FolderItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`, including another user's folder.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`NAME_INVALID` or `VALIDATION_ERROR`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn update_folder(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FolderId>() else {
        return folder_error(&FolderError::NotFound, request_id.as_ref());
    };
    let change = match json::read::<UpdateFolderRequest>(request.into_body()).await {
        Ok(body) => match FolderChange::parse(body) {
            Ok(change) => change,
            Err(error) => return folder_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update(principal.user_id, id, change).await {
        Ok(folder) => json_response(StatusCode::OK, &folder, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

fn folder_error(error: &FolderError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "folder request failed");
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
