import { delay, http, HttpResponse } from "msw";
import type { Bootstrap, Me } from "../app/bootstrap/queries";
import type { components } from "../shared/api/schema";
import { BOOTSTRAP_URL, bootstrapFixture, errorEnvelope, ME_URL, meFixture } from "./bootFixtures";
import { server } from "./server";

type SessionItem = components["schemas"]["SessionItem"];
type Preferences = components["schemas"]["Preferences"];
type EffectiveSettings = components["schemas"]["EffectiveSettings"];

const API = "*/api/v1";

export const CURRENT_SESSION_ID = meFixture().session.id;
export const OTHER_SESSION_ID = "019a0000-0000-7000-8000-0000000000bb";
export const PROXIED_SESSION_ID = "019a0000-0000-7000-8000-0000000000cc";

export const CHROME_MAC =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
export const FIREFOX_WINDOWS =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0";

export function sessionFixture(overrides: Partial<SessionItem> & { id: string }): SessionItem {
  return {
    isCurrent: false,
    createdAt: "2026-09-20T08:00:00Z",
    lastSeenAt: "2026-09-27T08:00:00Z",
    expiresAt: "2026-10-04T08:00:00Z",
    absoluteExpiresAt: "2026-10-20T08:00:00Z",
    ipAddress: "198.51.100.7",
    userAgent: FIREFOX_WINDOWS,
    origin: "password",
    ...overrides,
  };
}

export function defaultSessions(): SessionItem[] {
  return [
    sessionFixture({ id: OTHER_SESSION_ID }),
    sessionFixture({
      id: CURRENT_SESSION_ID,
      isCurrent: true,
      ipAddress: "203.0.113.24",
      userAgent: CHROME_MAC,
      lastSeenAt: "2026-09-28T00:00:00Z",
    }),
    sessionFixture({
      id: PROXIED_SESSION_ID,
      ipAddress: null,
      userAgent: null,
      origin: "external",
      lastSeenAt: "2026-09-26T08:00:00Z",
    }),
  ];
}

export interface SettingsServerOptions {
  me?: Me;
  bootstrap?: Bootstrap;
  sessions?: SessionItem[];
  recentAuth?: boolean;
  preferencesFailure?: () => Response;
  passwordFailure?: () => Response;
  latencyMs?: number;
}

export interface SettingsServerState {
  me: Me | null;
  sessions: SessionItem[];
  recentAuth: boolean;
  calls: {
    bootstrap: number;
    me: number;
    sessions: number;
    reauthenticate: number;
  };
  profileBodies: unknown[];
  preferenceBodies: unknown[];
  passwordBodies: unknown[];
  reauthBodies: unknown[];
  revoked: string[];
  bulkRevokeQueries: string[];
}

function effectiveSettings(): EffectiveSettings {
  return {
    aliasPattern: "^[A-Za-z0-9_-]{3,64}$",
    maxConcurrentTransfers: 3,
    maxFileSizeBytes: null,
    maxPublicLinkLifetimeDays: null,
    passwordMinLength: 8,
    publicLinkPasswordMinLength: 8,
    quotaBytes: null,
    receivedRetentionMaxDays: null,
    smtpConfigured: false,
    storageProvider: "local",
    trustedDeviceDurationDays: 30,
    trustedDevicesEnabled: true,
    twoFactorRequired: false,
  };
}

function recentAuthRequired() {
  return errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent-auth", {
    message: "recent authentication required",
  });
}

