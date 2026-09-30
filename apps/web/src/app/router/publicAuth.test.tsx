import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { installTwoFactorServer } from "../../test/authServer";
import { errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerState } from "../../test/settingsServer";
import type { Restriction } from "../bootstrap/queries";
import { appRoutes } from "./routes";

const RESET_TOKEN = "rst_4hG8kPq2vX9zL0mN3bC7dF1jT6wY5sR";
const INVITE_TOKEN = "inv_Qm2Lp8Xz4Vb7Nc1Kd9Hf3Js6Tg0Rw5Ye";
const CHECK_URL = "*/api/v1/auth/password/reset/check";
const RESET_URL = "*/api/v1/auth/password/reset";
const INVITE_URL = `*/api/v1/public/invites/:token`;
const ACCEPT_URL = `*/api/v1/public/invites/:token/accept`;
const INVITED_EMAIL = "grace@example.test";
const NEW_PASSWORD = "a fresh long passphrase";

type User = ReturnType<typeof userEvent.setup>;

function anonymous(): SettingsServerState {
  const settings = installSettingsServer();
  settings.me = null;
  return settings;
}

function start(entry: string) {
  const harness = renderSession({ routes: appRoutes, initialEntries: [entry] });
  return { ...harness, user: userEvent.setup({ delay: null }) };
}

interface ResetServer {
  checkBodies: unknown[];
  resetBodies: unknown[];
}

function installResetServer({
  check = () =>
    HttpResponse.json({
      valid: true,
      passwordMinLength: 12,
      expiresAt: "2026-09-28T01:00:00Z",
      email: "leaked@example.test",
    }),
  reset = () => new HttpResponse(null, { status: 204 }),
}: { check?: () => Response; reset?: () => Response } = {}): ResetServer {
  const state: ResetServer = { checkBodies: [], resetBodies: [] };
  server.use(
    http.post(CHECK_URL, async ({ request }) => {
      state.checkBodies.push(await request.json());
      return check();
    }),
    http.post(RESET_URL, async ({ request }) => {
      state.resetBodies.push(await request.json());
      return reset();
    }),
  );
  return state;
}

async function chooseNewPassword(user: User, password = NEW_PASSWORD) {
  await user.type(await screen.findByLabelText("New password"), password);
  await user.type(screen.getByLabelText("Confirm new password"), password);
  await user.click(screen.getByRole("button", { name: "Reset password" }));
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_reset_password_route", () => {
  test("the token is checked with POST, the policy comes from the check and no account identity is rendered", async () => {
    anonymous();
    const reset = installResetServer();
    start(`/reset-password/${RESET_TOKEN}`);

    expect(await screen.findByText("Use at least 12 characters.")).toBeDefined();
    expect(screen.getByRole("heading", { level: 1, name: "Choose a new password" })).toBeDefined();
    expect(reset.checkBodies).toEqual([{ token: RESET_TOKEN }]);
    expect(document.body.textContent).not.toContain("leaked@example.test");
    expect(document.body.textContent).not.toContain("ada@example.test");
    expect(document.body.innerHTML).not.toContain(RESET_TOKEN);
  });

  test("success consumes the token, replaces the route with /login and shows the reset notice", async () => {
    anonymous();
    const reset = installResetServer();
    const { user, router, locations } = start(`/reset-password/${RESET_TOKEN}`);

    await chooseNewPassword(user);

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(screen.getByText("Your password was reset")).toBeDefined();
    expect(reset.resetBodies).toEqual([{ token: RESET_TOKEN, newPassword: NEW_PASSWORD }]);
    expect(router.state.location.pathname).toBe("/login");
    expect(router.state.historyAction).toBe("REPLACE");
    expect(locations.filter((location) => location.includes(RESET_TOKEN))).toEqual([
      `/reset-password/${RESET_TOKEN}`,
    ]);
  });

  test.each([
    ["RESET_TOKEN_INVALID", 400, "This reset link isn't valid"],
    ["RESET_TOKEN_EXPIRED", 410, "This reset link has expired"],
    ["RESET_TOKEN_USED", 410, "This reset link was already used"],
  ])(
    "a dead token (%s) renders its own state instead of redirecting",
    async (code, status, title) => {
      anonymous();
      installResetServer({ check: () => errorEnvelope(code, status, "req-dead") });
      const { user, router } = start(`/reset-password/${RESET_TOKEN}`);

      expect(await screen.findByRole("heading", { level: 1, name: title })).toBeDefined();
      expect(screen.getByTestId("reset-password-dead").dataset.errorCode).toBe(code);
      expect(router.state.location.pathname).toBe(`/reset-password/${RESET_TOKEN}`);
      expect(screen.queryByLabelText("New password")).toBeNull();

      await user.click(screen.getByRole("button", { name: "Request a new link" }));
      expect(
        await screen.findByRole("heading", { level: 1, name: "Reset your password" }),
      ).toBeDefined();
      expect(router.state.location.pathname).toBe("/login");
    },
  );

  test("a token used elsewhere while the form was open turns into the dead state on submit", async () => {
    anonymous();
    installResetServer({ reset: () => errorEnvelope("RESET_TOKEN_USED", 410, "req-used") });
    const { user } = start(`/reset-password/${RESET_TOKEN}`);

    await chooseNewPassword(user);

    expect(
      await screen.findByRole("heading", { level: 1, name: "This reset link was already used" }),
    ).toBeDefined();
  });

  test("PASSWORD_POLICY_VIOLATION uses the server minimum on the field", async () => {
    anonymous();
    installResetServer({
      reset: () =>
        HttpResponse.json(
          {
            error: {
              code: "PASSWORD_POLICY_VIOLATION",
              message: "too short",
              requestId: "req-policy",
              details: { minLength: 20 },
            },
          },
          { status: 422 },
        ),
    });
    const { user } = start(`/reset-password/${RESET_TOKEN}`);

    await chooseNewPassword(user);

    expect(await screen.findByText("Use at least 20 characters.")).toBeDefined();
    expect(screen.getByLabelText("New password").getAttribute("aria-invalid")).toBe("true");
  });

  test("AUTH_PASSWORD_LOGIN_DISABLED is reported without leaving the page", async () => {
    anonymous();
    installResetServer({
      reset: () => errorEnvelope("AUTH_PASSWORD_LOGIN_DISABLED", 403, "req-off"),
    });
    const { user, router } = start(`/reset-password/${RESET_TOKEN}`);

    await chooseNewPassword(user);

    expect(await screen.findByText("Password sign-in is disabled on this instance.")).toBeDefined();
    expect(router.state.location.pathname).toBe(`/reset-password/${RESET_TOKEN}`);
  });
});

