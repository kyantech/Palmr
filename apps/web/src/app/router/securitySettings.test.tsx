import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../features/auth";
import { qk } from "../../shared/api/query-keys";
import {
  BACKUP_CODES,
  CURRENT_DEVICE_ID,
  ENROLLMENT_ID,
  installTwoFactorServer,
  OTHER_DEVICE_ID,
  trustedDevice,
  type TwoFactorServerOptions,
  twoFactorStatus,
} from "../../test/authServer";
import { errorEnvelope, meFixture } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerOptions } from "../../test/settingsServer";
import { appRoutes } from "./routes";

vi.mock("antd", async (importOriginal) => {
  const actual = await importOriginal<typeof import("antd")>();
  function QRCode({ value }: { value: string }) {
    return <div data-testid="qr-code" data-value={value} />;
  }
  return { ...actual, QRCode };
});

type User = ReturnType<typeof userEvent.setup>;

const PASSWORD = "correct horse battery";

function renderSecurity(
  settingsOptions: SettingsServerOptions = {},
  twoFactorOptions: TwoFactorServerOptions = {},
) {
  const settings = installSettingsServer(settingsOptions);
  const twoFactor = installTwoFactorServer(settings, twoFactorOptions);
  const harness = renderSession({ routes: appRoutes, initialEntries: ["/settings/security"] });
  return { settings, twoFactor, ...harness, user: userEvent.setup({ delay: null }) };
}

async function recentAuthDialog(): Promise<HTMLElement> {
  const title = await screen.findByText("Confirm it's you");
  const dialog = title.closest<HTMLElement>("[role='dialog']");
  if (dialog === null) {
    throw new Error("recent-auth title is not inside a dialog");
  }
  return dialog;
}

async function confirmPopover(user: User, title: string, action: string) {
  const heading = await screen.findByText(title);
  const popover = heading.closest(".ant-popover");
  if (!(popover instanceof HTMLElement)) {
    throw new Error(`no confirmation popover titled ${title}`);
  }
  await user.click(within(popover).getByRole("button", { name: action }));
}

function nth(items: HTMLElement[], index: number): HTMLElement {
  const item = items[index];
  if (item === undefined) {
    throw new Error(`no item at ${String(index)}`);
  }
  return item;
}

