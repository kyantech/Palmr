use http::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

// Every property of a code is generated from its single row, so a code cannot
// be rendered with a second status or a second wire spelling anywhere.
macro_rules! error_catalog {
    ($($variant:ident = $wire:literal, $status:ident, retryable: $retryable:literal, $message:literal;)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ToSchema)]
        pub enum ErrorCode {
            $(#[serde(rename = $wire)] #[schema(rename = $wire)] $variant,)+
        }

        impl ErrorCode {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub const fn status(self) -> StatusCode {
                match self {
                    $(Self::$variant => StatusCode::$status,)+
                }
            }

            pub const fn retryable(self) -> bool {
                match self {
                    $(Self::$variant => $retryable,)+
                }
            }

            pub const fn default_message(self) -> &'static str {
                match self {
                    $(Self::$variant => $message,)+
                }
            }
        }
    };
}

error_catalog! {
    ValidationError = "VALIDATION_ERROR", UNPROCESSABLE_ENTITY, retryable: false,
        "The request failed validation";
    InvalidJson = "INVALID_JSON", BAD_REQUEST, retryable: false,
        "Invalid JSON request body";
    NotFound = "NOT_FOUND", NOT_FOUND, retryable: false,
        "The requested resource was not found";
    MethodNotAllowed = "METHOD_NOT_ALLOWED", METHOD_NOT_ALLOWED, retryable: false,
        "The method is not allowed for this path";
    Forbidden = "FORBIDDEN", FORBIDDEN, retryable: false,
        "The action is not permitted";
    UnsupportedMediaType = "UNSUPPORTED_MEDIA_TYPE", UNSUPPORTED_MEDIA_TYPE, retryable: false,
        "The request content type is not supported";
    RequestBodyTooLarge = "REQUEST_BODY_TOO_LARGE", PAYLOAD_TOO_LARGE, retryable: false,
        "The request body is too large";
    RequestTimeout = "REQUEST_TIMEOUT", REQUEST_TIMEOUT, retryable: false,
        "The request exceeded the server deadline";
    InternalError = "INTERNAL_ERROR", INTERNAL_SERVER_ERROR, retryable: true,
        "An internal error occurred";
    ServiceUnavailable = "SERVICE_UNAVAILABLE", SERVICE_UNAVAILABLE, retryable: true,
        "The service is temporarily unavailable";
    CursorInvalid = "CURSOR_INVALID", BAD_REQUEST, retryable: false,
        "The pagination cursor is invalid";
    RateLimited = "RATE_LIMITED", TOO_MANY_REQUESTS, retryable: true,
        "Too many requests";
    CsrfTokenMissing = "CSRF_TOKEN_MISSING", FORBIDDEN, retryable: false,
        "The CSRF token is missing";
    CsrfTokenInvalid = "CSRF_TOKEN_INVALID", FORBIDDEN, retryable: false,
        "The CSRF token is invalid";
    OriginNotAllowed = "ORIGIN_NOT_ALLOWED", FORBIDDEN, retryable: false,
        "The request origin is not allowed";
    IdempotencyKeyConflict = "IDEMPOTENCY_KEY_CONFLICT", CONFLICT, retryable: false,
        "The idempotency key was already used for a different request";
    IdempotencyRequestInProgress = "IDEMPOTENCY_REQUEST_IN_PROGRESS", CONFLICT, retryable: true,
        "A request with this idempotency key is still in progress";
    BatchTooLarge = "BATCH_TOO_LARGE", UNPROCESSABLE_ENTITY, retryable: false,
        "The batch contains too many items";
    FeatureUnavailableSmtp = "FEATURE_UNAVAILABLE_SMTP", CONFLICT, retryable: false,
        "This action requires e-mail delivery to be configured";
    SetupAlreadyCompleted = "SETUP_ALREADY_COMPLETED", CONFLICT, retryable: false,
        "Setup has already been completed";
    AuthRequired = "AUTH_REQUIRED", UNAUTHORIZED, retryable: false,
        "Authentication is required";
    AuthInvalidCredentials = "AUTH_INVALID_CREDENTIALS", UNAUTHORIZED, retryable: false,
        "The credentials are invalid";
    AuthLocked = "AUTH_LOCKED", TOO_MANY_REQUESTS, retryable: true,
        "The account is temporarily locked";
    AuthPasswordLoginDisabled = "AUTH_PASSWORD_LOGIN_DISABLED", FORBIDDEN, retryable: false,
        "Password login is disabled";
    AuthRecentAuthRequired = "AUTH_RECENT_AUTH_REQUIRED", FORBIDDEN, retryable: false,
        "Recent authentication is required";
    AuthPasswordChangeRequired = "AUTH_PASSWORD_CHANGE_REQUIRED", FORBIDDEN, retryable: false,
        "A password change is required";
    Auth2faEnrollmentRequired = "AUTH_2FA_ENROLLMENT_REQUIRED", FORBIDDEN, retryable: false,
        "Two-factor authentication enrollment is required";
    Auth2faRequired = "AUTH_2FA_REQUIRED", UNAUTHORIZED, retryable: false,
        "Two-factor authentication is required";
    Auth2faInvalid = "AUTH_2FA_INVALID", UNAUTHORIZED, retryable: false,
        "The two-factor code is invalid";
    Auth2faChallengeExpired = "AUTH_2FA_CHALLENGE_EXPIRED", UNAUTHORIZED, retryable: false,
        "The two-factor challenge is unknown or has expired";
    BackupCodeInvalid = "BACKUP_CODE_INVALID", UNAUTHORIZED, retryable: false,
        "The backup code is invalid";
    TotpCodeReplayed = "TOTP_CODE_REPLAYED", UNAUTHORIZED, retryable: false,
        "The two-factor code was already used";
    TotpAlreadyEnabled = "TOTP_ALREADY_ENABLED", CONFLICT, retryable: false,
        "Two-factor authentication is already enabled";
    TotpNotEnrolled = "TOTP_NOT_ENROLLED", CONFLICT, retryable: false,
        "Two-factor authentication is not enabled";
    TotpRequiredByPolicy = "TOTP_REQUIRED_BY_POLICY", FORBIDDEN, retryable: false,
        "Two-factor authentication is required by instance policy";
    TotpEnrollmentPendingMissing = "TOTP_ENROLLMENT_PENDING_MISSING", CONFLICT, retryable: false,
        "The two-factor enrollment was not found or has expired";
    SessionNotFound = "SESSION_NOT_FOUND", NOT_FOUND, retryable: false,
        "The session was not found";
    TrustedDeviceDisabled = "TRUSTED_DEVICE_DISABLED", FORBIDDEN, retryable: false,
        "Trusted devices are disabled";
    TrustedDeviceNotFound = "TRUSTED_DEVICE_NOT_FOUND", NOT_FOUND, retryable: false,
        "The trusted device was not found";
    PasswordCurrentInvalid = "PASSWORD_CURRENT_INVALID", FORBIDDEN, retryable: false,
        "The current password is incorrect";
    PasswordPolicyViolation = "PASSWORD_POLICY_VIOLATION", UNPROCESSABLE_ENTITY, retryable: false,
        "The password does not meet the password policy";
    ResetTokenInvalid = "RESET_TOKEN_INVALID", BAD_REQUEST, retryable: false,
        "The password reset link is invalid";
    ResetTokenExpired = "RESET_TOKEN_EXPIRED", GONE, retryable: false,
        "The password reset link has expired";
    ResetTokenUsed = "RESET_TOKEN_USED", GONE, retryable: false,
        "The password reset link was already used";
    EmailVerificationTokenInvalid = "EMAIL_VERIFICATION_TOKEN_INVALID", BAD_REQUEST, retryable: false,
        "The e-mail verification link is invalid";
    EmailVerificationTokenExpired = "EMAIL_VERIFICATION_TOKEN_EXPIRED", GONE, retryable: false,
        "The e-mail verification link has expired";
    EmailVerificationNotPending = "EMAIL_VERIFICATION_NOT_PENDING", CONFLICT, retryable: false,
        "The user has no pending e-mail change to resend";
    InviteNotFound = "INVITE_NOT_FOUND", NOT_FOUND, retryable: false,
        "The invite was not found";
    InviteExpired = "INVITE_EXPIRED", GONE, retryable: false,
        "The invite has expired";
    InviteAlreadyUsed = "INVITE_ALREADY_USED", GONE, retryable: false,
        "The invite was already used";
    InviteRevoked = "INVITE_REVOKED", GONE, retryable: false,
        "The invite was revoked";
    UserNotFound = "USER_NOT_FOUND", NOT_FOUND, retryable: false,
        "The user was not found";
    UserEmailTaken = "USER_EMAIL_TAKEN", CONFLICT, retryable: false,
        "The e-mail address is already in use";
    UserUsernameTaken = "USER_USERNAME_TAKEN", CONFLICT, retryable: false,
        "The username is already in use";
    LastAdminProtected = "LAST_ADMIN_PROTECTED", CONFLICT, retryable: false,
        "The action would leave the instance without an active administrator";
    UserHasNoLocalAuth = "USER_HAS_NO_LOCAL_AUTH", CONFLICT, retryable: false,
        "The user has no local password to reset";
    DatabaseBusy = "DATABASE_BUSY", SERVICE_UNAVAILABLE, retryable: true,
        "The database is temporarily busy";
    FileNotFound = "FILE_NOT_FOUND", NOT_FOUND, retryable: false,
        "The file was not found";
    FileNameConflict = "FILE_NAME_CONFLICT", CONFLICT, retryable: false,
        "No unique name could be generated for this item";
    FolderNotFound = "FOLDER_NOT_FOUND", NOT_FOUND, retryable: false,
        "The folder was not found";
    FolderDepthExceeded = "FOLDER_DEPTH_EXCEEDED", UNPROCESSABLE_ENTITY, retryable: false,
        "The folder would exceed the maximum nesting depth";
    FolderCycle = "FOLDER_CYCLE", UNPROCESSABLE_ENTITY, retryable: false,
        "A folder cannot be moved into itself or one of its own subfolders";
    NameInvalid = "NAME_INVALID", UNPROCESSABLE_ENTITY, retryable: false,
        "The name is not valid";
    RangeNotSatisfiable = "RANGE_NOT_SATISFIABLE", RANGE_NOT_SATISFIABLE, retryable: false,
        "The requested range is not satisfiable";
    StorageUnavailable = "STORAGE_UNAVAILABLE", SERVICE_UNAVAILABLE, retryable: true,
        "The storage backend is temporarily unavailable";
    QuotaExceeded = "QUOTA_EXCEEDED", INSUFFICIENT_STORAGE, retryable: false,
        "The storage quota would be exceeded";
    StorageFull = "STORAGE_FULL", INSUFFICIENT_STORAGE, retryable: true,
        "The storage device is out of space";
    StorageProviderMismatch = "STORAGE_PROVIDER_MISMATCH", INTERNAL_SERVER_ERROR, retryable: false,
        "The stored object belongs to a different storage provider";
    StorageSizeMismatch = "STORAGE_SIZE_MISMATCH", INTERNAL_SERVER_ERROR, retryable: false,
        "The stored object size does not match the expected size";
    SettingUnknown = "SETTING_UNKNOWN", UNPROCESSABLE_ENTITY, retryable: false,
        "The settings group has no such setting";
    SettingValueInvalid = "SETTING_VALUE_INVALID", UNPROCESSABLE_ENTITY, retryable: false,
        "The setting value is invalid";
    SettingBelowFloor = "SETTING_BELOW_FLOOR", UNPROCESSABLE_ENTITY, retryable: false,
        "The setting value is below the platform floor";
    BrandingAssetUnknown = "BRANDING_ASSET_UNKNOWN", NOT_FOUND, retryable: false,
        "The branding asset was not found";
    SmtpTestFailed = "SMTP_TEST_FAILED", BAD_GATEWAY, retryable: false,
        "The SMTP test failed";
    ProviderNotFound = "PROVIDER_NOT_FOUND", NOT_FOUND, retryable: false,
        "The identity provider was not found";
    ProviderDisabled = "PROVIDER_DISABLED", FORBIDDEN, retryable: false,
        "The identity provider is disabled";
    ProviderSlugTaken = "PROVIDER_SLUG_TAKEN", CONFLICT, retryable: false,
        "An identity provider with that slug already exists";
    ProviderDiscoveryFailed = "PROVIDER_DISCOVERY_FAILED", BAD_GATEWAY, retryable: true,
        "OIDC discovery failed";
    ProviderValidationFailed = "PROVIDER_VALIDATION_FAILED", UNPROCESSABLE_ENTITY, retryable: false,
        "The identity provider checks failed";
    ProviderHasLinks = "PROVIDER_HAS_LINKS", CONFLICT, retryable: false,
        "The identity provider still has identity links";
    ProviderIdTokenInvalid = "PROVIDER_ID_TOKEN_INVALID", UNAUTHORIZED, retryable: false,
        "The identity provider ID token is invalid";
    ProviderUserinfoFailed = "PROVIDER_USERINFO_FAILED", BAD_GATEWAY, retryable: true,
        "The identity provider userinfo request failed";
    ProviderSubjectMissing = "PROVIDER_SUBJECT_MISSING", UNAUTHORIZED, retryable: false,
        "The identity provider did not return a usable subject";
    ProviderStateInvalid = "PROVIDER_STATE_INVALID", BAD_REQUEST, retryable: false,
        "The authorization state is invalid or was already used";
    ProviderAuthDenied = "PROVIDER_AUTH_DENIED", UNAUTHORIZED, retryable: false,
        "The identity provider denied the authorization request";
    ProviderCodeExchangeFailed = "PROVIDER_CODE_EXCHANGE_FAILED", BAD_GATEWAY, retryable: true,
        "The identity provider rejected the authorization code exchange";
    ProviderEmailUnverified = "PROVIDER_EMAIL_UNVERIFIED", FORBIDDEN, retryable: false,
        "The identity provider did not assert a verified e-mail address";
    ProviderAutoProvisionDisabled = "PROVIDER_AUTO_PROVISION_DISABLED", FORBIDDEN, retryable: false,
        "No account matches this identity and automatic account creation is off";
    ProviderIdentityAlreadyLinked = "PROVIDER_IDENTITY_ALREADY_LINKED", CONFLICT, retryable: false,
        "The requested link conflicts with an existing identity binding";
    ProviderLinkNotFound = "PROVIDER_LINK_NOT_FOUND", NOT_FOUND, retryable: false,
        "The identity link does not exist";
    IdentityLinkLastLoginPath = "IDENTITY_LINK_LAST_LOGIN_PATH", CONFLICT, retryable: false,
        "Removing this identity link would remove the account's only login path";
    PasswordLoginDisableUnsafe = "PASSWORD_LOGIN_DISABLE_UNSAFE", CONFLICT, retryable: false,
        "The change would remove the last safe administrator login path";
    NoValidatedProvider = "NO_VALIDATED_PROVIDER", CONFLICT, retryable: false,
        "No enabled provider has been successfully tested";
    AuthAccountInactive = "AUTH_ACCOUNT_INACTIVE", FORBIDDEN, retryable: false,
        "The account is deactivated";
    AuthExternalAmbiguousIdentity = "AUTH_EXTERNAL_AMBIGUOUS_IDENTITY", INTERNAL_SERVER_ERROR, retryable: false,
        "The external identity matched more than one account";
    AuthExternalUsernameUnavailable = "AUTH_EXTERNAL_USERNAME_UNAVAILABLE", CONFLICT, retryable: false,
        "No username could be allocated for the new account";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CatalogEntry {
    pub code: ErrorCode,
    pub status: u16,
    pub retryable: bool,
}

impl ErrorCode {
    pub fn catalog() -> Vec<CatalogEntry> {
        let mut entries: Vec<CatalogEntry> = Self::ALL
            .iter()
            .map(|&code| CatalogEntry {
                code,
                status: code.status().as_u16(),
                retryable: code.retryable(),
            })
            .collect();
        entries.sort_by_key(|entry| entry.code.as_str());
        entries
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ErrorCode;

    const ERROR_STATUSES: [u16; 21] = [
        400, 401, 403, 404, 405, 408, 409, 410, 412, 413, 415, 416, 422, 423, 429, 460, 500, 501,
        502, 503, 507,
    ];

    #[test]
    fn unit_error_code_status_mapping() {
        for &code in ErrorCode::ALL {
            let status = code.status();
            assert!(
                ERROR_STATUSES.contains(&status.as_u16()),
                "{} maps to {status}",
                code.as_str()
            );
            assert!(status.is_client_error() || status.is_server_error());
        }

        let catalog = serde_json::to_string_pretty(&ErrorCode::catalog()).unwrap();
        insta::with_settings!({
            snapshot_path => "../../tests/snapshots",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!("error_catalog", catalog);
        });
    }

    #[test]
    fn unit_error_code_wire_names_are_stable_identifiers() {
        let mut seen = BTreeSet::new();
        for &code in ErrorCode::ALL {
            let wire = code.as_str();
            assert!(seen.insert(wire), "{wire} is registered twice");
            assert!(
                !wire.starts_with("CLIENT_"),
                "{wire} uses the client namespace"
            );
            assert!(!wire.starts_with('_') && !wire.ends_with('_') && !wire.contains("__"));
            assert!(wire.as_bytes()[0].is_ascii_uppercase());
            assert!(wire
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'));
            assert_eq!(serde_json::to_value(code).unwrap(), wire);
            assert!(!code.default_message().is_empty());
        }
        assert_eq!(seen.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn unit_error_catalog_is_sorted_and_complete() {
        let catalog = ErrorCode::catalog();
        assert_eq!(catalog.len(), ErrorCode::ALL.len());
        assert!(catalog
            .windows(2)
            .all(|pair| pair[0].code.as_str() < pair[1].code.as_str()));
        for entry in &catalog {
            assert_eq!(entry.status, entry.code.status().as_u16());
            assert_eq!(entry.retryable, entry.code.retryable());
        }
        assert_eq!(catalog, ErrorCode::catalog());
    }

    #[test]
    fn unit_error_code_retryability() {
        let retryable: BTreeSet<&str> = ErrorCode::ALL
            .iter()
            .filter(|code| code.retryable())
            .map(|code| code.as_str())
            .collect();
        assert_eq!(
            retryable,
            BTreeSet::from([
                "AUTH_LOCKED",
                "DATABASE_BUSY",
                "IDEMPOTENCY_REQUEST_IN_PROGRESS",
                "INTERNAL_ERROR",
                "PROVIDER_CODE_EXCHANGE_FAILED",
                "PROVIDER_DISCOVERY_FAILED",
                "PROVIDER_USERINFO_FAILED",
                "RATE_LIMITED",
                "SERVICE_UNAVAILABLE",
                "STORAGE_FULL",
                "STORAGE_UNAVAILABLE",
            ])
        );

        assert_eq!(
            ErrorCode::IdempotencyKeyConflict.status(),
            ErrorCode::IdempotencyRequestInProgress.status()
        );
        assert!(!ErrorCode::IdempotencyKeyConflict.retryable());
        assert!(ErrorCode::IdempotencyRequestInProgress.retryable());
    }

    #[test]
    fn unit_middleware_error_codes_match_catalog() {
        assert_eq!(
            ErrorCode::RequestBodyTooLarge.as_str(),
            "REQUEST_BODY_TOO_LARGE"
        );
        assert_eq!(ErrorCode::RequestBodyTooLarge.status().as_u16(), 413);
        assert!(!ErrorCode::RequestBodyTooLarge.retryable());

        assert_eq!(ErrorCode::RequestTimeout.as_str(), "REQUEST_TIMEOUT");
        assert_eq!(ErrorCode::RequestTimeout.status().as_u16(), 408);
        assert!(!ErrorCode::RequestTimeout.retryable());

        assert_eq!(ErrorCode::InternalError.status().as_u16(), 500);
        assert!(ErrorCode::InternalError.retryable());
    }
}
