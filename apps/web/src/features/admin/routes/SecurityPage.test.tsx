import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { defaultSettings, GIB } from "../../../test/adminServer";
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

async function openSecurity(options: Parameters<typeof renderAdmin>[1] = {}) {
  const harness = renderAdmin("/admin/security", options);
  await screen.findByTestId("settings-general");
  await within(screen.getByTestId("settings-security")).findByLabelText(
    "Account password minimum length",
  );
  await within(screen.getByTestId("settings-quotas")).findByLabelText(
    "Default storage quota per user",
    { exact: false },
  );
  return harness;
}

const security = () => screen.getByTestId("settings-security");
const quotas = () => screen.getByTestId("settings-quotas");
const publicLinks = () => screen.getByTestId("settings-public-links");
const general = () => screen.getByTestId("settings-general");

function unlimitedSwitch(root: HTMLElement, index: number): HTMLElement {
  const found = within(root).getAllByRole("switch", { name: "Unlimited" })[index];
  if (found === undefined) {
    throw new Error(`no Unlimited switch at ${String(index)}`);
  }
  return found;
}

function numberField(root: HTMLElement, label: string): HTMLInputElement {
  return within(root).getByLabelText<HTMLInputElement>(label);
}

describe("admin security page", () => {
  test("renders the four implemented groups as separate panels with every accepted security field", async () => {
    await openSecurity();

    expect(screen.getByRole("heading", { level: 2, name: "Security and policy" })).toBeDefined();
    for (const heading of ["Sign-in and sessions", "Quotas", "Public links", "General"]) {
      expect(screen.getByRole("heading", { level: 3, name: heading })).toBeDefined();
    }
    const form = security();
    const expected: [string, string][] = [
      ["Account password minimum length", "8"],
      ["Public link password minimum length", "8"],
      ["Failed sign-ins before lockout", "5"],
      ["Lockout duration", "15"],
      ["Idle session timeout", "7"],
      ["Maximum session lifetime", "30"],
      ["Re-authentication window", "10"],
      ["Password reset link validity", "60"],
      ["Default invite validity", "72"],
      ["Trusted device duration", "30"],
    ];
    for (const [label, value] of expected) {
      expect(numberField(form, label).value).toBe(value);
    }
    expect(
      within(form).getByRole("switch", {
        name: "Require two-factor authentication for local accounts",
      }),
    ).toHaveProperty("ariaChecked", "false");
    expect(within(form).getByRole("switch", { name: "Allow trusted devices" })).toBeDefined();
    expect(within(form).getAllByText("minutes").length).toBeGreaterThan(0);
    expect(within(form).getAllByText("days").length).toBeGreaterThan(0);
    expect(within(form).getByText("hours")).toBeDefined();
    for (const panel of [security(), quotas(), publicLinks(), general()]) {
      expect(within(panel).getByRole("button", { name: "Save changes" })).toHaveProperty(
        "disabled",
        true,
      );
    }
  });

  test("each save operates on exactly one group and sends only the changed keys", async () => {
    const { state, user } = await openSecurity();

    const lockout = numberField(security(), "Lockout duration");
    await user.clear(lockout);
    await user.type(lockout, "30");
    await user.click(
      within(security()).getByRole("switch", {
        name: "Require two-factor authentication for local accounts",
      }),
    );
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(await within(security()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.security).toEqual([
      { loginLockoutMinutes: 30, twoFactorRequired: true },
    ]);
    expect(state.settingsPatches.quotas).toBeUndefined();
    expect(state.settingsPatches.general).toBeUndefined();
    expect(state.settingsPatches["public-links"]).toBeUndefined();
    expect(state.settings.security.loginLockoutMinutes).toBe(30);
    expect(
      within(security()).getByRole("button", { name: "Save changes" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  test("a cleared number is a client error and nothing is sent", async () => {
    const { state, user } = await openSecurity();

    await user.clear(numberField(security(), "Lockout duration"));
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(await within(security()).findByText("This field is required.")).toBeDefined();
    expect(state.settingsPatches.security).toBeUndefined();
  });
});

describe("component_settings_floor_errors_mapped", () => {
  test("SETTING_BELOW_FLOOR renders a field-level message from details.key and details.floor", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-security": () =>
          errorEnvelope("SETTING_BELOW_FLOOR", 422, "req-floor", {
            message: "password_min_length below the floor of 8",
            details: { key: "passwordMinLength", floor: 8 },
          }),
      },
    });

    const field = numberField(security(), "Account password minimum length");
    await user.clear(field);
    await user.type(field, "4");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(await within(security()).findByText("Must be at least 8.")).toBeDefined();
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(field.getAttribute("aria-describedby")).toContain("help");
    expect(
      within(
        numberField(security(), "Failed sign-ins before lockout").closest("form") as HTMLElement,
      ).queryByText("Must be at least 8."),
    ).not.toBeNull();
    expect(screen.queryByText("password_min_length below the floor of 8")).toBeNull();
    expect(
      screen.queryByText("This value is below the minimum allowed for this setting."),
    ).toBeNull();
    expect(within(security()).queryAllByText("Must be at least 8.").length).toBe(1);
  });

  test("a different key and floor land on the matching field", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-security": () =>
          errorEnvelope("SETTING_BELOW_FLOOR", 422, "req-floor-2", {
            details: { key: "maxLoginAttempts", floor: 3 },
          }),
      },
    });

    const attempts = numberField(security(), "Failed sign-ins before lockout");
    await user.clear(attempts);
    await user.type(attempts, "1");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(await within(security()).findByText("Must be at least 3.")).toBeDefined();
    expect(attempts.getAttribute("aria-invalid")).toBe("true");
    expect(
      numberField(security(), "Account password minimum length").getAttribute("aria-invalid"),
    ).toBe("false");
  });

  test("the public link lifetime floor maps onto its field", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-public-links": () =>
          errorEnvelope("SETTING_BELOW_FLOOR", 422, "req-floor-3", {
            details: { key: "maxPublicLinkLifetimeDays", floor: 1 },
          }),
      },
    });

    await user.click(within(publicLinks()).getByRole("switch", { name: "No maximum lifetime" }));
    await user.type(within(publicLinks()).getByLabelText("Maximum link lifetime"), "0");
    await user.click(within(publicLinks()).getByRole("button", { name: "Save changes" }));

    expect(await within(publicLinks()).findByText("Must be at least 1.")).toBeDefined();
  });

  test("SETTING_VALUE_INVALID uses the key and the documented ceiling", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-security": () =>
          errorEnvelope("SETTING_VALUE_INVALID", 422, "req-invalid", {
            details: { key: "sessionIdleDays", max: 30 },
          }),
      },
    });

    const idle = numberField(security(), "Idle session timeout");
    await user.clear(idle);
    await user.type(idle, "99");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(await within(security()).findByText("Must be at most 30.")).toBeDefined();
    expect(idle.getAttribute("aria-invalid")).toBe("true");
  });

  test("SETTING_VALUE_INVALID without details falls back to the shared mapper message", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-security": () => errorEnvelope("SETTING_VALUE_INVALID", 422, "req-invalid-2"),
      },
    });

    const idle = numberField(security(), "Idle session timeout");
    await user.clear(idle);
    await user.type(idle, "3");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    expect(
      await within(security()).findByText(
        "This setting value isn't valid. Review it and try again.",
      ),
    ).toBeDefined();
  });

  test("SETTING_UNKNOWN uses the shared mapper message", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-general": () =>
          errorEnvelope("SETTING_UNKNOWN", 422, "req-unknown", {
            message: "unknown member hideVersion2",
          }),
      },
    });

    const name = within(general()).getByLabelText("Instance name");
    await user.clear(name);
    await user.type(name, "Acme");
    await user.click(within(general()).getByRole("button", { name: "Save changes" }));

    expect(await within(general()).findByText("This setting doesn't exist.")).toBeDefined();
    expect(screen.queryByText("unknown member hideVersion2")).toBeNull();
  });
});

