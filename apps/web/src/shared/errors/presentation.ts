import { ApiError } from "./ApiError";
import type { ErrorCode } from "./codes";

export type ErrorSeverity = "error" | "warning" | "info";

export type ErrorSurface = "inline" | "toast" | "modal" | "page";

export type ErrorMessageKey = `message.${string}`;

export interface ErrorPresentation {
  i18nKey: ErrorMessageKey;
  severity: ErrorSeverity;
  retryable: boolean;
  surface: ErrorSurface;
  showRequestId: boolean;
  silent: boolean;
}

type Shape = Omit<ErrorPresentation, "i18nKey" | "silent">;

const inline = (severity: ErrorSeverity = "error"): Shape => ({
  severity,
  retryable: false,
  surface: "inline",
  showRequestId: false,
});
const toast = (severity: ErrorSeverity, retryable: boolean): Shape => ({
  severity,
  retryable,
  surface: "toast",
  showRequestId: true,
});
const modal = (retryable: boolean, severity: ErrorSeverity = "error"): Shape => ({
  severity,
  retryable,
  surface: "modal",
  showRequestId: true,
});
const page = (severity: ErrorSeverity): Shape => ({
  severity,
  retryable: false,
  surface: "page",
  showRequestId: true,
});

function entry(i18nKey: ErrorMessageKey, shape: Shape, silent = false): ErrorPresentation {
  return { i18nKey, ...shape, silent };
}

