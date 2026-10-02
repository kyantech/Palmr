import { cleanup, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import {
  ADMIN_ID,
  defaultSettings,
  GIB,
  GRACE_ID,
  SOLO_ID,
  userFixture,
} from "../../../test/adminServer";
import { errorEnvelope } from "../../../test/bootFixtures";
import { chooseOption, expectNoDialog, findDialog, renderAdmin } from "../../../test/renderAdmin";
import { resetSessionHarness, stubMatchMedia } from "../../../test/renderSession";

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

function section(testId: string): HTMLElement {
  return screen.getByTestId(testId);
}

async function openUser(
  id: string,
  options: Parameters<typeof renderAdmin>[1] = {},
): Promise<ReturnType<typeof renderAdmin>> {
  const harness = renderAdmin(`/admin/users/${id}`, options);
  await screen.findByTestId("user-identity");
  return harness;
}

describe("user detail", () => {
  test("shows identity, account, usage, sessions and counts from the server", async () => {
    await openUser(GRACE_ID, {
      users: [
        userFixture({
          id: GRACE_ID,
          pendingEmail: "grace@new.test",
          mustChangePassword: true,
          twoFactorEnabled: true,
          isLockedOut: true,
          sessionCount: 3,
          trustedDeviceCount: 2,
          lockout: { failedCount: 5, lockCount: 2, lockedUntil: "2026-09-28T12:00:00Z" },
        }),
      ],
    });

    expect(screen.getByRole("heading", { level: 2, name: "Grace Hopper" })).toBeDefined();
    expect(screen.getByText("@grace · grace@example.test")).toBeDefined();
    expect(within(section("user-identity")).getByLabelText("First name")).toHaveProperty(
      "value",
      "Grace",
    );
    expect(within(section("user-identity")).getByLabelText("Username")).toHaveProperty(
      "value",
      "grace",
    );

    const email = section("user-email");
    expect(within(email).getByTestId("canonical-email").textContent).toBe("grace@example.test");
    expect(within(email).getByTestId("pending-email-notice").textContent).toContain(
      "Waiting for verification of grace@new.test",
    );
    expect(within(email).getByTestId("pending-email-notice").textContent).toContain(
      "grace@example.test stays the active address until the new one is verified.",
    );

    const account = section("user-account");
    expect(within(account).getByText("Local password set")).toBeDefined();
    expect(within(account).getByText(/Locked until/)).toBeDefined();
    expect(within(account).getByText("5 failed attempts · 2 locks so far")).toBeDefined();
    expect(account.querySelector('[data-fact="sessions"]')?.textContent).toBe("3");
    expect(account.querySelector('[data-fact="trusted"]')?.textContent).toBe("2");
    expect(account.querySelector('[data-fact="twoFactor"]')?.textContent).toBe("On");
    expect(account.querySelector('[data-fact="mustChange"]')?.textContent).toBe("Yes");

    await waitFor(() => {
      expect(section("user-storage").querySelector('[data-fact="files"]')?.textContent).toBe("12");
    });
    const storage = section("user-storage");
    expect(storage.querySelector('[data-fact="received"]')?.textContent).toBe("3");
    expect(storage.querySelector('[data-fact="shares"]')?.textContent).toBe("2");
    expect(storage.querySelector('[data-fact="reverseShares"]')?.textContent).toBe("1");
    expect(storage.querySelector('[data-fact="used"]')?.textContent).toBe("2 GiB");
    expect(storage.querySelector('[data-fact="effective"]')?.textContent).toBe("10 GiB");
    await waitFor(() => {
      expect(storage.querySelector('[data-fact="default"]')?.textContent).toBe("10 GiB");
    });

    expect(await within(section("user-sessions")).findAllByTestId("user-session-row")).toHaveLength(
      1,
    );
  });

  test("shows linked identities without any provider management", async () => {
    await openUser(SOLO_ID);

    const links = within(section("user-account")).getAllByTestId("identity-link");
    expect(links).toHaveLength(1);
    expect(links[0]?.textContent).toContain("Pocket ID");
    expect(links[0]?.textContent).toContain("Linked manually");
    expect(
      within(section("user-account")).queryByRole("button", { name: /link|provider/i }),
    ).toBeNull();
    expect(
      within(section("user-account")).queryByRole("button", { name: "Reset password" }),
    ).toBeNull();
    expect(within(section("user-account")).getByText(/no local password to reset/)).toBeDefined();
  });

  test("an unknown user shows a not-found state inside the layout", async () => {
    renderAdmin("/admin/users/019a0000-0000-7000-8000-00000000ffff");

    expect(await screen.findByRole("heading", { level: 2, name: "User not found" })).toBeDefined();
    expect(await screen.findByText("This user doesn't exist.")).toBeDefined();
    expect(screen.getByTestId("app-shell")).toBeDefined();
    expect(screen.getByRole("link", { name: "← All users" }).getAttribute("href")).toBe(
      "/admin/users",
    );
  });

  test("the loader primes the target user when navigating from the list", async () => {
    const { state, router, user } = renderAdmin("/admin/users");
    await user.click(await screen.findByRole("link", { name: "Grace Hopper" }));

    await screen.findByTestId("user-identity");
    expect(router.state.location.pathname).toBe(`/admin/users/${GRACE_ID}`);
    expect(state.calls.user).toBe(1);
  });
});

describe("identity edit", () => {
  test("sends only the changed identity fields and never e-mail, role, activation or quota", async () => {
    const { state, user } = await openUser(GRACE_ID);
    const identity = section("user-identity");
    const save = within(identity).getByRole("button", { name: "Save changes" });
    expect(save.hasAttribute("disabled")).toBe(true);

    const first = within(identity).getByLabelText("First name");
    await user.clear(first);
    await user.type(first, "Grace M.");
    await user.click(save);

    expect(await within(identity).findByText("Identity saved.")).toBeDefined();
    expect(state.patchUserBodies).toEqual([{ firstName: "Grace M." }]);
    expect(await screen.findByRole("heading", { level: 2, name: "Grace M. Hopper" })).toBeDefined();
  });

  test("a taken username is reported on the username field", async () => {
    const { user } = await openUser(GRACE_ID, {
      failures: { "patch-user": () => errorEnvelope("USER_USERNAME_TAKEN", 409, "req-username") },
    });
    const identity = section("user-identity");
    const username = within(identity).getByLabelText("Username");
    await user.clear(username);
    await user.type(username, "ada");
    await user.click(within(identity).getByRole("button", { name: "Save changes" }));

    expect(await within(identity).findByText("This username is already in use.")).toBeDefined();
    expect(username.getAttribute("aria-invalid")).toBe("true");
  });
});

describe("role", () => {
  test("changing the role goes through the dedicated endpoint after a confirmation", async () => {
    const { state, user } = await openUser(GRACE_ID);
    const account = section("user-account");

    await chooseOption(user, within(account).getByRole("combobox", { name: "New role" }), "Admin");
    await user.click(within(account).getByRole("button", { name: "Change role" }));
    const dialog = await findDialog("Change the role of Grace Hopper?");
    expect(state.roleBodies).toEqual([]);
    expect(within(dialog).getByText(/signs the user out of every session/)).toBeDefined();
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));

    expect(
      await screen.findByText("The role was changed. The user was signed out everywhere."),
    ).toBeDefined();
    expect(state.roleBodies).toEqual([{ role: "admin" }]);
    expect(state.patchUserBodies).toEqual([]);
    await waitFor(() => {
      expect(within(account).getByTestId("role-tag").textContent).toBe("Admin");
    });
  });

  test("LAST_ADMIN_PROTECTED renders its specific message", async () => {
    const { user } = await openUser(ADMIN_ID, {
      failures: {
        role: () => errorEnvelope("LAST_ADMIN_PROTECTED", 409, "req-last-admin"),
      },
    });
    const account = section("user-account");

    await chooseOption(user, within(account).getByRole("combobox", { name: "New role" }), "User");
    await user.click(within(account).getByRole("button", { name: "Change role" }));
    const dialog = await findDialog("Change the role of Ada Lovelace?");
    await user.click(within(dialog).getByRole("button", { name: "Change role" }));

    expect(
      await within(account).findByText("Palmr must keep at least one active administrator."),
    ).toBeDefined();
    expect(screen.queryByText("Something went wrong")).toBeNull();
    await waitFor(() => {
      expect(within(account).getByTestId("role-tag").textContent).toBe("Admin");
    });
  });
});

