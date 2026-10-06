import { http, HttpResponse } from "msw";
import type { components } from "../shared/api/schema";
import type { AdminServerState } from "./adminServer";
import { errorEnvelope } from "./bootFixtures";
import { server } from "./server";

type Schemas = components["schemas"];
type Provider = Schemas["ProviderItem"];
type Preset = Schemas["PresetItem"];
type PasswordLoginState = Schemas["PasswordLoginState"];
type Discovered = Schemas["Discovered"];

const API = "*/api/v1";

export const SECRET_MARKER = "SECRET-MARKER-5b1e9c2a-never-render-me";
export const NEW_SECRET = "typed-new-secret-7d3f";
export const GOOGLE_ID = "019b0000-0000-7000-8000-000000000001";
export const AUTHENTIK_ID = "019b0000-0000-7000-8000-000000000002";
export const GITHUB_ID = "019b0000-0000-7000-8000-000000000003";

const OIDC_CLAIMS = {
  subject: "sub",
  email: "email",
  emailVerified: "email_verified",
  username: "preferred_username",
  name: "name",
  picture: "picture",
};

export function providerFixture(
  overrides: Partial<Provider> & { id: string; slug: string },
): Provider {
  return {
    displayName: "Company SSO",
    protocol: "oidc",
    preset: "authentik",
    enabled: true,
    sortOrder: 1,
    autoProvision: false,
    allowEmailLinking: true,
    issuerUrl: "https://sso.example.test/application/o/palmr/",
    clientId: "palmr-client",
    clientSecretConfigured: true,
    tokenAuthMethod: "client_secret_basic",
    scopes: ["openid", "email", "profile"],
    endpoints: {
      authorization: "https://sso.example.test/authorize",
      token: "https://sso.example.test/token",
      userinfo: "https://sso.example.test/userinfo",
      jwks: "https://sso.example.test/jwks",
    },
    claimMapping: { ...OIDC_CLAIMS },
    validatedAt: "2026-09-27T12:00:00Z",
    validationError: null,
    linkedUserCount: 0,
    redirectUri: `https://palmr.example.test/api/v1/auth/providers/${overrides.slug}/callback`,
    createdAt: "2026-09-01T08:00:00Z",
    updatedAt: "2026-09-27T12:00:00Z",
    ...overrides,
  };
}

export function defaultProviders(): Provider[] {
  return [
    providerFixture({
      id: GOOGLE_ID,
      slug: "google",
      displayName: "Google",
      preset: "google",
      sortOrder: 1,
      issuerUrl: "https://accounts.google.com",
      tokenAuthMethod: "client_secret_post",
      linkedUserCount: 4,
    }),
    providerFixture({
      id: AUTHENTIK_ID,
      slug: "authentik",
      displayName: "Company SSO",
      sortOrder: 2,
      enabled: false,
      validatedAt: null,
      autoProvision: true,
    }),
    providerFixture({
      id: GITHUB_ID,
      slug: "github",
      displayName: "GitHub",
      protocol: "oauth2",
      preset: "github",
      sortOrder: 3,
      issuerUrl: null,
      allowEmailLinking: false,
      validatedAt: null,
      validationError: "userinfo_endpoint:upstream_error,token_endpoint:timeout",
      endpoints: {
        authorization: "https://github.com/login/oauth/authorize",
        token: "https://github.com/login/oauth/access_token",
        userinfo: "https://api.github.com/user",
        jwks: null,
      },
    }),
  ];
}

export function presetCatalogue(): Preset[] {
  return [
    {
      preset: "google",
      protocol: "oidc",
      displayName: "Google",
      issuerUrl: "https://accounts.google.com",
      endpoints: { authorization: null, token: null, userinfo: null, jwks: null },
      scopes: ["openid", "email", "profile"],
      tokenAuthMethod: "client_secret_post",
      claimMapping: { ...OIDC_CLAIMS },
      allowEmailLinking: true,
    },
    {
      preset: "github",
      protocol: "oauth2",
      displayName: "GitHub",
      issuerUrl: null,
      endpoints: {
        authorization: "https://github.com/login/oauth/authorize",
        token: "https://github.com/login/oauth/access_token",
        userinfo: "https://api.github.com/user",
        jwks: null,
      },
      scopes: ["read:user", "user:email"],
      tokenAuthMethod: "client_secret_post",
      claimMapping: { ...OIDC_CLAIMS, subject: "id", username: "login", emailVerified: "verified" },
      allowEmailLinking: false,
    },
    {
      preset: "generic",
      protocol: "oidc",
      displayName: "Custom OIDC",
      issuerUrl: null,
      endpoints: { authorization: null, token: null, userinfo: null, jwks: null },
      scopes: ["openid", "email", "profile"],
      tokenAuthMethod: "client_secret_basic",
      claimMapping: { ...OIDC_CLAIMS },
      allowEmailLinking: true,
    },
    {
      preset: "generic",
      protocol: "oauth2",
      displayName: "Custom OAuth2",
      issuerUrl: null,
      endpoints: { authorization: null, token: null, userinfo: null, jwks: null },
      scopes: ["profile", "email"],
      tokenAuthMethod: "client_secret_basic",
      claimMapping: { ...OIDC_CLAIMS },
      allowEmailLinking: false,
    },
  ];
}

