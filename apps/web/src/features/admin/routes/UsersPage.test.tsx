import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { GIB, GRACE_ID, SOLO_ID, userFixture } from "../../../test/adminServer";
import {
  chooseOption,
  expectNoDialog,
  findDialog,
  findRow,
  renderAdmin,
} from "../../../test/renderAdmin";
import { resetSessionHarness, stubMatchMedia } from "../../../test/renderSession";

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_admin_users_table_usage", () => {
  test("shows identity, role, status, usage, effective quota and the over-quota state per user", async () => {
    renderAdmin("/admin/users");

    const grace = await findRow("Grace Hopper");
    expect(within(grace).getByText("@grace")).toBeDefined();
    expect(within(grace).getByText("grace@example.test")).toBeDefined();
    expect(within(grace).getByTestId("role-tag").textContent).toBe("User");
    expect(within(grace).getByTestId("active-tag").textContent).toBe("Active");
    expect(within(grace).getByText("2 GiB of 10 GiB")).toBeDefined();
    expect(within(grace).queryByText("Over quota")).toBeNull();
    expect(within(grace).getByTestId("storage-usage").getAttribute("data-over-quota")).toBe(
      "false",
    );

    const linus = await findRow("Linus Torvalds");
    expect(within(linus).getByText("12 GiB of 10 GiB")).toBeDefined();
    expect(within(linus).getByText("Over quota")).toBeDefined();
    expect(within(linus).getByTestId("storage-usage").getAttribute("data-over-quota")).toBe("true");
    expect(within(linus).getByTestId("active-tag").textContent).toBe("Inactive");

    const ada = await findRow("Ada Lovelace");
    expect(within(ada).getByTestId("role-tag").textContent).toBe("Admin");
    expect(within(ada).getByText("1 MiB · Unlimited")).toBeDefined();

    const sol = await findRow("Sol Single");
    expect(within(sol).getByText("0 B · Unlimited")).toBeDefined();
    expect(within(sol).getByText("SSO only")).toBeDefined();
    expect(screen.queryByText(/^0$/)).toBeNull();
    expect(screen.queryByText(/-1/)).toBeNull();
  });

  test("an unlimited quota is a label, never a number", async () => {
    renderAdmin("/admin/users", {
      users: [
        userFixture({
          id: GRACE_ID,
          usedBytes: 0,
          quotaBytes: null,
          effectiveQuotaBytes: null,
        }),
      ],
    });

    const row = await findRow("Grace Hopper");
    expect(within(row).getByText("0 B · Unlimited")).toBeDefined();
  });

  test("account details surface pending e-mail, locked, forced-change and 2FA states", async () => {
    renderAdmin("/admin/users", {
      users: [
        userFixture({
          id: GRACE_ID,
          pendingEmail: "grace@new.test",
          isLockedOut: true,
          mustChangePassword: true,
          twoFactorEnabled: true,
        }),
      ],
    });

    const row = await findRow("Grace Hopper");
    expect(within(row).getByTestId("pending-email").textContent).toBe(
      "Pending change to grace@new.test",
    );
    for (const label of ["Locked", "Must change password", "2FA on"]) {
      expect(within(row).getByText(label)).toBeDefined();
    }
  });

  test("a user row links to the detail route", async () => {
    renderAdmin("/admin/users");

    const grace = await findRow("Grace Hopper");
    const link = within(grace).getByTestId("user-link");
    expect(link.getAttribute("href")).toBe(`/admin/users/${GRACE_ID}`);
  });
});

function manyUsers(count: number) {
  return Array.from({ length: count }, (_value, index) =>
    userFixture({
      id: `019a0000-0000-7000-8000-00000000${String(1000 + index)}`,
      firstName: `User${String(index).padStart(2, "0")}`,
      lastName: "Person",
      username: `user${String(index).padStart(2, "0")}`,
      email: `user${String(index).padStart(2, "0")}@example.test`,
      createdAt: `2026-08-01T00:${String(index).padStart(2, "0")}:00Z`,
      usedBytes: index * GIB,
    }),
  );
}

