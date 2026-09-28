import { useMutation, useQueryClient } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { useState } from "react";
import { type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../features/auth";
import { apiFetch } from "../../shared/api/apiFetch";
import { qk } from "../../shared/api/query-keys";
import type { components } from "../../shared/api/schema";
import { ME_URL, bootHandlers, errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { authenticatedRoutes } from "../guards/chain";

type PasswordChange = components["schemas"]["PasswordChangeRequest"];

const PASSWORD_URL = "*/api/v1/profile/password";
const REAUTH_URL = "*/api/v1/auth/reauthenticate";
const USAGE_URL = "*/api/v1/profile/usage";
const SERVER_PROSE = "argon2 verify failed for user ada: mismatch";

interface Lifecycle {
  onSuccess: number;
  onError: number;
  onSettled: number;
  invalidated: number;
}

const lifecycle: Lifecycle = { onSuccess: 0, onError: 0, onSettled: 0, invalidated: 0 };

function PasswordChangeForm() {
  const queryClient = useQueryClient();
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const change = useMutation({
    mutationKey: ["profile", "password"],
    mutationFn: (body: PasswordChange) => apiFetch("post", "/profile/password", { body }),
    onSuccess: async () => {
      lifecycle.onSuccess += 1;
      lifecycle.invalidated += 1;
      await queryClient.invalidateQueries({ queryKey: ["me", "sessions"] });
    },
    onError: () => {
      lifecycle.onError += 1;
    },
    onSettled: () => {
      lifecycle.onSettled += 1;
    },
  });
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        change.mutate({ currentPassword, newPassword });
      }}
    >
      <label>
        Current password
        <input
          type="password"
          value={currentPassword}
          onChange={(event) => {
            setCurrentPassword(event.target.value);
          }}
        />
      </label>
      <label>
        New password
        <input
          type="password"
          value={newPassword}
          onChange={(event) => {
            setNewPassword(event.target.value);
          }}
        />
      </label>
      <button type="submit">Change password</button>
      <output data-testid="mutation-status">{change.status}</output>
    </form>
  );
}

function Elsewhere() {
  const { pathname } = useLocation();
  return <output data-testid="routed">{pathname}</output>;
}

const routes: RouteObject[] = [
  ...authenticatedRoutes([
    { path: "/settings/password", element: <PasswordChangeForm /> },
    { path: "/overview", element: <Elsewhere /> },
  ]),
  { path: "*", element: <Elsewhere /> },
];

interface ServerState {
  passwordBodies: unknown[];
  reauthBodies: unknown[];
  meCalls: number;
  recentAuthUntil: string;
}

function installServer({
  me = meFixture(),
  reauth = () => new HttpResponse(null, { status: 204 }),
}: {
  me?: ReturnType<typeof meFixture>;
  reauth?: (state: ServerState) => Response;
} = {}): ServerState {
  const state: ServerState = {
    passwordBodies: [],
    reauthBodies: [],
    meCalls: 0,
    recentAuthUntil: me.session.recentAuthUntil,
  };
  const { handlers } = bootHandlers({ me });
  server.use(
    http.get(ME_URL, () => {
      state.meCalls += 1;
      return HttpResponse.json({
        ...me,
        session: { ...me.session, recentAuthUntil: state.recentAuthUntil },
      });
    }),
    ...handlers,
    http.post(PASSWORD_URL, async ({ request }) => {
      state.passwordBodies.push(await request.json());
      return state.passwordBodies.length === 1
        ? errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent", {
            message: "recent authentication required",
          })
        : new HttpResponse(null, { status: 204 });
    }),
    http.post(REAUTH_URL, async ({ request }) => {
      state.reauthBodies.push(await request.json());
      const response = reauth(state);
      if (response.status === 204) {
        state.recentAuthUntil = "2026-09-28T12:05:00Z";
      }
      return response;
    }),
  );
  return state;
}

async function startBlockedChange() {
  const harness = renderSession({ routes, initialEntries: ["/settings/password"] });
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("Current password"), "old-secret");
  await user.type(screen.getByLabelText("New password"), "new-secret-123");
  await user.click(screen.getByRole("button", { name: "Change password" }));
  const dialog = await screen.findByRole("dialog", { name: "Confirm it's you" });
  return { ...harness, user, dialog };
}