describe("activation, unlock and sessions", () => {
  test("deactivation requires a confirmation and the page then offers reactivation", async () => {
    const { state, user } = await openUser(GRACE_ID);
    const danger = section("user-danger");

    await user.click(within(danger).getByRole("button", { name: "Deactivate user" }));
    const dialog = await findDialog("Deactivate Grace Hopper?");
    expect(state.actions).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Deactivate" }));

    expect(
      await within(danger).findByText("The user was deactivated and signed out everywhere."),
    ).toBeDefined();
    expect(state.actions).toEqual(["deactivate"]);
    expect(await screen.findByText(/This account is deactivated/)).toBeDefined();

    await user.click(await within(danger).findByRole("button", { name: "Reactivate user" }));
    expect(await within(danger).findByText("The user was reactivated.")).toBeDefined();
    expect(state.actions).toEqual(["deactivate", "activate"]);
    await waitFor(() => {
      expect(screen.queryByText(/This account is deactivated/)).toBeNull();
    });
  });

  test("deactivating the last active admin shows the protection message and changes nothing", async () => {
    const { state, user } = await openUser(ADMIN_ID, {
      failures: {
        deactivate: () => errorEnvelope("LAST_ADMIN_PROTECTED", 409, "req-last-admin-2"),
      },
    });
    const danger = section("user-danger");

    await user.click(within(danger).getByRole("button", { name: "Deactivate user" }));
    const dialog = await findDialog("Deactivate Ada Lovelace?");
    await user.click(within(dialog).getByRole("button", { name: "Deactivate" }));

    expect(
      await within(danger).findByText("Palmr must keep at least one active administrator."),
    ).toBeDefined();
    expect(state.actions).toEqual([]);
    expect(screen.queryByText(/This account is deactivated/)).toBeNull();
  });

  test("a locked account offers Unlock, which refreshes the detail", async () => {
    const { state, user } = await openUser(GRACE_ID, {
      users: [
        userFixture({
          id: GRACE_ID,
          isLockedOut: true,
          lockout: { failedCount: 5, lockCount: 1, lockedUntil: "2026-09-28T12:00:00Z" },
        }),
      ],
    });
    const account = section("user-account");

    await user.click(within(account).getByRole("button", { name: "Unlock" }));

    expect(await within(account).findByText("The account was unlocked.")).toBeDefined();
    expect(state.actions).toEqual(["unlock"]);
    await waitFor(() => {
      expect(within(account).getByText("Not locked")).toBeDefined();
    });
    expect(within(account).queryByRole("button", { name: "Unlock" })).toBeNull();
  });

  test("an unlocked account has no Unlock action", async () => {
    await openUser(GRACE_ID);

    expect(within(section("user-account")).queryByRole("button", { name: "Unlock" })).toBeNull();
  });

  test("revoking every session needs a confirmation and refreshes the counts", async () => {
    const { state, user } = await openUser(GRACE_ID);
    const danger = section("user-danger");
    await within(section("user-sessions")).findAllByTestId("user-session-row");

    await user.click(within(danger).getByRole("button", { name: "Sign out everywhere" }));
    const dialog = await findDialog("Sign Grace Hopper out everywhere?");
    expect(state.actions).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Sign out everywhere" }));

    expect(
      await within(danger).findByText("Every session of this user was signed out."),
    ).toBeDefined();
    expect(state.actions).toEqual([`revoke-sessions:${GRACE_ID}`]);
    expect(
      await within(section("user-sessions")).findByText("This user has no active sessions."),
    ).toBeDefined();
  });

  test("an admin revoking their own sessions is signed out through the 401 path", async () => {
    const { router, state, user } = await openUser(ADMIN_ID);
    const danger = section("user-danger");

    await user.click(within(danger).getByRole("button", { name: "Sign out everywhere" }));
    const dialog = await findDialog("Sign Ada Lovelace out everywhere?");
    await user.click(within(dialog).getByRole("button", { name: "Sign out everywhere" }));

    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(state.actions).toEqual([`revoke-sessions:${ADMIN_ID}`]);
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
  });
});