describe("unexpected failures", () => {
  test("a server fault keeps the request id behavior of the shared error view", async () => {
    const { user } = await openSecurity({
      failures: { "patch-general": () => errorEnvelope("INTERNAL_ERROR", 500, "req-internal-77") },
    });

    const name = within(general()).getByLabelText("Instance name");
    await user.clear(name);
    await user.type(name, "Acme");
    await user.click(within(general()).getByRole("button", { name: "Save changes" }));

    const alert = await within(general()).findByRole("alert");
    expect(alert.textContent).toContain("Palmr ran into an unexpected problem");
    expect(within(alert).getByText("Request ID: req-internal-77")).toBeDefined();
    expect(name).toHaveProperty("value", "Acme");
  });
});

describe("quotas and public links", () => {
  test("null means Unlimited and zero is a different value", async () => {
    const { state, user } = await openSecurity();
    const form = quotas();

    await user.click(unlimitedSwitch(form, 0));
    await user.click(within(form).getByRole("button", { name: "Save changes" }));
    expect(await within(form).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.quotas).toEqual([{ defaultUserQuotaBytes: null }]);

    const maxFile = unlimitedSwitch(form, 1);
    expect(maxFile.getAttribute("aria-checked")).toBe("true");
    await user.click(maxFile);
    await user.type(within(form).getByLabelText("Maximum file size"), "0");
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(state.settingsPatches.quotas).toHaveLength(2);
    });
    expect(state.settingsPatches.quotas?.[1]).toEqual({ maxFileSizeBytes: 0 });
    expect(state.settings.quotas).toEqual({ defaultUserQuotaBytes: null, maxFileSizeBytes: 0 });
  });

  test("a changed amount is sent as bytes and an untouched field is absent", async () => {
    const { state, user } = await openSecurity();
    const form = quotas();

    const amount = within(form).getByLabelText("Default storage quota per user", { exact: false });
    expect((amount as HTMLInputElement).value).toBe("10");
    await user.clear(amount);
    await user.type(amount, "25");
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(state.settingsPatches.quotas).toEqual([{ defaultUserQuotaBytes: 25 * GIB }]);
    });
    expect(state.settingsPatches.quotas?.[0]).not.toHaveProperty("maxFileSizeBytes");
  });

  test("quota floors map onto the quota field", async () => {
    const { user } = await openSecurity({
      failures: {
        "patch-quotas": () =>
          errorEnvelope("SETTING_BELOW_FLOOR", 422, "req-quota-floor", {
            details: { key: "defaultUserQuotaBytes", floor: 0 },
          }),
      },
    });
    const form = quotas();

    await user.clear(
      within(form).getByLabelText("Default storage quota per user", { exact: false }),
    );
    await user.type(
      within(form).getByLabelText("Default storage quota per user", { exact: false }),
      "1",
    );
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    expect(await within(form).findByText("Must be at least 0.")).toBeDefined();
  });

  test("the public link maximum distinguishes no maximum from a number of days", async () => {
    const { state, user } = await openSecurity();
    const form = publicLinks();
    expect(
      within(form)
        .getByRole("switch", { name: "No maximum lifetime" })
        .getAttribute("aria-checked"),
    ).toBe("true");

    await user.click(within(form).getByRole("switch", { name: "No maximum lifetime" }));
    await user.type(within(form).getByLabelText("Maximum link lifetime"), "30");
    await user.click(within(form).getByRole("button", { name: "Save changes" }));
    await waitFor(() => {
      expect(state.settingsPatches["public-links"]).toEqual([{ maxPublicLinkLifetimeDays: 30 }]);
    });

    await user.click(within(form).getByRole("switch", { name: "No maximum lifetime" }));
    await user.click(within(form).getByRole("button", { name: "Save changes" }));
    await waitFor(() => {
      expect(state.settingsPatches["public-links"]).toHaveLength(2);
    });
    expect(state.settingsPatches["public-links"]?.[1]).toEqual({ maxPublicLinkLifetimeDays: null });
  });

  test("without any change the public link panel sends nothing", async () => {
    const { state, user } = await openSecurity();

    const save = within(publicLinks()).getByRole("button", { name: "Save changes" });
    expect(save.hasAttribute("disabled")).toBe(true);
    await user.click(save);

    expect(state.settingsPatches["public-links"]).toBeUndefined();
  });
});

