import { http, HttpResponse } from "msw";
import type { Me } from "../app/bootstrap/queries";
import type { components } from "../shared/api/schema";
import { bootHandlers, errorEnvelope, ME_URL, meFixture } from "./bootFixtures";
import { server } from "./server";

type Schemas = components["schemas"];
type UserDetail = Schemas["AdminUserDetail"];
type UserRow = Schemas["AdminUserItem"];
type Invite = Schemas["InviteItem"];
type Session = Schemas["SessionItem"];
type General = Schemas["GeneralSettings"];
type Security = Schemas["SecuritySettings"];
type Quotas = Schemas["QuotaSettings"];
type PublicLinks = Schemas["PublicLinkSettings"];
type Smtp = Schemas["SmtpSettings"];

const API = "*/api/v1";

export const ADMIN_ID = meFixture().user.id;
export const GRACE_ID = "019a0000-0000-7000-8000-000000000101";
export const LINUS_ID = "019a0000-0000-7000-8000-000000000102";
export const SOLO_ID = "019a0000-0000-7000-8000-000000000103";

export const GIB = 1024 ** 3;

export function userFixture(overrides: Partial<UserDetail> & { id: string }): UserDetail {
  return {
    firstName: "Grace",
    lastName: "Hopper",
    username: "grace",
    email: "grace@example.test",
    pendingEmail: null,
    role: "user",
    isActive: true,
    isLockedOut: false,
    hasLocalPassword: true,
    mustChangePassword: false,
    twoFactorEnabled: false,
    identityLinkCount: 0,
    usedBytes: 2 * GIB,
    quotaBytes: null,
    effectiveQuotaBytes: 10 * GIB,
    counts: { files: 12, receivedFiles: 3, shares: 2, reverseShares: 1 },
    lastLoginAt: "2026-09-27T08:00:00Z",
    createdAt: "2026-09-01T08:00:00Z",
    overQuota: false,
    quotaOverrideMode: "inherit",
    sessionCount: 1,
    trustedDeviceCount: 0,
    lockout: { failedCount: 0, lockCount: 0, lockedUntil: null },
    identityLinks: [],
    ...overrides,
  };
}

export function defaultUsers(): UserDetail[] {
  return [
    userFixture({
      id: ADMIN_ID,
      firstName: "Ada",
      lastName: "Lovelace",
      username: "ada",
      email: "ada@example.test",
      role: "admin",
      usedBytes: 1024 ** 2,
      effectiveQuotaBytes: null,
      createdAt: "2026-09-01T00:00:00Z",
    }),
    userFixture({ id: GRACE_ID }),
    userFixture({
      id: LINUS_ID,
      firstName: "Linus",
      lastName: "Torvalds",
      username: "linus",
      email: "linus@example.test",
      usedBytes: 12 * GIB,
      quotaBytes: 10 * GIB,
      quotaOverrideMode: "bytes",
      effectiveQuotaBytes: 10 * GIB,
      overQuota: true,
      isActive: false,
      createdAt: "2026-09-03T08:00:00Z",
    }),
    userFixture({
      id: SOLO_ID,
      firstName: "Sol",
      lastName: "Single",
      username: "sol",
      email: "sol@example.test",
      hasLocalPassword: false,
      identityLinkCount: 1,
      identityLinks: [
        {
          id: "019a0000-0000-7000-8000-000000000201",
          providerKey: "pocket-id",
          providerName: "Pocket ID",
          state: "active",
          linkMethod: "manual",
          createdAt: "2026-09-02T08:00:00Z",
          lastLoginAt: null,
        },
      ],
      usedBytes: 0,
      effectiveQuotaBytes: null,
      createdAt: "2026-09-04T08:00:00Z",
    }),
  ];
}

export function inviteFixture(overrides: Partial<Invite> & { id: string }): Invite {
  return {
    email: "invitee@example.test",
    role: "user",
    status: "pending",
    createdAt: "2026-09-20T08:00:00Z",
    expiresAt: "2026-09-30T08:00:00Z",
    lastSentAt: null,
    acceptedAt: null,
    acceptedUserId: null,
    createdBy: { id: ADMIN_ID, username: "ada" },
    ...overrides,
  };
}

