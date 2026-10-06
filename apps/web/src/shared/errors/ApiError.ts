import type { ErrorCode, ErrorDetails } from "./codes";

export interface ApiRequestDescription {
  readonly method: string;
  readonly path: string;
}

export interface ApiErrorInit {
  code: ErrorCode;
  status: number;
  requestId: string | null;
  details: ErrorDetails;
  request: ApiRequestDescription;
  serverMessage: string;
  retryAfterSeconds?: number | null;
  cause?: unknown;
}

export class ApiError extends Error {
  override readonly name = "ApiError";
  readonly code: ErrorCode;
  readonly status: number;
  readonly requestId: string | null;
  readonly details: ErrorDetails;
  readonly request: ApiRequestDescription;
  readonly retryAfterSeconds: number | null;

  constructor({
    code,
    status,
    requestId,
    details,
    request,
    serverMessage,
    retryAfterSeconds = null,
    cause,
  }: ApiErrorInit) {
    super(serverMessage, cause === undefined ? undefined : { cause });
    this.code = code;
    this.status = status;
    this.requestId = requestId;
    this.details = details;
    this.request = request;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

export function detailFields(error: ApiError): string[] {
  const fields = error.details.fields;
  return (Array.isArray(fields) ? fields : []).filter(
    (field): field is string => typeof field === "string",
  );
}

export interface ProviderCheck {
  readonly name: string;
  readonly ok: boolean;
  readonly detail: string | null;
}

export interface Blocker {
  readonly code: string;
  readonly detail: string;
}

function recordsOf(value: unknown): readonly Record<string, unknown>[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is Record<string, unknown> => typeof item === "object" && item !== null,
      )
    : [];
}

export function detailChecks(error: ApiError): ProviderCheck[] {
  return recordsOf(error.details.checks).flatMap((item) =>
    typeof item.name === "string" && typeof item.ok === "boolean"
      ? [
          {
            name: item.name,
            ok: item.ok,
            detail: typeof item.detail === "string" ? item.detail : null,
          },
        ]
      : [],
  );
}

export function detailBlockers(error: ApiError): Blocker[] {
  return recordsOf(error.details.blockers).flatMap((item) =>
    typeof item.code === "string" && typeof item.detail === "string"
      ? [{ code: item.code, detail: item.detail }]
      : [],
  );
}
