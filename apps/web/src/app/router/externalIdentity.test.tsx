import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { externalNavigation, recentAuthStore } from "../../test/authProbe";
import { qk } from "../../shared/api/query-keys";
import { bootHandlers, bootstrapFixture, errorEnvelope, meFixture } from "../../test/bootFixtures";
import {
  AUTHORIZATION_URL,
  identityLink,
  installIdentityServer,
  type IdentityServerOptions,
} from "../../test/identityServer";
import { CHANNEL_ID, listenOnChannel } from "../../test/reauthChannel";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { installSettingsServer, type SettingsServerOptions } from "../../test/settingsServer";
import { appRoutes } from "./routes";
import { must } from "../../test/must";

const AUTHORIZE_URL = "*/api/v1/auth/providers/:slug/authorize";
const PASSWORD = "correct horse battery";
const PROVIDERS = [
  { slug: "authentik", displayName: "Company SSO", iconKey: "authentik", sortOrder: 1 },
  { slug: "github", displayName: "GitHub", iconKey: "github", sortOrder: 2 },
];

let assign: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  stubMatchMedia();
  assign = vi.spyOn(externalNavigation, "assign").mockImplementation(() => undefined);
});

afterEach(() => {
  resetSessionHarness();
  vi.restoreAllMocks();
});

function renderAnonymous(
  path: string,
  { passwordLoginEnabled = true }: { passwordLoginEnabled?: boolean } = {},
) {
  const { handlers } = bootHandlers({
    bootstrap: bootstrapFixture({ providers: PROVIDERS, passwordLoginEnabled }),
    me: null,
  });
  server.use(...handlers);
  const harness = renderSession({ routes: appRoutes, initialEntries: [path] });
  return { ...harness, user: userEvent.setup({ delay: null }) };
}