export function passwordLoginFixture(
  overrides: Partial<PasswordLoginState> = {},
): PasswordLoginState {
  return {
    passwordLoginEnabled: true,
    canDisable: true,
    blockers: [],
    safeAdminLoginPaths: [
      {
        userId: "019a0000-0000-7000-8000-000000000001",
        username: "ada",
        providerSlug: "google",
        providerValidated: true,
      },
    ],
    ...overrides,
  };
}

export type TestOutcome =
  | { kind: "ok" }
  | { kind: "checks-failed"; checks: { name: string; ok: boolean; detail?: string }[] }
  | { kind: "error"; code: string; status: number };

export interface ProviderServerOptions {
  providers?: Provider[];
  passwordLogin?: PasswordLoginState;
  discovery?: Discovered | (() => Response);
  tests?: TestOutcome[];
  failures?: Partial<Record<string, () => Response>>;
}

export interface ProviderServerState {
  providers: Provider[];
  passwordLogin: PasswordLoginState;
  createBodies: unknown[];
  patchBodies: { id: string; body: Record<string, unknown> }[];
  deleted: string[];
  orderBodies: string[][];
  discoverBodies: unknown[];
  testedIds: string[];
  passwordLoginBodies: unknown[];
  calls: { providers: number; presets: number; passwordLogin: number; bootstrap: number };
}