export function installSettingsServer({
  me = meFixture(),
  bootstrap = bootstrapFixture(),
  sessions = defaultSessions(),
  recentAuth = false,
  preferencesFailure,
  passwordFailure,
  latencyMs = 0,
}: SettingsServerOptions = {}): SettingsServerState {
  const state: SettingsServerState = {
    me,
    sessions,
    recentAuth,
    calls: { bootstrap: 0, me: 0, sessions: 0, reauthenticate: 0 },
    profileBodies: [],
    preferenceBodies: [],
    passwordBodies: [],
    reauthBodies: [],
    revoked: [],
    bulkRevokeQueries: [],
  };
  const signedIn = () =>
    state.me === null ? errorEnvelope("AUTH_REQUIRED", 401, "req-401") : null;
  const preferences = (current: Me): Preferences => ({
    locale: current.user.locale,
    theme: current.user.theme,
    accent: current.user.accent,
  });

  server.use(
    http.get(BOOTSTRAP_URL, () => {
      state.calls.bootstrap += 1;
      return HttpResponse.json(bootstrap);
    }),
    http.get(ME_URL, () => {
      state.calls.me += 1;
      return signedIn() ?? HttpResponse.json(state.me);
    }),
    http.get(
      `${API}/identity-links`,
      () => signedIn() ?? HttpResponse.json({ items: [], nextCursor: null, totalCount: 0 }),
    ),
    http.get(`${API}/profile`, () => signedIn() ?? HttpResponse.json(state.me?.user)),
    http.patch(`${API}/profile`, async ({ request }) => {
      const body = (await request.json()) as { firstName: string; lastName: string };
      state.profileBodies.push(body);
      if (state.me === null) {
        return errorEnvelope("AUTH_REQUIRED", 401, "req-401");
      }
      state.me = { ...state.me, user: { ...state.me.user, ...body } };
      return HttpResponse.json(state.me.user);
    }),
    http.get(
      `${API}/profile/preferences`,
      () => signedIn() ?? HttpResponse.json(state.me === null ? null : preferences(state.me)),
    ),
    http.patch(`${API}/profile/preferences`, async ({ request }) => {
      const body = (await request.json()) as Partial<Preferences>;
      state.preferenceBodies.push(body);
      await delay(latencyMs);
      if (preferencesFailure !== undefined) {
        return preferencesFailure();
      }
      if (state.me === null) {
        return errorEnvelope("AUTH_REQUIRED", 401, "req-401");
      }
      state.me = { ...state.me, user: { ...state.me.user, ...body } };
      return HttpResponse.json(preferences(state.me));
    }),
    http.get(
      `${API}/settings/effective`,
      () => signedIn() ?? HttpResponse.json(effectiveSettings()),
    ),
    http.post(`${API}/auth/reauthenticate`, async ({ request }) => {
      state.calls.reauthenticate += 1;
      state.reauthBodies.push(await request.json());
      state.recentAuth = true;
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${API}/profile/password`, async ({ request }) => {
      state.passwordBodies.push(await request.json());
      if (!state.recentAuth) {
        return recentAuthRequired();
      }
      if (passwordFailure !== undefined) {
        return passwordFailure();
      }
      state.sessions = state.sessions.filter((session) => session.isCurrent);
      return new HttpResponse(null, { status: 204 });
    }),
    http.get(`${API}/sessions`, () => {
      state.calls.sessions += 1;
      return (
        signedIn() ??
        HttpResponse.json({
          items: state.sessions,
          nextCursor: null,
          totalCount: state.sessions.length,
        })
      );
    }),
    http.delete(`${API}/sessions/:id`, ({ params }) => {
      const id = String(params.id);
      const target = state.sessions.find((session) => session.id === id);
      if (target === undefined) {
        return errorEnvelope("SESSION_NOT_FOUND", 404, "req-missing-session");
      }
      state.revoked.push(id);
      state.sessions = state.sessions.filter((session) => session.id !== id);
      if (target.isCurrent) {
        state.me = null;
      }
      return new HttpResponse(null, { status: 204 });
    }),
    http.delete(`${API}/sessions`, ({ request }) => {
      state.bulkRevokeQueries.push(new URL(request.url).search);
      if (!state.recentAuth) {
        return recentAuthRequired();
      }
      state.sessions = state.sessions.filter((session) => session.isCurrent);
      return new HttpResponse(null, { status: 204 });
    }),
  );
  return state;
}
