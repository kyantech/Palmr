import { ApiError, type ApiRequestDescription } from "../../shared/errors";
import { isExternalReauthChannelId } from "./externalReauthChannel";

export interface ExternalReauthStart {
  readonly url: string;
  readonly channel: string;
}

export const externalNavigation = {
  assign(url: string): void {
    window.location.assign(url);
  },
};

export function safeProviderUrl(value: unknown): string | null {
  if (typeof value !== "string" || value === "") {
    return null;
  }
  try {
    const { protocol } = new URL(value);
    return protocol === "https:" || protocol === "http:" ? value : null;
  } catch {
    return null;
  }
}

function unexpectedResponse(request: ApiRequestDescription): ApiError {
  return new ApiError({
    code: "CLIENT_UNEXPECTED_RESPONSE",
    status: 200,
    requestId: null,
    details: {},
    request,
    serverMessage: "CLIENT_UNEXPECTED_RESPONSE",
  });
}

export function providerUrlOrFail(value: unknown, request: ApiRequestDescription): string {
  const url = safeProviderUrl(value);
  if (url === null) {
    throw unexpectedResponse(request);
  }
  return url;
}

export function externalReauthChannelOrFail(
  value: unknown,
  request: ApiRequestDescription,
): string {
  if (!isExternalReauthChannelId(value)) {
    throw unexpectedResponse(request);
  }
  return value;
}
