import { type QueryClient, useMutation, useQuery } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http } from "msw";
import { useEffect } from "react";
import { Outlet, type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { apiFetch } from "../../shared/api/apiFetch";
import { qk } from "../../shared/api/query-keys";
import { ApiError } from "../../shared/errors";
import { bootHandlers, errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { authenticatedRoutes } from "../guards/chain";
import { RootRedirect } from "../router/RootRedirect";

const USAGE_URL = "*/api/v1/profile/usage";
const SESSION_URL = "*/api/v1/sessions/:id";
const PASSWORD_URL = "*/api/v1/profile/password";
const PUBLIC_BRANDING_URL = "*/api/v1/public/branding/:asset";

const lifecycle = { mounts: 0, unmounts: 0 };

function MountProbe() {
  useEffect(() => {
    lifecycle.mounts += 1;
    return () => {
      lifecycle.unmounts += 1;
    };
  }, []);
  return <Outlet />;
}

function Routed() {
  const { pathname } = useLocation();
  return <output data-testid="routed">{pathname}</output>;
}

function UsageScreen() {
  useQuery({
    queryKey: ["files", "list", "root"],
    queryFn: () => apiFetch("get", "/profile/usage"),
  });
  return <Routed />;
}

function QueryAndMutationScreen() {
  useQuery({
    queryKey: ["files", "list", "root"],
    queryFn: () => apiFetch("get", "/profile/usage"),
  });
  const { mutate } = useMutation({
    mutationFn: () => apiFetch("delete", "/sessions/{id}", { path: { id: "other" } }),
  });
  useEffect(() => {
    mutate();
  }, [mutate]);
  return <Routed />;
}

function PublicScreen() {
  useQuery({
    queryKey: ["public", "branding", "logo"],
    queryFn: () => apiFetch("get", "/public/branding/{asset}", { path: { asset: "logo" } }),
  });
  return <Routed />;
}

function PasswordScreen() {
  const change = useMutation({
    mutationFn: () =>
      apiFetch("post", "/profile/password", {
        body: { currentPassword: "old", newPassword: "new-password-1" },
      }),
  });
  return (
    <button
      type="button"
      onClick={() => {
        change.mutate();
      }}
    >
      Change
    </button>
  );
}

const routes: RouteObject[] = [
  {
    element: <MountProbe />,
    children: [
      { path: "/", element: <RootRedirect /> },
      ...authenticatedRoutes([
        { path: "/overview", element: <UsageScreen /> },
        { path: "/files", element: <QueryAndMutationScreen /> },
        { path: "/settings/password", element: <PasswordScreen /> },
      ]),
      { path: "/login/forced-password-change", element: <UsageScreen /> },
      { path: "/s/:alias", element: <PublicScreen /> },
      { path: "*", element: <Routed /> },
    ],
  },
];

function seedCaches(queryClient: QueryClient) {
  queryClient.setQueryData(qk.public.all(), { kept: true });
  queryClient.setQueryData(["public", "share", "abc"], { alias: "abc" });
  queryClient.setQueryData(["files", "detail", "f1"], { id: "f1" });
  queryClient.setQueryData(["me", "sessions"], { items: [] });
  queryClient.setQueryData(["admin", "users"], { items: [] });
}

function expectSignedOutInPlace(queryClient: QueryClient) {
  expect(queryClient.getQueryData(qk.me.current())).toBeNull();
  expect(queryClient.getQueryData(qk.bootstrap())).toMatchObject({ setupCompleted: true });
  expect(queryClient.getQueryData(qk.public.all())).toEqual({ kept: true });
  expect(queryClient.getQueryData(["public", "share", "abc"])).toEqual({ alias: "abc" });
  expect(queryClient.getQueryData(["files", "detail", "f1"])).toBeUndefined();
  expect(queryClient.getQueryData(["files", "list", "root"])).toBeUndefined();
  expect(queryClient.getQueryData(["me", "sessions"])).toBeUndefined();
  expect(queryClient.getQueryData(["admin", "users"])).toBeUndefined();
  expect(screen.queryByText("Loading Palmr")).toBeNull();
  expect(lifecycle).toEqual({ mounts: 1, unmounts: 0 });
}

beforeEach(() => {
  stubMatchMedia();
  lifecycle.mounts = 0;
  lifecycle.unmounts = 0;
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_auth_required_reconciliation", () => {
  test("an authenticated query failing with AUTH_REQUIRED signs out in place and routes to login with next", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    let usageCalls = 0;
    server.use(
      http.get(USAGE_URL, () => {
        usageCalls += 1;
        return errorEnvelope("AUTH_REQUIRED", 401, "req-expired");
      }),
    );

    const { queryClient, router, locations } = renderSession({
      routes,
      initialEntries: ["/overview?tab=recent"],
      seed: seedCaches,
    });
    await screen.findByTestId("routed");

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });

    expect(router.state.location.search).toBe(
      `?next=${encodeURIComponent("/overview?tab=recent")}`,
    );
    expect(router.state.historyAction).toBe("REPLACE");
    expect(locations).toEqual([
      "/overview?tab=recent",
      `/login?next=${encodeURIComponent("/overview?tab=recent")}`,
    ]);
    expectSignedOutInPlace(queryClient);
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
    expect(usageCalls).toBe(1);
    expect((await screen.findByTestId("routed")).textContent).toBe("/login");
  });

  test("a mutation failing with AUTH_REQUIRED reconciles the same way", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    server.use(http.post(PASSWORD_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-m401")));

    const { queryClient, router, locations } = renderSession({
      routes,
      initialEntries: ["/settings/password"],
      seed: seedCaches,
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Change" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });

    expect(locations).toEqual([
      "/settings/password",
      `/login?next=${encodeURIComponent("/settings/password")}`,
    ]);
    expectSignedOutInPlace(queryClient);
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });

  test("near-simultaneous AUTH_REQUIRED failures reconcile exactly once", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    server.use(
      http.get(USAGE_URL, async () => {
        await delay(20);
        return errorEnvelope("AUTH_REQUIRED", 401, "req-a");
      }),
      http.delete(SESSION_URL, async () => {
        await delay(20);
        return errorEnvelope("AUTH_REQUIRED", 401, "req-b");
      }),
    );

    const { queryClient, router, locations, errorEvents } = renderSession({
      routes,
      initialEntries: ["/files"],
      seed: seedCaches,
    });
    const setQueryData = vi.spyOn(queryClient, "setQueryData");
    const removeQueries = vi.spyOn(queryClient, "removeQueries");
    const navigate = vi.spyOn(router, "navigate");
    setQueryData.mockClear();

    await waitFor(() => {
      expect(errorEvents).toHaveLength(2);
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });

    expect(errorEvents.map(({ source }) => source).sort()).toEqual(["mutation", "query"]);
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(removeQueries).toHaveBeenCalledTimes(1);
    expect(
      setQueryData.mock.calls.filter(
        ([key]) => JSON.stringify(key) === JSON.stringify(qk.me.current()),
      ),
    ).toHaveLength(1);
    expect(locations).toEqual(["/files", `/login?next=${encodeURIComponent("/files")}`]);
    expectSignedOutInPlace(queryClient);
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });

  test("regression: AUTH_REQUIRED keeps qk.bootstrap() and BootGate stays ready (no queryClient.clear())", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    server.use(http.get(USAGE_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-x")));

    const { queryClient, router } = renderSession({ routes, initialEntries: ["/overview"] });
    const clear = vi.spyOn(queryClient, "clear");
    const bootstrapBefore = await waitFor(() => {
      const data = queryClient.getQueryData(qk.bootstrap());
      expect(data).toBeDefined();
      return data;
    });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });

    expect(clear).not.toHaveBeenCalled();
    expect(queryClient.getQueryData(qk.bootstrap())).toBe(bootstrapBefore);
    expect(queryClient.getQueryState(qk.bootstrap())?.status).toBe("success");
    expect(queryClient.getQueryState(qk.me.current())?.status).toBe("success");
    expect(screen.queryByText("Loading Palmr")).toBeNull();
    expect(lifecycle).toEqual({ mounts: 1, unmounts: 0 });
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });

  test("me becomes null synchronously inside the error hook, without another /auth/me request", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    const { session, queryClient, router } = renderSession({
      routes,
      initialEntries: ["/settings/password"],
      seed: seedCaches,
    });
    await screen.findByRole("button", { name: "Change" });
    const query = queryClient
      .getQueryCache()
      .find<unknown, unknown, unknown>({ queryKey: qk.bootstrap() });
    if (query === undefined) {
      throw new Error("bootstrap query missing");
    }

    act(() => {
      session.handleError({
        source: "query",
        error: new ApiError({
          code: "AUTH_REQUIRED",
          status: 401,
          requestId: "req-sync",
          details: {},
          request: { method: "GET", path: "/profile/usage" },
          serverMessage: "session expired",
        }),
        client: queryClient,
        query,
      });
      expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });

  test("an AUTH_REQUIRED on an auth route falls back to plain /login instead of looping next", async () => {
    const { handlers } = bootHandlers({ me: meFixture({ restriction: "must_change_password" }) });
    server.use(...handlers);
    server.use(http.get(USAGE_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-lock")));

    const { router, locations } = renderSession({
      routes,
      initialEntries: ["/login/forced-password-change"],
      seed: seedCaches,
    });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(router.state.location.search).toBe("");
    expect(locations).toEqual(["/login/forced-password-change", "/login"]);
  });
});

describe("component_public_request_exclusion", () => {
  test("an AUTH_REQUIRED from a classified public request neither signs out nor navigates", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);
    server.use(
      http.get(PUBLIC_BRANDING_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-public")),
    );

    const { queryClient, router, locations, errorEvents } = renderSession({
      routes,
      initialEntries: ["/s/abc"],
      seed: seedCaches,
    });
    await screen.findByTestId("routed");

    await waitFor(() => {
      expect(errorEvents).toHaveLength(1);
    });

    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({ user: { username: "ada" } });
    expect(queryClient.getQueryData(["files", "detail", "f1"])).toEqual({ id: "f1" });
    expect(queryClient.getQueryData(["me", "sessions"])).toEqual({ items: [] });
    expect(router.state.location.pathname).toBe("/s/abc");
    expect(locations).toEqual(["/s/abc"]);
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });

  test("the expected boot-time /auth/me 401 produces no global error event and no second redirect", async () => {
    const { calls, handlers } = bootHandlers({ me: null });
    server.use(...handlers);

    const { queryClient, router, locations, errorEvents } = renderSession({
      routes,
      initialEntries: ["/"],
      seed: seedCaches,
    });

    expect((await screen.findByTestId("routed")).textContent).toBe("/login");
    expect(errorEvents).toEqual([]);
    expect(locations).toEqual(["/", "/login"]);
    expect(router.state.location.search).toBe("");
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
  });
});
