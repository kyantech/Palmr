import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { qk } from "../../shared/api/query-keys";
import { errorEnvelope } from "../../test/bootFixtures";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerState } from "../../test/settingsServer";
import { appRoutes } from "./routes";

const TOKEN = "Zk3nP0x9Qw2Lr7TbVd5HcJm8YfXs1AeUoGi4NyKqW6E";
const VERIFY_URL = "*/api/v1/auth/email/verify";

interface VerifyServer {
  bodies: unknown[];
}

function installVerify(
  settings: SettingsServerState,
  respond: (settings: SettingsServerState) => Response = () =>
    new HttpResponse(null, { status: 204 }),
): VerifyServer {
  const state: VerifyServer = { bodies: [] };
  server.use(
    http.post(VERIFY_URL, async ({ request }) => {
      state.bodies.push(await request.json());
      return respond(settings);
    }),
  );
  return state;
}

function start(entry: string, seed?: Parameters<typeof renderSession>[0]["seed"]) {
  const harness = renderSession({
    routes: appRoutes,
    initialEntries: [entry],
    ...(seed === undefined ? {} : { seed }),
  });
  return { ...harness, user: userEvent.setup({ delay: null }) };
}

function anonymous(): SettingsServerState {
  const settings = installSettingsServer();
  settings.me = null;
  return settings;
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("the e-mailed verification link", () => {
  test("the exact T05 URL shape /verify-email/{token} is a public route, not a 404", async () => {
    const settings = anonymous();
    const verify = installVerify(settings);
    const { router } = start(`/verify-email/${TOKEN}`);

    expect(
      await screen.findByRole("heading", { level: 1, name: "Confirm your new e-mail address" }),
    ).toBeDefined();
    expect(router.state.location.pathname).toBe(`/verify-email/${TOKEN}`);
    expect(screen.queryByText("Page not found")).toBeNull();
    expect(verify.bodies).toEqual([]);
    expect(document.body.textContent).not.toContain(TOKEN);
  });

  test("a valid token confirms, posts only the token and renders success without the token", async () => {
    const settings = anonymous();
    const verify = installVerify(settings);
    const { router, user, queryClient } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    expect(await screen.findByTestId("verify-email-success")).toBeDefined();
    expect(
      screen.getByRole("heading", { level: 1, name: "E-mail address confirmed" }),
    ).toBeDefined();
    expect(verify.bodies).toEqual([{ token: TOKEN }]);
    expect(document.body.textContent).not.toContain(TOKEN);
    expect(
      JSON.stringify(
        queryClient
          .getMutationCache()
          .getAll()
          .map((mutation) => mutation.state),
      ),
    ).not.toContain(TOKEN);

    await user.click(screen.getByRole("button", { name: "Go to sign in" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(router.state.location.search).toBe("");
    expect(router.state.location.pathname).not.toContain(TOKEN);
  });

  test("an invalid token is mapped through the error system", async () => {
    const settings = anonymous();
    installVerify(settings, () =>
      errorEnvelope("EMAIL_VERIFICATION_TOKEN_INVALID", 400, "req-verify-invalid"),
    );
    const { user } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    const panel = await screen.findByTestId("verify-email-dead");
    expect(panel.getAttribute("data-error-code")).toBe("EMAIL_VERIFICATION_TOKEN_INVALID");
    expect(
      screen.getByRole("heading", { level: 1, name: "This verification link isn't valid" }),
    ).toBeDefined();
    expect(screen.getByText(/e-mail verification link isn't valid/)).toBeDefined();
    expect(document.body.textContent).not.toContain(TOKEN);
  });

  test("an expired token is mapped through the error system", async () => {
    const settings = anonymous();
    installVerify(settings, () =>
      errorEnvelope("EMAIL_VERIFICATION_TOKEN_EXPIRED", 410, "req-verify-expired"),
    );
    const { user } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    expect(
      await screen.findByRole("heading", { level: 1, name: "This verification link has expired" }),
    ).toBeDefined();
    expect(screen.getByTestId("verify-email-dead").getAttribute("data-error-code")).toBe(
      "EMAIL_VERIFICATION_TOKEN_EXPIRED",
    );
  });

  test("an address claimed in the meantime is mapped to USER_EMAIL_TAKEN", async () => {
    const settings = anonymous();
    installVerify(settings, () => errorEnvelope("USER_EMAIL_TAKEN", 409, "req-verify-taken"));
    const { user } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    expect(
      await screen.findByRole("heading", { level: 1, name: "This address is no longer available" }),
    ).toBeDefined();
    expect(screen.getByText("This e-mail address is already in use.")).toBeDefined();
  });

  test("a missing or malformed token is handled locally and never sent", async () => {
    const settings = anonymous();
    const verify = installVerify(settings);

    const missing = start("/verify-email");
    expect(await screen.findByTestId("verify-email-malformed")).toBeDefined();
    expect(
      screen.getByRole("heading", { level: 1, name: "This verification link is incomplete" }),
    ).toBeDefined();
    missing.view.unmount();
    resetSessionHarness();
    stubMatchMedia();

    installSettingsServer().me = null;
    start("/verify-email/not%20a%20token!");
    expect(await screen.findByTestId("verify-email-malformed")).toBeDefined();
    expect(verify.bodies).toEqual([]);
    expect(document.body.textContent).not.toContain("not a token!");
  });

  test("the token is never written to browser storage or cookies", async () => {
    const settings = anonymous();
    installVerify(settings);
    const { user } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));
    await screen.findByTestId("verify-email-success");

    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
    expect(document.cookie).not.toContain(TOKEN);
    expect(document.title).not.toContain(TOKEN);
  });

  test("a verified account whose session the server revoked is reconciled to signed out", async () => {
    const settings = installSettingsServer();
    installVerify(settings, (state) => {
      state.me = null;
      return new HttpResponse(null, { status: 204 });
    });
    const { user, queryClient, router } = start(`/verify-email/${TOKEN}`, (client) => {
      client.setQueryData(qk.me.sessions(), { pages: [], pageParams: [] });
    });

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    expect(await screen.findByTestId("verify-email-success")).toBeDefined();
    expect(
      screen.getByText(
        "Your new address is now the one you sign in with. Sign in again to continue.",
      ),
    ).toBeDefined();
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(queryClient.getQueryData(qk.me.sessions())).toBeUndefined();
    await user.click(screen.getByRole("button", { name: "Go to sign in" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/login");
    });
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
  });

  test("a different signed-in account keeps its session and continues to Overview", async () => {
    const settings = installSettingsServer();
    installVerify(settings);
    const { user, queryClient, router } = start(`/verify-email/${TOKEN}`);

    await user.click(await screen.findByRole("button", { name: "Confirm new address" }));

    expect(await screen.findByTestId("verify-email-success")).toBeDefined();
    expect(
      screen.getByText(
        "The new address is now active for this account. Your current session was not affected.",
      ),
    ).toBeDefined();
    expect(queryClient.getQueryData(qk.me.current())).not.toBeNull();
    await user.click(screen.getByRole("button", { name: "Go to Overview" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/overview");
    });
  });
});