describe("users list URL state", () => {
  test("search, role, status and sort live in the URL and reset the cursor", async () => {
    const { router, state, user } = renderAdmin("/admin/users", { users: manyUsers(30) });
    await findRow("User29 Person");

    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => {
      expect(router.state.location.search).toMatch(/cursor=/);
    });
    expect(await findRow("User00 Person")).toBeDefined();

    await chooseOption(user, screen.getByRole("combobox", { name: "Role" }), "User");
    await waitFor(() => {
      expect(router.state.location.search).toContain("role=user");
    });
    expect(router.state.location.search).not.toContain("cursor=");
    expect(state.userListQueries.at(-1)).not.toContain("cursor=");

    await chooseOption(user, screen.getByRole("combobox", { name: "Status" }), "Inactive");
    await waitFor(() => {
      expect(router.state.location.search).toContain("status=inactive");
    });

    await chooseOption(
      user,
      screen.getByRole("combobox", { name: "Sort users" }),
      "Most storage used",
    );
    await waitFor(() => {
      expect(router.state.location.search).toContain("sort=usedBytes%3Adesc");
    });
    expect(state.userListQueries.at(-1)).toContain("sort=usedBytes%3Adesc");
    expect(router.state.location.search).toBe(
      "?view=users&role=user&status=inactive&sort=usedBytes%3Adesc",
    );
  });

  test("the debounced search is reflected in the URL and sent to the server", async () => {
    const { router, state, user } = renderAdmin("/admin/users");
    await findRow("Grace Hopper");

    await user.type(screen.getByRole("searchbox", { name: "Search users" }), "grace");

    await waitFor(() => {
      expect(router.state.location.search).toBe("?view=users&q=grace");
    });
    await waitFor(() => {
      expect(state.userListQueries.at(-1)).toContain("q=grace");
    });
    expect(await findRow("Grace Hopper")).toBeDefined();
    await waitFor(() => {
      expect(screen.queryByRole("link", { name: "Linus Torvalds" })).toBeNull();
    });
  });

  test("opaque cursors page forward and back without page numbers", async () => {
    const { router, state, user } = renderAdmin("/admin/users", { users: manyUsers(30) });
    await findRow("User29 Person");
    expect(screen.getByTestId("pager-summary").textContent).toBe("Showing 25 of 30");
    expect(screen.queryByRole("button", { name: "Previous" })?.hasAttribute("disabled")).toBe(true);

    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(await findRow("User00 Person")).toBeDefined();
    expect(screen.getByTestId("pager-summary").textContent).toBe("Showing 5 of 30");
    const cursor = new URLSearchParams(router.state.location.search).get("cursor");
    expect(cursor).not.toBeNull();
    expect(state.userListQueries.at(-1)).toContain(`cursor=${encodeURIComponent(cursor ?? "")}`);
    expect(screen.getByRole("button", { name: "Next" }).hasAttribute("disabled")).toBe(true);

    await user.click(screen.getByRole("button", { name: "Previous" }));
    expect(await findRow("User29 Person")).toBeDefined();
    expect(new URLSearchParams(router.state.location.search).get("cursor")).toBeNull();
  });

  test("a stale cursor in the URL shows the listing-changed message and a way back", async () => {
    const { router, user } = renderAdmin("/admin/users?view=users&cursor=not-a-cursor");

    expect(await screen.findByText(/This list changed while you were browsing/)).toBeDefined();
    await user.click(screen.getByRole("button", { name: "First page" }));
    await waitFor(() => {
      expect(router.state.location.search).toBe("?view=users");
    });
    expect(await screen.findByText("Grace Hopper")).toBeDefined();
  });

  test("clearing filters from the empty state restores the list", async () => {
    const { router, user } = renderAdmin("/admin/users?view=users&q=nobody");

    expect(await screen.findByText("No users match these filters.")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => {
      expect(router.state.location.search).toBe("?view=users");
    });
    expect(await findRow("Grace Hopper")).toBeDefined();
  });
});

