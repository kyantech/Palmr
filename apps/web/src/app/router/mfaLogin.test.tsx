import type { QueryClient } from "@tanstack/react-query";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, type MockInstance, test, vi } from "vitest";
import { clearMfaChallenge, mfaChallengeStore } from "../../features/auth";
import { qk } from "../../shared/api/query-keys";
import { BACKUP_CODES, MFA_TOKEN, mfaChallengeEnvelope } from "../../test/authServer";
import { errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerState } from "../../test/settingsServer";
import { appRoutes } from "./routes";

const LOGIN_URL = "*/api/v1/auth/login";
const TOTP_URL = "*/api/v1/auth/login/totp";
const NEXT = "/settings/profile";

type User = ReturnType<typeof userEvent.setup>;

interface MfaServer {
  settings: SettingsServerState;
  loginBodies: unknown[];
  secondFactorBodies: Record<string, unknown>[];
}

function installMfaServer({
  challenge = () => mfaChallengeEnvelope(),
  secondFactor,
}: {
  challenge?: () => Response;
  secondFactor?: (body: Record<string, unknown>, calls: number) => Response | null;
} = {}): MfaServer {
  const settings = installSettingsServer();
  settings.me = null;
  const state: MfaServer = { settings, loginBodies: [], secondFactorBodies: [] };
  server.use(
    http.post(LOGIN_URL, async ({ request }) => {
      state.loginBodies.push(await request.json());
      return challenge();
    }),
    http.post(TOTP_URL, async ({ request }) => {
      const body = (await request.json()) as Record<string, unknown>;
      state.secondFactorBodies.push(body);
      const override = secondFactor?.(body, state.secondFactorBodies.length) ?? null;
      if (override !== null) {
        return override;
      }
      if (body.mfaToken !== MFA_TOKEN) {
        return errorEnvelope("AUTH_2FA_CHALLENGE_EXPIRED", 401, "req-expired");
      }
      settings.me = meFixture({ capabilities: { twoFactorEnabled: true } });
      return HttpResponse.json({
        user: { id: "u1", username: "ada", role: "user" },
        mustChangePassword: false,
        mfaEnrollmentRequired: false,
      });
    }),
  );
  return state;
}

async function passwordStep(user: User) {
  await user.type(await screen.findByLabelText("E-mail or username"), "ada");
  await user.type(screen.getByLabelText("Password"), "correct horse battery");
  await user.click(screen.getByRole("button", { name: "Sign in" }));
  return screen.findByRole("heading", { level: 1, name: "Two-factor authentication" });
}

function start(entry = `/login?next=${encodeURIComponent(NEXT)}`) {
  const harness = renderSession({ routes: appRoutes, initialEntries: [entry] });
  return { ...harness, user: userEvent.setup({ delay: null }) };
}

function serialized(client: QueryClient): string {
  const queries = client
    .getQueryCache()
    .getAll()
    .map((query) => ({ key: query.queryKey, state: query.state }));
  const mutations = client
    .getMutationCache()
    .getAll()
    .map((mutation) => ({
      variables: mutation.state.variables,
      data: mutation.state.data,
      error: mutation.state.error,
    }));
  return JSON.stringify({ queries, mutations }, (_key, value: unknown) =>
    value instanceof Error ? Object.fromEntries(Object.entries(value)) : value,
  );
}

let setItem: MockInstance<Storage["setItem"]>;

beforeEach(() => {
  stubMatchMedia();
  setItem = vi.spyOn(Storage.prototype, "setItem");
});