export function defaultInvites(): Invite[] {
  return [
    inviteFixture({ id: "019a0000-0000-7000-8000-000000000301" }),
    inviteFixture({
      id: "019a0000-0000-7000-8000-000000000302",
      email: "used@example.test",
      status: "accepted",
    }),
  ];
}

export function defaultSettings() {
  const general: General = {
    appName: "Palmr",
    appDescription: "Self-hosted file transfer",
    defaultLocale: "en-US",
    hideVersion: false,
    poweredByVisible: true,
    thumbnailSourceLimit: "128MiB",
  };
  const security: Security = {
    passwordMinLength: 8,
    publicLinkPasswordMinLength: 8,
    maxLoginAttempts: 5,
    loginLockoutMinutes: 15,
    sessionIdleDays: 7,
    sessionAbsoluteDays: 30,
    recentAuthMinutes: 10,
    passwordResetValidityMinutes: 60,
    inviteValidityHours: 72,
    twoFactorRequired: false,
    trustedDevicesEnabled: true,
    trustedDeviceDurationDays: 30,
  };
  const quotas: Quotas = { defaultUserQuotaBytes: 10 * GIB, maxFileSizeBytes: null };
  const publicLinks: PublicLinks = { maxPublicLinkLifetimeDays: null };
  const smtp: Smtp = {
    enabled: true,
    host: "smtp.example.test",
    port: 587,
    security: "starttls",
    username: "mailer",
    passwordConfigured: true,
    fromName: "Palmr",
    fromEmail: "palmr@example.test",
    allowSelfSignedCertificate: false,
    noAuth: false,
  };
  return { general, security, quotas, "public-links": publicLinks, smtp };
}

export type SettingsGroupName = keyof ReturnType<typeof defaultSettings>;

export function defaultUserSessions(): Session[] {
  return [
    {
      id: "019a0000-0000-7000-8000-000000000401",
      isCurrent: false,
      createdAt: "2026-09-20T08:00:00Z",
      lastSeenAt: "2026-09-27T08:00:00Z",
      expiresAt: "2026-10-04T08:00:00Z",
      absoluteExpiresAt: "2026-10-20T08:00:00Z",
      ipAddress: "198.51.100.7",
      userAgent: "Mozilla/5.0 (X11; Linux x86_64) Firefox/143.0",
      origin: "password",
    },
  ];
}

export interface AdminServerOptions {
  me?: Me;
  users?: UserDetail[];
  invites?: Invite[];
  settings?: ReturnType<typeof defaultSettings>;
  sessions?: Session[];
  recentAuth?: boolean;
  failures?: Partial<Record<string, () => Response>>;
}

export interface AdminServerState {
  me: Me | null;
  users: UserDetail[];
  invites: Invite[];
  settings: ReturnType<typeof defaultSettings>;
  sessions: Session[];
  reauthenticated: boolean;
  userListQueries: string[];
  inviteListQueries: string[];
  createUserBodies: unknown[];
  createUserHeaders: Headers[];
  patchUserBodies: unknown[];
  roleBodies: unknown[];
  quotaBodies: unknown[];
  emailBodies: unknown[];
  inviteBodies: unknown[];
  settingsPatches: Record<string, unknown[]>;
  smtpTestBodies: unknown[];
  actions: string[];
  reauthBodies: unknown[];
  calls: { users: number; user: number; invites: number; settings: Record<string, number> };
}

const SENSITIVE = new Set([
  "role",
  "deactivate",
  "password-reset",
  "revoke-sessions",
  "email-start",
  "email-resend",
  "email-cancel",
  "settings-security",
  "settings-smtp",
  "smtp-test",
]);

