import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../features/auth";
import { qk } from "../../shared/api/query-keys";
import { errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import {
  CURRENT_SESSION_ID,
  installSettingsServer,
  OTHER_SESSION_ID,
  PROXIED_SESSION_ID,
  type SettingsServerOptions,
} from "../../test/settingsServer";
import { appRoutes } from "./routes";

const OLD_PASSWORD = "correct horse battery";
const NEW_PASSWORD = "a much longer passphrase";

function renderSettings(path: string, options: SettingsServerOptions = {}) {
  const state = installSettingsServer(options);
  const session = renderSession({ routes: appRoutes, initialEntries: [path] });
  return { state, ...session, user: userEvent.setup({ delay: null }) };
}

type User = ReturnType<typeof userEvent.setup>;

async function fillPasswordForm(user: User, confirmation = NEW_PASSWORD) {
  await user.type(await screen.findByLabelText("Current password"), OLD_PASSWORD);
  await user.type(screen.getByLabelText("New password"), NEW_PASSWORD);
  await user.type(screen.getByLabelText("Confirm new password"), confirmation);
  await user.click(screen.getByRole("button", { name: "Change password" }));
}

async function recentAuthDialog(): Promise<HTMLElement> {
  const title = await screen.findByText("Confirm it's you");
  const dialog = title.closest<HTMLElement>("[role='dialog']");
  if (dialog === null) {
    throw new Error("the recent-auth title is not inside a dialog");
  }
  return dialog;
}

async function confirmRecentAuth(user: User) {
  const dialog = await recentAuthDialog();
  await user.type(within(dialog).getByLabelText("Password"), OLD_PASSWORD);
  await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
}

async function confirmPopover(user: User, title: string, action: string) {
  const heading = await screen.findByText(title);
  const popover = heading.closest(".ant-popover");
  if (!(popover instanceof HTMLElement)) {
    throw new Error(`no confirmation popover titled ${title}`);
  }
  await user.click(within(popover).getByRole("button", { name: action }));
}

function sessionRows() {
  return screen.getAllByTestId("session-row");
}

function row(id: string) {
  const match = sessionRows().find((item) => item.dataset.sessionId === id);
  if (match === undefined) {
    throw new Error(`no row for session ${id}`);
  }
  return match;
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("self password change", () => {
  test("uses the shared recent-auth modal and replays the same DTO including currentPassword", async () => {
    const { state, user, queryClient } = renderSettings("/settings/security");
    await screen.findByRole("heading", { level: 2, name: "Password" });
    queryClient.setQueryData(qk.me.sessions(), { pages: [], pageParams: [null] });
    expect(await screen.findByText("Use at least 8 characters.")).toBeDefined();

    await fillPasswordForm(user);
    expect(screen.queryByRole("alert")).toBeNull();
    await confirmRecentAuth(user);

    expect(await screen.findByText("Password changed.")).toBeDefined();
    expect(state.reauthBodies).toEqual([{ password: OLD_PASSWORD }]);
    expect(state.passwordBodies).toEqual([
      { currentPassword: OLD_PASSWORD, newPassword: NEW_PASSWORD },
      { currentPassword: OLD_PASSWORD, newPassword: NEW_PASSWORD },
    ]);
    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(screen.getByLabelText<HTMLInputElement>("Current password").value).toBe("");
    expect(queryClient.getQueryState(qk.me.sessions())?.isInvalidated).toBe(true);
    expect(state.calls.me).toBeGreaterThanOrEqual(3);
    expect(screen.queryByText("Confirm it's you")).toBeNull();
  });

  test("cancelling the challenge keeps the form filled and changes nothing", async () => {
    const { state, user } = renderSettings("/settings/security");
    await fillPasswordForm(user);
    const dialog = await recentAuthDialog();

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(recentAuthStore.getState().challenge).toBeNull();
    });
    expect(state.passwordBodies).toHaveLength(1);
    expect(screen.getByLabelText<HTMLInputElement>("Current password").value).toBe(OLD_PASSWORD);
    expect(screen.queryByText("Password changed.")).toBeNull();
  });

  test("PASSWORD_CURRENT_INVALID is shown on the current-password field by code, not message", async () => {
    const { user } = renderSettings("/settings/security", {
      recentAuth: true,
      passwordFailure: () =>
        errorEnvelope("PASSWORD_CURRENT_INVALID", 403, "req-bad-current", {
          message: "argon2 mismatch for user ada",
        }),
    });

    await fillPasswordForm(user);

    expect(await screen.findByText("The current password is incorrect.")).toBeDefined();
    expect(screen.getByLabelText("Current password").getAttribute("aria-invalid")).toBe("true");
    expect(screen.queryByText(/argon2/)).toBeNull();
    expect(screen.queryByText("Confirm it's you")).toBeNull();
  });

  test("PASSWORD_POLICY_VIOLATION uses the server's minLength detail on the new-password field", async () => {
    const { user } = renderSettings("/settings/security", {
      recentAuth: true,
      passwordFailure: () =>
        HttpResponse.json(
          {
            error: {
              code: "PASSWORD_POLICY_VIOLATION",
              message: "too short",
              requestId: "req-policy",
              details: { minLength: 30 },
            },
          },
          { status: 422, headers: { "X-Request-Id": "req-policy" } },
        ),
    });

    await fillPasswordForm(user);

    expect(await screen.findByText("Use at least 30 characters.")).toBeDefined();
    expect(screen.getByLabelText("New password").getAttribute("aria-invalid")).toBe("true");
  });

  test("AUTH_PASSWORD_LOGIN_DISABLED surfaces the mapped alert", async () => {
    const { user } = renderSettings("/settings/security", {
      recentAuth: true,
      passwordFailure: () => errorEnvelope("AUTH_PASSWORD_LOGIN_DISABLED", 403, "req-disabled"),
    });

    await fillPasswordForm(user);

    expect(await screen.findByText("Password sign-in is disabled on this instance.")).toBeDefined();
  });

  test("a mismatched confirmation is caught before any request", async () => {
    const { state, user } = renderSettings("/settings/security");

    await fillPasswordForm(user, "something else entirely");

    expect(await screen.findByText("The passwords don't match.")).toBeDefined();
    expect(state.passwordBodies).toEqual([]);
  });

  test("an account without a usable local password gets no password form", async () => {
    renderSettings("/settings/security", {
      me: meFixture({ capabilities: { hasLocalPassword: false, canChangePassword: false } }),
    });

    expect(
      await screen.findByText(
        "This account signs in with an external provider and has no password to change.",
      ),
    ).toBeDefined();
    expect(screen.queryByLabelText("Current password")).toBeNull();
  });
});

