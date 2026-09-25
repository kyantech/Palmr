use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, StatusCode};
use utoipa_axum::routes;

use super::effective::{EffectiveSettings, EffectiveSettingsError, EffectiveSettingsService};
use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::error_code::ErrorCode;
use crate::features::users::error::UserError;
use crate::infra::http::error::{ApiError, ApiErrorBody, JSON_CONTENT_TYPE};
use crate::infra::http::extractors::Authenticated;
use crate::infra::http::request_id::{tag_error, RequestId};

pub const EFFECTIVE_SETTINGS_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Authenticated,
    RateLimitClass::Read,
    Transport::ControlPlane,
);

const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

pub fn routes() -> Routes<AppState> {
    Routes::new().route(EFFECTIVE_SETTINGS_ROUTE, routes!(effective_settings))
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/effective",
    tag = "settings",
    responses(
        (
            status = 200,
            description = "The read-only policy slice the SPA must respect for the caller.",
            body = EffectiveSettings
        ),
        (status = 401, description = "Authentication required.", body = ApiErrorBody),
    )
)]
async fn effective_settings(
    Extension(service): Extension<EffectiveSettingsService>,
    Authenticated(principal): Authenticated,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let effective = match service.for_user(principal.user_id).await {
        Ok(effective) => effective,
        Err(EffectiveSettingsError::UserMissing) => {
            return tag_error(ApiError::new(ErrorCode::AuthRequired), request_id.as_ref())
                .into_response();
        }
        Err(EffectiveSettingsError::User(error)) => {
            tracing::error!(kind = error.kind(), "effective settings could not be read");
            let api_error = match error {
                UserError::Db(error) => ApiError::from(error),
                _ => ApiError::internal(),
            };
            return tag_error(api_error, request_id.as_ref()).into_response();
        }
    };
    match serde_json::to_vec(&effective) {
        Ok(body) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE)),
                (CACHE_CONTROL, NO_STORE),
            ],
            body,
        )
            .into_response(),
        Err(_) => tag_error(ApiError::internal(), request_id.as_ref()).into_response(),
    }
}