const PAGE_SORTS: Readonly<Record<string, (a: UserDetail, b: UserDetail) => number>> = {
  "createdAt:asc": (a, b) => a.createdAt.localeCompare(b.createdAt),
  "createdAt:desc": (a, b) => b.createdAt.localeCompare(a.createdAt),
  "username:asc": (a, b) => a.username.localeCompare(b.username),
  "username:desc": (a, b) => b.username.localeCompare(a.username),
  "email:asc": (a, b) => a.email.localeCompare(b.email),
  "email:desc": (a, b) => b.email.localeCompare(a.email),
  "usedBytes:asc": (a, b) => a.usedBytes - b.usedBytes,
  "usedBytes:desc": (a, b) => b.usedBytes - a.usedBytes,
};

function quotaModeOf(mode: string): UserDetail["quotaOverrideMode"] {
  return mode === "bytes" || mode === "unlimited" ? mode : "inherit";
}

function encodeCursor(offset: number): string {
  return btoa(`offset:${String(offset)}`);
}

function decodeCursor(cursor: string | null): number | null {
  if (cursor === null) {
    return 0;
  }
  try {
    const match = /^offset:(\d+)$/.exec(atob(cursor));
    return match?.[1] === undefined ? null : Number(match[1]);
  } catch {
    return null;
  }
}

function page<Item>(items: Item[], url: URL) {
  const limit = Number(url.searchParams.get("limit") ?? "50");
  const offset = decodeCursor(url.searchParams.get("cursor"));
  if (offset === null) {
    return null;
  }
  const slice = items.slice(offset, offset + limit);
  const next = offset + limit < items.length ? encodeCursor(offset + limit) : null;
  return { items: slice, nextCursor: next, totalCount: items.length };
}

