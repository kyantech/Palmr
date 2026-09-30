import type { QueryClient } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import {
  BACKUP_CODES,
  ENROLLMENT_ID,
  installTwoFactorServer,
  OTPAUTH_URI,
  TOTP_SECRET,
} from "../../test/authServer";
import { errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerState } from "../../test/settingsServer";
import type { Restriction } from "../bootstrap/queries";
import { appRoutes } from "./routes";

vi.mock("antd", async (importOriginal) => {
  const actual = await importOriginal<typeof import("antd")>();
  function QRCode({ value }: { value: string }) {
    return <div data-testid="qr-code" data-value={value} />;
  }
  return { ...actual, QRCode };
});

const PASSWORD_URL = "*/api/v1/profile/password";
const LOGOUT_URL = "*/api/v1/auth/logout";
const NEW_PASSWORD = "a brand new passphrase";

type User = ReturnType<typeof userEvent.setup>;

function restrictedMe(restriction: Restriction) {
  return meFixture({ restriction });
}

function start(entry: string, settings: SettingsServerState) {
  const harness = renderSession({ routes: appRoutes, initialEntries: [entry] });
  return { settings, ...harness, user: userEvent.setup({ delay: null }) };
}

function installForcedChange(
  settings: SettingsServerState,
  after: Restriction | null,
): { bodies: unknown[] } {
  const bodies: unknown[] = [];
  server.use(
    http.post(PASSWORD_URL, async ({ request }) => {
      bodies.push(await request.json());
      if (settings.me !== null) {
        settings.me = { ...settings.me, restriction: after };
      }
      return new HttpResponse(null, { status: 204 });
    }),
  );
  return { bodies };
}

async function setNewPassword(user: User, confirmation = NEW_PASSWORD) {
  await user.type(await screen.findByLabelText("New password"), NEW_PASSWORD);
  await user.type(screen.getByLabelText("Confirm new password"), confirmation);
  await user.click(screen.getByRole("button", { name: "Set new password" }));
}

