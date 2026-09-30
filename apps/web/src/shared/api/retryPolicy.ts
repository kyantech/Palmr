import { ApiError, type ErrorCode } from "../errors";

export type RetryClass = "transient" | "serverFault" | "rateLimited" | "never";

export type RetryOperation = "query" | "idempotentMutation" | "mutation";

export const RETRY_CLASS: Readonly<Record<ErrorCode, RetryClass>> = {
  CLIENT_OFFLINE: "transient",
  CLIENT_NETWORK_ERROR: "transient",
  CLIENT_PROXY_BAD_GATEWAY: "transient",
  CLIENT_PROXY_TIMEOUT: "transient",
  CLIENT_ABORTED: "never",
  CLIENT_PROXY_BODY_LIMIT: "never",
  CLIENT_UNEXPECTED_RESPONSE: "never",
  CLIENT_RATE_LIMITED: "rateLimited",

  INTERNAL_ERROR: "serverFault",
  SERVICE_UNAVAILABLE: "serverFault",
  DATABASE_BUSY: "serverFault",
  STORAGE_UNAVAILABLE: "serverFault",
  STORAGE_FULL: "serverFault",

  RATE_LIMITED: "rateLimited",
  IDEMPOTENCY_REQUEST_IN_PROGRESS: "rateLimited",

  AUTH_REQUIRED: "never",
  AUTH_RECENT_AUTH_REQUIRED: "never",
  AUTH_INVALID_CREDENTIALS: "never",
  AUTH_LOCKED: "never",
  AUTH_PASSWORD_LOGIN_DISABLED: "never",
  AUTH_PASSWORD_CHANGE_REQUIRED: "never",
  AUTH_2FA_ENROLLMENT_REQUIRED: "never",
  AUTH_2FA_REQUIRED: "never",
  AUTH_2FA_INVALID: "never",
  AUTH_2FA_CHALLENGE_EXPIRED: "never",
  BACKUP_CODE_INVALID: "never",
  TOTP_CODE_REPLAYED: "never",
  TOTP_ALREADY_ENABLED: "never",
  TOTP_NOT_ENROLLED: "never",
  TOTP_REQUIRED_BY_POLICY: "never",
  TOTP_ENROLLMENT_PENDING_MISSING: "never",
  SESSION_NOT_FOUND: "never",
  TRUSTED_DEVICE_DISABLED: "never",
  TRUSTED_DEVICE_NOT_FOUND: "never",
  PASSWORD_CURRENT_INVALID: "never",
  PASSWORD_POLICY_VIOLATION: "never",
  RESET_TOKEN_INVALID: "never",
  RESET_TOKEN_EXPIRED: "never",
  RESET_TOKEN_USED: "never",
  EMAIL_VERIFICATION_TOKEN_INVALID: "never",
  EMAIL_VERIFICATION_TOKEN_EXPIRED: "never",
  EMAIL_VERIFICATION_NOT_PENDING: "never",
  INVITE_NOT_FOUND: "never",
  INVITE_EXPIRED: "never",
  INVITE_ALREADY_USED: "never",
  INVITE_REVOKED: "never",

  VALIDATION_ERROR: "never",
  INVALID_JSON: "never",
  NOT_FOUND: "never",
  METHOD_NOT_ALLOWED: "never",
  FORBIDDEN: "never",
  UNSUPPORTED_MEDIA_TYPE: "never",
  REQUEST_BODY_TOO_LARGE: "never",
  REQUEST_TIMEOUT: "never",
  CURSOR_INVALID: "never",
  CSRF_TOKEN_MISSING: "never",
  CSRF_TOKEN_INVALID: "never",
  ORIGIN_NOT_ALLOWED: "never",
  IDEMPOTENCY_KEY_CONFLICT: "never",
  BATCH_TOO_LARGE: "never",
  FEATURE_UNAVAILABLE_SMTP: "never",
  SETUP_ALREADY_COMPLETED: "never",
  USER_EMAIL_TAKEN: "never",
  USER_USERNAME_TAKEN: "never",
  USER_NOT_FOUND: "never",
  LAST_ADMIN_PROTECTED: "never",
  USER_HAS_NO_LOCAL_AUTH: "never",
  FILE_NOT_FOUND: "never",
  RANGE_NOT_SATISFIABLE: "never",
  STORAGE_PROVIDER_MISMATCH: "never",
  STORAGE_SIZE_MISMATCH: "never",
  BRANDING_ASSET_UNKNOWN: "never",
};

const MAX_RETRIES: Readonly<Record<RetryClass, number>> = {
  transient: 3,
  serverFault: 2,
  rateLimited: 1,
  never: 0,
};

const ALLOWED_CLASSES: Readonly<Record<RetryOperation, ReadonlySet<RetryClass>>> = {
  query: new Set(["transient", "serverFault", "rateLimited"]),
  idempotentMutation: new Set(["transient", "rateLimited"]),
  mutation: new Set(["rateLimited"]),
};

const IDEMPOTENT_METHODS: ReadonlySet<string> = new Set(["PUT", "DELETE"]);

export const BACKOFF_BASE_MS = 1_000;
export const BACKOFF_CAP_MS = 15_000;
export const BACKOFF_JITTER = 0.2;
export const RETRY_AFTER_CAP_MS = 60_000;

export type RandomSource = () => number;

export function retryClassOf(error: unknown): RetryClass {
  if (!(error instanceof ApiError) || !Object.hasOwn(RETRY_CLASS, error.code)) {
    return "never";
  }
  return RETRY_CLASS[error.code];
}

export function shouldRetry(operation: RetryOperation, failureCount: number, error: unknown) {
  const retryClass = retryClassOf(error);
  return ALLOWED_CLASSES[operation].has(retryClass) && failureCount < MAX_RETRIES[retryClass];
}

export function retryDelay(
  failureCount: number,
  error: unknown,
  random: RandomSource = Math.random,
): number {
  if (
    retryClassOf(error) === "rateLimited" &&
    error instanceof ApiError &&
    error.retryAfterSeconds !== null
  ) {
    return Math.min(error.retryAfterSeconds * 1_000, RETRY_AFTER_CAP_MS);
  }
  const base = Math.min(BACKOFF_BASE_MS * 2 ** failureCount, BACKOFF_CAP_MS);
  const jitter = 1 - BACKOFF_JITTER + 2 * BACKOFF_JITTER * random();
  return Math.round(base * jitter);
}

export function mutationOperationOf(error: unknown): RetryOperation {
  return error instanceof ApiError && IDEMPOTENT_METHODS.has(error.request.method)
    ? "idempotentMutation"
    : "mutation";
}

export interface RetryOptions {
  retry: (failureCount: number, error: unknown) => boolean;
  retryDelay: (failureCount: number, error: unknown) => number;
}

export function retryOptions(
  operation: RetryOperation | ((error: unknown) => RetryOperation),
  random: RandomSource = Math.random,
): RetryOptions {
  const operationOf = typeof operation === "function" ? operation : () => operation;
  return {
    retry: (failureCount, error) => shouldRetry(operationOf(error), failureCount, error),
    retryDelay: (failureCount, error) => retryDelay(failureCount, error, random),
  };
}

export const idempotentMutationRetry: RetryOptions = retryOptions("idempotentMutation");
