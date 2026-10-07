import { type ReportedError, reportedError } from "../../shared/errors";
import { isExternalReauthChannelId } from "./externalReauthChannel";

export const EXTERNAL_REAUTH_MESSAGE_TYPE = "palmr:external-reauth";

export type ExternalReauthMessage =
  | { readonly type: typeof EXTERNAL_REAUTH_MESSAGE_TYPE; readonly status: "success" }
  | {
      readonly type: typeof EXTERNAL_REAUTH_MESSAGE_TYPE;
      readonly status: "error";
      readonly error: string;
      readonly requestId: string | null;
    };

export type ExternalReauthLanding =
  { readonly status: "success" } | { readonly status: "error"; readonly reported: ReportedError };

const SUCCESS_KEYS = ["status", "type"];
const ERROR_KEYS = ["error", "requestId", "status", "type"];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
  const keys = Object.keys(value).sort();
  return keys.length === expected.length && keys.every((key, index) => key === expected[index]);
}

export interface ExternalReauthCompletion {
  readonly channel: string;
  readonly landing: ExternalReauthLanding;
}

export function readExternalReauthCompletion(search: string): ExternalReauthCompletion | null {
  const params = new URLSearchParams(search);
  const channel = params.get("channel");
  if (!isExternalReauthChannelId(channel)) {
    return null;
  }
  const status = params.get("status");
  if (status === "success") {
    return { channel, landing: { status } };
  }
  if (status === "error") {
    const reported = reportedError(params.get("error"), params.get("requestId"));
    return reported === null ? null : { channel, landing: { status, reported } };
  }
  return null;
}

export function messageForLanding(landing: ExternalReauthLanding): ExternalReauthMessage {
  if (landing.status === "success") {
    return { type: EXTERNAL_REAUTH_MESSAGE_TYPE, status: "success" };
  }
  return {
    type: EXTERNAL_REAUTH_MESSAGE_TYPE,
    status: "error",
    error: landing.reported.code,
    requestId: landing.reported.requestId,
  };
}

export function parseExternalReauthMessage(data: unknown): ExternalReauthLanding | null {
  if (!isRecord(data) || data.type !== EXTERNAL_REAUTH_MESSAGE_TYPE) {
    return null;
  }
  if (data.status === "success") {
    return hasExactKeys(data, SUCCESS_KEYS) ? { status: "success" } : null;
  }
  if (data.status !== "error" || !hasExactKeys(data, ERROR_KEYS)) {
    return null;
  }
  const { error, requestId } = data;
  if (typeof error !== "string" || (requestId !== null && typeof requestId !== "string")) {
    return null;
  }
  const reported = reportedError(error, requestId);
  if (reported === null) {
    return null;
  }
  return reported.requestId === requestId ? { status: "error", reported } : null;
}
