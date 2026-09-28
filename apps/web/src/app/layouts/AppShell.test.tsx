import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import type { RouteObject } from "react-router";
import { afterEach, expect, test, vi } from "vitest";
import { bootHandlers, bootstrapFixture, meFixture } from "../../test/bootFixtures";
import { renderRouter } from "../../test/renderRouter";
import { renderSession, resetSessionHarness } from "../../test/renderSession";
import { server } from "../../test/server";
import type { BootState } from "../bootstrap/bootState";
import { authenticatedRoutes } from "../guards/chain";
import { AppShell, type AppShellProps } from "./AppShell";
import { NAV_ENTRIES, type NavigationEntry } from "./navigation/registry";
import { PALMR_REPOSITORY_URL, POWERED_BY_TEXT } from "./ShellFooter";

const LOGOUT_URL = "*/api/v1/auth/logout";

const ADMIN_ENTRY: NavigationEntry = {
  key: "admin",
  path: "/admin",
  labelKey: "nav.more",
  icon: null,
  order: 60,
  requiredRole: "admin",
};

const FORBIDDEN_LABELS = ["Files", "Shared", "Received", "Transfers", "Admin"];

function stubViewportWidth(width: number) {
  vi.stubGlobal("matchMedia", (query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query)?.[1];
    const max = /max-width:\s*(\d+)px/.exec(query)?.[1];
    const matches =
      min !== undefined ? width >= Number(min) : max !== undefined ? width <= Number(max) : false;
    return {
      matches,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    };
  });
}

function shellRoutes(props: Partial<AppShellProps> = {}): RouteObject[] {
  return authenticatedRoutes([
    {
      element: <AppShell {...props} />,
      children: [{ path: "/overview", element: null }],
    },
  ]);
}

function bootState(role: "admin" | "user", bootstrap = bootstrapFixture()): BootState {
  return { bootstrap, me: meFixture({ role }) };
}

afterEach(() => {
  resetSessionHarness();
  vi.unstubAllGlobals();
});

test("component_appshell_desktop_navigation_uses_the_registry", async () => {
  stubViewportWidth(1280);
  await renderRouter(shellRoutes(), { state: bootState("user"), initialEntries: ["/overview"] });

  const sider = await screen.findByTestId("app-sider");
  const navigation = within(sider).getByRole("navigation", { name: "Primary navigation" });
  const link = within(navigation).getByRole("link", { name: "Overview" });
  expect(link.getAttribute("href")).toBe("/overview");
  expect(link.getAttribute("aria-current")).toBe("page");
  const settings = within(navigation).getByRole("link", { name: "Settings" });
  expect(settings.getAttribute("href")).toBe("/settings");
  expect(settings.getAttribute("aria-current")).toBeNull();
  expect(screen.getByTestId("app-main")).toBeDefined();
  expect(document.querySelector("[data-transfer-dock]")).not.toBeNull();
  expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull();
  expect(screen.queryByTestId("bottom-tabs")).toBeNull();
  for (const label of FORBIDDEN_LABELS) {
    expect(screen.queryByRole("link", { name: label })).toBeNull();
  }
});

test("component_appshell_desktop_sider_collapses_accessibly", async () => {
  stubViewportWidth(1280);
  await renderRouter(shellRoutes(), { state: bootState("user"), initialEntries: ["/overview"] });
  await screen.findByTestId("app-sider");

  const collapse = screen.getByRole("button", { name: "Collapse navigation" });
  expect(collapse.getAttribute("aria-expanded")).toBe("true");

  await userEvent.click(collapse);

  const expand = await screen.findByRole("button", { name: "Expand navigation" });
  expect(expand.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByTestId("app-sider")).toBeDefined();
});