describe("general settings", () => {
  test("saves only what changed, with the version toggle mapped onto hideVersion", async () => {
    const { state, user } = await openSecurity();
    const form = general();

    expect(within(form).getByLabelText("Instance name")).toHaveProperty("value", "Palmr");
    expect(within(form).queryByLabelText(/Powered by text/i)).toBeNull();
    await user.click(within(form).getByRole("switch", { name: "Show the Palmr version number" }));
    await user.click(within(form).getByRole("switch", { name: "Show “Powered by Palmr”" }));
    await chooseOption(
      user,
      within(form).getByRole("combobox", { name: "Thumbnail source limit" }),
      "No limit",
    );
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    expect(await within(form).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.general).toEqual([
      { hideVersion: true, poweredByVisible: false, thumbnailSourceLimit: "unlimited" },
    ]);
  });

  test("an emptied description is sent as an empty string, not omitted", async () => {
    const { state, user } = await openSecurity();
    const form = general();

    await user.clear(within(form).getByLabelText("Description"));
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(state.settingsPatches.general).toEqual([{ appDescription: "" }]);
    });
  });

  test("saving the general group refreshes the bootstrap and other groups refresh effective settings", async () => {
    const { queryClient, user } = await openSecurity();
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");

    const name = within(general()).getByLabelText("Instance name");
    await user.clear(name);
    await user.type(name, "Acme Files");
    await user.click(within(general()).getByRole("button", { name: "Save changes" }));
    await within(general()).findByText("Settings saved.");
    const keys = invalidate.mock.calls.map(([filter]) => JSON.stringify(filter?.queryKey));
    expect(keys).toContain(JSON.stringify(["admin", "settings", "general"]));
    expect(keys).toContain(JSON.stringify(["bootstrap"]));
    expect(keys).not.toContain(JSON.stringify(["me", "effective-settings"]));

    invalidate.mockClear();
    const lockout = numberField(security(), "Lockout duration");
    await user.clear(lockout);
    await user.type(lockout, "20");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));
    await within(security()).findByText("Settings saved.");
    const securityKeys = invalidate.mock.calls.map(([filter]) => JSON.stringify(filter?.queryKey));
    expect(securityKeys).toContain(JSON.stringify(["admin", "settings", "security"]));
    expect(securityKeys).toContain(JSON.stringify(["me", "effective-settings"]));
    expect(securityKeys).not.toContain(JSON.stringify(["bootstrap"]));
  });

  test("defaults come from the instance defaults the server reports", async () => {
    const settings = defaultSettings();
    settings.general.defaultLocale = "de-DE";
    await openSecurity({ settings });

    expect(
      within(general()).getByRole("combobox", { name: "Default language" }).closest(".ant-select")
        ?.textContent,
    ).toContain("Deutsch");
  });
});