describe("password reset", () => {
  test("shows the temporary password once, never persists it, and clears it on close", async () => {
    const { state, queryClient, user } = await openUser(GRACE_ID);
    const account = section("user-account");

    await user.click(within(account).getByRole("button", { name: "Reset password" }));
    const confirm = await findDialog("Reset the password of Grace Hopper?");
    expect(state.actions).toEqual([]);
    await user.click(within(confirm).getByRole("button", { name: "Reset password" }));

    const result = await findDialog("Temporary password");
    const field = within(result).getByLabelText<HTMLInputElement>("Temporary password");
    expect(field.value).toBe("Tmp-Pass-Once-123");
    expect(within(result).getByText(/will not be shown again/)).toBeDefined();
    expect(state.actions).toEqual(["password-reset"]);

    const cached = JSON.stringify([
      ...queryClient
        .getQueryCache()
        .getAll()
        .map((query) => query.state.data),
      ...queryClient
        .getMutationCache()
        .getAll()
        .map((mutation) => mutation.state.data),
    ]);
    expect(cached).not.toContain("Tmp-Pass-Once-123");
    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
    expect(window.location.href).not.toContain("Tmp-Pass");

    await user.click(within(result).getByRole("button", { name: "Done" }));
    await expectNoDialog();
    expect(document.body.textContent).not.toContain("Tmp-Pass-Once-123");
  });

  test("the temporary password is only copied when Copy is pressed", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const { user } = await openUser(GRACE_ID);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const account = section("user-account");
    await user.click(within(account).getByRole("button", { name: "Reset password" }));
    const confirm = await findDialog("Reset the password of Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Reset password" }));
    const result = await findDialog("Temporary password");
    expect(writeText).not.toHaveBeenCalled();

    await user.click(within(result).getByRole("button", { name: "Copy" }));

    expect(writeText).toHaveBeenCalledWith("Tmp-Pass-Once-123");
  });

  test("USER_HAS_NO_LOCAL_AUTH is surfaced when the server refuses an SSO-only reset", async () => {
    const { user } = await openUser(GRACE_ID, {
      users: [userFixture({ id: GRACE_ID, hasLocalPassword: true })],
      failures: {
        "password-reset": () => errorEnvelope("USER_HAS_NO_LOCAL_AUTH", 409, "req-no-local"),
      },
    });
    const account = section("user-account");
    await user.click(within(account).getByRole("button", { name: "Reset password" }));
    const confirm = await findDialog("Reset the password of Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Reset password" }));

    expect(
      await within(account).findByText("This account has no local password to reset."),
    ).toBeDefined();
  });

  test("AUTH_PASSWORD_LOGIN_DISABLED goes through the shared error mapper", async () => {
    const { user } = await openUser(GRACE_ID, {
      failures: {
        "password-reset": () => errorEnvelope("AUTH_PASSWORD_LOGIN_DISABLED", 403, "req-disabled"),
      },
    });
    const account = section("user-account");
    await user.click(within(account).getByRole("button", { name: "Reset password" }));
    const confirm = await findDialog("Reset the password of Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Reset password" }));

    expect(
      await within(account).findByText("Password sign-in is disabled on this instance."),
    ).toBeDefined();
  });
});

