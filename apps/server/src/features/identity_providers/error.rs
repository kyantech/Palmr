use std::fmt;

use super::discovery::DiscoveryFailure;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::InvalidTimestamp;
use crate::features::audit::error::AuditError;
use crate::features::auth::error::LoginError;
use crate::features::auth::sessions::SessionError;
use crate::features::users::error::UserError;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;
use crate::infra::http::error::{ApiError, CheckDetail};
use crate::infra::jobs::JobsError;

#[derive(Debug)]
pub enum ProviderError {
    NotFound,
    Disabled,
    SlugTaken,
    Invalid { fields: Vec<&'static str> },
    DiscoveryFailed(DiscoveryFailure),
    ValidationFailed { checks: Vec<CheckDetail> },
    HasLinks,
    Stale,
    RepositoryInvariant { column: &'static str },
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl ProviderError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "provider_not_found",
            Self::Disabled => "provider_disabled",
            Self::SlugTaken => "provider_slug_taken",
            Self::Invalid { .. } => "provider_invalid",
            Self::DiscoveryFailed(_) => "provider_discovery_failed",
            Self::ValidationFailed { .. } => "provider_validation_failed",
            Self::HasLinks => "provider_has_identity_links",
            Self::Stale => "provider_changed_concurrently",
            Self::RepositoryInvariant { .. } => "provider_repository_invariant",
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "provider_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "provider_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound => ApiError::new(ErrorCode::ProviderNotFound),
            Self::Disabled => ApiError::new(ErrorCode::ProviderDisabled),
            Self::SlugTaken => ApiError::new(ErrorCode::ProviderSlugTaken),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::DiscoveryFailed(failure) => ApiError::new(ErrorCode::ProviderDiscoveryFailed)
                .with_detail("reason", failure.code()),
            Self::ValidationFailed { checks } => ApiError::new(ErrorCode::ProviderValidationFailed)
                .with_detail("checks", checks.clone()),
            Self::HasLinks => ApiError::new(ErrorCode::ProviderHasLinks),
            Self::Stale => ApiError::new(ErrorCode::DatabaseBusy),
            Self::Db(error) | Self::Audit(AuditError::Db(error)) => ApiError::new(error.api_code()),
            Self::RepositoryInvariant { .. } | Self::Audit(_) | Self::Crypto(_) | Self::Time(_) => {
                ApiError::internal()
            }
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the identity provider does not exist"),
            Self::Disabled => f.write_str("the identity provider is disabled"),
            Self::SlugTaken => f.write_str("an identity provider with that slug already exists"),
            Self::Invalid { fields } => {
                write!(f, "the provider fields {} are invalid", fields.join(", "))
            }
            Self::DiscoveryFailed(failure) => {
                write!(f, "OIDC discovery failed: {}", failure.code())
            }
            Self::ValidationFailed { .. } => f.write_str("the provider checks failed"),
            Self::HasLinks => f.write_str("the provider still has identity links"),
            Self::Stale => f.write_str("the provider changed while the request was running"),
            Self::RepositoryInvariant { column } => {
                write!(
                    f,
                    "the identity_providers row holds an invalid value in {column}"
                )
            }
            Self::Audit(error) => write!(f, "provider audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "provider secret operation failed: {error}"),
            Self::Db(error) => write!(f, "provider database operation failed: {error}"),
            Self::Time(error) => write!(f, "provider timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<AuditError> for ProviderError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for ProviderError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for ProviderError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for ProviderError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for ProviderError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalLoginError {
    code: ErrorCode,
    kind: &'static str,
}

impl ExternalLoginError {
    pub const fn refused(code: ErrorCode) -> Self {
        Self {
            code,
            kind: code.as_str(),
        }
    }

    pub const fn internal(kind: &'static str) -> Self {
        Self {
            code: ErrorCode::InternalError,
            kind,
        }
    }

    pub const fn code(self) -> ErrorCode {
        self.code
    }

    pub const fn kind(self) -> &'static str {
        self.kind
    }

    pub fn is_server_fault(self) -> bool {
        self.code.status().is_server_error()
    }
}

impl fmt::Display for ExternalLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "external login failed: {}", self.kind)
    }
}

impl std::error::Error for ExternalLoginError {}

impl From<DbError> for ExternalLoginError {
    fn from(error: DbError) -> Self {
        Self {
            code: error.api_code(),
            kind: error.kind().as_str(),
        }
    }
}

impl From<sqlx::Error> for ExternalLoginError {
    fn from(error: sqlx::Error) -> Self {
        DbError::from(error).into()
    }
}

impl From<ProviderError> for ExternalLoginError {
    fn from(error: ProviderError) -> Self {
        Self {
            code: error.api_error().code(),
            kind: error.kind(),
        }
    }
}

impl From<AuditError> for ExternalLoginError {
    fn from(error: AuditError) -> Self {
        match error {
            AuditError::Db(error) => error.into(),
            other => Self::internal(other.kind()),
        }
    }
}

impl From<UserError> for ExternalLoginError {
    fn from(error: UserError) -> Self {
        match error {
            UserError::Db(error) => error.into(),
            other => Self::internal(other.kind()),
        }
    }
}

impl From<LoginError> for ExternalLoginError {
    fn from(error: LoginError) -> Self {
        Self {
            code: error.api_error().code(),
            kind: error.kind(),
        }
    }
}

impl From<SessionError> for ExternalLoginError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Db(error) => error.into(),
            other => Self::internal(other.kind()),
        }
    }
}

impl From<JobsError> for ExternalLoginError {
    fn from(error: JobsError) -> Self {
        match error {
            JobsError::Db(error) => error.into(),
            other => Self::internal(other.kind()),
        }
    }
}

impl From<CryptoError> for ExternalLoginError {
    fn from(_: CryptoError) -> Self {
        Self::internal("external_login_crypto")
    }
}

impl From<InvalidTimestamp> for ExternalLoginError {
    fn from(_: InvalidTimestamp) -> Self {
        Self::internal("external_login_time_out_of_range")
    }
}
