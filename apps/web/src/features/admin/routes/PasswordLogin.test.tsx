import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../../test/authProbe";
import { installAdminServer } from "../../../test/adminServer";
import {
  BOOTSTRAP_URL,
  bootstrapFixture,
  errorEnvelope,
  ME_URL,
  meFixture,
} from "../../../test/bootFixtures";
import { dispatchWindowMessage, stubWindowOpen } from "../../../test/fakePopup";
import {
  installProviderServer,
  passwordLoginFixture,
  type ProviderServerOptions,
} from "../../../test/providerServer";
import { renderApp, renderProviders } from "../../../test/renderAdmin";
import { resetSessionHarness, stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { must } from "../../../test/must";

const PASSWORD = "correct horse battery";

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

function render(providers: ProviderServerOptions = {}, recentAuth = false) {
  return renderProviders("/admin/providers", { admin: { recentAuth }, providers });
}

async function panel() {
  return screen.findByTestId("password-login-panel");
}

async function disableDialog(user: ReturnType<typeof userEvent.setup>) {
  const scope = within(await panel());
  await user.click(await scope.findByRole("button", { name: "Turn off password sign-in…" }));
  return screen.findByTestId("password-login-disable-dialog");
}

function countBootstrap() {
  const calls = { bootstrap: 0 };
  server.use(
    http.get(BOOTSTRAP_URL, () => {
      calls.bootstrap += 1;
      return HttpResponse.json(bootstrapFixture());
    }),
  );
  return calls;
}

describe("component_password_login_preflight", () => {
  test("the state, safe paths and blockers come from the server, with fixed localized text for blocker codes", async () => {
    render({
      passwordLogin: passwordLoginFixture({
        canDisable: false,
        blockers: [
          {
            code: "NO_VALIDATED_PROVIDER",
            detail: "No enabled provider has been successfully tested",
          },
          { code: "PASSWORD_LOGIN_DISABLE_UNSAFE", detail: "The calling Admin is not linked" },
        ],
        safeAdminLoginPaths: [],
      }),
    });

    const scope = within(await panel());
    expect((await scope.findByTestId("password-login-state")).getAttribute("data-enabled")).toBe(
      "true",
    );
    const blockers = scope.getByTestId("password-login-blockers");
    expect(
      within(blockers)
        .getAllByRole("listitem")
        .map((item) => item.getAttribute("data-blocker")),
    ).toEqual(["NO_VALIDATED_PROVIDER", "PASSWORD_LOGIN_DISABLE_UNSAFE"]);
    expect(
      within(blockers).getByText(/No enabled sign-in provider has been tested successfully yet/),
    ).toBeDefined();
    expect(within(blockers).getByText(/no safe administrator sign-in path/)).toBeDefined();
    expect(blockers.textContent).not.toContain("successfully tested");
    expect(scope.getByTestId("password-login-paths-empty")).toBeDefined();
    expect(
      scope.getByRole("button", { name: "Turn off password sign-in…" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  test("canDisable is taken from the server and never recomputed from the listed paths", async () => {
    render({
      passwordLogin: passwordLoginFixture({
        canDisable: true,
        blockers: [],
        safeAdminLoginPaths: [
          { userId: "u1", username: "ada", providerSlug: "google", providerValidated: false },
        ],
      }),
    });

    const scope = within(await panel());
    const button = await scope.findByRole("button", { name: "Turn off password sign-in…" });
    expect(button.hasAttribute("disabled")).toBe(false);
  });

  test("a structural admin path is not labelled validated unless the provider was tested recently", async () => {
    render({
      passwordLogin: passwordLoginFixture({
        safeAdminLoginPaths: [
          { userId: "u1", username: "ada", providerSlug: "google", providerValidated: true },
          { userId: "u2", username: "grace", providerSlug: "authentik", providerValidated: false },
        ],
      }),
    });

    const paths = await within(await panel()).findAllByTestId("password-login-path");
    expect(paths.map((path) => path.getAttribute("data-validated"))).toEqual(["true", "false"]);
    expect(within(must(paths[0])).getByText("Tested in the last 24 hours")).toBeDefined();
    expect(within(must(paths[1])).getByText("Not tested recently")).toBeDefined();
    expect(within(must(paths[1])).queryByText("Tested in the last 24 hours")).toBeNull();
    expect(
      within(await panel()).getByText(
        /also requires a provider tested successfully in the last 24 hours/,
      ),
    ).toBeDefined();
    expect(
      within(await panel()).getByText(
        /changes that would remove the last of these paths are refused/,
      ),
    ).toBeDefined();
  });
});

describe("component_password_login_disable", () => {
  test("the confirmation explains admission, the standing rule and CLI recovery, and Cancel sends nothing", async () => {
    const { user, providers } = render();

    const dialog = await disableDialog(user);
    expect(within(dialog).getByText(/unavailable for everyone, including you/)).toBeDefined();
    expect(
      within(dialog).getByText(
        /tested successfully in the last 24 hours and is linked to an active administrator/,
      ),
    ).toBeDefined();
    expect(
      within(dialog).getByText(/changes that would remove the last administrator sign-in path/),
    ).toBeDefined();
    expect(
      within(dialog).getByText(/operator can recover access from the command line/),
    ).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByTestId("password-login-disable-dialog")).toBeNull();
    });
    expect(providers.passwordLoginBodies).toEqual([]);
  });

  test("the confirm button stays disabled until the explicit acknowledgement", async () => {
    const { user, providers } = render();

    const dialog = await disableDialog(user);
    const confirm = screen.getByRole("button", { name: "Turn off password sign-in" });
    expect(confirm.hasAttribute("disabled")).toBe(true);
    await user.click(confirm);
    expect(providers.passwordLoginBodies).toEqual([]);

    await user.click(
      within(dialog).getByLabelText("I understand that password sign-in will stop working."),
    );
    expect(confirm.hasAttribute("disabled")).toBe(false);
  });

  test("disabling sends the exact confirm contract, goes through local recent auth once, then refreshes preflight and bootstrap", async () => {
    const { user, providers, state } = render({}, true);
    await panel();
    const bootstrap = countBootstrap();
    const before = { ...providers.calls };
    const bootstrapBefore = bootstrap.bootstrap;

    const dialog = await disableDialog(user);
    await user.click(
      within(dialog).getByLabelText("I understand that password sign-in will stop working."),
    );
    await user.click(screen.getByRole("button", { name: "Turn off password sign-in" }));
    const title = await screen.findByText("Confirm it's you");
    const recent = title.closest<HTMLElement>("[role='dialog']");
    expect(recent).not.toBeNull();
    await user.type(within(must(recent)).getByLabelText("Password"), PASSWORD);
    await user.click(within(must(recent)).getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(providers.passwordLoginBodies).toHaveLength(2);
    });
    expect(providers.passwordLoginBodies).toEqual([
      { enabled: false, confirm: true },
      { enabled: false, confirm: true },
    ]);
    expect(state.reauthBodies).toEqual([{ password: PASSWORD }]);
    await waitFor(() => {
      expect(screen.queryByTestId("password-login-disable-dialog")).toBeNull();
    });
    const scope = within(await panel());
    await waitFor(() => {
      expect(scope.getByTestId("password-login-state").getAttribute("data-enabled")).toBe("false");
    });
    expect(scope.getByText("Password sign-in is off.")).toBeDefined();
    await waitFor(() => {
      expect(providers.calls.passwordLogin).toBeGreaterThan(before.passwordLogin);
      expect(bootstrap.bootstrap).toBeGreaterThan(bootstrapBefore);
    });
    expect(recentAuthStore.getState().challenge).toBeNull();
  });

  test("a server refusal is shown inside the dialog by code with its blockers, and nothing changes", async () => {
    const { user } = render({
      failures: {
        "password-login": () =>
          errorEnvelope("PASSWORD_LOGIN_DISABLE_UNSAFE", 409, "req-unsafe", {
            details: {
              blockers: [{ code: "NO_VALIDATED_PROVIDER", detail: "fixed text" }],
            } as never,
          }),
      },
    });

    const dialog = await disableDialog(user);
    await user.click(
      within(dialog).getByLabelText("I understand that password sign-in will stop working."),
    );
    await user.click(screen.getByRole("button", { name: "Turn off password sign-in" }));

    expect(await within(dialog).findByText(/no safe administrator sign-in path/)).toBeDefined();
    const blockers = within(dialog).getByTestId("password-login-blockers");
    expect(
      within(blockers).getByText(/No enabled sign-in provider has been tested successfully yet/),
    ).toBeDefined();
    expect(blockers.textContent).not.toContain("fixed text");
    expect(
      within(await panel())
        .getByTestId("password-login-state")
        .getAttribute("data-enabled"),
    ).toBe("true");
  });
});

describe("component_password_login_enable", () => {
  test("re-enabling uses the server contract directly with no disable-only requirements and refreshes bootstrap", async () => {
    const { user, providers } = render({
      passwordLogin: passwordLoginFixture({
        passwordLoginEnabled: false,
        canDisable: false,
        blockers: [{ code: "NO_VALIDATED_PROVIDER", detail: "fixed" }],
        safeAdminLoginPaths: [],
      }),
    });
    const scope = within(await panel());
    await scope.findByText(/Only external providers can be used to sign in/);
    expect(scope.queryByTestId("password-login-blockers-alert")).toBeNull();
    const bootstrap = countBootstrap();
    const before = bootstrap.bootstrap;

    await user.click(scope.getByRole("button", { name: "Turn password sign-in back on" }));

    await waitFor(() => {
      expect(providers.passwordLoginBodies).toEqual([{ enabled: true, confirm: true }]);
    });
    await waitFor(() => {
      expect(scope.getByTestId("password-login-state").getAttribute("data-enabled")).toBe("true");
    });
    expect(scope.getByText("Password sign-in is on.")).toBeDefined();
    await waitFor(() => {
      expect(bootstrap.bootstrap).toBeGreaterThan(before);
    });
  });
});

describe("component_password_login_external_recent_auth", () => {
  const SSO_ME = meFixture({
    role: "admin",
    capabilities: { hasLocalPassword: false },
    recentAuthUntil: "2026-09-28T00:00:00Z",
  });
  const IDP_URL = "https://idp.example.test/authorize?prompt=login&state=server";

  test("an SSO-only admin confirms through the popup and the toggle replays once after the session is re-read", async () => {
    const popups = stubWindowOpen();
    const admin = installAdminServer({ me: SSO_ME, recentAuth: true });
    const providers = installProviderServer(admin);
    let recentAuthUntil = "2026-09-28T00:00:00Z";
    const reauthBodies: unknown[] = [];
    const meCalls = { count: 0 };
    server.use(
      http.get(ME_URL, () => {
        meCalls.count += 1;
        return HttpResponse.json({
          ...SSO_ME,
          session: { ...SSO_ME.session, recentAuthUntil },
        });
      }),
      http.post("*/api/v1/auth/reauthenticate", async ({ request }) => {
        reauthBodies.push(await request.json());
        return HttpResponse.json({ accepted: true, externalReauthUrl: IDP_URL }, { status: 202 });
      }),
    );
    const { router } = renderApp("/admin/providers");
    const user = userEvent.setup({ delay: null });

    const dialog = await disableDialog(user);
    await user.click(
      within(dialog).getByLabelText("I understand that password sign-in will stop working."),
    );
    await user.click(screen.getByRole("button", { name: "Turn off password sign-in" }));
    const title = await screen.findByText("Confirm it's you");
    const recent = must(title.closest<HTMLElement>("[role='dialog']"));
    expect(within(recent).queryByLabelText("Password")).toBeNull();
    expect(providers.passwordLoginBodies).toHaveLength(1);

    await user.click(
      within(recent).getByRole("button", { name: /Continue with your sign-in provider/ }),
    );
    await waitFor(() => {
      expect(popups.popups[0]?.location.href).toBe(IDP_URL);
    });
    expect(reauthBodies).toEqual([{}]);
    const meBefore = meCalls.count;

    recentAuthUntil = "2026-09-28T12:05:00Z";
    admin.reauthenticated = true;
    await act(async () => {
      dispatchWindowMessage(
        { type: "palmr:external-reauth", status: "success" },
        { source: popups.popups[0] ?? null },
      );
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(providers.passwordLoginBodies).toHaveLength(2);
    });
    expect(meCalls.count).toBeGreaterThan(meBefore);
    expect(providers.passwordLoginBodies[1]).toEqual({ enabled: false, confirm: true });
    await waitFor(() => {
      expect(
        within(screen.getByTestId("password-login-panel"))
          .getByTestId("password-login-state")
          .getAttribute("data-enabled"),
      ).toBe("false");
    });
    expect(router.state.location.pathname).toBe("/admin/providers");
    expect(recentAuthStore.getState().challenge).toBeNull();
    popups.restore();
  });
});