describe("create user", () => {
  async function openCreate(user: ReturnType<typeof renderAdmin>["user"]) {
    await user.click(await screen.findByRole("button", { name: "Create user" }));
    return findDialog("Create user");
  }

  async function fillIdentity(
    dialog: HTMLElement,
    user: ReturnType<typeof renderAdmin>["user"],
    overrides: Partial<Record<"first" | "last" | "username" | "email", string>> = {},
  ) {
    await user.type(within(dialog).getByLabelText("First name"), overrides.first ?? "Barbara");
    await user.type(within(dialog).getByLabelText("Last name"), overrides.last ?? "Liskov");
    await user.type(within(dialog).getByLabelText("Username"), overrides.username ?? "barbara");
    await user.type(
      within(dialog).getByLabelText("E-mail address"),
      overrides.email ?? "barbara@example.test",
    );
  }

  test("creates a local user with a temporary password that must be changed, and lists them", async () => {
    const { state, user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await fillIdentity(dialog, user);
    await user.type(within(dialog).getByLabelText("Temporary password"), "temporary-pass-1");
    expect(
      within(dialog)
        .getByRole("switch", { name: /Require a new password/ })
        .getAttribute("aria-checked"),
    ).toBe("true");
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    await waitFor(() => {
      expect(state.createUserBodies).toHaveLength(1);
    });
    expect(state.createUserBodies[0]).toEqual({
      firstName: "Barbara",
      lastName: "Liskov",
      username: "barbara",
      email: "barbara@example.test",
      role: "user",
      isActive: true,
      password: "temporary-pass-1",
      requirePasswordChange: true,
    });
    expect(state.createUserHeaders[0]?.get("Idempotency-Key")).toMatch(/^[0-9a-f]{32}$/);
    expect(await screen.findByText("Barbara Liskov was created.")).toBeDefined();
    expect(await findRow("Barbara Liskov")).toBeDefined();
    expect(screen.getByRole("link", { name: "View user" }).getAttribute("href")).toMatch(
      /^\/admin\/users\//,
    );
    await expectNoDialog();
  });

  test("omitting the password creates an SSO-only account and never sends requirePasswordChange", async () => {
    const { state, user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await fillIdentity(dialog, user);
    expect(
      within(dialog)
        .getByRole("switch", { name: /Require a new password/ })
        .hasAttribute("disabled"),
    ).toBe(true);
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    await waitFor(() => {
      expect(state.createUserBodies).toHaveLength(1);
    });
    expect(state.createUserBodies[0]).not.toHaveProperty("password");
    expect(state.createUserBodies[0]).not.toHaveProperty("requirePasswordChange");
  });

  test("a custom storage quota is sent as a byte count", async () => {
    const { state, user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await fillIdentity(dialog, user);
    await user.click(within(dialog).getByRole("radio", { name: "Custom amount" }));
    await user.type(within(dialog).getByLabelText("Quota"), "5");
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    await waitFor(() => {
      expect(state.createUserBodies).toHaveLength(1);
    });
    expect(state.createUserBodies[0]).toMatchObject({ quotaBytes: 5 * GIB });
  });

  test("duplicate e-mail and username are mapped onto their own fields", async () => {
    const { user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await fillIdentity(dialog, user, { email: "GRACE@example.test", username: "newname" });
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    expect(await within(dialog).findByText("This e-mail address is already in use.")).toBeDefined();
    expect(within(dialog).getByLabelText("E-mail address").getAttribute("aria-invalid")).toBe(
      "true",
    );

    const username = within(dialog).getByLabelText("Username");
    await user.clear(username);
    await user.type(username, "Grace");
    const email = within(dialog).getByLabelText("E-mail address");
    await user.clear(email);
    await user.type(email, "fresh@example.test");
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    expect(await within(dialog).findByText("This username is already in use.")).toBeDefined();
  });

  test("a password-policy rejection lands on the password field with the server's minimum", async () => {
    const { user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await fillIdentity(dialog, user);
    await user.type(within(dialog).getByLabelText("Temporary password"), "short");
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    expect(await within(dialog).findByText("Use at least 8 characters.")).toBeDefined();
  });

  test("client validation keeps an empty form from being submitted", async () => {
    const { state, user } = renderAdmin("/admin/users");
    const dialog = await openCreate(user);
    await user.click(within(dialog).getByRole("button", { name: "Create user" }));

    expect((await within(dialog).findAllByText("This field is required.")).length).toBe(4);
    expect(state.createUserBodies).toHaveLength(0);
  });
});

describe("row actions", () => {
  test("deactivation needs an explicit confirmation and reactivation does not", async () => {
    const { state, user } = renderAdmin("/admin/users");
    await findRow("Grace Hopper");

    await user.click(screen.getByRole("button", { name: "Actions for Grace Hopper" }));
    await user.click(await screen.findByText("Deactivate"));
    const dialog = await findDialog("Deactivate Grace Hopper?");
    expect(state.actions).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Deactivate" }));

    expect(await screen.findByText("The user was deactivated.")).toBeDefined();
    expect(state.actions).toEqual(["deactivate"]);
    await waitFor(() => {
      expect(
        within(screen.getByText("Grace Hopper").closest("tr") as HTMLElement).getByTestId(
          "active-tag",
        ).textContent,
      ).toBe("Inactive");
    });

    await user.click(screen.getByRole("button", { name: "Actions for Grace Hopper" }));
    await user.click(await screen.findByText("Reactivate"));
    expect(await screen.findByText("The user was reactivated.")).toBeDefined();
    expect(state.actions).toEqual(["deactivate", "activate"]);
  });

  test("locked accounts expose Unlock, which clears the lock", async () => {
    const { state, user } = renderAdmin("/admin/users", {
      users: [
        userFixture({ id: SOLO_ID, firstName: "Sol", lastName: "Single", isLockedOut: true }),
      ],
    });
    await findRow("Sol Single");

    await user.click(screen.getByRole("button", { name: "Actions for Sol Single" }));
    await user.click(await screen.findByText("Unlock"));

    expect(await screen.findByText("The account was unlocked.")).toBeDefined();
    expect(state.actions).toEqual(["unlock"]);
    await waitFor(() => {
      expect(screen.queryByText("Locked")).toBeNull();
    });
  });
});
