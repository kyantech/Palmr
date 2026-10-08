use axum::extract::{Extension, Path, RawQuery, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa::openapi::schema::Schema;
use utoipa::openapi::RefOr;
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::openapi::{with_query_parameters, with_schemas};
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::json;
use crate::infra::http::request_id::{tag_error, RequestId};

use super::error::FileError;
use super::model::{
    BatchMove, BatchMoveRequest, BatchMoveResult, BrowseItem, FileChange, FileId, FileItem,
    FileMove, FileResult, FilesPage, MoveFileRequest, NameCheck, SearchFileItem, UpdateFileRequest,
};
use super::service::FileService;

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

fn named_schemas() -> Vec<(String, RefOr<Schema>)> {
    let mut schemas = Vec::new();
    for (name, schema) in [
        (BrowseItem::name(), BrowseItem::schema()),
        (SearchFileItem::name(), SearchFileItem::schema()),
    ] {
        schemas.push((name.into_owned(), schema));
    }
    BrowseItem::schemas(&mut schemas);
    SearchFileItem::schemas(&mut schemas);
    schemas
}

pub fn routes() -> Routes<AppState> {
    Routes::new()
        .route(
            READ_ROUTE,
            with_schemas(
                with_query_parameters(routes!(list_files), &FileService::list_parameters()),
                named_schemas(),
            ),
        )
        .route(
            READ_ROUTE,
            with_query_parameters(routes!(name_check), &FileService::name_check_parameters()),
        )
        .route(READ_ROUTE, routes!(get_file))
        .route(WRITE_ROUTE, routes!(update_file))
        .route(WRITE_ROUTE, routes!(move_file))
        .route(WRITE_ROUTE, routes!(batch_move_files))
}

#[utoipa::path(
    get,
    path = "/api/v1/files",
    tag = "files",
    responses(
        (status = 200, description = "The shape depends on the request. Without `q` (browse) it is a page of the direct children of the folder, subfolders first and then files, each item with a `kind` of `folder` or `file`; folders always precede files across the whole traversal whatever the `sort`, a folder sorts by `totalBytes` for `size`, `totalCount` is the exact number of direct children, and it never descends below the folder. With `q` (search) it is a page of files only, each a file item plus `path`, the folders that contain it from the root down (empty at the My Files root); the search covers the caller's whole My Files tree, `folderId` is ignored, results are ordered by relevance unless `sort` says otherwise, and `totalCount` is always `null`. Cursors are keyset cursors bound to the `sort` and, in search, to the query; a cursor reused with another `sort` or another `q`, a cursor from the other mode, or an altered cursor is `CURSOR_INVALID`.", body = FilesPage),
        (status = 400, description = "`CURSOR_INVALID`.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND` in browse mode: the folder is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR` for an invalid `sort` or `limit`, or for a `q` that is empty, repeated, whitespace-only or outside 2 to 128 characters (`details.fields = [\"q\"]`).", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn list_files(
    Extension(service): Extension<FileService>,
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
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/files/name-check",
    tag = "files",
    responses(
        (status = 200, description = "Advisory only. `available` is `true` when no file in the folder already has the name, compared case-insensitively and Unicode-normalized; otherwise `suggestedName` is the first free name the server would pick today. Nothing is reserved: a later upload, rename or move still resolves collisions atomically and may pick a different name.", body = NameCheck),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FOLDER_NOT_FOUND`: the folder is unknown or belongs to another user.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no free name could be generated.", body = ApiErrorBody),
        (status = 422, description = "`NAME_INVALID` for a name that is not a valid file name, or `VALIDATION_ERROR` for a missing or repeated `name`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn name_check(
    Extension(service): Extension<FileService>,
    Authenticated(principal): Authenticated,
    RawQuery(raw_query): RawQuery,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let query = match FileService::name_check_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.name_check(principal.user_id, query).await {
        Ok(check) => json_response(StatusCode::OK, &check, request_id.as_ref()),
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/files/{id}",
    tag = "files",
    params(("id" = String, Path, description = "File UUIDv7")),
    responses(
        (status = 200, description = "The file's metadata. The response never carries a storage identifier or object key.", body = FileItem),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND`, including another user's file.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn get_file(
    Extension(service): Extension<FileService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FileId>() else {
        return file_error(&FileError::NotFound, request_id.as_ref());
    };
    match service.get(principal.user_id, id).await {
        Ok(file) => json_response(StatusCode::OK, &file, request_id.as_ref()),
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/files/{id}",
    tag = "files",
    params(("id" = String, Path, description = "File UUIDv7")),
    request_body(
        content = UpdateFileRequest,
        content_type = "application/json",
        description = "Absent members are unchanged. `description: null` clears the description; `name` cannot be `null`. A name that collides with a sibling is disambiguated deterministically (`photo.jpg`, `photo (1).jpg`); `renamedTo` in the response is then the stored name. Renaming never touches storage, and a request that changes nothing writes nothing. Moving a file is a separate operation."
    ),
    responses(
        (status = 200, description = "The updated file, with the stored name.", body = FileResult),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND`, including another user's file.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`NAME_INVALID` or `VALIDATION_ERROR`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn update_file(
    Extension(service): Extension<FileService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FileId>() else {
        return file_error(&FileError::NotFound, request_id.as_ref());
    };
    let change = match json::read::<UpdateFileRequest>(request.into_body()).await {
        Ok(body) => match FileChange::parse(body) {
            Ok(change) => change,
            Err(error) => return file_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.update(principal.user_id, id, change).await {
        Ok(file) => json_response(StatusCode::OK, &file, request_id.as_ref()),
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/files/{id}/move",
    tag = "files",
    params(("id" = String, Path, description = "File UUIDv7")),
    request_body(
        content = MoveFileRequest,
        content_type = "application/json",
        description = "`folderId` is the destination folder, or `null` for the My Files root; the member is required. A move into the file's current folder changes nothing. A name that collides in the destination is disambiguated deterministically (`report.pdf`, `report (1).pdf`); `renamedTo` in the response is then the stored name. Moving never touches storage."
    ),
    responses(
        (status = 200, description = "The moved file, with the stored name.", body = FileResult),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND`, including another user's file, or `FOLDER_NOT_FOUND` for an unknown or foreign destination.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated in the destination.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`VALIDATION_ERROR`.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn move_file(
    Extension(service): Extension<FileService>,
    Authenticated(principal): Authenticated,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let Ok(id) = id.parse::<FileId>() else {
        return file_error(&FileError::NotFound, request_id.as_ref());
    };
    let destination = match json::read::<MoveFileRequest>(request.into_body()).await {
        Ok(body) => match FileMove::parse(body) {
            Ok(destination) => destination,
            Err(error) => return file_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service
        .move_file(principal.user_id, id, destination.folder_id)
        .await
    {
        Ok(file) => json_response(StatusCode::OK, &file, request_id.as_ref()),
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/files/batch/move",
    tag = "files",
    request_body(
        content = BatchMoveRequest,
        content_type = "application/json",
        description = "Moves every listed file, and every listed folder, to `targetFolderId` (`null` is the My Files root) in one transaction: all of them move or none does. At most 500 ids in total; an id may not repeat. Folders are moved first, then files, each in request order, so name collisions are resolved in a fixed order (`report.pdf`, `report (1).pdf`, `report (2).pdf`). An item already in the destination is left as it is. Any failure rolls the whole batch back: an unknown or foreign id (`FILE_NOT_FOUND`, `FOLDER_NOT_FOUND`), a folder moved into itself or its own subtree (`FOLDER_CYCLE`), a subtree that would pass depth 64 (`FOLDER_DEPTH_EXCEEDED`) or a name for which no unique variant exists (`FILE_NAME_CONFLICT`). There is no partial result."
    ),
    responses(
        (status = 200, description = "Every item moved, with its stored name. `renamedTo` is set where a collision changed the name.", body = BatchMoveResult),
        (status = 400, description = "The body is not parseable JSON.", body = ApiErrorBody),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
        (status = 403, description = "The CSRF proof or origin is missing or not allowed, or the session is restricted.", body = ApiErrorBody),
        (status = 404, description = "`FILE_NOT_FOUND` or `FOLDER_NOT_FOUND`: an item or the destination is unknown or belongs to another user. Nothing moved.", body = ApiErrorBody),
        (status = 409, description = "`FILE_NAME_CONFLICT`: no unique name could be generated for one item. Nothing moved.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "`BATCH_TOO_LARGE` beyond 500 ids, `FOLDER_CYCLE`, `FOLDER_DEPTH_EXCEEDED`, or `VALIDATION_ERROR` for no ids, a repeated id or a missing `targetFolderId`. Nothing moved.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn batch_move_files(
    Extension(service): Extension<FileService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let batch = match json::read::<BatchMoveRequest>(request.into_body()).await {
        Ok(body) => match BatchMove::parse(body) {
            Ok(batch) => batch,
            Err(error) => return file_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.batch_move(principal.user_id, &batch).await {
        Ok(moved) => json_response(StatusCode::OK, &moved, request_id.as_ref()),
        Err(error) => file_error(&error, request_id.as_ref()),
    }
}

fn file_error(error: &FileError, request_id: Option<&RequestId>) -> Response {
    let api_error = error.api_error();
    if api_error.status().is_server_error() {
        tracing::error!(kind = error.kind(), "file request failed");
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
