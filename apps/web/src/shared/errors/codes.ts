import type { components } from "../api/schema";

export type ServerErrorCode = components["schemas"]["ErrorCode"];

export type ClientErrorCode =
  | "CLIENT_OFFLINE"
  | "CLIENT_NETWORK_ERROR"
  | "CLIENT_ABORTED"
  | "CLIENT_PROXY_BODY_LIMIT"
  | "CLIENT_PROXY_BAD_GATEWAY"
  | "CLIENT_PROXY_TIMEOUT"
  | "CLIENT_RATE_LIMITED"
  | "CLIENT_UNEXPECTED_RESPONSE";

export type ErrorCode = ServerErrorCode | ClientErrorCode;

export type ErrorDetails = Readonly<components["schemas"]["ApiErrorPayload"]["details"]>;
