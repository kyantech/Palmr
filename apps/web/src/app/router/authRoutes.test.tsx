import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { useEffect } from "react";
import { Outlet, type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { qk } from "../../shared/api/query-keys";
import {
  BOOTSTRAP_URL,
  bootstrapFixture,
  errorEnvelope,
  ME_URL,
  meFixture,
} from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { authenticatedRoutes } from "../guards/chain";
import { authRoutes } from "./authRoutes";
import { RootRedirect } from "./RootRedirect";

const STATUS_URL = "*/api/v1/setup/status";
const SETUP_URL = "*/api/v1/setup";
const LOGIN_URL = "*/api/v1/auth/login";

const lifecycle = { mounts: 0 };

function MountProbe() {
  useEffect(() => {
    lifecycle.mounts += 1;
  }, []);
  return <Outlet />;
}

function Landing() {
  const { pathname, search } = useLocation();
  return <output data-testid="landing">{`${pathname}${search}`}</output>;
}

const routes: RouteObject[] = [
  {
    element: <MountProbe />,
    children: [
      { path: "/", element: <RootRedirect /> },
      ...authRoutes,
      ...authenticatedRoutes([
        { path: "/overview", element: <Landing /> },
        { path: "/files/:id", element: <Landing /> },
      ]),
      { path: "*", element: <Landing /> },
    ],
  },
];

interface Instance {
  setupCompleted: boolean;
  signedIn: boolean;
  calls: { bootstrap: number; me: number; status: number; setup: number; login: number };
  setupBodies: unknown[];
}

function installInstance(initial: Pick<Instance, "setupCompleted" | "signedIn">): Instance {
  const instance: Instance = {
    ...initial,
    calls: { bootstrap: 0, me: 0, status: 0, setup: 0, login: 0 },
    setupBodies: [],
  };
  server.use(
    http.get(BOOTSTRAP_URL, () => {
      instance.calls.bootstrap += 1;
      return HttpResponse.json(
        bootstrapFixture({
          setupCompleted: instance.setupCompleted,
          appName: instance.setupCompleted ? "Acme Files" : "Palmr",
        }),
      );
    }),
    http.get(ME_URL, () => {
      instance.calls.me += 1;
      return instance.signedIn
        ? HttpResponse.json(meFixture({ role: "admin" }))
        : errorEnvelope("AUTH_REQUIRED", 401, "req-anon");
    }),
    http.get(STATUS_URL, () => {
      instance.calls.status += 1;
      return HttpResponse.json(
        instance.setupCompleted
          ? { setupCompleted: true }
          : { setupCompleted: false, passwordMinLength: 8 },
      );
    }),
    http.post(SETUP_URL, async ({ request }) => {
      instance.calls.setup += 1;
      instance.setupBodies.push(await request.json());
      if (instance.setupCompleted) {
        return errorEnvelope("SETUP_ALREADY_COMPLETED", 409, "req-409");
      }
      instance.setupCompleted = true;
      instance.signedIn = true;
      return HttpResponse.json(
        { user: { id: "u1", username: "ada", role: "admin" }, mustChangePassword: false },
        { status: 201 },
      );
    }),
    http.post(LOGIN_URL, () => {
      instance.calls.login += 1;
      instance.signedIn = true;
      return HttpResponse.json({
        user: { id: "u1", username: "ada", role: "admin" },
        mustChangePassword: false,
        mfaEnrollmentRequired: false,
      });
    }),
  );
  return instance;
}

async function landing() {
  return (await screen.findByTestId("landing")).textContent;
}

beforeEach(() => {
  stubMatchMedia();
  lifecycle.mounts = 0;
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_setup_completion_reconciles", () => {
  test("setup reconciles /auth/me, refreshes bootstrap, and the guards route to /overview without a boot flash", async () => {
    const instance = installInstance({ setupCompleted: false, signedIn: false });
    const { router, locations, queryClient } = renderSession({ routes, initialEntries: ["/"] });
    const user = userEvent.setup();

    await screen.findByRole("heading", { level: 1, name: "Set up your instance" });
    expect(router.state.location.pathname).toBe("/setup");
    await user.clear(screen.getByLabelText("Instance name"));
    await user.type(screen.getByLabelText("Instance name"), "Acme Files");
    await user.type(screen.getByLabelText("First name"), "Ada");
    await user.type(screen.getByLabelText("Last name"), "Lovelace");
    await user.type(screen.getByLabelText("Username"), "ada");
    await user.type(screen.getByLabelText("E-mail"), "ada@example.test");
    await user.type(screen.getByLabelText("Password"), "correct horse battery");
    const submit = screen.getByRole<HTMLButtonElement>("button", { name: "Complete setup" });
    await waitFor(() => {
      expect(submit.disabled).toBe(false);
    });
    await user.click(submit);

    expect(await landing()).toBe("/overview");
    expect(locations).toEqual(["/", "/setup", "/", "/overview"]);
    expect(instance.calls).toMatchObject({ bootstrap: 2, me: 1, setup: 1 });
    expect(instance.setupBodies).toEqual([
      {
        appName: "Acme Files",
        firstName: "Ada",
        lastName: "Lovelace",
        username: "ada",
        email: "ada@example.test",
        password: "correct horse battery",
        locale: "en-US",
      },
    ]);
    expect(queryClient.getQueryData(qk.bootstrap())).toMatchObject({ setupCompleted: true });
    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({ user: { role: "admin" } });
    expect(lifecycle.mounts).toBe(1);
    expect(screen.queryByText("Loading Palmr")).toBeNull();
  });

  test("a setup lost to another browser reconciles to /login instead of re-opening setup", async () => {
    const instance = installInstance({ setupCompleted: false, signedIn: false });
    renderSession({ routes, initialEntries: ["/setup"] });
    const user = userEvent.setup();
    await screen.findByRole("heading", { level: 1, name: "Set up your instance" });
    instance.setupCompleted = true;

    await user.type(screen.getByLabelText("First name"), "Eve");
    await user.type(screen.getByLabelText("Last name"), "Late");
    await user.type(screen.getByLabelText("Username"), "eve");
    await user.type(screen.getByLabelText("E-mail"), "eve@example.test");
    await user.type(screen.getByLabelText("Password"), "another password");
    await user.click(screen.getByRole("button", { name: "Complete setup" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(instance.calls.setup).toBe(1);
    expect(instance.signedIn).toBe(false);
  });

  test("/login is unreachable while setup is incomplete", async () => {
    installInstance({ setupCompleted: false, signedIn: false });
    const { router } = renderSession({ routes, initialEntries: ["/login"] });

    await screen.findByRole("heading", { level: 1, name: "Set up your instance" });
    expect(router.state.location.pathname).toBe("/setup");
  });
});

describe("component_login_reconciles_and_preserves_next", () => {
  async function loginFrom(entry: string) {
    const instance = installInstance({ setupCompleted: true, signedIn: false });
    const view = renderSession({ routes, initialEntries: [entry] });
    const user = userEvent.setup();
    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    expect(screen.getByTestId("auth-brand").textContent).toBe("Acme Files");
    await user.type(screen.getByLabelText("E-mail or username"), "ada");
    await user.type(screen.getByLabelText("Password"), "correct horse battery");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    return { instance, ...view };
  }

  test("a validated next survives login and the session comes from /auth/me", async () => {
    const { instance, queryClient } = await loginFrom(
      `/login?next=${encodeURIComponent("/files/abc?sort=name")}`,
    );

    expect(await landing()).toBe("/files/abc?sort=name");
    expect(instance.calls).toMatchObject({ login: 1, me: 2, bootstrap: 2 });
    expect(queryClient.getQueryData(qk.me.current())).toEqual(meFixture({ role: "admin" }));
    expect(lifecycle.mounts).toBe(1);
  });

  test("a successful login reconciles /auth/me and explicitly refetches qk.bootstrap()", async () => {
    const { instance, queryClient } = await loginFrom("/login");

    expect(await landing()).toBe("/overview");
    expect(instance.calls).toMatchObject({ login: 1, me: 2, bootstrap: 2 });
    expect(instance.calls.bootstrap).toBeGreaterThan(1);
    expect(queryClient.getQueryData(qk.bootstrap())).toMatchObject({ setupCompleted: true });
    expect(queryClient.getQueryData(qk.me.current())).toEqual(meFixture({ role: "admin" }));
  });

  test.each([
    `/login?next=${encodeURIComponent("//evil.example")}`,
    `/login?next=${encodeURIComponent("https://evil.example/")}`,
    "/login",
  ])("%s falls back to root routing", async (entry) => {
    await loginFrom(entry);

    expect(await landing()).toBe("/overview");
  });
});

describe("component_bootstrap_has_no_session_state", () => {
  test("a stray session field in the bootstrap payload is ignored; /auth/me stays authoritative", async () => {
    server.use(
      http.get(BOOTSTRAP_URL, () =>
        HttpResponse.json({
          ...bootstrapFixture({ setupCompleted: true, appName: "Acme Files" }),
          session: { user: { username: "ghost" } },
        }),
      ),
      http.get(ME_URL, () => errorEnvelope("AUTH_REQUIRED", 401, "req-anon")),
    );
    const { router, queryClient } = renderSession({ routes, initialEntries: ["/"] });

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(screen.getByTestId("auth-brand").textContent).toBe("Acme Files");
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
  });
});
