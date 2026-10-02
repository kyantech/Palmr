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