function section(testId: string) {
  return screen.getByTestId(testId);
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_security_two_factor_settings", () => {
  test("the existing password section stays, followed by two-factor, connected-account and trusted-device sections", async () => {
    renderSecurity();

    const headings = (await screen.findAllByRole("heading", { level: 2 })).map(
      (heading) => heading.textContent,
    );
    expect(headings).toEqual([
      "Password",
      "Two-factor authentication",
      "Connected accounts",
      "Trusted devices",
    ]);
    expect(screen.getByLabelText("Current password")).toBeDefined();
  });

  test("voluntary enrollment goes through recent auth, reuses the enrollment component and shows codes once", async () => {
    const { user, settings, twoFactor, queryClient } = renderSecurity();
    const twoFactorSection = await screen.findByTestId("settings-two-factor");
    expect((await within(twoFactorSection).findByTestId("two-factor-state")).textContent).toBe(
      "Off",
    );

    await user.click(
      await within(twoFactorSection).findByRole("button", {
        name: "Turn on two-factor authentication",
      }),
    );
    const dialog = await recentAuthDialog();
    await user.type(within(dialog).getByLabelText("Password"), PASSWORD);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect(await within(section("settings-two-factor")).findByTestId("qr-code")).toBeDefined();
    expect(twoFactor.enrollCalls).toBe(2);
    expect(settings.reauthBodies).toEqual([{ password: PASSWORD }]);
    expect(recentAuthStore.getState().challenge).toBeNull();

    await user.type(screen.getByLabelText("Authentication code"), "492013");
    await user.click(screen.getByRole("button", { name: "Verify and turn on" }));

    expect(await screen.findByText("Save your backup codes")).toBeDefined();
    expect(twoFactor.verifyBodies).toEqual([{ enrollmentId: ENROLLMENT_ID, code: "492013" }]);
    await waitFor(() => {
      expect(queryClient.getQueryData(qk.me.current())).toMatchObject({
        capabilities: { twoFactorEnabled: true },
      });
    });
    await user.click(
      screen.getByRole("checkbox", { name: "I saved these backup codes somewhere safe" }),
    );
    await user.click(screen.getByRole("button", { name: "Done" }));

    expect(await screen.findByText("Two-factor authentication is on.")).toBeDefined();
    expect(within(section("settings-two-factor")).getByTestId("two-factor-state").textContent).toBe(
      "On",
    );
    expect(screen.getByTestId("backup-codes-remaining").textContent).toBe(
      "Backup codes remaining: 10",
    );
    expect(document.body.textContent).not.toContain(BACKUP_CODES[0]);
    expect(screen.getByTestId("app-shell")).toBeDefined();
  });

  test("a policy that requires 2FA removes the disable affordance and explains why", async () => {
    renderSecurity(
      { me: meFixture({ capabilities: { twoFactorEnabled: true } }) },
      {
        status: twoFactorStatus({
          enabled: true,
          enrolledAt: "2026-09-01T10:00:00Z",
          backupCodesRemaining: 8,
          requiredByPolicy: true,
          canDisable: false,
        }),
      },
    );

    const twoFactorSection = await screen.findByTestId("settings-two-factor");
    expect(await within(twoFactorSection).findByTestId("two-factor-policy")).toBeDefined();
    expect(within(twoFactorSection).getByText("Required by this instance")).toBeDefined();
    expect(within(twoFactorSection).queryByRole("button", { name: "Turn off" })).toBeNull();
    expect(
      within(twoFactorSection).getByRole("button", { name: "Generate new backup codes" }),
    ).toBeDefined();
  });

  test("disabling 2FA asks for password + TOTP, then ends every session and returns to sign-in", async () => {
    const { user, settings, twoFactor, router, queryClient } = renderSecurity(
      { me: meFixture({ capabilities: { twoFactorEnabled: true } }) },
      {
        status: twoFactorStatus({ enabled: true, backupCodesRemaining: 10, canDisable: true }),
      },
    );

    await user.click(await screen.findByRole("button", { name: "Turn off" }));
    await confirmPopover(user, "Turn off two-factor authentication?", "Turn off");
    const dialog = await recentAuthDialog();
    expect(within(dialog).queryByLabelText("Password")).not.toBeNull();
    await user.type(within(dialog).getByLabelText("Password"), PASSWORD);
    await user.type(within(dialog).getByLabelText("Authentication code"), "123456");
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(screen.getByText("Two-factor authentication is off")).toBeDefined();
    expect(router.state.location.pathname).toBe("/login");
    expect(settings.reauthBodies).toEqual([{ password: PASSWORD, totpCode: "123456" }]);
    expect(twoFactor.disableCalls).toBe(2);
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(queryClient.getQueryData(qk.me.twoFactor())).toBeUndefined();
    expect(screen.queryByTestId("app-shell")).toBeNull();
  });

  test("TOTP_REQUIRED_BY_POLICY from the server is shown by code", async () => {
    const { user } = renderSecurity(
      { recentAuth: true },
      {
        status: twoFactorStatus({ enabled: true, backupCodesRemaining: 10, canDisable: true }),
      },
    );
    server.use(
      http.post("*/api/v1/auth/2fa/disable", () =>
        errorEnvelope("TOTP_REQUIRED_BY_POLICY", 403, "req-policy", { message: "policy prose" }),
      ),
    );

    await user.click(await screen.findByRole("button", { name: "Turn off" }));
    await confirmPopover(user, "Turn off two-factor authentication?", "Turn off");

    expect(
      await screen.findByText(
        "This instance requires two-factor authentication, so it can't be turned off.",
      ),
    ).toBeDefined();
    expect(document.body.textContent).not.toContain("policy prose");
    expect(screen.getByTestId("app-shell")).toBeDefined();
  });

  test("regenerating backup codes shows the new codes once and keeps the session", async () => {
    const { user, twoFactor, router, queryClient } = renderSecurity(
      { recentAuth: true, me: meFixture({ capabilities: { twoFactorEnabled: true } }) },
      { status: twoFactorStatus({ enabled: true, backupCodesRemaining: 2, canDisable: true }) },
    );

    expect(await screen.findByText("Backup codes remaining: 2")).toBeDefined();
    expect(
      screen.getByText("You're running low on backup codes. Generate new ones to stay prepared."),
    ).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Generate new backup codes" }));
    await confirmPopover(user, "Generate new backup codes?", "Generate");

    const codes = await screen.findAllByTestId("backup-code");
    expect(codes).toHaveLength(10);
    expect(twoFactor.regenerateCalls).toBe(1);
    const shown = codes.map((code) => code.textContent);
    const cached = JSON.stringify(
      queryClient
        .getQueryCache()
        .getAll()
        .map((query) => query.state.data),
    );
    for (const code of shown) {
      expect(cached).not.toContain(code);
    }

    await user.click(
      screen.getByRole("checkbox", { name: "I saved these backup codes somewhere safe" }),
    );
    await user.click(screen.getByRole("button", { name: "Done" }));

    expect(
      await screen.findByText(
        "Your new backup codes are active. The previous codes no longer work.",
      ),
    ).toBeDefined();
    expect(await screen.findByText("Backup codes remaining: 10")).toBeDefined();
    expect(document.body.textContent).not.toContain(shown[0]);
    expect(router.state.location.pathname).toBe("/settings/security");
    expect(screen.getByTestId("app-shell")).toBeDefined();
  });
});

