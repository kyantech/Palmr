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
  cause?: unknown;
}

export class ApiError extends Error {
  override readonly name = "ApiError";
  readonly code: ErrorCode;
  readonly status: number;
  readonly requestId: string | null;
  readonly details: ErrorDetails;
  readonly request: ApiRequestDescription;

  constructor({ code, status, requestId, details, request, serverMessage, cause }: ApiErrorInit) {
    super(serverMessage, cause === undefined ? undefined : { cause });
    this.code = code;
    this.status = status;
    this.requestId = requestId;
    this.details = details;
    this.request = request;
  }
}