describe("quota override", () => {
  const storage = () => section("user-storage");

  async function storageReady() {
    await waitFor(() => {
      expect(storage().querySelector('[data-fact="default"]')?.textContent).toBe("10 GiB");
    });
  }

  test("starts from the effective policy and shows the instance default", async () => {
    await openUser(GRACE_ID);
    await storageReady();

    expect(
      within(storage()).getByRole("radio", { name: "Use the instance default (10 GiB)" }),
    ).toHaveProperty("checked", true);
    expect(within(storage()).getByRole("radio", { name: "Unlimited" })).toHaveProperty(
      "checked",
      false,
    );
    expect(within(storage()).queryByLabelText("Quota")).toBeNull();
  });

  test("an explicit byte cap shows its amount and effective value", async () => {
    await openUser(GRACE_ID, {
      users: [
        userFixture({
          id: GRACE_ID,
          quotaBytes: 5 * GIB,
          quotaOverrideMode: "bytes",
          effectiveQuotaBytes: 5 * GIB,
        }),
      ],
    });
    await storageReady();

    expect(within(storage()).getByRole("radio", { name: "Custom amount" })).toHaveProperty(
      "checked",
      true,
    );
    expect(within(storage()).getByLabelText("Quota")).toHaveProperty("value", "5");
    expect(storage().querySelector('[data-fact="effective"]')?.textContent).toBe("5 GiB");
  });

  test("an unlimited override is recognized when the instance default is finite", async () => {
    await openUser(GRACE_ID, {
      users: [
        userFixture({
          id: GRACE_ID,
          quotaBytes: null,
          quotaOverrideMode: "unlimited",
          effectiveQuotaBytes: null,
        }),
      ],
    });
    await storageReady();

    expect(within(storage()).getByRole("radio", { name: "Unlimited" })).toHaveProperty(
      "checked",
      true,
    );
    expect(storage().querySelector('[data-fact="effective"]')?.textContent).toBe("Unlimited");
  });

  test("inherit and explicit Unlimited stay distinct when the instance default is Unlimited", async () => {
    const settings = defaultSettings();
    settings.quotas.defaultUserQuotaBytes = null;
    const { state, user } = await openUser(GRACE_ID, {
      settings,
      users: [
        userFixture({
          id: GRACE_ID,
          quotaBytes: null,
          quotaOverrideMode: "inherit",
          effectiveQuotaBytes: null,
        }),
      ],
    });
    await waitFor(() => {
      expect(storage().querySelector('[data-fact="default"]')?.textContent).toBe("Unlimited");
    });

    expect(
      within(storage()).getByRole("radio", { name: "Use the instance default (Unlimited)" }),
    ).toHaveProperty("checked", true);
    expect(within(storage()).getByRole("radio", { name: "Unlimited" })).toHaveProperty(
      "checked",
      false,
    );
    expect(storage().querySelector('[data-fact="effective"]')?.textContent).toBe("Unlimited");

    await user.click(within(storage()).getByRole("radio", { name: "Unlimited" }));
    await user.click(within(storage()).getByRole("button", { name: "Save quota" }));
    expect(await within(storage()).findByText("The quota was updated.")).toBeDefined();
    expect(state.quotaBodies).toEqual([{ mode: "unlimited" }]);
    expect(storage().querySelector('[data-fact="effective"]')?.textContent).toBe("Unlimited");
    expect(state.users.find((candidate) => candidate.id === GRACE_ID)?.quotaOverrideMode).toBe(
      "unlimited",
    );

    cleanup();
    renderAdmin(`/admin/users/${GRACE_ID}`, { settings, users: state.users });
    await screen.findByTestId("user-identity");
    await waitFor(() => {
      expect(
        within(screen.getByTestId("user-storage")).getByRole("radio", { name: "Unlimited" }),
      ).toHaveProperty("checked", true);
    });
    expect(
      within(screen.getByTestId("user-storage")).getByRole("radio", {
        name: "Use the instance default (Unlimited)",
      }),
    ).toHaveProperty("checked", false);
  });

  test("the three states are distinct request bodies", async () => {
    const { state, user } = await openUser(GRACE_ID);
    await storageReady();
    const save = () => within(storage()).getByRole("button", { name: "Save quota" });

    await user.click(within(storage()).getByRole("radio", { name: "Unlimited" }));
    await user.click(save());
    expect(await within(storage()).findByText("The quota was updated.")).toBeDefined();
    expect(state.quotaBodies.at(-1)).toEqual({ mode: "unlimited" });

    await user.click(within(storage()).getByRole("radio", { name: "Custom amount" }));
    await user.type(within(storage()).getByLabelText("Quota"), "20");
    await user.click(save());
    await waitFor(() => {
      expect(state.quotaBodies).toHaveLength(2);
    });
    expect(state.quotaBodies.at(-1)).toEqual({ mode: "bytes", quotaBytes: 20 * GIB });

    await user.click(within(storage()).getByRole("radio", { name: /Use the instance default/ }));
    await user.click(save());
    await waitFor(() => {
      expect(state.quotaBodies).toHaveLength(3);
    });
    expect(state.quotaBodies.at(-1)).toEqual({ mode: "inherit" });
    expect(state.quotaBodies.at(-1)).not.toHaveProperty("quotaBytes");
  });

  test("zero bytes is a real cap and never Unlimited", async () => {
    const { state, user } = await openUser(GRACE_ID);
    await storageReady();

    await user.click(within(storage()).getByRole("radio", { name: "Custom amount" }));
    await user.type(within(storage()).getByLabelText("Quota"), "0");
    await user.click(within(storage()).getByRole("button", { name: "Save quota" }));

    await waitFor(() => {
      expect(state.quotaBodies).toHaveLength(1);
    });
    expect(state.quotaBodies[0]).toEqual({ mode: "bytes", quotaBytes: 0 });
    await waitFor(() => {
      expect(storage().querySelector('[data-fact="effective"]')?.textContent).toBe("0 B");
    });
  });

  test("a cap below current usage is applied with a visible warning and is not blocked client-side", async () => {
    const { state, user } = await openUser(GRACE_ID);
    await storageReady();

    await user.click(within(storage()).getByRole("radio", { name: "Custom amount" }));
    await user.type(within(storage()).getByLabelText("Quota"), "1");
    await user.click(within(storage()).getByRole("button", { name: "Save quota" }));

    expect(await within(storage()).findByTestId("below-usage-warning")).toBeDefined();
    expect(
      within(storage()).getByText("This quota is below the storage already in use"),
    ).toBeDefined();
    expect(state.quotaBodies).toEqual([{ mode: "bytes", quotaBytes: GIB }]);
    await waitFor(() => {
      expect(within(storage()).getAllByText("Over quota").length).toBeGreaterThan(0);
    });
    expect(within(storage()).getByText(/no quota bypass/)).toBeDefined();
  });

  test("a custom amount is required before a byte cap can be saved", async () => {
    const { state, user } = await openUser(GRACE_ID);
    await storageReady();

    await user.click(within(storage()).getByRole("radio", { name: "Custom amount" }));
    await user.click(within(storage()).getByRole("button", { name: "Save quota" }));

    expect(
      await within(storage()).findByText(
        "Enter an amount between 0 and the maximum supported size.",
      ),
    ).toBeDefined();
    expect(state.quotaBodies).toEqual([]);
  });

  test("an unreadable instance default does not block the override form", async () => {
    renderAdmin(`/admin/users/${GRACE_ID}`, { settings: defaultSettings() });
    await screen.findByTestId("user-storage");
    expect(await screen.findByRole("radio", { name: "Custom amount" })).toBeDefined();
  });
});