export function installProviderServer(
  admin: AdminServerState,
  {
    providers = defaultProviders(),
    passwordLogin = passwordLoginFixture(),
    discovery,
    tests = [],
    failures = {},
  }: ProviderServerOptions = {},
): ProviderServerState {
  const state: ProviderServerState = {
    providers,
    passwordLogin,
    createBodies: [],
    patchBodies: [],
    deleted: [],
    orderBodies: [],
    discoverBodies: [],
    testedIds: [],
    passwordLoginBodies: [],
    calls: { providers: 0, presets: 0, passwordLogin: 0, bootstrap: 0 },
  };
  const recent = () =>
    admin.reauthenticated
      ? null
      : errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent-auth");
  const guard = (name: string, sensitive: boolean): Response | null =>
    failures[name]?.() ?? (sensitive ? recent() : null);
  const find = (id: string) => state.providers.find((provider) => provider.id === id);
  const missing = () => errorEnvelope("PROVIDER_NOT_FOUND", 404, "req-missing-provider");
  const sorted = () => [...state.providers].sort((a, b) => a.sortOrder - b.sortOrder);

  server.use(
    http.get(`${API}/admin/providers`, () => {
      state.calls.providers += 1;
      return HttpResponse.json({
        items: sorted(),
        nextCursor: null,
        totalCount: state.providers.length,
      });
    }),
    http.get(`${API}/admin/providers/presets`, () => {
      state.calls.presets += 1;
      return HttpResponse.json({ items: presetCatalogue() });
    }),
    http.put(`${API}/admin/providers/order`, async ({ request }) => {
      const body = (await request.json()) as { order: string[] };
      state.orderBodies.push(body.order);
      const blocked = guard("order", false);
      if (blocked !== null) {
        return blocked;
      }
      body.order.forEach((id, index) => {
        const provider = find(id);
        if (provider !== undefined) {
          provider.sortOrder = index + 1;
        }
      });
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${API}/admin/providers/discover`, async ({ request }) => {
      state.discoverBodies.push(await request.json());
      if (typeof discovery === "function") {
        return discovery();
      }
      return HttpResponse.json(
        discovery ?? {
          issuerUrl: "https://sso.example.test/application/o/palmr/",
          endpoints: {
            authorization: "https://sso.example.test/discovered/authorize",
            token: "https://sso.example.test/discovered/token",
            userinfo: "https://sso.example.test/discovered/userinfo",
            jwks: "https://sso.example.test/discovered/jwks",
          },
          scopesSupported: ["openid", "email", "profile", "offline_access"],
          tokenEndpointAuthMethodsSupported: ["client_secret_basic", "client_secret_post"],
        },
      );
    }),
    http.post(`${API}/admin/providers`, async ({ request }) => {
      const body = (await request.json()) as Record<string, unknown>;
      state.createBodies.push(body);
      const blocked = guard("create", true);
      if (blocked !== null) {
        return blocked;
      }
      if (state.providers.some((provider) => provider.slug === body.slug)) {
        return errorEnvelope("PROVIDER_SLUG_TAKEN", 409, "req-slug-taken");
      }
      const created = providerFixture({
        id: `019b0000-0000-7000-8000-0000000009${String(state.providers.length).padStart(2, "0")}`,
        slug: String(body.slug),
        displayName: String(body.displayName),
        protocol: body.protocol as Provider["protocol"],
        preset: body.preset as Provider["preset"],
        enabled: body.enabled === true,
        sortOrder: state.providers.length + 1,
        autoProvision: body.autoProvision === true,
        allowEmailLinking: body.allowEmailLinking === true,
        issuerUrl: typeof body.issuerUrl === "string" ? body.issuerUrl : null,
        clientId: String(body.clientId),
        clientSecretConfigured: typeof body.clientSecret === "string",
        validatedAt: null,
      });
      state.providers.push(created);
      return HttpResponse.json(created, { status: 201 });
    }),
    http.patch(`${API}/admin/providers/:id`, async ({ params, request }) => {
      const body = (await request.json()) as Record<string, unknown>;
      const id = String(params.id);
      state.patchBodies.push({ id, body });
      const blocked = guard("update", true);
      if (blocked !== null) {
        return blocked;
      }
      const provider = find(id);
      if (provider === undefined) {
        return missing();
      }
      const { clientSecret, ...rest } = body;
      Object.assign(
        provider,
        Object.fromEntries(
          Object.entries(rest).filter(([key]) => key !== "endpoints" && key !== "claimMapping"),
        ),
      );
      if (typeof clientSecret === "string") {
        provider.clientSecretConfigured = true;
      } else if (clientSecret === null) {
        provider.clientSecretConfigured = false;
      }
      if ("enabled" in body || "issuerUrl" in body || "clientId" in body) {
        provider.validatedAt = "enabled" in body ? provider.validatedAt : null;
      }
      return HttpResponse.json(provider);
    }),
    http.delete(`${API}/admin/providers/:id`, ({ params }) => {
      const id = String(params.id);
      const blocked = guard("delete", true);
      if (blocked !== null) {
        return blocked;
      }
      const provider = find(id);
      if (provider === undefined) {
        return missing();
      }
      if (provider.linkedUserCount > 0) {
        return errorEnvelope("PROVIDER_HAS_LINKS", 409, "req-has-links");
      }
      state.deleted.push(id);
      state.providers = state.providers.filter((item) => item.id !== id);
      return new HttpResponse(null, { status: 204 });
    }),
    http.post(`${API}/admin/providers/:id/test`, ({ params }) => {
      const id = String(params.id);
      state.testedIds.push(id);
      const provider = find(id);
      if (provider === undefined) {
        return missing();
      }
      const outcome = tests.shift() ?? { kind: "ok" };
      if (outcome.kind === "ok") {
        provider.validatedAt = "2026-09-28T01:00:00Z";
        provider.validationError = null;
        return HttpResponse.json({
          ok: true,
          checks: [
            { name: "discovery", ok: true, detail: null },
            { name: "jwks", ok: true, detail: null },
          ],
          validatedAt: provider.validatedAt,
        });
      }
      if (outcome.kind === "checks-failed") {
        provider.validatedAt = null;
        provider.validationError = outcome.checks
          .filter((check) => !check.ok)
          .map((check) => `${check.name}:${check.detail ?? "failed"}`)
          .join(",");
        return errorEnvelope("PROVIDER_VALIDATION_FAILED", 422, "req-validation", {
          details: {
            checks: outcome.checks.map((c) => ({ ...c, detail: c.detail ?? null })),
          } as never,
        });
      }
      return errorEnvelope(outcome.code, outcome.status, "req-test-error");
    }),
    http.get(`${API}/admin/auth/password-login`, () => {
      state.calls.passwordLogin += 1;
      return HttpResponse.json(state.passwordLogin);
    }),
    http.put(`${API}/admin/auth/password-login`, async ({ request }) => {
      const body = (await request.json()) as { enabled: boolean; confirm: boolean };
      state.passwordLoginBodies.push(body);
      const blocked = guard("password-login", true);
      if (blocked !== null) {
        return blocked;
      }
      state.passwordLogin = {
        ...state.passwordLogin,
        passwordLoginEnabled: body.enabled,
        canDisable: body.enabled && state.passwordLogin.blockers.length === 0,
      };
      return HttpResponse.json(state.passwordLogin);
    }),
  );
  return state;
}
