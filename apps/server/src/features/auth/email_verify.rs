use std::fmt;

use axum::extract::{Extension, Request};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::routes;

use crate::app::auth_class::AuthClass;
use crate::app::router::{RateLimitClass, RoutePolicy, Routes, Transport};
use crate::app::state::AppState;
use crate::domain::secret::REDACTED;
use crate::features::auth::sessions::routes::client_metadata;
use crate::features::users::email_change::{EmailChangeService, PresentedToken};
use crate::features::users::email_change_routes::{email_change_error, status_only};
use crate::infra::http::error::ApiErrorBody;
use crate::infra::http::json::{self, JsonField, JsonKind, JsonRequest};
use crate::infra::http::request_id::{tag_error, RequestId};

pub const VERIFY_ROUTE: RoutePolicy = RoutePolicy::new(
    AuthClass::Public,
    RateLimitClass::AuthToken,
    Transport::ControlPlane,
);

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyEmailRequest {
    /// The opaque token from the e-mailed `/verify-email/{token}` link.
    pub token: String,
}

impl fmt::Debug for VerifyEmailRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifyEmailRequest")
            .field("token", &REDACTED)
            .finish()
    }
}

impl JsonRequest for VerifyEmailRequest {
    const FIELDS: &'static [JsonField] = &[JsonField::required("token", JsonKind::String)];
}

pub fn routes() -> Routes<AppState> {
    Routes::new().route(VERIFY_ROUTE, routes!(verify_email))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/email/verify",
    tag = "auth",
    request_body(content = VerifyEmailRequest, content_type = "application/json"),
    responses(
        (
            status = 204,
            description = "The pending address becomes the canonical address and sign-in identity and the old address stops resolving. In one transaction every session and trusted device of the account is revoked, including the session of an Administrator who changed their own address, and `USER_EMAIL_CHANGE_CONFIRMED` is audited. Activation, role, password and two-factor enrollment are unchanged and no session is created."
        ),
        (status = 400, description = "`EMAIL_VERIFICATION_TOKEN_INVALID` for a malformed, unknown, already used or superseded token, or the body is not parseable JSON.", body = ApiErrorBody),
        (status = 409, description = "`USER_EMAIL_TAKEN` when the address became canonical on another account while the verification was pending; nothing is changed.", body = ApiErrorBody),
        (status = 410, description = "`EMAIL_VERIFICATION_TOKEN_EXPIRED`.", body = ApiErrorBody),
        (status = 415, description = "The request is not JSON.", body = ApiErrorBody),
        (status = 422, description = "The request failed validation.", body = ApiErrorBody),
        (status = 429, description = "Rate limited.", body = ApiErrorBody),
    )
)]
async fn verify_email(
    Extension(service): Extension<EmailChangeService>,
    request: Request,
) -> Response {
    let request_id = RequestId::of(&request);
    let client = client_metadata(&request);
    let token = match json::read::<VerifyEmailRequest>(request.into_body()).await {
        Ok(body) => match PresentedToken::parse(&body.token) {
            Ok(token) => token,
            Err(error) => return email_change_error(&error, request_id.as_ref()),
        },
        Err(error) => return tag_error(error, request_id.as_ref()).into_response(),
    };
    match service.verify(&token, &client).await {
        Ok(()) => status_only(StatusCode::NO_CONTENT),
        Err(error) => email_change_error(&error, request_id.as_ref()),
    }
}