function cacheText(client: QueryClient): string {
  return JSON.stringify({
    queries: client
      .getQueryCache()
      .getAll()
      .map((query) => query.state.data),
    mutations: client
      .getMutationCache()
      .getAll()
      .map((mutation) => [mutation.state.data, mutation.state.variables]),
  });
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_forced_password_change", () => {
  test("every application route is locked to an AuthLayout screen that never asks for the current password", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    const { router } = start("/settings/sessions", settings);

    expect(
      await screen.findByRole("heading", { level: 1, name: "Set a new password" }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe("/login/forced-password-change");
    expect(screen.queryByTestId("app-shell")).toBeNull();
    expect(screen.getByTestId("auth-brand")).toBeDefined();
    expect(screen.getByText("You signed in with a temporary password")).toBeDefined();
    expect(screen.queryByLabelText("Current password")).toBeNull();
    expect(await screen.findByText("Use at least 8 characters.")).toBeDefined();
    expect(screen.getByRole("button", { name: "Sign out" })).toBeDefined();
    expect(settings.calls.sessions).toBe(0);
  });

  test("the new password policy is enforced before any request", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    const { bodies } = installForcedChange(settings, null);
    const { user } = start("/login/forced-password-change", settings);

    await user.type(await screen.findByLabelText("New password"), "short");
    await user.type(screen.getByLabelText("Confirm new password"), "short");
    await user.click(screen.getByRole("button", { name: "Set new password" }));
    expect(await screen.findAllByText("Use at least 8 characters.")).toHaveLength(2);

    await user.clear(screen.getByLabelText("New password"));
    await user.clear(screen.getByLabelText("Confirm new password"));
    await setNewPassword(user, "something else entirely");
    expect(await screen.findByText("The passwords don't match.")).toBeDefined();
    expect(bodies).toEqual([]);
  });

  test("success sends only newPassword and the refreshed server state lands on the validated next", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    const { bodies } = installForcedChange(settings, null);
    const { user, router } = start("/settings/appearance", settings);

    await setNewPassword(user);

    expect(await screen.findByTestId("app-shell")).toBeDefined();
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/settings/appearance");
    });
    expect(bodies).toEqual([{ newPassword: NEW_PASSWORD }]);
  });

  test("forced password change is followed by mandatory enrollment when the server says so", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    installForcedChange(settings, "mfa_enrollment_required");
    installTwoFactorServer(settings);
    const { user, router } = start("/overview", settings);

    await setNewPassword(user);

    expect(
      await screen.findByRole("heading", { level: 1, name: "Set up two-factor authentication" }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe("/login/enroll-2fa");
    expect(screen.queryByTestId("app-shell")).toBeNull();
  });

  test("the client never lifts the restriction itself", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    installForcedChange(settings, "must_change_password");
    const { user, router } = start("/login/forced-password-change", settings);

    await setNewPassword(user);

    await waitFor(() => {
      expect(settings.calls.me).toBeGreaterThanOrEqual(2);
    });
    expect(router.state.location.pathname).toBe("/login/forced-password-change");
    expect(screen.queryByTestId("app-shell")).toBeNull();
  });

  test("sign out is always available and leaves the lock screen for /login", async () => {
    const settings = installSettingsServer({ me: restrictedMe("must_change_password") });
    server.use(
      http.post(LOGOUT_URL, () => {
        settings.me = null;
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const { user, router } = start("/login/forced-password-change", settings);

    await user.click(await screen.findByRole("button", { name: "Sign out" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
  });
});

describe("component_mandatory_two_factor_enrollment", () => {
  test("enrollment renders the AntD QR code from otpauthUri and verifies without sending the secret back", async () => {
    const settings = installSettingsServer({ me: restrictedMe("mfa_enrollment_required") });
    const twoFactor = installTwoFactorServer(settings);
    const { user, router, queryClient } = start("/overview", settings);

    const qr = await screen.findByTestId("qr-code");
    expect(router.state.location.pathname).toBe("/login/enroll-2fa");
    expect(qr.dataset.value).toBe(OTPAUTH_URI);
    expect(screen.getByTestId("two-factor-secret").textContent).toBe(
      "JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP",
    );
    expect(screen.getByRole("img", { name: /QR code for adding this account/ })).toBeDefined();
    expect(twoFactor.enrollCalls).toBe(1);
    expect(cacheText(queryClient)).not.toContain(TOTP_SECRET);

    await user.type(screen.getByLabelText("Authentication code"), "492013");
    await user.click(screen.getByRole("button", { name: "Verify and turn on" }));

    expect(await screen.findByText("Save your backup codes")).toBeDefined();
    expect(twoFactor.verifyBodies).toEqual([{ enrollmentId: ENROLLMENT_ID, code: "492013" }]);
    expect(JSON.stringify(twoFactor.verifyBodies)).not.toContain(TOTP_SECRET);
    expect(screen.queryByTestId("qr-code")).toBeNull();
    expect(screen.queryByTestId("two-factor-secret")).toBeNull();
    const codes = screen.getAllByTestId("backup-code").map((item) => item.textContent);
    expect(codes).toEqual(BACKUP_CODES);
    expect(codes.every((code) => /^[A-Z2-7]{4}(-[A-Z2-7]{4}){3}$/.test(code))).toBe(true);
    expect(router.state.location.pathname).toBe("/login/enroll-2fa");

    const text = cacheText(queryClient);
    for (const code of BACKUP_CODES) {
      expect(text).not.toContain(code);
    }
    expect(text).not.toContain(TOTP_SECRET);

    const proceed = screen.getByRole<HTMLButtonElement>("button", { name: "Continue" });
    expect(proceed.disabled).toBe(true);
    await user.click(
      screen.getByRole("checkbox", { name: "I saved these backup codes somewhere safe" }),
    );
    await user.click(proceed);

    expect(await screen.findByTestId("app-shell")).toBeDefined();
    expect(router.state.location.pathname).toBe("/overview");
    expect(document.body.textContent).not.toContain(BACKUP_CODES[0]);
  });

  test("an expired pending enrollment offers a clean restart", async () => {
    const settings = installSettingsServer({ me: restrictedMe("mfa_enrollment_required") });
    const twoFactor = installTwoFactorServer(settings, {
      verify: () => errorEnvelope("TOTP_ENROLLMENT_PENDING_MISSING", 409, "req-gone"),
    });
    const { user } = start("/login/enroll-2fa", settings);

    await user.type(await screen.findByLabelText("Authentication code"), "492013");
    await user.click(screen.getByRole("button", { name: "Verify and turn on" }));

    expect(await screen.findByText("This setup expired")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Start again" }));
    expect(await screen.findByTestId("qr-code")).toBeDefined();
    expect(twoFactor.enrollCalls).toBe(2);
  });

  test("a wrong code stays on the step with the mapped error", async () => {
    const settings = installSettingsServer({ me: restrictedMe("mfa_enrollment_required") });
    installTwoFactorServer(settings, {
      verify: () => errorEnvelope("AUTH_2FA_INVALID", 401, "req-bad"),
    });
    const { user } = start("/login/enroll-2fa", settings);

    await user.type(await screen.findByLabelText("Authentication code"), "000000");
    await user.click(screen.getByRole("button", { name: "Verify and turn on" }));

    const setup = screen.getByTestId("two-factor-setup");
    expect(
      await within(setup).findByText(
        "The verification code is incorrect. Check your authenticator app and try again.",
      ),
    ).toBeDefined();
    expect(screen.getByTestId("qr-code")).toBeDefined();
  });

  test("an unrestricted session cannot open the mandatory enrollment screen", async () => {
    const settings = installSettingsServer({ me: meFixture() });
    const twoFactor = installTwoFactorServer(settings);
    const { router } = start("/login/enroll-2fa", settings);

    expect(await screen.findByTestId("app-shell")).toBeDefined();
    expect(router.state.location.pathname).toBe("/overview");
    expect(twoFactor.enrollCalls).toBe(0);
  });
});