describe("component_security_trusted_devices", () => {
  const devices = () => [
    trustedDevice({ id: OTHER_DEVICE_ID }),
    trustedDevice({
      id: CURRENT_DEVICE_ID,
      isCurrent: true,
      label: "Chrome on macOS",
      ipAtEnrollment: null,
    }),
  ];

  function rows() {
    return within(section("settings-trusted-devices")).getAllByTestId("trusted-device-row");
  }

  test("lists devices with the current one first, display-only metadata and the policy", async () => {
    renderSecurity({}, { devices: devices() });

    const list = await screen.findByRole("list", { name: "Trusted devices" });
    const items = within(list).getAllByTestId("trusted-device-row");
    expect(items.map((item) => item.dataset.deviceId)).toEqual([
      CURRENT_DEVICE_ID,
      OTHER_DEVICE_ID,
    ]);
    expect(within(nth(items, 0)).getByText("This browser")).toBeDefined();
    expect(within(nth(items, 0)).queryByText(/IP address/)).toBeNull();
    expect(within(nth(items, 1)).getByText("IP address when trusted: 198.51.100.7")).toBeDefined();
    expect(screen.getByTestId("trusted-devices-policy").textContent).toBe(
      "A trusted device skips the two-factor step for 30 days after you choose to remember it.",
    );
  });

  test("revoking a device goes through recent auth, refetches the list and never signs out", async () => {
    const { user, settings, twoFactor, router } = renderSecurity({}, { devices: devices() });
    await screen.findByRole("list", { name: "Trusted devices" });
    const before = twoFactor.calls.devices;

    await user.click(
      within(nth(rows(), 1)).getByRole("button", {
        name: "Remove trusted device: Firefox on Windows",
      }),
    );
    await confirmPopover(user, "Remove this trusted device?", "Remove");
    const dialog = await recentAuthDialog();
    await user.type(within(dialog).getByLabelText("Password"), PASSWORD);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    expect(await screen.findByText("The trusted device was removed.")).toBeDefined();
    await waitFor(() => {
      expect(rows().map((row) => row.dataset.deviceId)).toEqual([CURRENT_DEVICE_ID]);
    });
    expect(twoFactor.revokedDevices).toEqual([OTHER_DEVICE_ID]);
    expect(twoFactor.calls.devices).toBeGreaterThan(before);
    expect(settings.revoked).toEqual([]);
    expect(router.state.location.pathname).toBe("/settings/security");
  });

  test("revoking the current browser's trust keeps the current session", async () => {
    const { user, twoFactor, router } = renderSecurity(
      { recentAuth: true },
      { devices: devices() },
    );
    await screen.findByRole("list", { name: "Trusted devices" });

    await user.click(
      within(nth(rows(), 0)).getByRole("button", {
        name: "Remove trusted device: Chrome on macOS",
      }),
    );
    await confirmPopover(user, "Stop trusting this browser?", "Remove");

    await waitFor(() => {
      expect(rows().map((row) => row.dataset.deviceId)).toEqual([OTHER_DEVICE_ID]);
    });
    expect(twoFactor.revokedDevices).toEqual([CURRENT_DEVICE_ID]);
    expect(router.state.location.pathname).toBe("/settings/security");
    expect(screen.getByTestId("app-shell")).toBeDefined();
  });

  test("remove all empties the list into a deliberate empty state", async () => {
    const { user, twoFactor } = renderSecurity({ recentAuth: true }, { devices: devices() });
    await screen.findByRole("list", { name: "Trusted devices" });

    await user.click(screen.getByRole("button", { name: "Remove all" }));
    await confirmPopover(user, "Remove all trusted devices?", "Remove all");

    expect(await screen.findByTestId("trusted-devices-empty")).toBeDefined();
    expect(screen.getByText("All trusted devices were removed.")).toBeDefined();
    expect(twoFactor.revokeAllCalls).toBe(1);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Remove all" }).disabled).toBe(
      true,
    );
  });

  test("a policy that disables trusted devices is explained", async () => {
    renderSecurity({}, { policy: { enabled: false, durationDays: 30 } });

    expect(
      await screen.findByText(
        "Remembering devices is turned off on this instance, so trusted devices don't skip two-factor verification.",
      ),
    ).toBeDefined();
    expect(screen.getByTestId("trusted-devices-empty")).toBeDefined();
  });
});
