import type { QueryClient } from "@tanstack/react-query";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { useState } from "react";
import { type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { openRecentAuthChallenge, recentAuthStore } from "../../features/auth";
import { qk } from "../../shared/api/query-keys";
import { bootHandlers, errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { authenticatedRoutes } from "../guards/chain";
import { authRoutes } from "../router/authRoutes";
import { useSignOut } from "./useSignOut";

const LOGOUT_URL = "*/api/v1/auth/logout";

function SignOutProbe() {
  const { signOut, isPending } = useSignOut();
  const { pathname } = useLocation();
  const [failed, setFailed] = useState(false);
  return (
    <>
      <output data-testid="routed">{pathname}</output>
      <button
        type="button"
        disabled={isPending}
        onClick={() => {
          signOut().catch(() => {
            setFailed(true);
          });
        }}
      >
        Sign out
      </button>
      {failed ? <p role="alert">sign-out failed</p> : null}
    </>
  );
}

const routes: RouteObject[] = [
  ...authRoutes,
  ...authenticatedRoutes([{ path: "/overview", element: <SignOutProbe /> }]),
];

function seed(queryClient: QueryClient) {
  queryClient.setQueryData(qk.public.all(), { kept: true });
  queryClient.setQueryData(["files", "list", "root"], { items: ["secret.pdf"] });
  queryClient.setQueryData(["me", "sessions"], { items: [] });
}

function installLogout(respond: () => Response) {
  const calls = { logout: 0 };
  server.use(
    http.post(LOGOUT_URL, () => {
      calls.logout += 1;
      return respond();
    }),
  );
  return calls;
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_logout_reconciles", () => {
  test("logout clears the session through the shared reconciliation and the guards route to /login", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    const logout = installLogout(() => new HttpResponse(null, { status: 204 }));
    const { queryClient, router } = renderSession({
      routes,
      initialEntries: ["/overview"],
      seed,
    });
    const user = userEvent.setup();
    await screen.findByTestId("routed");
    openRecentAuthChallenge({
      originPath: "/overview",
      requestId: null,
      replay: () => Promise.resolve(),
    });

    await user.click(screen.getByRole("button", { name: "Sign out" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(logout.logout).toBe(1);
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(queryClient.getQueryData(["files", "list", "root"])).toBeUndefined();
    expect(queryClient.getQueryData(["me", "sessions"])).toBeUndefined();
    expect(queryClient.getQueryData(qk.public.all())).toEqual({ kept: true });
    expect(queryClient.getQueryData(qk.bootstrap())).toMatchObject({ setupCompleted: true });
    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(calls).toEqual({ bootstrap: 2, me: 1 });
  });

  test("a failed logout keeps the server-backed session and reports the failure", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    installLogout(() => errorEnvelope("CSRF_TOKEN_INVALID", 403, "req-csrf"));
    const { queryClient, router } = renderSession({
      routes,
      initialEntries: ["/overview"],
      seed,
    });
    const user = userEvent.setup();
    await screen.findByTestId("routed");

    await user.click(screen.getByRole("button", { name: "Sign out" }));

    expect(await screen.findByRole("alert")).toBeDefined();
    expect(router.state.location.pathname).toBe("/overview");
    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({ user: { username: "ada" } });
    expect(queryClient.getQueryData(["files", "list", "root"])).toEqual({ items: ["secret.pdf"] });
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });
});