afterEach(() => {
  clearMfaChallenge();
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_mfa_token_memory_only", () => {
  test("the password step fills memory, /login/2fa consumes it, and success clears it without the token ever leaving memory", async () => {
    const mfa = installMfaServer();
    const { user, router, locations, queryClient } = start();

    await passwordStep(user);

    expect(router.state.location.pathname).toBe("/login/2fa");
    expect(router.state.location.search).toBe(`?next=${encodeURIComponent(NEXT)}`);
    expect(mfaChallengeStore.getState().challenge?.mfaToken).toBe(MFA_TOKEN);
    expect(document.body.innerHTML).not.toContain(MFA_TOKEN);
    expect(serialized(queryClient)).not.toContain(MFA_TOKEN);
    expect(screen.queryByTestId("app-shell")).toBeNull();

    await user.type(screen.getByLabelText("Authentication code"), "123 456");
    await user.click(screen.getByRole("checkbox", { name: "Remember this device" }));
    await user.click(screen.getByRole("button", { name: "Verify" }));

    expect(await screen.findByRole("heading", { level: 2, name: "Profile" })).toBeDefined();
    expect(router.state.location.pathname).toBe(NEXT);
    expect(mfa.secondFactorBodies).toEqual([
      { mfaToken: MFA_TOKEN, code: "123456", rememberDevice: true },
    ]);
    expect(mfaChallengeStore.getState().challenge).toBeNull();
    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({ user: { username: "ada" } });
    for (const location of [...locations, window.location.href]) {
      expect(location).not.toContain(MFA_TOKEN);
    }
    expect(serialized(queryClient)).not.toContain(MFA_TOKEN);
    expect(document.cookie).not.toContain(MFA_TOKEN);
    const stored = setItem.mock.calls.flat().map(String);
    expect(stored.some((value) => value.includes(MFA_TOKEN))).toBe(false);
    expect(mfa.loginBodies).toHaveLength(1);
  });

  test("the challenge store is a plain in-memory Zustand store with no persistence adapter", () => {
    expect(Object.keys(mfaChallengeStore)).not.toContain("persist");
    expect("persist" in mfaChallengeStore).toBe(false);
  });

  test("without an in-memory challenge (a reload) /login/2fa returns to /login", async () => {
    installMfaServer();
    const { router } = start(`/login/2fa?next=${encodeURIComponent(NEXT)}`);

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.location.search).toBe(`?next=${encodeURIComponent(NEXT)}`);
  });

  test("cancel clears the challenge and goes back to the password step", async () => {
    const mfa = installMfaServer();
    const { user, router } = start();
    await passwordStep(user);

    await user.click(screen.getByRole("button", { name: "Back to sign in" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(mfaChallengeStore.getState().challenge).toBeNull();
    expect(mfa.secondFactorBodies).toHaveLength(0);
  });

  test("an expired or burnt challenge is cleared and the user is sent back to sign in with an explanation", async () => {
    installMfaServer({
      secondFactor: () => errorEnvelope("AUTH_2FA_CHALLENGE_EXPIRED", 401, "req-burnt"),
    });
    const { user, router } = start();
    await passwordStep(user);

    await user.type(screen.getByLabelText("Authentication code"), "123456");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    expect(await screen.findByText("Your sign-in attempt expired")).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(mfaChallengeStore.getState().challenge).toBeNull();
  });
});

describe("component_mfa_second_factor_step", () => {
  test("remember-device is not offered when the server does not offer it, and never sent as true", async () => {
    const mfa = installMfaServer({
      challenge: () => mfaChallengeEnvelope({ trustedDeviceOffered: false }),
    });
    const { user } = start();
    await passwordStep(user);

    expect(screen.queryByRole("checkbox")).toBeNull();
    await user.type(screen.getByLabelText("Authentication code"), "123456");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    await waitFor(() => {
      expect(mfa.secondFactorBodies).toEqual([
        { mfaToken: MFA_TOKEN, code: "123456", rememberDevice: false },
      ]);
    });
  });

  test("TRUSTED_DEVICE_DISABLED is reported, the option is withdrawn and nothing is retried automatically", async () => {
    const mfa = installMfaServer({
      secondFactor: (_body, calls) =>
        calls === 1 ? errorEnvelope("TRUSTED_DEVICE_DISABLED", 403, "req-td") : null,
    });
    const { user, router } = start();
    await passwordStep(user);

    await user.type(screen.getByLabelText("Authentication code"), "123456");
    await user.click(screen.getByRole("checkbox", { name: "Remember this device" }));
    await user.click(screen.getByRole("button", { name: "Verify" }));

    expect(
      await screen.findByText(
        "Remembering devices is turned off on this instance. Sign in without remembering this device.",
      ),
    ).toBeDefined();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(mfa.secondFactorBodies).toHaveLength(1);

    expect(screen.getByLabelText<HTMLInputElement>("Authentication code").value).toBe("123456");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(NEXT);
    });
    expect(mfa.secondFactorBodies[1]).toEqual({
      mfaToken: MFA_TOKEN,
      code: "123456",
      rememberDevice: false,
    });
  });

  test.each([
    [
      "AUTH_2FA_INVALID",
      "The verification code is incorrect. Check your authenticator app and try again.",
    ],
    [
      "TOTP_CODE_REPLAYED",
      "This code was already used. Wait for your authenticator app to show a new code.",
    ],
  ])("%s is shown on the code field by code and the challenge is kept", async (code, text) => {
    const mfa = installMfaServer({
      secondFactor: () => errorEnvelope(code, 401, "req-code", { message: "server prose" }),
    });
    const { user } = start();
    await passwordStep(user);

    await user.type(screen.getByLabelText("Authentication code"), "000000");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    expect(await screen.findByText(text)).toBeDefined();
    expect(screen.getByLabelText("Authentication code").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByLabelText<HTMLInputElement>("Authentication code").value).toBe("");
    expect(mfaChallengeStore.getState().challenge?.mfaToken).toBe(MFA_TOKEN);
    expect(document.body.textContent).not.toContain("server prose");
    expect(mfa.secondFactorBodies).toHaveLength(1);
  });

  test("backup codes use the same code field in the full XXXX-XXXX-XXXX-XXXX form", async () => {
    const mfa = installMfaServer();
    const { user } = start();
    await passwordStep(user);

    await user.click(screen.getByRole("button", { name: "Use a backup code instead" }));
    const field = screen.getByLabelText("Backup code");
    await user.type(field, "ABCD-EFGH");
    await user.click(screen.getByRole("button", { name: "Verify" }));
    expect(
      await screen.findByText(
        "Backup codes have 16 letters and digits in four groups, for example ABCD-EFGH-JKLM-NPQR.",
      ),
    ).toBeDefined();
    expect(mfa.secondFactorBodies).toHaveLength(0);

    await user.clear(field);
    await user.type(field, BACKUP_CODES[0] ?? "");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    await waitFor(() => {
      expect(mfa.secondFactorBodies).toHaveLength(1);
    });
    expect(mfa.secondFactorBodies[0]).toMatchObject({ code: BACKUP_CODES[0] });
  });

  test("RATE_LIMITED disables verification for Retry-After and never resubmits", async () => {
    const mfa = installMfaServer({
      secondFactor: () =>
        errorEnvelope("RATE_LIMITED", 429, "req-429", { headers: { "Retry-After": "30" } }),
    });
    const { user } = start();
    await passwordStep(user);

    await user.type(screen.getByLabelText("Authentication code"), "123456");
    await user.click(screen.getByRole("button", { name: "Verify" }));

    expect(
      await screen.findByText("Too many attempts. Wait a moment and try again."),
    ).toBeDefined();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Verify" }).disabled).toBe(true);
    expect(mfa.secondFactorBodies).toHaveLength(1);
  });
});