interface InviteServer {
  lookups: string[];
  accepts: { token: string; body: Record<string, unknown> }[];
}

function installInviteServer(
  settings: SettingsServerState,
  {
    lookup = () =>
      HttpResponse.json({
        valid: true,
        email: INVITED_EMAIL,
        passwordMinLength: 10,
        expiresAt: "2026-10-01T12:00:00Z",
        role: "admin",
        inviter: "root-admin",
      }),
    accept,
    restriction = null,
  }: {
    lookup?: () => Response;
    accept?: (calls: number) => Response | null;
    restriction?: Restriction | null;
  } = {},
): InviteServer {
  const state: InviteServer = { lookups: [], accepts: [] };
  server.use(
    http.get(INVITE_URL, ({ params }) => {
      state.lookups.push(String(params.token));
      return lookup();
    }),
    http.post(ACCEPT_URL, async ({ params, request }) => {
      state.accepts.push({
        token: String(params.token),
        body: (await request.json()) as Record<string, unknown>,
      });
      const override = accept?.(state.accepts.length) ?? null;
      if (override !== null) {
        return override;
      }
      settings.me = meFixture({ restriction });
      return HttpResponse.json(
        {
          user: { id: "u2", username: "grace", role: "user" },
          mustChangePassword: false,
          mfaEnrollmentRequired: restriction === "mfa_enrollment_required",
        },
        { status: 201 },
      );
    }),
  );
  return state;
}

async function fillInvite(user: User, username = "grace") {
  await user.type(await screen.findByLabelText("First name"), "Grace");
  await user.type(screen.getByLabelText("Last name"), "Hopper");
  await user.type(screen.getByLabelText("Username"), username);
  await user.type(screen.getByLabelText("Password"), NEW_PASSWORD);
  await user.type(screen.getByLabelText("Confirm new password"), NEW_PASSWORD);
  await user.click(screen.getByRole("button", { name: "Create account" }));
}