describe("recent authentication on settings", () => {
  test("a blocked security save keeps the form, replays once after the challenge", async () => {
    const { state, user } = await openSecurity({ recentAuth: true });

    const lockout = numberField(security(), "Lockout duration");
    await user.clear(lockout);
    await user.type(lockout, "45");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));

    const challenge = await findDialog("Confirm it's you");
    expect(numberField(security(), "Lockout duration").value).toBe("45");
    expect(state.settingsPatches.security).toHaveLength(1);
    await user.type(within(challenge).getByLabelText("Password"), "correct horse");
    await user.click(within(challenge).getByRole("button", { name: "Confirm" }));

    expect(await within(security()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.security).toEqual([
      { loginLockoutMinutes: 45 },
      { loginLockoutMinutes: 45 },
    ]);
    expect(state.settings.security.loginLockoutMinutes).toBe(45);
    await expectNoDialog();
  });

  test("cancelling the challenge keeps the typed value and sends nothing more", async () => {
    const { state, user } = await openSecurity({ recentAuth: true });

    const lockout = numberField(security(), "Lockout duration");
    await user.clear(lockout);
    await user.type(lockout, "45");
    await user.click(within(security()).getByRole("button", { name: "Save changes" }));
    const challenge = await findDialog("Confirm it's you");
    await user.click(within(challenge).getByRole("button", { name: "Cancel" }));

    await expectNoDialog();
    expect(numberField(security(), "Lockout duration").value).toBe("45");
    expect(state.settingsPatches.security).toHaveLength(1);
    expect(state.settings.security.loginLockoutMinutes).toBe(15);
    expect(screen.queryByText("Settings saved.")).toBeNull();
  });
});