function expectOriginalFormIntact() {
  expect(screen.getByLabelText<HTMLInputElement>("Current password").value).toBe("old-secret");
  expect(screen.getByLabelText<HTMLInputElement>("New password").value).toBe("new-secret-123");
}

async function expectDialogClosed() {
  await waitFor(() => {
    expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).toBeNull();
  });
}

beforeEach(() => {
  stubMatchMedia();
  Object.assign(lifecycle, { onSuccess: 0, onError: 0, onSettled: 0, invalidated: 0 });
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

test("component_recent_auth_replays_request", async () => {
  const state = installServer();
  const { user, dialog, queryClient } = await startBlockedChange();

  expect(state.passwordBodies).toHaveLength(1);
  expect(lifecycle).toMatchObject({ onError: 1, onSuccess: 0 });
  const challenge = recentAuthStore.getState().challenge;
  expect(challenge).toMatchObject({ originPath: "/settings/password", requestId: "req-recent" });
  expectOriginalFormIntact();
  expect(within(dialog).getByText("Signed in as ada@example.test")).toBeDefined();

  const password = within(dialog).getByLabelText("Password");
  expect(password.getAttribute("autocomplete")).toBe("current-password");
  await waitFor(() => {
    expect(document.activeElement).toBe(password);
  });
  await user.type(password, "correct horse");
  await user.dblClick(within(dialog).getByRole("button", { name: "Confirm" }));

  await waitFor(() => {
    expect(screen.getByTestId("mutation-status").textContent).toBe("success");
  });
  expect(state.reauthBodies).toEqual([{ password: "correct horse" }]);
  expect(state.passwordBodies).toHaveLength(2);
  expect(state.passwordBodies[1]).toEqual(state.passwordBodies[0]);
  expect(state.passwordBodies[1]).toEqual({
    currentPassword: "old-secret",
    newPassword: "new-secret-123",
  });
  expect(lifecycle).toEqual({ onSuccess: 1, onError: 1, onSettled: 2, invalidated: 1 });
  expect(state.meCalls).toBe(2);
  expect(queryClient.getQueryData(qk.me.current())).toMatchObject({
    session: { recentAuthUntil: "2026-09-28T12:05:00Z" },
  });
  expect(recentAuthStore.getState().challenge).toBeNull();
  await expectDialogClosed();

  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 50));
  });
  expect(state.passwordBodies).toHaveLength(2);
  expect(state.reauthBodies).toHaveLength(1);
});