describe("component_invite_accept_route", () => {
  test("the bound e-mail is shown read-only and nothing else about the invite is disclosed", async () => {
    const settings = anonymous();
    const invite = installInviteServer(settings);
    start(`/invite/${INVITE_TOKEN}`);

    expect(await screen.findByTestId("invite-email")).toBeDefined();
    expect(screen.getByTestId("invite-email").textContent).toBe(INVITED_EMAIL);
    expect(screen.queryByDisplayValue(INVITED_EMAIL)).toBeNull();
    expect(document.querySelector("input[type='email']")).toBeNull();
    expect(screen.queryByLabelText("E-mail")).not.toBeInstanceOf(HTMLInputElement);
    expect(document.body.textContent).not.toMatch(/root-admin|admin/);
    expect(screen.getByText("Use at least 10 characters.")).toBeDefined();
    expect(invite.lookups).toEqual([INVITE_TOKEN]);
  });

  test("the accept DTO carries only the profile fields; e-mail, role and token never enter the body", async () => {
    const settings = anonymous();
    const invite = installInviteServer(settings);
    const { user, router } = start(`/invite/${INVITE_TOKEN}`);

    await fillInvite(user);

    expect(await screen.findByTestId("app-shell")).toBeDefined();
    expect(router.state.location.pathname).toBe("/overview");
    expect(invite.accepts).toHaveLength(1);
    const [{ token, body }] = invite.accepts as [InviteServer["accepts"][number]];
    expect(token).toBe(INVITE_TOKEN);
    expect(Object.keys(body).sort()).toEqual(
      ["firstName", "lastName", "locale", "password", "username"].sort(),
    );
    expect(body).toEqual({
      firstName: "Grace",
      lastName: "Hopper",
      username: "grace",
      password: NEW_PASSWORD,
      locale: "en-US",
    });
    expect(JSON.stringify(body)).not.toContain(INVITED_EMAIL);
    expect(JSON.stringify(body)).not.toContain(INVITE_TOKEN);
  });

  test("the server restriction decides where a new account lands", async () => {
    const settings = anonymous();
    installInviteServer(settings, { restriction: "mfa_enrollment_required" });
    installTwoFactorServer(settings);
    const { user, router } = start(`/invite/${INVITE_TOKEN}`);

    await fillInvite(user);

    expect(
      await screen.findByRole("heading", { level: 1, name: "Set up two-factor authentication" }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe("/login/enroll-2fa");
  });

  test("a taken username keeps the form filled so another one can be chosen", async () => {
    const settings = anonymous();
    const invite = installInviteServer(settings, {
      accept: (calls) =>
        calls === 1 ? errorEnvelope("USER_USERNAME_TAKEN", 409, "req-taken") : null,
    });
    const { user, router } = start(`/invite/${INVITE_TOKEN}`);

    await fillInvite(user);

    expect(await screen.findByText("This username is already in use.")).toBeDefined();
    expect(screen.getByLabelText("Username").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByLabelText<HTMLInputElement>("First name").value).toBe("Grace");
    expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe(NEW_PASSWORD);

    await user.clear(screen.getByLabelText("Username"));
    await user.type(screen.getByLabelText("Username"), "ghopper");
    await user.click(screen.getByRole("button", { name: "Create account" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/overview");
    });
    expect(invite.accepts[1]?.body).toMatchObject({ username: "ghopper" });
  });

  test("USER_EMAIL_TAKEN is reported and no e-mail override is offered", async () => {
    const settings = anonymous();
    installInviteServer(settings, {
      accept: () => errorEnvelope("USER_EMAIL_TAKEN", 409, "req-email"),
    });
    const { user } = start(`/invite/${INVITE_TOKEN}`);

    await fillInvite(user);

    expect(await screen.findByText("This e-mail address is already in use.")).toBeDefined();
    expect(document.querySelector("input[type='email']")).toBeNull();
    expect(screen.queryByDisplayValue(INVITED_EMAIL)).toBeNull();
  });

  test.each([
    ["INVITE_NOT_FOUND", 404, "This invite link isn't valid"],
    ["INVITE_EXPIRED", 410, "This invite has expired"],
    ["INVITE_ALREADY_USED", 410, "This invite was already used"],
    ["INVITE_REVOKED", 410, "This invite was withdrawn"],
  ])("a dead invite (%s) renders a terminal state, not a redirect", async (code, status, title) => {
    const settings = anonymous();
    installInviteServer(settings, { lookup: () => errorEnvelope(code, status, "req-dead") });
    const { router } = start(`/invite/${INVITE_TOKEN}`);

    expect(await screen.findByRole("heading", { level: 1, name: title })).toBeDefined();
    expect(router.state.location.pathname).toBe(`/invite/${INVITE_TOKEN}`);
    expect(screen.getByRole("button", { name: "Go to sign in" })).toBeDefined();
  });

  test("an invite consumed while the form was open becomes the used state on submit", async () => {
    const settings = anonymous();
    installInviteServer(settings, {
      accept: () => errorEnvelope("INVITE_ALREADY_USED", 410, "req-used"),
    });
    const { user } = start(`/invite/${INVITE_TOKEN}`);

    await fillInvite(user);

    expect(
      await screen.findByRole("heading", { level: 1, name: "This invite was already used" }),
    ).toBeDefined();
  });
});