describe("e-mail change", () => {
  test("starting a change leaves the canonical address alone and shows the pending address", async () => {
    const { state, user } = await openUser(GRACE_ID);
    const email = section("user-email");

    await user.type(within(email).getByLabelText("New e-mail address"), "grace@new.test");
    await user.click(within(email).getByRole("button", { name: "Start e-mail change" }));

    expect(
      await within(email).findByText(
        "Verification requested. The address changes only after it is confirmed.",
      ),
    ).toBeDefined();
    expect(state.emailBodies).toEqual([{ email: "grace@new.test" }]);
    await waitFor(() => {
      expect(within(email).getByTestId("pending-email-notice").textContent).toContain(
        "grace@new.test",
      );
    });
    expect(within(email).getByTestId("canonical-email").textContent).toBe("grace@example.test");
  });

  test("a taken address is reported on the field", async () => {
    const { user } = await openUser(GRACE_ID);
    const email = section("user-email");

    await user.type(within(email).getByLabelText("New e-mail address"), "ada@example.test");
    await user.click(within(email).getByRole("button", { name: "Start e-mail change" }));

    expect(await within(email).findByText("This e-mail address is already in use.")).toBeDefined();
  });

  test("a pending change can be resent and cancelled after a confirmation", async () => {
    const { state, user } = await openUser(GRACE_ID, {
      users: [userFixture({ id: GRACE_ID, pendingEmail: "grace@new.test" })],
    });
    const email = section("user-email");

    await user.click(within(email).getByRole("button", { name: "Resend verification" }));
    expect(await within(email).findByText("A new verification message was queued.")).toBeDefined();
    expect(state.actions).toEqual(["email-resend"]);

    await user.click(within(email).getByRole("button", { name: "Cancel change" }));
    const dialog = await findDialog("Cancel the pending change?");
    expect(state.actions).toEqual(["email-resend"]);
    await user.click(within(dialog).getByRole("button", { name: "Cancel change" }));

    expect(
      await within(email).findByText("The pending e-mail change was cancelled."),
    ).toBeDefined();
    expect(state.actions).toEqual(["email-resend", "email-cancel"]);
    await waitFor(() => {
      expect(within(email).queryByTestId("pending-email-notice")).toBeNull();
    });
  });

  test("EMAIL_VERIFICATION_NOT_PENDING and FEATURE_UNAVAILABLE_SMTP use stable messages", async () => {
    const stale = await openUser(GRACE_ID, {
      users: [userFixture({ id: GRACE_ID, pendingEmail: "grace@new.test" })],
      failures: {
        "email-resend": () =>
          errorEnvelope("EMAIL_VERIFICATION_NOT_PENDING", 409, "req-not-pending"),
      },
    });
    const email = section("user-email");
    await stale.user.click(within(email).getByRole("button", { name: "Resend verification" }));
    expect(
      await within(email).findByText("This user has no pending e-mail change to resend."),
    ).toBeDefined();
  });

  test("resending without SMTP shows the stable e-mail-unavailable message", async () => {
    const { user } = await openUser(GRACE_ID, {
      users: [userFixture({ id: GRACE_ID, pendingEmail: "grace@new.test" })],
      failures: {
        "email-resend": () => errorEnvelope("FEATURE_UNAVAILABLE_SMTP", 409, "req-no-smtp"),
      },
    });
    const email = section("user-email");
    await user.click(within(email).getByRole("button", { name: "Resend verification" }));

    expect(
      await within(email).findByText(/E-mail isn't configured on this instance/),
    ).toBeDefined();
  });
});

describe("recent authentication on Admin mutations", () => {
  test("a role change replays once after the global challenge and keeps the form", async () => {
    const { state, user } = await openUser(GRACE_ID, { recentAuth: true });
    const account = section("user-account");

    await chooseOption(user, within(account).getByRole("combobox", { name: "New role" }), "Admin");
    await user.click(within(account).getByRole("button", { name: "Change role" }));
    const confirm = await findDialog("Change the role of Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Change role" }));

    const challenge = await findDialog("Confirm it's you");
    expect(state.roleBodies).toHaveLength(1);
    expect(state.actions).toEqual([]);
    expect(screen.queryByText("Something went wrong")).toBeNull();
    await user.type(within(challenge).getByLabelText("Password"), "correct horse");
    await user.click(within(challenge).getByRole("button", { name: "Confirm" }));

    expect(
      await screen.findByText("The role was changed. The user was signed out everywhere."),
    ).toBeDefined();
    expect(state.reauthBodies).toEqual([{ password: "correct horse" }]);
    expect(state.roleBodies).toEqual([{ role: "admin" }, { role: "admin" }]);
    expect(state.actions).toEqual(["role:admin"]);
    await expectNoDialog();
  });

  test("cancelling the challenge never runs the action and leaves the choice in place", async () => {
    const { state, user } = await openUser(GRACE_ID, { recentAuth: true });
    const danger = section("user-danger");

    await user.click(within(danger).getByRole("button", { name: "Deactivate user" }));
    const confirm = await findDialog("Deactivate Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Deactivate" }));
    const challenge = await findDialog("Confirm it's you");
    await user.click(within(challenge).getByRole("button", { name: "Cancel" }));

    await expectNoDialog();
    expect(state.actions).toEqual([]);
    expect(state.reauthBodies).toEqual([]);
    expect(screen.queryByText(/This account is deactivated/)).toBeNull();
    expect(within(danger).getByRole("button", { name: "Deactivate user" })).toBeDefined();
  });

  test("a password reset replays after re-authentication and then reveals the temporary password", async () => {
    const { state, user } = await openUser(GRACE_ID, { recentAuth: true });
    const account = section("user-account");

    await user.click(within(account).getByRole("button", { name: "Reset password" }));
    const confirm = await findDialog("Reset the password of Grace Hopper?");
    await user.click(within(confirm).getByRole("button", { name: "Reset password" }));
    const challenge = await findDialog("Confirm it's you");
    await user.type(within(challenge).getByLabelText("Password"), "correct horse");
    await user.click(within(challenge).getByRole("button", { name: "Confirm" }));

    const result = await findDialog("Temporary password");
    expect(within(result).getByLabelText("Temporary password")).toHaveProperty(
      "value",
      "Tmp-Pass-Once-123",
    );
    expect(state.actions).toEqual(["password-reset"]);
  });
});
