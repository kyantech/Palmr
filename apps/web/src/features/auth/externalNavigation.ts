import { ApiError, type ApiRequestDescription } from "../../shared/errors";

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

export function providerUrlOrFail(value: unknown, request: ApiRequestDescription): string {
  const url = safeProviderUrl(value);
  if (url === null) {
    throw new ApiError({
      code: "CLIENT_UNEXPECTED_RESPONSE",
      status: 200,
      requestId: null,
      details: {},
      request,
      serverMessage: "CLIENT_UNEXPECTED_RESPONSE",
    });
  }
  return url;
}