describe("session management", () => {
  test("marks the current session, lists it first and never guesses a missing IP", async () => {
    renderSettings("/settings/sessions");
    await screen.findByRole("list", { name: "Active sessions" });

    expect(sessionRows().map((item) => item.dataset.sessionId)).toEqual([
      CURRENT_SESSION_ID,
      OTHER_SESSION_ID,
      PROXIED_SESSION_ID,
    ]);
    const current = row(CURRENT_SESSION_ID);
    expect(current.dataset.current).toBe("true");
    expect(within(current).getByText("Current session")).toBeDefined();
    expect(within(current).getByText("Chrome on macOS")).toBeDefined();
    expect(within(current).getByText("IP address 203.0.113.24")).toBeDefined();
    expect(within(current).getByRole("button", { name: /Sign out of the current session/ }));
    expect(within(current).queryByRole("button", { name: /Revoke/ })).toBeNull();

    const other = row(OTHER_SESSION_ID);
    expect(within(other).queryByText("Current session")).toBeNull();
    expect(within(other).getByText("Firefox on Windows")).toBeDefined();
    expect(within(other).getByText("Password")).toBeDefined();

    const proxied = row(PROXIED_SESSION_ID);
    expect(within(proxied).getByText("IP address unavailable")).toBeDefined();
    expect(within(proxied).queryByText(/IP address \d/)).toBeNull();
    expect(within(proxied).getByText("Unknown browser")).toBeDefined();
    expect(within(proxied).getByText("External provider")).toBeDefined();
    expect(screen.getAllByText("Current session")).toHaveLength(1);
  });

  test("revoking another session deletes exactly that session and removes it from the list", async () => {
    const { state, user } = renderSettings("/settings/sessions");
    await screen.findByRole("list", { name: "Active sessions" });

    await user.click(
      within(row(OTHER_SESSION_ID)).getByRole("button", {
        name: "Revoke session: Firefox on Windows",
      }),
    );
    await confirmPopover(user, "Revoke this session?", "Revoke");

    expect(await screen.findByText("The session was revoked.")).toBeDefined();
    expect(state.revoked).toEqual([OTHER_SESSION_ID]);
    await waitFor(() => {
      expect(sessionRows().map((item) => item.dataset.sessionId)).toEqual([
        CURRENT_SESSION_ID,
        PROXIED_SESSION_ID,
      ]);
    });
  });

  test("a session that is already gone reports SESSION_NOT_FOUND and refreshes the list", async () => {
    const { state, user } = renderSettings("/settings/sessions");
    await screen.findByRole("list", { name: "Active sessions" });
    state.sessions = state.sessions.filter((session) => session.id !== OTHER_SESSION_ID);

    await user.click(
      within(row(OTHER_SESSION_ID)).getByRole("button", {
        name: "Revoke session: Firefox on Windows",
      }),
    );
    await confirmPopover(user, "Revoke this session?", "Revoke");

    expect(
      await screen.findByText(
        "This session no longer exists. It may already have been signed out.",
      ),
    ).toBeDefined();
    await waitFor(() => {
      expect(sessionRows()).toHaveLength(2);
    });
  });

  test("sign out other sessions goes through recent auth and never revokes the current session", async () => {
    const { state, user, router } = renderSettings("/settings/sessions");
    await screen.findByRole("list", { name: "Active sessions" });

    await user.click(screen.getByRole("button", { name: "Sign out other sessions" }));
    await confirmPopover(user, "Sign out all other sessions?", "Sign out others");
    await confirmRecentAuth(user);

    expect(await screen.findByText("All other sessions were signed out.")).toBeDefined();
    expect(state.bulkRevokeQueries).toEqual(["", ""]);
    await waitFor(() => {
      expect(sessionRows().map((item) => item.dataset.sessionId)).toEqual([CURRENT_SESSION_ID]);
    });
    expect(state.revoked).toEqual([]);
    expect(router.state.location.pathname).toBe("/settings/sessions");
    expect(screen.getByRole("button", { name: "Sign out other sessions" })).toHaveProperty(
      "disabled",
      true,
    );
  });

  test("ending the current session requires explicit confirmation and signs the SPA out", async () => {
    const { state, user, router, queryClient } = renderSettings("/settings/sessions");
    await screen.findByRole("list", { name: "Active sessions" });

    await user.click(
      within(row(CURRENT_SESSION_ID)).getByRole("button", {
        name: "Sign out of the current session: Chrome on macOS",
      }),
    );
    expect(
      await screen.findByText("This ends your current session. You will need to sign in again."),
    ).toBeDefined();
    expect(state.revoked).toEqual([]);
    await confirmPopover(user, "Sign out of this browser?", "Sign out");

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(state.revoked).toEqual([CURRENT_SESSION_ID]);
    expect(router.state.location.pathname).toBe("/login");
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(queryClient.getQueryData(qk.me.sessions())).toBeUndefined();
    expect(state.calls.bootstrap).toBe(2);
  });
});