test("component_appshell_below_lg_navigation_uses_a_drawer_with_the_same_registry", async () => {
  stubViewportWidth(768);
  const { router } = await renderRouter(shellRoutes(), {
    state: bootState("user"),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-shell");
  expect(screen.queryByTestId("app-sider")).toBeNull();
  expect(screen.queryByTestId("bottom-tabs")).toBeNull();
  expect(screen.queryByRole("link", { name: "Overview" })).toBeNull();

  await userEvent.click(screen.getByRole("button", { name: "Open navigation" }));

  const link = await screen.findByRole("link", { name: "Overview" });
  expect(link.getAttribute("href")).toBe("/overview");
  expect(link.getAttribute("aria-current")).toBe("page");
  expect(router.state.location.pathname).toBe("/overview");
});

test("component_appshell_xs_bottom_tabs_use_the_same_registry", async () => {
  stubViewportWidth(375);
  await renderRouter(shellRoutes(), { state: bootState("user"), initialEntries: ["/overview"] });

  const tabs = await screen.findByTestId("bottom-tabs");
  const link = within(tabs).getByRole("link", { name: "Overview" });
  expect(link.getAttribute("href")).toBe("/overview");
  expect(link.getAttribute("aria-current")).toBe("page");
  expect(within(tabs).queryByRole("link", { name: "Settings" })).toBeNull();
  expect(screen.queryByTestId("app-sider")).toBeNull();

  await userEvent.click(within(tabs).getByRole("button", { name: "More" }));

  const settings = await screen.findByRole("link", { name: "Settings" });
  expect(settings.getAttribute("href")).toBe("/settings");
});

test("component_nav_admin_only_for_admin", async () => {
  stubViewportWidth(1280);
  const entries = [...NAV_ENTRIES, ADMIN_ENTRY];

  const asUser = await renderRouter(shellRoutes({ entries }), {
    state: bootState("user"),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-sider");
  expect(screen.queryByRole("link", { name: "More" })).toBeNull();
  asUser.view.unmount();

  await renderRouter(shellRoutes({ entries }), {
    state: bootState("admin"),
    initialEntries: ["/overview"],
  });
  const sider = await screen.findByTestId("app-sider");
  expect(within(sider).getByRole("link", { name: "More" }).getAttribute("href")).toBe("/admin");
});

test("component_role_comes_from_auth_me_never_bootstrap", async () => {
  stubViewportWidth(1280);
  const strayBootstrap = { ...bootstrapFixture() };
  Object.assign(strayBootstrap, { role: "admin", session: { user: { role: "admin" } } });

  await renderRouter(shellRoutes({ entries: [...NAV_ENTRIES, ADMIN_ENTRY] }), {
    state: { bootstrap: strayBootstrap, me: meFixture({ role: "user" }) },
    initialEntries: ["/overview"],
  });

  await screen.findByTestId("app-sider");
  expect(screen.queryByRole("link", { name: "More" })).toBeNull();
});

test("component_footer_version_and_powered_by_toggles", async () => {
  stubViewportWidth(1280);

  const shown = await renderRouter(shellRoutes(), {
    state: bootState("user"),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-shell");
  expect(screen.getByTestId("app-version").textContent).toBe("4.0.0");
  const poweredBy = screen.getByTestId("powered-by");
  expect(poweredBy.textContent).toBe(POWERED_BY_TEXT);
  expect(poweredBy.getAttribute("href")).toBe(PALMR_REPOSITORY_URL);
  shown.view.unmount();

  const withoutVersion = await renderRouter(shellRoutes(), {
    state: bootState("user", bootstrapFixture({ version: null })),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-shell");
  expect(screen.queryByTestId("app-version")).toBeNull();
  expect(screen.getByTestId("powered-by")).toBeDefined();
  withoutVersion.view.unmount();

  const withoutCredit = await renderRouter(shellRoutes(), {
    state: bootState("user", bootstrapFixture({ poweredByVisible: false })),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-shell");
  expect(screen.getByTestId("app-version").textContent).toBe("4.0.0");
  expect(screen.queryByTestId("powered-by")).toBeNull();
  withoutCredit.view.unmount();

  const withoutEither = await renderRouter(shellRoutes(), {
    state: bootState("user", bootstrapFixture({ version: null, poweredByVisible: false })),
    initialEntries: ["/overview"],
  });
  await screen.findByTestId("app-shell");
  expect(screen.queryByTestId("app-footer")).toBeNull();
  withoutEither.view.unmount();
});

test("component_powered_by_credit_text_and_link_are_fixed", async () => {
  stubViewportWidth(1280);
  const strayBootstrap = { ...bootstrapFixture() };
  Object.assign(strayBootstrap, {
    poweredBy: { text: "Sponsored by Someone", href: "https://evil.example/" },
    version: "9.9.9",
  });

  await renderRouter(shellRoutes(), {
    state: bootState("user", strayBootstrap),
    initialEntries: ["/overview"],
  });

  const poweredBy = await screen.findByTestId("powered-by");
  expect(poweredBy.textContent).toBe(POWERED_BY_TEXT);
  expect(poweredBy.getAttribute("href")).toBe(PALMR_REPOSITORY_URL);
  expect(screen.getByTestId("app-version").textContent).toBe("9.9.9");
});

test("component_user_menu_sign_out_delegates_to_the_accepted_logout", async () => {
  stubViewportWidth(1280);
  const calls = { logout: 0 };
  server.use(
    ...bootHandlers({ me: meFixture({ role: "admin" }) }).handlers,
    http.post(LOGOUT_URL, () => {
      calls.logout += 1;
      return new HttpResponse(null, { status: 204 });
    }),
  );
  const routes: RouteObject[] = [...shellRoutes(), { path: "/login", element: null }];
  const { router } = renderSession({ routes, initialEntries: ["/overview"] });
  const user = userEvent.setup();
  await screen.findByTestId("app-sider");

  await user.click(screen.getByRole("button", { name: "Account menu" }));
  const signOut = await screen.findByRole("menuitem", { name: "Sign out" });
  const menu = signOut.closest('[role="menu"]');
  expect(menu?.querySelectorAll('[role="menuitem"]')).toHaveLength(1);
  await user.click(signOut);

  await waitFor(() => {
    expect(router.state.location.pathname).toBe("/login");
  });
  expect(calls.logout).toBe(1);
});