describe("component_login_callback_landing", () => {
  test("a failed external login lands on /login with the code and the real request id, then cleans the URL with a replace", async () => {
    const { router, locations } = renderAnonymous(
      "/login?error=PROVIDER_STATE_INVALID&requestId=req-cb-1&next=%2Fsettings%2Fsecurity",
    );

    const alert = await screen.findByTestId("login-callback-error");
    expect(within(alert).getByText(/expired or was already used/)).toBeDefined();
    expect(within(alert).getByText("Request ID: req-cb-1")).toBeDefined();
    await waitFor(() => {
      expect(router.state.location.search).toBe("?next=%2Fsettings%2Fsecurity");
    });
    expect(router.state.historyAction).toBe("REPLACE");
    expect(locations.at(-1)).toBe("/login?next=%2Fsettings%2Fsecurity");
    expect(screen.getByTestId("login-callback-error")).toBeDefined();
  });

  test("the cleaned URL does not present the stale error again after a reload", async () => {
    renderAnonymous("/login?next=%2Fsettings%2Fsecurity");

    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    expect(screen.queryByTestId("login-callback-error")).toBeNull();
  });

  test("an unknown code falls back to the generic message with the raw code and request id", async () => {
    renderAnonymous("/login?error=SOMETHING_NEW&requestId=req-cb-2");

    const alert = await screen.findByTestId("login-callback-error");
    expect(within(alert).getByText(/couldn't complete this action/)).toBeDefined();
    expect(within(alert).getByText("Error code: SOMETHING_NEW")).toBeDefined();
    expect(within(alert).getByText("Request ID: req-cb-2")).toBeDefined();
  });

  test("provider prose and markup in the query string are never rendered, and the parameters are still removed", async () => {
    const { router } = renderAnonymous(
      "/login?error=%3Cimg%20src%3Dx%20onerror%3Dalert(1)%3E&error_description=Access%20denied%20by%20rule%207&requestId=%3Cb%3Ex%3C%2Fb%3E",
    );

    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    expect(screen.queryByTestId("login-callback-error")).toBeNull();
    expect(screen.queryByText(/Access denied by rule 7/)).toBeNull();
    expect(document.querySelector("img[src='x']")).toBeNull();
    await waitFor(() => {
      expect(router.state.location.search).toBe("?error_description=Access+denied+by+rule+7");
    });
  });

  test("the callback error survives the cleanup navigation and stays until the user moves on", async () => {
    const { user, router } = renderAnonymous(
      "/login?error=PROVIDER_AUTH_DENIED&requestId=req-cb-3",
    );

    await screen.findByTestId("login-callback-error");
    await waitFor(() => {
      expect(router.state.location.search).toBe("");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    await user.click(screen.getByRole("button", { name: "Forgot password?" }));
    expect(
      await screen.findByRole("heading", { level: 1, name: "Reset your password" }),
    ).toBeDefined();
  });
});

describe("component_login_external_from_bootstrap", () => {
  test("the bootstrap providers are shown in server order and start a login that keeps the safe next path", async () => {
    const bodies: unknown[] = [];
    server.use(
      http.post(AUTHORIZE_URL, async ({ request }) => {
        bodies.push(await request.json());
        return HttpResponse.json({ authorizationUrl: AUTHORIZATION_URL });
      }),
    );
    const { user } = renderAnonymous("/login?next=%2Ffiles");

    const area = await screen.findByTestId("login-providers");
    expect(
      within(area)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Continue with Company SSO", "Continue with GitHub"]);
    await user.click(within(area).getByRole("button", { name: "Continue with GitHub" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith(AUTHORIZATION_URL);
    });
    expect(bodies).toEqual([{ purpose: "login", returnTo: "/files" }]);
  });

  test("an unsafe next path is never forwarded as returnTo", async () => {
    const bodies: unknown[] = [];
    server.use(
      http.post(AUTHORIZE_URL, async ({ request }) => {
        bodies.push(await request.json());
        return HttpResponse.json({ authorizationUrl: AUTHORIZATION_URL });
      }),
    );
    const { user } = renderAnonymous("/login?next=%2F%2Fevil.example%2Fpath");

    await user.click(await screen.findByRole("button", { name: "Continue with GitHub" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledTimes(1);
    });
    expect(bodies).toEqual([{ purpose: "login" }]);
  });

  test("with password login disabled the page shows only the providers", async () => {
    renderAnonymous("/login", { passwordLoginEnabled: false });

    expect(await screen.findByTestId("login-providers")).toBeDefined();
    expect(screen.queryByLabelText("Password")).toBeNull();
    expect(screen.queryByRole("button", { name: "Forgot password?" })).toBeNull();
    expect(screen.queryByText("or")).toBeNull();
  });
});

function renderSecurity(
  path = "/settings/security",
  settingsOptions: SettingsServerOptions = {},
  identityOptions: IdentityServerOptions = {},
) {
  const settings = installSettingsServer({
    bootstrap: bootstrapFixture({ providers: PROVIDERS }),
    ...settingsOptions,
  });
  const identity = installIdentityServer(settings, identityOptions);
  const harness = renderSession({ routes: appRoutes, initialEntries: [path] });
  return { settings, identity, ...harness, user: userEvent.setup({ delay: null }) };
}

async function recentAuthDialog(): Promise<HTMLElement> {
  const title = await screen.findByText("Confirm it's you");
  const dialog = title.closest<HTMLElement>("[role='dialog']");
  if (dialog === null) {
    throw new Error("the recent-auth title is not inside a dialog");
  }
  return dialog;
}

async function confirmWithPassword(user: ReturnType<typeof userEvent.setup>) {
  const dialog = await recentAuthDialog();
  await user.type(within(dialog).getByLabelText("Password"), PASSWORD);
  await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
}

describe("component_identity_links", () => {
  test("linked accounts render provider, e-mail and dates but never the external subject or an avatar", async () => {
    renderSecurity(
      "/settings/security",
      {},
      {
        links: [
          identityLink({ id: "link-1" }),
          identityLink({
            id: "link-2",
            providerSlug: "github",
            providerDisplayName: "GitHub",
            emailAtLink: null,
            lastUsedAt: null,
          }),
        ],
      },
    );

    const section = await screen.findByTestId("settings-identity-links");
    const rows = await within(section).findAllByTestId("identity-link-row");
    expect(rows).toHaveLength(2);
    expect(within(must(rows[0])).getByText("Company SSO")).toBeDefined();
    expect(within(must(rows[0])).getByText("Linked as ada@example.test")).toBeDefined();
    expect(within(must(rows[0])).getByText(/^Connected /)).toBeDefined();
    expect(within(must(rows[0])).getByText(/^Last used /)).toBeDefined();
    expect(within(must(rows[1])).getByText("Not used to sign in yet")).toBeDefined();
    expect(within(must(rows[1])).queryByText(/Linked as/)).toBeNull();
    expect(section.textContent).not.toContain("subject-that-must-not-render");
    expect(section.querySelector("img")).toBeNull();
    expect(screen.queryByTestId("identity-links-available")).toBeNull();
  });

  test("only providers that are not linked yet can be connected", async () => {
    renderSecurity(
      "/settings/security",
      {},
      { links: [identityLink({ id: "link-1", providerSlug: "authentik" })] },
    );

    const available = await screen.findByTestId("identity-links-available");
    expect(
      within(available)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Connect GitHub"]);
  });

  test("an account with no links and no providers says so", async () => {
    renderSecurity("/settings/security", { bootstrap: bootstrapFixture({ providers: [] }) });

    expect(await screen.findByTestId("identity-links-empty")).toBeDefined();
    expect(await screen.findByTestId("identity-links-no-providers")).toBeDefined();
  });

  test("linking asks for recent auth, replays the link request once and then navigates to the server URL", async () => {
    const { user, settings, identity } = renderSecurity();
    await screen.findByTestId("identity-links-available");

    await user.click(await screen.findByRole("button", { name: "Connect Company SSO" }));
    await confirmWithPassword(user);

    await waitFor(() => {
      expect(assign).toHaveBeenCalledTimes(1);
    });
    expect(assign).toHaveBeenCalledWith(AUTHORIZATION_URL);
    expect(identity.linkStarts).toEqual(["authentik", "authentik"]);
    expect(settings.reauthBodies).toEqual([{ password: PASSWORD }]);
    expect(recentAuthStore.getState().challenge).toBeNull();
  });

  test("with a fresh recent-auth window linking starts immediately", async () => {
    const { user, identity } = renderSecurity("/settings/security", { recentAuth: true });

    await user.click(await screen.findByRole("button", { name: "Connect GitHub" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith(AUTHORIZATION_URL);
    });
    expect(identity.linkStarts).toEqual(["github"]);
    expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).toBeNull();
  });

  test("a refused link start is shown by its code and nothing is navigated", async () => {
    const { user } = renderSecurity(
      "/settings/security",
      { recentAuth: true },
      { linkFailure: () => errorEnvelope("PROVIDER_IDENTITY_ALREADY_LINKED", 409, "req-linked") },
    );

    await user.click(await screen.findByRole("button", { name: "Connect GitHub" }));

    expect(await screen.findByText(/already linked to an account/)).toBeDefined();
    expect(assign).not.toHaveBeenCalled();
  });

  test("a link callback failure is rendered from the code, shows the request id and is removed from the URL without leaving the page", async () => {
    const { router, locations } = renderSecurity(
      "/settings/security?error=PROVIDER_IDENTITY_ALREADY_LINKED&requestId=req-link-77",
    );

    const alert = await screen.findByTestId("identity-link-callback-error");
    expect(within(alert).getByText(/already linked to an account/)).toBeDefined();
    expect(within(alert).getByText("Request ID: req-link-77")).toBeDefined();
    await waitFor(() => {
      expect(router.state.location.search).toBe("");
    });
    expect(router.state.location.pathname).toBe("/settings/security");
    expect(router.state.historyAction).toBe("REPLACE");
    expect(locations.some((location) => location.startsWith("/login"))).toBe(false);
    expect(screen.getByTestId("identity-link-callback-error")).toBeDefined();
  });
});

describe("component_identity_unlink", () => {
  const LINK = identityLink({ id: "link-1" });

  async function openUnlinkDialog(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: "Remove Company SSO" }));
    return screen.findByRole("dialog", { name: "Remove Company SSO?" });
  }

  test("the destructive confirmation explains the consequences and Cancel changes nothing", async () => {
    const { user, identity } = renderSecurity("/settings/security", {}, { links: [LINK] });

    const dialog = await openUnlinkDialog(user);
    expect(
      within(dialog).getByText(/no longer be able to sign in to Palmr with this account/),
    ).toBeDefined();
    expect(within(dialog).getByText(/signed out of Palmr on every device/)).toBeDefined();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Remove Company SSO?" })).toBeNull();
    });
    expect(identity.unlinkAttempts).toEqual([]);
    expect(screen.getByTestId("identity-link-row")).toBeDefined();
  });

  test("unlinking requires recent auth, replays once, then ends the revoked session and says why", async () => {
    const { user, identity, settings, queryClient } = renderSecurity(
      "/settings/security",
      {},
      { links: [LINK] },
    );

    const dialog = await openUnlinkDialog(user);
    await user.click(within(dialog).getByRole("button", { name: "Remove account" }));
    await confirmWithPassword(user);

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeDefined();
    expect(identity.unlinkAttempts).toEqual(["link-1", "link-1"]);
    expect(identity.unlinked).toEqual(["link-1"]);
    expect(settings.reauthBodies).toEqual([{ password: PASSWORD }]);
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(await screen.findByTestId("login-notice")).toBeDefined();
    expect(screen.getByText("Sign-in method removed")).toBeDefined();
    expect(recentAuthStore.getState().challenge).toBeNull();
  });

  test.each([
    ["IDENTITY_LINK_LAST_LOGIN_PATH", /only way to sign in to this account/],
    ["PASSWORD_LOGIN_DISABLE_UNSAFE", /no safe administrator sign-in path/],
    ["PROVIDER_LINK_NOT_FOUND", /no longer exists/],
  ])(
    "%s is shown through the shared error system and the session stays signed in",
    async (code, text) => {
      const { user, identity, queryClient } = renderSecurity(
        "/settings/security",
        { recentAuth: true },
        { links: [LINK], unlinkFailure: () => errorEnvelope(code, 409, "req-unlink-refused") },
      );
      const listCalls = () => identity.listCalls;

      const dialog = await openUnlinkDialog(user);
      const before = listCalls();
      await user.click(within(dialog).getByRole("button", { name: "Remove account" }));

      expect(await screen.findByText(text)).toBeDefined();
      await waitFor(() => {
        expect(screen.queryByRole("dialog", { name: "Remove Company SSO?" })).toBeNull();
      });
      expect(identity.unlinked).toEqual([]);
      expect(queryClient.getQueryData(qk.me.current())).not.toBeNull();
      expect(screen.queryByRole("heading", { level: 1, name: "Sign in" })).toBeNull();
      await waitFor(() => {
        expect(listCalls()).toBeGreaterThan(before);
      });
    },
  );
});

describe("component_reauth_complete_route", () => {
  test("an authenticated popup landing renders the minimal status panel, outside the app shell, and announces on its channel", async () => {
    installSettingsServer();
    const own = listenOnChannel(CHANNEL_ID);
    vi.spyOn(window, "close").mockImplementation(() => undefined);
    renderSession({
      routes: appRoutes,
      initialEntries: [`/auth/reauth-complete?status=success&channel=${CHANNEL_ID}`],
    });

    const page = await screen.findByTestId("reauth-complete");
    expect(page.getAttribute("data-outcome")).toBe("success");
    expect(screen.queryByTestId("app-shell")).toBeNull();
    expect(screen.queryByTestId("auth-brand")).toBeNull();
    await waitFor(() => {
      expect(own.messages).toEqual([{ type: "palmr:external-reauth", status: "success" }]);
    });
    own.close();
  });

  test("it shows the localized outcome and a way back to /overview when the window stays open", async () => {
    installSettingsServer();
    const { router, user } = (() => {
      const harness = renderSession({
        routes: appRoutes,
        initialEntries: [
          `/auth/reauth-complete?status=error&error=PROVIDER_AUTH_DENIED&requestId=req-ra-1&channel=${CHANNEL_ID}`,
        ],
      });
      return { ...harness, user: userEvent.setup({ delay: null }) };
    })();

    expect(await screen.findByText("Request ID: req-ra-1")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Continue to Palmr" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/overview");
    });
  });

  test("it is behind RequireAuth: an anonymous visitor is sent to /login", async () => {
    const { handlers } = bootHandlers({ me: null });
    server.use(...handlers);
    const { router } = renderSession({
      routes: appRoutes,
      initialEntries: [`/auth/reauth-complete?status=success&channel=${CHANNEL_ID}`],
    });

    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    expect(router.state.location.pathname).toBe("/login");
    expect(screen.queryByTestId("reauth-complete")).toBeNull();
  });

  test("it never calls an API route of its own", async () => {
    const settings = installSettingsServer({ me: meFixture() });
    renderSession({
      routes: appRoutes,
      initialEntries: [`/auth/reauth-complete?status=success&channel=${CHANNEL_ID}`],
    });

    await screen.findByTestId("reauth-complete");
    expect(settings.calls.sessions).toBe(0);
  });
});