export function installAdminServer({
  me = meFixture({ role: "admin" }),
  users = defaultUsers(),
  invites = defaultInvites(),
  settings = defaultSettings(),
  sessions = defaultUserSessions(),
  recentAuth = false,
  failures = {},
}: AdminServerOptions = {}): AdminServerState {
  const state: AdminServerState = {
    me,
    users,
    invites,
    settings,
    sessions,
    reauthenticated: !recentAuth,
    userListQueries: [],
    inviteListQueries: [],
    createUserBodies: [],
    createUserHeaders: [],
    patchUserBodies: [],
    roleBodies: [],
    quotaBodies: [],
    emailBodies: [],
    inviteBodies: [],
    settingsPatches: {},
    smtpTestBodies: [],
    actions: [],
    reauthBodies: [],
    calls: { users: 0, user: 0, invites: 0, settings: {} },
  };
  const { handlers } = bootHandlers({ me });
  const guard = (action: string): Response | null => {
    const failure = failures[action];
    if (failure !== undefined) {
      return failure();
    }
    if (SENSITIVE.has(action) && !state.reauthenticated) {
      return errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent-auth");
    }
    return null;
  };
  const find = (id: string) => state.users.find((user) => user.id === id);
  const missing = () => errorEnvelope("USER_NOT_FOUND", 404, "req-missing-user");
  const asItem = (detail: UserDetail): UserRow => {
    const copy: Partial<UserDetail> = { ...detail };
    delete copy.overQuota;
    delete copy.sessionCount;
    delete copy.trustedDeviceCount;
    delete copy.lockout;
    delete copy.identityLinks;
    return copy as UserRow;
  };

  server.use(
    http.get(ME_URL, () =>
      state.me === null
        ? errorEnvelope("AUTH_REQUIRED", 401, "req-401")
        : HttpResponse.json(state.me),
    ),
    ...handlers,
    http.post(`${API}/auth/reauthenticate`, async ({ request }) => {
      state.reauthBodies.push(await request.json());
      state.reauthenticated = true;
      return new HttpResponse(null, { status: 204 });
    }),
    http.get(`${API}/admin/users`, ({ request }) => {
      state.calls.users += 1;
      if (state.me === null) {
        return errorEnvelope("AUTH_REQUIRED", 401, "req-401");
      }
      const url = new URL(request.url);
      state.userListQueries.push(url.search);
      const failure = failures["list-users"]?.();
      if (failure !== undefined) {
        return failure;
      }
      const q = url.searchParams.get("q")?.toLowerCase() ?? "";
      const role = url.searchParams.get("role");
      const status = url.searchParams.get("status");
      const sort = PAGE_SORTS[url.searchParams.get("sort") ?? "createdAt:desc"];
      const filtered = state.users
        .filter(
          (user) =>
            q === "" ||
            `${user.firstName} ${user.lastName} ${user.username} ${user.email}`
              .toLowerCase()
              .includes(q),
        )
        .filter((user) => role === null || user.role === role)
        .filter((user) => status === null || (status === "active" ? user.isActive : !user.isActive))
        .sort(sort ?? PAGE_SORTS["createdAt:desc"]);
      const result = page(filtered.map(asItem), url);
      return result === null
        ? errorEnvelope("CURSOR_INVALID", 400, "req-cursor")
        : HttpResponse.json(result);
    }),
    http.post(`${API}/admin/users`, async ({ request }) => {
      const body = (await request.json()) as Record<string, unknown>;
      state.createUserBodies.push(body);
      state.createUserHeaders.push(request.headers);
      const failure = failures["create-user"]?.();
      if (failure !== undefined) {
        return failure;
      }
      if (
        state.users.some((user) => user.email.toLowerCase() === String(body.email).toLowerCase())
      ) {
        return errorEnvelope("USER_EMAIL_TAKEN", 409, "req-email-taken");
      }
      if (
        state.users.some(
          (user) => user.username.toLowerCase() === String(body.username).toLowerCase(),
        )
      ) {
        return errorEnvelope("USER_USERNAME_TAKEN", 409, "req-username-taken");
      }
      if (typeof body.password === "string" && body.password.length < 8) {
        return errorEnvelope("PASSWORD_POLICY_VIOLATION", 422, "req-policy", {
          details: { minLength: 8 },
        });
      }
      const created = userFixture({
        id: `019a0000-0000-7000-8000-0000000009${String(state.users.length).padStart(2, "0")}`,
        firstName: String(body.firstName),
        lastName: String(body.lastName),
        username: String(body.username),
        email: String(body.email),
        role: String(body.role),
        isActive: body.isActive !== false,
        hasLocalPassword: typeof body.password === "string",
        mustChangePassword:
          typeof body.password === "string" && body.requirePasswordChange !== false,
        usedBytes: 0,
        sessionCount: 0,
        quotaBytes: typeof body.quotaBytes === "number" ? body.quotaBytes : null,
        quotaOverrideMode: typeof body.quotaBytes === "number" ? "bytes" : "inherit",
        effectiveQuotaBytes:
          typeof body.quotaBytes === "number"
            ? body.quotaBytes
            : state.settings.quotas.defaultUserQuotaBytes,
        createdAt: "2026-09-28T00:00:00Z",
        counts: { files: 0, receivedFiles: 0, shares: 0, reverseShares: 0 },
      });
      state.users.push(created);
      return HttpResponse.json(asItem(created), { status: 201 });
    }),
    http.get(`${API}/admin/users/:id`, ({ params }) => {
      state.calls.user += 1;
      if (state.me === null) {
        return errorEnvelope("AUTH_REQUIRED", 401, "req-401");
      }
      const failure = failures["get-user"]?.();
      if (failure !== undefined) {
        return failure;
      }
      const user = find(String(params.id));
      return user === undefined ? missing() : HttpResponse.json(user);
    }),
    http.patch(`${API}/admin/users/:id`, async ({ params, request }) => {
      const body = (await request.json()) as Record<string, string>;
      state.patchUserBodies.push(body);
      const failure = failures["patch-user"]?.();
      if (failure !== undefined) {
        return failure;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      Object.assign(user, body);
      return HttpResponse.json(asItem(user));
    }),
    http.put(`${API}/admin/users/:id/role`, async ({ params, request }) => {
      const body = (await request.json()) as { role: string };
      state.roleBodies.push(body);
      const blocked = guard("role");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      user.role = body.role;
      state.actions.push(`role:${body.role}`);
      return HttpResponse.json(asItem(user));
    }),
    http.post(`${API}/admin/users/:id/activate`, ({ params }) => {
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      user.isActive = true;
      state.actions.push("activate");
      return HttpResponse.json(asItem(user));
    }),
    http.post(`${API}/admin/users/:id/deactivate`, ({ params }) => {
      const blocked = guard("deactivate");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      user.isActive = false;
      state.actions.push("deactivate");
      return HttpResponse.json(asItem(user));
    }),
    http.post(`${API}/admin/users/:id/unlock`, ({ params }) => {
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      user.isLockedOut = false;
      user.lockout = { failedCount: 0, lockCount: user.lockout.lockCount, lockedUntil: null };
      state.actions.push("unlock");
      return new HttpResponse(null, { status: 204 });
    }),
    http.put(`${API}/admin/users/:id/quota`, async ({ params, request }) => {
      const body = (await request.json()) as { mode: string; quotaBytes?: number };
      state.quotaBodies.push(body);
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      const instanceDefault = state.settings.quotas.defaultUserQuotaBytes;
      const quotaBytes = body.mode === "bytes" ? (body.quotaBytes ?? 0) : null;
      const effective =
        body.mode === "bytes" ? quotaBytes : body.mode === "unlimited" ? null : instanceDefault;
      user.quotaBytes = quotaBytes;
      user.quotaOverrideMode = quotaModeOf(body.mode);
      user.effectiveQuotaBytes = effective;
      user.overQuota = effective !== null && user.usedBytes > effective;
      return HttpResponse.json({
        mode: body.mode,
        quotaBytes,
        instanceDefaultQuotaBytes: instanceDefault,
        effectiveQuotaBytes: effective,
        belowCurrentUsage: user.overQuota,
      });
    }),
    http.post(`${API}/admin/users/:id/email`, async ({ params, request }) => {
      const body = (await request.json()) as { email: string };
      state.emailBodies.push(body);
      const blocked = guard("email-start");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      if (state.users.some((other) => other.email === body.email)) {
        return errorEnvelope("USER_EMAIL_TAKEN", 409, "req-email-taken");
      }
      user.pendingEmail = body.email;
      state.actions.push("email-start");
      return new HttpResponse(null, { status: 202 });
    }),
    http.post(`${API}/admin/users/:id/email/resend`, ({ params }) => {
      const blocked = guard("email-resend");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      if (user.pendingEmail === null) {
        return errorEnvelope("EMAIL_VERIFICATION_NOT_PENDING", 409, "req-not-pending");
      }
      state.actions.push("email-resend");
      return new HttpResponse(null, { status: 202 });
    }),
    http.delete(`${API}/admin/users/:id/email`, ({ params }) => {
      const blocked = guard("email-cancel");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      user.pendingEmail = null;
      state.actions.push("email-cancel");
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${API}/admin/users/:id/password-reset`, ({ params }) => {
      const blocked = guard("password-reset");
      if (blocked !== null) {
        return blocked;
      }
      const user = find(String(params.id));
      if (user === undefined) {
        return missing();
      }
      if (!user.hasLocalPassword) {
        return errorEnvelope("USER_HAS_NO_LOCAL_AUTH", 409, "req-no-local");
      }
      state.actions.push("password-reset");
      user.mustChangePassword = true;
      return HttpResponse.json({
        temporaryPassword: "Tmp-Pass-Once-123",
        mustChangePassword: true,
      });
    }),
    http.get(`${API}/admin/users/:userId/sessions`, ({ request }) => {
      if (state.me === null) {
        return errorEnvelope("AUTH_REQUIRED", 401, "req-401");
      }
      const url = new URL(request.url);
      return HttpResponse.json(
        page(state.sessions, url) ?? { items: [], nextCursor: null, totalCount: 0 },
      );
    }),
    http.delete(`${API}/admin/users/:userId/sessions`, ({ params }) => {
      const blocked = guard("revoke-sessions");
      if (blocked !== null) {
        return blocked;
      }
      state.actions.push(`revoke-sessions:${String(params.userId)}`);
      state.sessions = [];
      const user = find(String(params.userId));
      if (user !== undefined) {
        user.sessionCount = 0;
      }
      if (String(params.userId) === ADMIN_ID) {
        state.me = null;
      }
      return new HttpResponse(null, { status: 204 });
    }),
    http.get(`${API}/admin/invites`, ({ request }) => {
      state.calls.invites += 1;
      const url = new URL(request.url);
      state.inviteListQueries.push(url.search);
      const status = url.searchParams.get("status");
      const filtered = state.invites.filter(
        (invite) => status === null || invite.status === status,
      );
      return HttpResponse.json(page(filtered, url));
    }),
    http.post(`${API}/admin/invites`, async ({ request }) => {
      const body = (await request.json()) as {
        email: string;
        role: string;
        sendEmail: boolean;
        expiresInHours?: number;
      };
      state.inviteBodies.push(body);
      const failure = failures["create-invite"]?.();
      if (failure !== undefined) {
        return failure;
      }
      if (body.sendEmail && !state.settings.smtp.enabled) {
        return errorEnvelope("FEATURE_UNAVAILABLE_SMTP", 409, "req-smtp");
      }
      const invite = inviteFixture({
        id: `019a0000-0000-7000-8000-0000000008${String(state.invites.length).padStart(2, "0")}`,
        email: body.email,
        role: body.role,
      });
      state.invites.unshift(invite);
      return HttpResponse.json(
        {
          id: invite.id,
          expiresAt: invite.expiresAt,
          inviteUrl: `https://palmr.example.test/invite/token-${invite.id.slice(-4)}`,
        },
        { status: 201 },
      );
    }),
    http.post(`${API}/admin/invites/:id/resend`, () => {
      if (!state.settings.smtp.enabled) {
        return errorEnvelope("FEATURE_UNAVAILABLE_SMTP", 409, "req-smtp");
      }
      state.actions.push("invite-resend");
      return new HttpResponse(null, { status: 202 });
    }),
    http.delete(`${API}/admin/invites/:id`, ({ params }) => {
      const invite = state.invites.find((candidate) => candidate.id === String(params.id));
      if (invite !== undefined) {
        invite.status = "revoked";
      }
      state.actions.push("invite-revoke");
      return new HttpResponse(null, { status: 204 });
    }),
    ...(["general", "security", "quotas", "public-links", "smtp"] as const).flatMap((group) => [
      http.get(`${API}/admin/settings/${group}`, () => {
        state.calls.settings[group] = (state.calls.settings[group] ?? 0) + 1;
        return HttpResponse.json(state.settings[group]);
      }),
      http.patch(`${API}/admin/settings/${group}`, async ({ request }) => {
        const body = (await request.json()) as Record<string, unknown>;
        (state.settingsPatches[group] ??= []).push(body);
        const blocked = guard(`settings-${group}`);
        if (blocked !== null) {
          return blocked;
        }
        const failure = failures[`patch-${group}`]?.();
        if (failure !== undefined) {
          return failure;
        }
        Object.assign(state.settings[group], body);
        if (group === "smtp") {
          const patch = body as { password?: string | null };
          if (typeof patch.password === "string") {
            state.settings.smtp.passwordConfigured = true;
          } else if (patch.password === null) {
            state.settings.smtp.passwordConfigured = false;
          }
          delete (state.settings.smtp as unknown as Record<string, unknown>).password;
        }
        return HttpResponse.json(state.settings[group]);
      }),
    ]),
    http.post(`${API}/admin/settings/smtp/test`, async ({ request }) => {
      const body = (await request.json()) as { to: string; useUnsavedSettings?: unknown };
      state.smtpTestBodies.push(body);
      const blocked = guard("smtp-test");
      if (blocked !== null) {
        return blocked;
      }
      const failure = failures["smtp-test"]?.();
      if (failure !== undefined) {
        return failure;
      }
      return HttpResponse.json({
        ok: true,
        durationMs: 120,
        stages: [
          { name: "connect", ok: true },
          { name: "starttls", ok: true },
          { name: "auth", ok: true },
          { name: "send", ok: true },
        ],
      });
    }),
  );
  return state;
}
