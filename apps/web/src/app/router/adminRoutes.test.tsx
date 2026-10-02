import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { GRACE_ID } from "../../test/adminServer";
import { meFixture } from "../../test/bootFixtures";
import { renderAdmin } from "../../test/renderAdmin";
import { resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { qk } from "../../shared/api/query-keys";

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

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

const ADMIN_PATHS = [
  "/admin",
  "/admin/users",
  `/admin/users/${GRACE_ID}`,
  "/admin/security",
  "/admin/smtp",
];

describe("RequireAdmin on every Admin route", () => {
  test.each(ADMIN_PATHS)(
    "a non-admin is shown the 403 panel at %s and nothing is fetched",
    async (path) => {
      const { state, router } = renderAdmin(path, { me: meFixture({ role: "user" }) });

      expect(await screen.findByRole("heading", { level: 1, name: "Access denied" })).toBeDefined();
      expect(router.state.location.pathname).toBe(path);
      expect(screen.queryByTestId("admin-page")).toBeNull();
      expect(screen.queryByTestId("app-shell")).toBeNull();
      expect(state.calls.users).toBe(0);
      expect(state.calls.user).toBe(0);
      expect(state.calls.invites).toBe(0);
      expect(state.calls.settings).toEqual({});
    },
  );

  test("an anonymous visitor is sent to sign-in with the Admin path preserved", async () => {
    const { router } = renderAdmin("/admin/security", { me: meFixture() });
    await screen.findByRole("heading", { level: 1, name: "Access denied" });
    expect(router.state.location.pathname).toBe("/admin/security");
  });

  test("an administrator with a forced password change never reaches Admin", async () => {
    const { router } = renderAdmin("/admin/users", {
      me: meFixture({ role: "admin", restriction: "must_change_password" }),
    });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login/forced-password-change");
    });
    expect(screen.queryByTestId("admin-page")).toBeNull();
  });

  test("an administrator that must enroll two-factor never reaches Admin", async () => {
    const { router } = renderAdmin("/admin/smtp", {
      me: meFixture({ role: "admin", restriction: "mfa_enrollment_required" }),
    });

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login/enroll-2fa");
    });
    expect(screen.queryByTestId("admin-page")).toBeNull();
  });
});

describe("AdminLayout", () => {
  test("/admin redirects to /admin/users inside the shell with section navigation", async () => {
    const { router } = renderAdmin("/admin");

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/admin/users");
    });
    expect(await screen.findByTestId("admin-page")).toBeDefined();
    expect(screen.getByTestId("app-shell")).toBeDefined();
    expect(screen.getByRole("heading", { level: 1, name: "Administration" })).toBeDefined();
    const sections = screen.getByRole("navigation", { name: "Administration sections" });
    expect(
      within(sections)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    ).toEqual([
      ["Users", "/admin/users"],
      ["Security", "/admin/security"],
      ["SMTP", "/admin/smtp"],
    ]);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  test("the active section follows the route, including a user detail page", async () => {
    const { router, user } = renderAdmin(`/admin/users/${GRACE_ID}`);
    await screen.findByTestId("user-identity");
    const sections = () => screen.getByRole("navigation", { name: "Administration sections" });
    const active = () =>
      within(sections())
        .getAllByRole("link")
        .filter((link) => link.getAttribute("aria-current") === "page")
        .map((link) => link.textContent);
    expect(active()).toEqual(["Users"]);

    await user.click(within(sections()).getByRole("link", { name: "SMTP" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/admin/smtp");
    });
    expect(await screen.findByTestId("admin-smtp-page")).toBeDefined();
    expect(active()).toEqual(["SMTP"]);

    await user.click(within(sections()).getByRole("link", { name: "Security" }));
    expect(await screen.findByTestId("admin-security-page")).toBeDefined();
    expect(active()).toEqual(["Security"]);
  });

  test("the primary navigation offers Admin to administrators and marks it selected", async () => {
    stubViewportWidth(1280);
    renderAdmin("/admin/users");
    await screen.findByTestId("admin-page");

    const sider = await screen.findByTestId("app-sider");
    const link = within(sider).getByRole("link", { name: "Admin" });
    expect(link.getAttribute("href")).toBe("/admin");
    expect(link.getAttribute("aria-current")).toBe("page");
  });

  test.each([
    "/admin/storage",
    "/admin/branding",
    "/admin/audit",
    "/admin/providers",
    "/admin/invites",
  ])("%s is not registered yet", async (path) => {
    renderAdmin(path);

    expect(await screen.findByRole("heading", { level: 1, name: "Page not found" })).toBeDefined();
  });
});

describe("route loaders", () => {
  async function startAtOverview(options: Parameters<typeof renderAdmin>[1] = {}) {
    const harness = renderAdmin("/overview", options);
    await screen.findByTestId("overview-page");
    return harness;
  }

  test("/admin/security primes the security settings query before the page asks for it", async () => {
    const { router, queryClient, state } = await startAtOverview();
    expect(queryClient.getQueryState(qk.admin.settings("security"))).toBeUndefined();

    await act(async () => {
      await router.navigate("/admin/security");
    });

    expect(queryClient.getQueryState(qk.admin.settings("security"))).toBeDefined();
    await screen.findByTestId("admin-security-page");
    await waitFor(() => {
      expect(state.calls.settings.security).toBe(1);
    });
  });

  test("/admin/smtp primes the smtp settings query", async () => {
    const { router, queryClient, state } = await startAtOverview();

    await act(async () => {
      await router.navigate("/admin/smtp");
    });

    expect(queryClient.getQueryState(qk.admin.settings("smtp"))).toBeDefined();
    await screen.findByTestId("smtp-settings");
    await waitFor(() => {
      expect(state.calls.settings.smtp).toBe(1);
    });
  });

  test("/admin/users primes the users query for the URL params", async () => {
    const { router, queryClient, state } = await startAtOverview();

    await act(async () => {
      await router.navigate("/admin/users?view=users&role=admin&sort=username%3Aasc");
    });

    const key = qk.admin.users({
      q: "",
      role: "admin",
      status: null,
      sort: "username:asc",
      cursor: null,
      limit: 25,
    });
    expect(queryClient.getQueryState(key)).toBeDefined();
    await screen.findByRole("link", { name: "Ada Lovelace" });
    expect(state.userListQueries).toHaveLength(1);
    expect(state.userListQueries[0]).toContain("role=admin");
    expect(state.userListQueries[0]).toContain("sort=username%3Aasc");
  });

  test("the invites view primes the invites query instead", async () => {
    const { router, queryClient, state } = await startAtOverview();

    await act(async () => {
      await router.navigate("/admin/users?view=invites&status=pending");
    });

    expect(
      queryClient.getQueryState(qk.admin.invites({ status: "pending", cursor: null, limit: 25 })),
    ).toBeDefined();
    await screen.findByText("invitee@example.test");
    expect(state.calls.invites).toBe(1);
    expect(state.calls.users).toBe(0);
  });

  test("a non-admin navigation never primes an Admin query", async () => {
    const { router, queryClient, state } = await startAtOverview({
      me: meFixture({ role: "user" }),
    });

    await act(async () => {
      await router.navigate("/admin/security");
    });

    expect(await screen.findByRole("heading", { level: 1, name: "Access denied" })).toBeDefined();
    expect(queryClient.getQueryState(qk.admin.settings("security"))).toBeUndefined();
    expect(state.calls.settings).toEqual({});
  });
});