describe("component_recent_auth_challenge_lifecycle", () => {
  test("Cancel discards the challenge, never replays, and leaves the form untouched", async () => {
    const state = installServer();
    const { user, dialog } = await startBlockedChange();

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    expect(recentAuthStore.getState().challenge).toBeNull();
    await expectDialogClosed();
    expectOriginalFormIntact();
    expect(state.passwordBodies).toHaveLength(1);
    expect(state.reauthBodies).toHaveLength(0);
    expect(screen.getByTestId("mutation-status").textContent).toBe("error");
  });

  test("Escape behaves as Cancel", async () => {
    const state = installServer();
    const { user } = await startBlockedChange();

    await user.keyboard("{Escape}");

    expect(recentAuthStore.getState().challenge).toBeNull();
    await expectDialogClosed();
    expectOriginalFormIntact();
    expect(state.passwordBodies).toHaveLength(1);
  });

  test("leaving the originating route discards the challenge and nothing replays later", async () => {
    const state = installServer();
    const { router } = await startBlockedChange();
    const challenge = recentAuthStore.getState().challenge;
    expect(challenge).not.toBeNull();

    await act(async () => {
      await router.navigate("/overview");
    });

    expect(recentAuthStore.getState().challenge).toBeNull();
    await expectDialogClosed();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(state.passwordBodies).toHaveLength(1);
    expect(state.reauthBodies).toHaveLength(0);
  });

  test("a search-only change on the same route keeps the challenge", async () => {
    installServer();
    const { router } = await startBlockedChange();
    const challenge = recentAuthStore.getState().challenge;

    await act(async () => {
      await router.navigate("/settings/password?tab=security");
    });

    expect(recentAuthStore.getState().challenge).toBe(challenge);
  });

  test("session loss (AUTH_REQUIRED) destroys an open challenge", async () => {
    const state = installServer();
    server.use(http.get(USAGE_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-lost")));
    const { queryClient, router } = await startBlockedChange();
    expect(recentAuthStore.getState().challenge).not.toBeNull();

    await act(async () => {
      await queryClient
        .query({ queryKey: ["usage"], queryFn: () => apiFetch("get", "/profile/usage") })
        .catch(() => undefined);
    });

    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(state.passwordBodies).toHaveLength(1);
    expect(state.reauthBodies).toHaveLength(0);
    expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).toBeNull();
  });

  test("a wrong password shows the mapped error, keeps the modal and challenge, and replays nothing", async () => {
    const state = installServer({
      reauth: () =>
        errorEnvelope("AUTH_INVALID_CREDENTIALS", 401, "req-wrong", { message: SERVER_PROSE }),
    });
    const { user, dialog } = await startBlockedChange();
    const challenge = recentAuthStore.getState().challenge;

    await user.type(within(dialog).getByLabelText("Password"), "wrong");
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect(
      await within(dialog).findByText("Incorrect sign-in details. Check them and try again."),
    ).toBeDefined();
    expect(dialog.textContent).not.toContain(SERVER_PROSE);
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
    expect(within(dialog).getByLabelText("Password").getAttribute("aria-invalid")).toBe("true");
    expect(recentAuthStore.getState().challenge).toBe(challenge);
    expect(state.reauthBodies).toEqual([{ password: "wrong" }]);
    expect(state.passwordBodies).toHaveLength(1);
    expect(state.meCalls).toBe(1);
    expect(screen.getByRole("dialog", { name: "Confirm it's you" })).toBe(dialog);
    expectOriginalFormIntact();
  });

  test("RATE_LIMITED blocks resubmission for the Retry-After window and never auto-resubmits", async () => {
    const state = installServer({
      reauth: () =>
        errorEnvelope("RATE_LIMITED", 429, "req-429", { headers: { "Retry-After": "30" } }),
    });
    const { user, dialog } = await startBlockedChange();

    await user.type(within(dialog).getByLabelText("Password"), "maybe");
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect(
      await within(dialog).findByText("Too many attempts. Wait a moment and try again."),
    ).toBeDefined();
    expect(
      within(dialog).getByRole<HTMLButtonElement>("button", { name: "Confirm" }).disabled,
    ).toBe(true);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(state.reauthBodies).toHaveLength(1);
    expect(state.passwordBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
  });

  test("an unexpected reauthentication failure shows the request id in the modal", async () => {
    installServer({ reauth: () => errorEnvelope("INTERNAL_ERROR", 500, "req-500") });
    const { user, dialog } = await startBlockedChange();

    await user.type(within(dialog).getByLabelText("Password"), "whatever");
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain("Palmr ran into an unexpected problem.");
    expect(alert.textContent).toContain("Request ID: req-500");
    expect(recentAuthStore.getState().challenge).not.toBeNull();
  });
});

describe("component_recent_auth_fails_closed", () => {
  test("TOTP-enabled accounts get no password-only proof and nothing replays", async () => {
    const state = installServer({ me: meFixture({ capabilities: { twoFactorEnabled: true } }) });
    const { user, dialog } = await startBlockedChange();

    expect(within(dialog).queryByLabelText("Password")).toBeNull();
    expect(within(dialog).getByText("Two-factor confirmation required")).toBeDefined();
    expect(within(dialog).queryByRole("button", { name: "Confirm" })).toBeNull();

    await user.click(within(dialog).getByText("Close"));

    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(state.reauthBodies).toHaveLength(0);
    expect(state.passwordBodies).toHaveLength(1);
  });

  test("SSO-only accounts get no fabricated provider URL and nothing replays", async () => {
    const state = installServer({ me: meFixture({ capabilities: { hasLocalPassword: false } }) });
    const { user, dialog } = await startBlockedChange();

    expect(within(dialog).queryByLabelText("Password")).toBeNull();
    expect(within(dialog).getByText("Confirm with your sign-in provider")).toBeDefined();
    expect(within(dialog).queryAllByRole("link")).toHaveLength(0);
    expect(dialog.querySelector("a[href], form[action]")).toBeNull();

    await user.click(within(dialog).getByText("Close"));

    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(state.reauthBodies).toHaveLength(0);
    expect(state.passwordBodies).toHaveLength(1);
  });
});