export const ERROR_PRESENTATION: Readonly<Record<ErrorCode, ErrorPresentation>> = {
  VALIDATION_ERROR: entry("message.validation", inline()),
  INVALID_JSON: entry("message.requestRejected", modal(false)),
  NOT_FOUND: entry("message.notFound", page("warning")),
  METHOD_NOT_ALLOWED: entry("message.requestRejected", modal(false)),
  FORBIDDEN: entry("message.forbidden", page("error")),
  UNSUPPORTED_MEDIA_TYPE: entry("message.requestRejected", modal(false)),
  REQUEST_BODY_TOO_LARGE: entry("message.requestTooLarge", modal(false)),
  REQUEST_TIMEOUT: entry("message.requestTimeout", toast("warning", false)),
  INTERNAL_ERROR: entry("message.internal", modal(true)),
  SERVICE_UNAVAILABLE: entry("message.serviceUnavailable", toast("warning", true)),
  CURSOR_INVALID: entry("message.listingChanged", inline("warning")),
  RATE_LIMITED: entry("message.rateLimited", { ...inline("warning"), retryable: true }),
  CSRF_TOKEN_MISSING: entry("message.securityCheckFailed", modal(false)),
  CSRF_TOKEN_INVALID: entry("message.securityCheckFailed", modal(false)),
  ORIGIN_NOT_ALLOWED: entry("message.securityCheckFailed", modal(false)),
  IDEMPOTENCY_KEY_CONFLICT: entry("message.idempotencyConflict", modal(false)),
  IDEMPOTENCY_REQUEST_IN_PROGRESS: entry("message.requestInProgress", toast("info", true)),
  BATCH_TOO_LARGE: entry("message.batchTooLarge", inline()),
  FEATURE_UNAVAILABLE_SMTP: entry("message.emailUnavailable", inline("warning")),
  SETUP_ALREADY_COMPLETED: entry("message.setupCompleted", page("info")),
  AUTH_REQUIRED: entry("message.sessionEnded", page("warning")),
  AUTH_INVALID_CREDENTIALS: entry("message.invalidCredentials", inline()),
  AUTH_LOCKED: entry("message.accountLocked", { ...inline("warning"), retryable: true }),
  AUTH_PASSWORD_LOGIN_DISABLED: entry("message.passwordLoginDisabled", inline("warning")),
  AUTH_RECENT_AUTH_REQUIRED: entry("message.recentAuthRequired", modal(false, "info")),
  AUTH_PASSWORD_CHANGE_REQUIRED: entry("message.passwordChangeRequired", page("warning")),
  AUTH_2FA_ENROLLMENT_REQUIRED: entry("message.twoFactorEnrollmentRequired", page("warning")),
  AUTH_2FA_REQUIRED: entry("message.unexpected", modal(false)),
  AUTH_2FA_INVALID: entry("message.twoFactorCodeInvalid", inline()),
  AUTH_2FA_CHALLENGE_EXPIRED: entry("message.twoFactorChallengeExpired", inline("warning")),
  BACKUP_CODE_INVALID: entry("message.backupCodeInvalid", inline()),
  TOTP_CODE_REPLAYED: entry("message.twoFactorCodeReplayed", inline()),
  TOTP_ALREADY_ENABLED: entry("message.twoFactorAlreadyEnabled", inline("warning")),
  TOTP_NOT_ENROLLED: entry("message.twoFactorNotEnabled", inline("warning")),
  TOTP_REQUIRED_BY_POLICY: entry("message.twoFactorRequiredByPolicy", inline("warning")),
  TOTP_ENROLLMENT_PENDING_MISSING: entry("message.twoFactorEnrollmentExpired", inline("warning")),
  SESSION_NOT_FOUND: entry("message.sessionNotFound", inline("warning")),
  TRUSTED_DEVICE_DISABLED: entry("message.trustedDeviceDisabled", inline("warning")),
  TRUSTED_DEVICE_NOT_FOUND: entry("message.trustedDeviceNotFound", inline("warning")),
  PASSWORD_CURRENT_INVALID: entry("message.currentPasswordInvalid", inline()),
  PASSWORD_POLICY_VIOLATION: entry("message.passwordPolicy", inline()),
  RESET_TOKEN_INVALID: entry("message.resetLinkInvalid", inline("warning")),
  RESET_TOKEN_EXPIRED: entry("message.resetLinkExpired", inline("warning")),
  RESET_TOKEN_USED: entry("message.resetLinkUsed", inline("warning")),
  INVITE_NOT_FOUND: entry("message.inviteNotFound", inline("warning")),
  INVITE_EXPIRED: entry("message.inviteExpired", inline("warning")),
  INVITE_ALREADY_USED: entry("message.inviteUsed", inline("warning")),
  INVITE_REVOKED: entry("message.inviteRevoked", inline("warning")),
  USER_EMAIL_TAKEN: entry("message.emailTaken", inline()),
  USER_USERNAME_TAKEN: entry("message.usernameTaken", inline()),
  USER_NOT_FOUND: entry("message.userNotFound", inline("warning")),
  LAST_ADMIN_PROTECTED: entry("message.lastAdminProtected", inline("warning")),
  USER_HAS_NO_LOCAL_AUTH: entry("message.userHasNoLocalAuth", inline("warning")),
  DATABASE_BUSY: entry("message.serverBusy", toast("warning", true)),
  FILE_NOT_FOUND: entry("message.fileNotFound", inline()),
  RANGE_NOT_SATISFIABLE: entry("message.rangeNotSatisfiable", inline()),
  STORAGE_UNAVAILABLE: entry("message.storageUnavailable", modal(true)),
  STORAGE_FULL: entry("message.storageFull", modal(true)),
  STORAGE_PROVIDER_MISMATCH: entry("message.storageProviderMismatch", modal(false)),
  STORAGE_SIZE_MISMATCH: entry("message.storageSizeMismatch", modal(false)),
  BRANDING_ASSET_UNKNOWN: entry("message.brandingAssetUnknown", inline()),

  CLIENT_OFFLINE: entry("message.offline", { ...toast("warning", true), showRequestId: false }),
  CLIENT_NETWORK_ERROR: entry("message.networkError", {
    ...toast("error", true),
    showRequestId: false,
  }),
  CLIENT_ABORTED: entry("message.aborted", inline("info"), true),
  CLIENT_PROXY_BODY_LIMIT: entry("message.proxyBodyLimit", modal(false)),
  CLIENT_PROXY_BAD_GATEWAY: entry("message.proxyBadGateway", toast("error", true)),
  CLIENT_PROXY_TIMEOUT: entry("message.proxyTimeout", toast("error", true)),
  CLIENT_RATE_LIMITED: entry("message.rateLimited", { ...inline("warning"), retryable: true }),
  CLIENT_UNEXPECTED_RESPONSE: entry("message.unexpectedResponse", modal(false)),
};

export const UNKNOWN_ERROR_PRESENTATION: ErrorPresentation = entry(
  "message.unexpected",
  modal(false),
);

export interface PresentedError {
  presentation: ErrorPresentation;
  code: string | null;
  known: boolean;
  requestId: string | null;
  retryAfterSeconds: number | null;
}

export function isKnownErrorCode(code: string): code is ErrorCode {
  return Object.hasOwn(ERROR_PRESENTATION, code);
}

export function presentError(error: unknown): PresentedError {
  if (!(error instanceof ApiError)) {
    return {
      presentation: UNKNOWN_ERROR_PRESENTATION,
      code: null,
      known: false,
      requestId: null,
      retryAfterSeconds: null,
    };
  }
  const known = isKnownErrorCode(error.code);
  return {
    presentation: known ? ERROR_PRESENTATION[error.code] : UNKNOWN_ERROR_PRESENTATION,
    code: error.code,
    known,
    requestId: error.requestId,
    retryAfterSeconds: error.retryAfterSeconds,
  };
}

export function isApiErrorCode(error: unknown, code: ErrorCode): error is ApiError {
  return error instanceof ApiError && error.code === code;
}
