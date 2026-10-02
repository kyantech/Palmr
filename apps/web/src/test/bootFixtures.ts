import { delay, http, HttpResponse } from "msw";
import type { Bootstrap, Me, Restriction } from "../app/bootstrap/queries";
import { SUPPORTED_LOCALES } from "../app/i18n/catalog";

export const BOOTSTRAP_URL = "*/api/v1/bootstrap";
export const ME_URL = "*/api/v1/auth/me";

export function bootstrapFixture(overrides: Partial<Bootstrap> = {}): Bootstrap {
  return {
    setupCompleted: true,
    appName: "Palmr",
    appDescription: "Self-hosted file transfer",
    logoUrl: null,
    faviconUrl: null,
    primaryColor: "#1668dc",
    defaultLocale: "en-US",
    supportedLocales: [...SUPPORTED_LOCALES],
    passwordLoginEnabled: true,
    providers: [],
    poweredByVisible: true,
    version: "4.0.0",
    ...overrides,
  };
}

interface MeOptions {
  role?: "admin" | "user";
  restriction?: Restriction | null;
  locale?: string;
  theme?: string;
  accent?: string;
  capabilities?: Partial<Me["capabilities"]>;
  recentAuthUntil?: string;
}

export function meFixture({
  role = "user",
  restriction = null,
  locale = "en-US",
  theme = "system",
  accent = "default",
  capabilities = {},
  recentAuthUntil = "2026-09-28T00:10:00Z",
}: MeOptions = {}): Me {
  return {
    user: {
      id: "019a0000-0000-7000-8000-000000000001",
      email: "ada@example.test",
      username: "ada",
      firstName: "Ada",
      lastName: "Lovelace",
      role,
      isActive: true,
      locale,
      theme,
      accent,
      avatarUrl: null,
      pendingEmail: null,
      createdAt: "2026-09-01T00:00:00Z",
    },
    session: {
      id: "019a0000-0000-7000-8000-0000000000aa",
      createdAt: "2026-09-28T00:00:00Z",
      lastSeenAt: "2026-09-28T00:00:00Z",
      expiresAt: "2026-10-05T00:00:00Z",
      recentAuthUntil,
    },
    restriction,
    capabilities: {
      canChangePassword: true,
      hasLocalPassword: true,
      identityLinkCount: 0,
      twoFactorEnabled: false,
      ...capabilities,
    },
  };
}

export function errorEnvelope(
  code: string,
  status: number,
  requestId: string,
  {
    message = code,
    headers = {},
    details = {},
  }: {
    message?: string;
    headers?: Record<string, string>;
    details?: Record<string, boolean | number | string | string[]>;
  } = {},
) {
  return HttpResponse.json(
    { error: { code, message, requestId, details } },
    { status, headers: { "X-Request-Id": requestId, ...headers } },
  );
}

export interface BootHandlerOptions {
  bootstrap?: Bootstrap;
  me?: Me | null;
  latencyMs?: number;
}

export function bootHandlers({
  bootstrap = bootstrapFixture(),
  me = null,
  latencyMs = 0,
}: BootHandlerOptions = {}) {
  const calls = { bootstrap: 0, me: 0 };
  const handlers = [
    http.get(BOOTSTRAP_URL, async () => {
      calls.bootstrap += 1;
      await delay(latencyMs);
      return HttpResponse.json(bootstrap);
    }),
    http.get(ME_URL, async () => {
      calls.me += 1;
      await delay(latencyMs);
      return me === null
        ? errorEnvelope("AUTH_REQUIRED", 401, "req-anonymous")
        : HttpResponse.json(me);
    }),
  ];
  return { calls, handlers };
}
