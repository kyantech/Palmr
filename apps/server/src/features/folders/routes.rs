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
use crate::app::router::{IdempotencyMode, RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::files::delete::DeletionCaller;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::idempotency::{Admission, Claim, IdempotencyRequest};
use crate::infra::http::json;
use crate::infra::http::pagination::Page;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::FolderError;
use super::impact::DeletionImpact;
use super::model::{
    CreateFolderRequest, EnsurePath, EnsurePathRequest, EnsurePathResponse, FolderChange,
    FolderDetail, FolderId, FolderItem, FolderMove, FolderTree, MoveFolderRequest, NewFolder,
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

const ENSURE_PATH_ROUTE: RoutePolicy = WRITE_ROUTE.with_idempotency(IdempotencyMode::Plaintext);

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
        .route(WRITE_ROUTE, routes!(move_folder))
        .route(ENSURE_PATH_ROUTE, routes!(ensure_path))
        .route(READ_ROUTE, routes!(folder_deletion_impact))
        .route(WRITE_ROUTE, routes!(delete_folder))
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
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated, or `FOLDER_DELETING`: the parent folder is being deleted.", body = ApiErrorBody),
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
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated, or `FOLDER_DELETING`: the folder is being deleted.", body = ApiErrorBody),
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

#[utoipa::path(
    post,
    path = "/api/v1/folders/{id}/move",
    tag = "folders",
    params(("id" = String, Path, description = "Folder UUIDv7")),
    request_body(
        content = MoveFolderRequest,
        content_type = "application/json",
        description = "`parentId` is the destination folder, or `null` for the My Files root; the member is required. A move into the folder's current parent changes nothing. A name that collides in the destination is disambiguated deterministically (`Docs`, `Docs (1)`, `Docs (2)`); the response carries the name that was stored. The whole subtree moves; the moved folder and every descendant end at the depth their new location implies, and the deepest descendant may not pass depth 64."
    ),
    responses(
        (status = 200, description = "The moved folder, with the stored name and its recursive aggregates.", body = FolderItem),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the folder or the destination is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated in the destination, or `FOLDER_DELETING`: the folder or the destination is being deleted.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`FOLDER_CYCLE` when the destination is the folder itself or one of its descendants, `FOLDER_DEPTH_EXCEEDED` when the moved subtree would pass depth 64, or `VALIDATION_ERROR`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn move_folder(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FolderId>() else {
        return folder_error(&FolderError::NotFound, request_id.as_ref());
    };
    let destination = match json::read::<MoveFolderRequest>(request.into_body()).await {
        Ok(body) => match FolderMove::parse(body) {
            Ok(destination) => destination,
            Err(error) => return folder_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service
        .move_folder(principal.user_id, id, destination.parent_id)
        .await
    {
        Ok(folder) => json_response(StatusCode::OK, &folder, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/folders/ensure-path",
    tag = "folders",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "16–128 characters. A replay within 24 hours returns the original response with `Idempotency-Replayed: true` without executing again. The call is also idempotent without the header: it reuses the folders that already exist.")
    ),
    request_body(
        content = EnsurePathRequest,
        content_type = "application/json",
        description = "Resolves or creates the chain of folders named by `segments` under `parentId` in one transaction. A segment that matches an existing sibling by normalized name (case-insensitive, Unicode-normalized) resolves to that folder: nothing is suffixed, renamed or overwritten. Unlike a folder create, rename or move, this call never disambiguates a name."
    ),
    responses(
        (status = 200, description = "One id per segment in request order, the id of the last segment, and the ids this call created.", body = EnsurePathResponse),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the parent is unknown, malformed or belongs to another user.", body = ApiErrorBody),
        (status = 409, description = "`IDEMPOTENCY_KEY_CONFLICT` or `IDEMPOTENCY_REQUEST_IN_PROGRESS` for a reused key, or `FOLDER_DELETING`: the parent or an existing segment is being deleted.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`NAME_INVALID` for an invalid segment, `FOLDER_DEPTH_EXCEEDED` when the chain would pass depth 64, or `VALIDATION_ERROR` for a missing, empty or over-long `segments` array.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn ensure_path(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    idempotency: IdempotencyRequest,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let body = match json::read_value(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    let claim = match service.idempotency().claim(idempotency, &body).await {
        Ok(Admission::Execute(claim)) => claim,
        Ok(Admission::Replay(mut response)) => {
            response.headers_mut().insert(CACHE_CONTROL, NO_STORE);
            return response;
        }
        Err(rejection) => return rejection.into_response(),
    };
    let parsed = match json::parse::<EnsurePathRequest>(body) {
        Ok(parsed) => parsed,
        Err(error) => {
            release(&service, claim).await;
            return tag_error(error, request_id.as_ref()).into_response();
        }
    };
    let ensured = match EnsurePath::parse(parsed) {
        Ok(input) => service.ensure_path(principal.user_id, &input, &claim).await,
        Err(error) => Err(error),
    };
    match ensured {
        Ok(ensured) => json_response(StatusCode::OK, &ensured, request_id.as_ref()),
        Err(error) => {
            release(&service, claim).await;
            folder_error(&error, request_id.as_ref())
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/folders/{id}/deletion-impact",
    tag = "folders",
    params(("id" = String, Path, description = "Folder UUIDv7")),
    responses(
        (status = 200, description = "What deleting the folder and everything below it would remove, from one bounded recursive statement: `folders` counts the folder rows that would be deleted, the selected folder included, `files` and `totalBytes` cover the whole subtree. `affectedShares` lists, without duplicates, the caller's Shares that reference the folder, a folder or file inside it, or a live folder root above it, each with `remainingItems`, the number of that Share's root items still present after the deletion; `affectedShareCount` is the total when the list is capped at 100. `affectedEmbeds` counts the active, unexpired embed grants of the files below. Read-only and advisory.", body = DeletionImpact),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`, including another user's folder and a folder that is being deleted.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn folder_deletion_impact(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FolderId>() else {
        return folder_error(&FolderError::NotFound, request_id.as_ref());
    };
    match service.folder_impact(principal.user_id, id).await {
        Ok(impact) => json_response(StatusCode::OK, &impact, request_id.as_ref()),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/folders/{id}",
    tag = "folders",
    params(("id" = String, Path, description = "Folder UUIDv7")),
    responses(
        (status = 204, description = "The deletion is durably claimed: the folder and everything below it are invisible at once and further writes into them are refused with `FOLDER_DELETING`. Its rows are removed and its bytes erased in the background in bounded batches, so physical cleanup may still be pending. Deleting a folder the caller owned and already deleted, or that is being deleted, is also `204`. There is no Trash and no restore."),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the id never existed or never belonged to the caller.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn delete_folder(
    Extension(service): Extension<FolderService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FolderId>() else {
        return folder_error(&FolderError::NotFound, request_id.as_ref());
    };
    let caller = DeletionCaller {
        owner: principal.user_id,
        username: principal.username.clone(),
        client: client_metadata(&request),
    };
    match service.delete_folder(&caller, id).await {
        Ok(_) => (StatusCode::NO_CONTENT, [(CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(error) => folder_error(&error, request_id.as_ref()),
    }
}

async fn release(service: &FolderService, claim: Claim) {
    if let Err(error) = service.idempotency().release(claim).await {
        tracing::error!(
            kind = error.kind(),
            "folder idempotency claim could not be released"
        );
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
