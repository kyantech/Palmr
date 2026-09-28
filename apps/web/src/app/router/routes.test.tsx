import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConfigProvider } from "antd";
import { I18nextProvider } from "react-i18next";
import type { RouteObject } from "react-router";
import { RouterProvider } from "react-router/dom";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { bootstrapFixture, meFixture } from "../../test/bootFixtures";
import { loadedI18n, renderRouter } from "../../test/renderRouter";
import { type BootState, BootStateContext } from "../bootstrap/bootState";
import { PATHS } from "./paths";
import { rootDestination } from "./RootRedirect";
import { appRoutes, createAppRouter } from "./routes";

const SETUP_INCOMPLETE: BootState = {
  bootstrap: bootstrapFixture({ setupCompleted: false }),
  me: null,
};
const ANONYMOUS: BootState = { bootstrap: bootstrapFixture(), me: null };
const AUTHENTICATED: BootState = { bootstrap: bootstrapFixture(), me: meFixture() };

function collectPaths(routes: RouteObject[]): (string | undefined)[] {
  return routes.flatMap((route) => [route.path, ...collectPaths(route.children ?? [])]);
}

let fetchSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  fetchSpy = vi.spyOn(globalThis, "fetch");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("production route table", () => {
  test("registers only the infrastructure routes owned so far", () => {
    expect(collectPaths(appRoutes).filter((path) => path !== undefined)).toEqual(["/", "*"]);
  });

  test("never declares /e, public Share, or unbuilt product routes", () => {
    const paths = collectPaths(appRoutes).join(" ");
    for (const forbidden of ["/e", "/s", "/r", "/login", "/setup", "/overview", "/admin"]) {
      expect(paths).not.toContain(forbidden);
    }
  });
});

describe("component_root_redirect_is_deterministic", () => {
  test.each([
    ["setup incomplete", SETUP_INCOMPLETE, PATHS.setup],
    [
      "setup incomplete even with a stale session",
      { ...SETUP_INCOMPLETE, me: meFixture() },
      PATHS.setup,
    ],
    ["anonymous", ANONYMOUS, PATHS.login],
    ["authenticated", AUTHENTICATED, PATHS.overview],
    [
      "authenticated and restricted",
      { ...AUTHENTICATED, me: meFixture({ restriction: "must_change_password" }) },
      PATHS.overview,
    ],
    [
      "authenticated admin needing 2FA",
      {
        ...AUTHENTICATED,
        me: meFixture({ role: "admin", restriction: "mfa_enrollment_required" }),
      },
      PATHS.overview,
    ],
  ])("%s → %s", async (_label, state, destination) => {
    expect(rootDestination(state)).toBe(destination);

    const { router } = await renderRouter(appRoutes, { state, initialEntries: ["/"] });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(destination);
    });
    expect(router.state.historyAction).toBe("REPLACE");
    expect(router.state.location.search).toBe("");
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  test("/ renders no content of its own before redirecting", async () => {
    const i18n = await loadedI18n();
    const { createMemoryRouter } = await import("react-router");
    const router = createMemoryRouter(
      [
        { path: "/", element: appRoutes[0]?.children?.[0]?.element },
        { path: "/login", element: null },
      ],
      { initialEntries: ["/"] },
    );
    const { container } = render(
      <I18nextProvider i18n={i18n}>
        <BootStateContext value={ANONYMOUS}>
          <RouterProvider router={router} />
        </BootStateContext>
      </I18nextProvider>,
    );

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(container.textContent).toBe("");
  });
});

describe("component_unknown_route_renders_404", () => {
  test("an unknown path renders the 404 panel and keeps the URL", async () => {
    const { router } = await renderRouter(appRoutes, {
      state: AUTHENTICATED,
      initialEntries: ["/definitely/not/a/route?x=1"],
    });

    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeDefined();
    expect(screen.getByText("404")).toBeDefined();
    expect(router.state.location.pathname).toBe("/definitely/not/a/route");
    expect(router.state.location.search).toBe("?x=1");
    expect(router.state.historyAction).toBe("POP");
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  test("/e/:token is not an SPA route: it falls through to the 404 panel", async () => {
    await renderRouter(appRoutes, { state: AUTHENTICATED, initialEntries: ["/e/abc123"] });

    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeDefined();
  });

  test("the 404 action navigates to / only when the user asks", async () => {
    const { router } = await renderRouter(appRoutes, {
      state: ANONYMOUS,
      initialEntries: ["/nope"],
    });

    await userEvent.click(await screen.findByRole("button", { name: "Go to the home page" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe(PATHS.login);
    });
  });
});

describe("sub-path deployment (R-072)", () => {
  afterEach(() => {
    document.head.querySelectorAll("base").forEach((element) => {
      element.remove();
    });
    window.history.replaceState(null, "", "/");
  });

  async function mountUnder(prefix: string, path: string, state: BootState) {
    const base = document.createElement("base");
    base.href = prefix;
    document.head.append(base);
    window.history.replaceState(null, "", `${prefix}${path.replace(/^\//, "")}`);
    const i18n = await loadedI18n();
    const router = createAppRouter();
    render(
      <I18nextProvider i18n={i18n}>
        <ConfigProvider>
          <BootStateContext value={state}>
            <RouterProvider router={router} />
          </BootStateContext>
        </ConfigProvider>
      </I18nextProvider>,
    );
    return router;
  }

  test("root redirect keeps the /palmr basename in the browser URL", async () => {
    const router = await mountUnder("/palmr/", "/", ANONYMOUS);

    await waitFor(() => {
      expect(window.location.pathname).toBe("/palmr/login");
    });
    expect(router.state.location.pathname).toBe("/palmr/login");
    expect(router.basename).toBe("/palmr");
  });

  test("a deep unknown path under the prefix renders 404 and navigation keeps the prefix", async () => {
    const router = await mountUnder("/services/palmr/", "/files/abc/def", AUTHENTICATED);

    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeDefined();
    expect(router.basename).toBe("/services/palmr");
    expect(window.location.pathname).toBe("/services/palmr/files/abc/def");

    await userEvent.click(screen.getByRole("button", { name: "Go to the home page" }));

    await waitFor(() => {
      expect(window.location.pathname).toBe("/services/palmr/overview");
    });
    expect(window.location.origin).toBe("http://localhost:3000");
  });

  test("application-relative paths are prefixed exactly once", async () => {
    const router = await mountUnder("/palmr/", "/somewhere", AUTHENTICATED);
    await screen.findByRole("heading", { name: "Page not found" });

    await act(async () => {
      await router.navigate("/elsewhere?tab=1");
    });

    expect(window.location.pathname).toBe("/palmr/elsewhere");
    expect(window.location.search).toBe("?tab=1");
    expect(window.location.pathname).not.toContain("/palmr/palmr");
  });
});
