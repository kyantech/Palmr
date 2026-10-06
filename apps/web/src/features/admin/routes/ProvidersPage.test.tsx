import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../../test/authProbe";
import { qk } from "../../../shared/api/query-keys";
import { BOOTSTRAP_URL, bootstrapFixture, errorEnvelope } from "../../../test/bootFixtures";
import {
  AUTHENTIK_ID,
  GITHUB_ID,
  GOOGLE_ID,
  NEW_SECRET,
  providerFixture,
  SECRET_MARKER,
} from "../../../test/providerServer";
import { chooseOption, renderProviders } from "../../../test/renderAdmin";
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

async function rows() {
  const list = await screen.findByTestId("provider-list");
  return within(list).getAllByTestId("provider-row");
}

function rowBySlug(slug: string): HTMLElement {
  return must(
    screen
      .getByTestId("provider-list")
      .querySelector<HTMLElement>(`[data-provider-slug='${slug}']`),
  );
}

async function rowOf(name: string): Promise<HTMLElement> {
  const found = (await rows()).find((row) => within(row).queryByText(name) !== null);
  if (found === undefined) {
    throw new Error(`no provider row named ${name}`);
  }
  return found;
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

async function confirmRecentAuth(user: ReturnType<typeof renderProviders>["user"]) {
  const title = await screen.findByText("Confirm it's you");
  const dialog = title.closest<HTMLElement>("[role='dialog']");
  if (dialog === null) {
    throw new Error("the recent-auth title is not inside a dialog");
  }
  await user.type(within(dialog).getByLabelText("Password"), PASSWORD);
  await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
}

describe("component_admin_providers_list", () => {
  test("the route is registered, primes the provider queries and lists providers in server order", async () => {
    const { providers, queryClient } = renderProviders();

    expect(await screen.findByTestId("admin-providers-page")).toBeDefined();
    expect(screen.getByTestId("app-shell")).toBeDefined();
    expect(queryClient.getQueryState(qk.admin.providers())).toBeDefined();
    const names = (await rows()).map((row) => row.getAttribute("data-provider-slug"));
    expect(names).toEqual(["google", "authentik", "github"]);
    expect(providers.calls.providers).toBe(1);
  });

  test("each row shows the operational state the API reports", async () => {
    renderProviders();

    const google = await rowOf("Google");
    expect(within(google).getByText("Position 1 of 3")).toBeDefined();
    expect(within(google).getByText("OpenID Connect")).toBeDefined();
    expect(within(google).getByText("google")).toBeDefined();
    expect(within(google).getByTestId("provider-enabled").getAttribute("data-enabled")).toBe(
      "true",
    );
    expect(within(google).getByTestId("provider-validation").getAttribute("data-state")).toBe(
      "validated",
    );
    expect(within(google).getByText("Linked accounts: 4")).toBeDefined();
    expect(within(google).getByTestId("provider-secret").getAttribute("data-configured")).toBe(
      "true",
    );
    expect(within(google).getByText("Auto-provision off")).toBeDefined();
    expect(within(google).getByText("E-mail linking on")).toBeDefined();

    const company = await rowOf("Company SSO");
    expect(within(company).getByTestId("provider-enabled").getAttribute("data-enabled")).toBe(
      "false",
    );
    expect(within(company).getByText("Auto-provision on")).toBeDefined();

    const github = await rowOf("GitHub");
    expect(within(github).getByText("OAuth2")).toBeDefined();
    expect(within(github).getByText("E-mail linking off")).toBeDefined();
  });

  test("validation is never claimed without a recorded successful test, and failed checks are listed from the stored summary", async () => {
    renderProviders();

    const company = await rowOf("Company SSO");
    expect(within(company).getByTestId("provider-validation").getAttribute("data-state")).toBe(
      "untested",
    );
    expect(within(company).queryByText(/^Tested /)).toBeNull();

    const github = await rowOf("GitHub");
    expect(within(github).getByTestId("provider-validation").getAttribute("data-state")).toBe(
      "failed",
    );
    const checks = within(github).getByTestId("provider-checks");
    expect(within(checks).getByText("User info endpoint")).toBeDefined();
    expect(within(checks).getByText("Token endpoint")).toBeDefined();
    expect(within(checks).getByText("upstream_error")).toBeDefined();
    expect(within(checks).getByText("timeout")).toBeDefined();
  });

  test("the redirect URI is the server value, read-only and copyable, never built from the page origin", async () => {
    renderProviders();

    const google = await rowOf("Google");
    const redirect = within(google).getByTestId("provider-redirect-uri");
    expect(redirect.querySelector("[data-redirect-uri]")?.getAttribute("data-redirect-uri")).toBe(
      "https://palmr.example.test/api/v1/auth/providers/google/callback",
    );
    expect(redirect.textContent).not.toContain(window.location.origin);
    expect(within(redirect).queryByRole("textbox")).toBeNull();
    expect(within(redirect).getByRole("button", { name: /copy/i })).toBeDefined();
  });

  test("an empty provider list explains how to start", async () => {
    renderProviders("/admin/providers", { providers: { providers: [] } });

    expect(await screen.findByTestId("providers-empty")).toBeDefined();
    expect(screen.getByRole("button", { name: "Add provider" })).toBeDefined();
  });
});

describe("component_provider_secret_never_rendered", () => {
  const LEAKY = [
    providerFixture({
      id: GOOGLE_ID,
      slug: "google",
      displayName: "Google",
      ...({ clientSecret: SECRET_MARKER } as object),
    }),
  ];

  function markerAnywhere(): boolean {
    const inputs = [...document.querySelectorAll("input, textarea")] as HTMLInputElement[];
    return (
      document.documentElement.outerHTML.includes(SECRET_MARKER) ||
      inputs.some((input) => input.value.includes(SECRET_MARKER) || input.value === "********")
    );
  }

  test("a secret that leaked into a response is never rendered in the list or the edit form, and the secret field is empty", async () => {
    const { user } = renderProviders("/admin/providers", { providers: { providers: LEAKY } });

    const row = await rowOf("Google");
    expect(markerAnywhere()).toBe(false);
    expect(within(row).getByTestId("provider-secret").getAttribute("data-configured")).toBe("true");

    await user.click(within(row).getByRole("button", { name: "Edit Google" }));
    const form = await screen.findByTestId("provider-form");
    const secret = within(form).getByLabelText("Client secret");
    expect((secret as HTMLInputElement).value).toBe("");
    expect(secret.getAttribute("type")).toBe("password");
    expect(
      within(form).getByTestId("provider-form-secret-state").getAttribute("data-configured"),
    ).toBe("true");
    expect(markerAnywhere()).toBe(false);
    expect(form.textContent).not.toContain(SECRET_MARKER);
  });

  test("a newly typed secret is sent once, then the field and the page never show it again", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
      providers: { providers: LEAKY },
    });
    const row = await rowOf("Google");
    await user.click(within(row).getByRole("button", { name: "Edit Google" }));
    const form = await screen.findByTestId("provider-form");

    await user.type(within(form).getByLabelText("Client secret"), NEW_SECRET);
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(providers.patchBodies).toHaveLength(1);
    });
    expect(providers.patchBodies[0]?.body).toEqual({ clientSecret: NEW_SECRET });
    await waitFor(() => {
      expect(screen.queryByTestId("provider-form")).toBeNull();
    });
    expect(document.documentElement.outerHTML).not.toContain(NEW_SECRET);
    expect(markerAnywhere()).toBe(false);

    await user.click(within(await rowOf("Google")).getByRole("button", { name: "Edit Google" }));
    const reopened = await screen.findByTestId("provider-form");
    expect(within(reopened).getByLabelText<HTMLInputElement>("Client secret").value).toBe("");
  });
});

describe("component_provider_create", () => {
  test("presets come from the server catalogue and initialise the form", async () => {
    const { user, providers } = renderProviders();
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    expect(within(form).queryByLabelText("Display name")).toBeNull();
    await chooseOption(user, within(form).getByLabelText("Provider type"), "GitHub");

    expect(providers.calls.presets).toBe(1);
    expect(within(form).getByLabelText<HTMLInputElement>("Display name").value).toBe("GitHub");
    expect(within(form).getByLabelText<HTMLInputElement>("Slug").value).toBe("github");
    expect(within(form).getByTestId("provider-form-endpoints")).toBeDefined();
    expect(within(form).getByLabelText<HTMLInputElement>("Authorization endpoint").value).toBe(
      "https://github.com/login/oauth/authorize",
    );
    expect(within(form).queryByLabelText("Issuer URL")).toBeNull();
  });

  test("the catalogue offers Custom OIDC and Custom OAuth2 alongside the presets", async () => {
    const { user } = renderProviders();
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await user.click(within(form).getByLabelText("Provider type"));

    for (const label of ["Google", "GitHub", "Custom OpenID Connect", "Custom OAuth2"]) {
      expect(await screen.findByTitle(label)).toBeDefined();
    }
  });

  test("an OAuth2 provider is created after recent auth, replayed once, and the login state is refetched", async () => {
    const { user, providers, state } = renderProviders("/admin/providers", {
      admin: { recentAuth: true },
    });
    await rows();
    const bootstrap = countBootstrap();
    const bootstrapBefore = bootstrap.bootstrap;

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "GitHub");
    await user.type(within(form).getByLabelText("Client ID"), "gh-client");
    await user.type(within(form).getByLabelText("Client secret"), NEW_SECRET);
    await user.clear(within(form).getByLabelText("Slug"));
    await user.type(within(form).getByLabelText("Slug"), "github-corp");
    await user.click(within(form).getByTestId("provider-flag-enabled"));
    await user.click(within(form).getByRole("button", { name: "Add provider" }));
    await confirmRecentAuth(user);

    await waitFor(() => {
      expect(providers.createBodies).toHaveLength(2);
    });
    expect(providers.createBodies[1]).toEqual(providers.createBodies[0]);
    expect(providers.createBodies[0]).toMatchObject({
      slug: "github-corp",
      displayName: "GitHub",
      protocol: "oauth2",
      preset: "github",
      clientId: "gh-client",
      clientSecret: NEW_SECRET,
      tokenAuthMethod: "client_secret_post",
      endpoints: {
        authorization: "https://github.com/login/oauth/authorize",
        token: "https://github.com/login/oauth/access_token",
        userinfo: "https://api.github.com/user",
      },
      autoProvision: false,
      allowEmailLinking: false,
      enabled: true,
    });
    expect(providers.createBodies[0]).not.toHaveProperty("role");
    expect(providers.createBodies[0]).not.toHaveProperty("redirectUri");
    expect(state.reauthBodies).toEqual([{ password: PASSWORD }]);
    await waitFor(() => {
      expect(screen.queryByTestId("provider-form")).toBeNull();
    });
    expect(await rowOf("GitHub")).toBeDefined();
    expect((await rows()).map((row) => row.getAttribute("data-provider-slug"))).toContain(
      "github-corp",
    );
    await waitFor(() => {
      expect(bootstrap.bootstrap).toBeGreaterThan(bootstrapBefore);
    });
    expect(recentAuthStore.getState().challenge).toBeNull();
  });

  test("validation explains missing and malformed values before anything is sent", async () => {
    const { user, providers } = renderProviders();
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "Custom OAuth2");
    await user.type(within(form).getByLabelText("Slug"), "Bad Slug!");
    await user.type(within(form).getByLabelText("Token endpoint"), "not a url");
    await user.click(within(form).getByRole("button", { name: "Add provider" }));

    expect(await within(form).findByText(/Use 2–40 lowercase letters/)).toBeDefined();
    expect(within(form).getAllByText("Enter a valid https:// URL.").length).toBeGreaterThan(0);
    expect(within(form).getAllByText("This field is required.").length).toBeGreaterThan(0);
    expect(providers.createBodies).toHaveLength(0);
  });

  test("a slug that is already taken is reported on the slug field by code", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "Google");
    await user.type(within(form).getByLabelText("Client ID"), "g-client");
    await user.type(within(form).getByLabelText("Client secret"), NEW_SECRET);
    await user.click(within(form).getByRole("button", { name: "Add provider" }));

    expect(
      await within(form).findByText("This slug is already used by another provider."),
    ).toBeDefined();
    expect(providers.createBodies).toHaveLength(1);
    expect(screen.getByTestId("provider-form")).toBeDefined();
  });

  test("a custom OIDC provider can discover settings on the server and apply them to the form", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "Custom OpenID Connect");
    await user.type(
      within(form).getByLabelText("Issuer URL"),
      "https://sso.example.test/application/o/palmr/",
    );
    await user.click(within(form).getByRole("button", { name: "Discover settings" }));

    const preview = await within(form).findByTestId("provider-discovery");
    expect(providers.discoverBodies).toEqual([
      { issuerUrl: "https://sso.example.test/application/o/palmr/" },
    ]);
    expect(within(preview).getByText("https://sso.example.test/discovered/token")).toBeDefined();
    expect(within(preview).getByText("offline_access")).toBeDefined();
    expect(within(preview).getByText("client_secret_post")).toBeDefined();
    await user.click(within(preview).getByRole("button", { name: "Use these values" }));

    expect(within(form).getByLabelText<HTMLInputElement>("Token endpoint").value).toBe(
      "https://sso.example.test/discovered/token",
    );
    await user.type(within(form).getByLabelText("Slug"), "company");
    await user.type(within(form).getByLabelText("Display name"), "Company");
    await user.type(within(form).getByLabelText("Client ID"), "company-client");
    await user.type(within(form).getByLabelText("Client secret"), NEW_SECRET);
    await user.click(within(form).getByRole("button", { name: "Add provider" }));

    await waitFor(() => {
      expect(providers.createBodies).toHaveLength(1);
    });
    expect(providers.createBodies[0]).toMatchObject({
      protocol: "oidc",
      issuerUrl: "https://sso.example.test/application/o/palmr/",
      endpoints: { token: "https://sso.example.test/discovered/token" },
    });
  });

  test("a discovery failure is mapped by code and never shows upstream text", async () => {
    const { user } = renderProviders("/admin/providers", {
      providers: {
        discovery: () =>
          errorEnvelope("PROVIDER_DISCOVERY_FAILED", 502, "req-discover", {
            message: "upstream body: <html>502 Bad Gateway from nginx/1.2</html>",
            details: { reason: "issuer_mismatch" },
          }),
      },
    });
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "Custom OpenID Connect");
    await user.type(within(form).getByLabelText("Issuer URL"), "https://sso.example.test/");
    await user.click(within(form).getByRole("button", { name: "Discover settings" }));

    expect(
      await within(form).findByText(/couldn't read the provider's OpenID Connect configuration/),
    ).toBeDefined();
    expect(screen.queryByText(/nginx/)).toBeNull();
    expect(screen.queryByText(/issuer_mismatch/)).toBeNull();
    expect(within(form).queryByTestId("provider-discovery")).toBeNull();
  });

  test("the form offers no role, domain or redirect-URI fields", async () => {
    const { user } = renderProviders();
    await rows();

    await user.click(screen.getByRole("button", { name: "Add provider" }));
    const form = await screen.findByTestId("provider-form");
    await chooseOption(user, within(form).getByLabelText("Provider type"), "Custom OpenID Connect");

    for (const label of [/role/i, /domain/i, /redirect uri/i]) {
      expect(within(form).queryByLabelText(label)).toBeNull();
    }
    expect(within(form).getByText(/always regular users, never administrators/)).toBeDefined();
  });
});

describe("component_provider_edit", () => {
  test("only changed members are sent and a blank secret keeps the stored one", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    const row = await rowOf("Company SSO");
    await user.click(within(row).getByRole("button", { name: "Edit Company SSO" }));
    const form = await screen.findByTestId("provider-form");

    const name = within(form).getByLabelText("Display name");
    await user.clear(name);
    await user.type(name, "Acme SSO");
    await user.click(within(form).getByTestId("provider-flag-auto-provision"));
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(providers.patchBodies).toHaveLength(1);
    });
    expect(providers.patchBodies[0]).toEqual({
      id: AUTHENTIK_ID,
      body: { displayName: "Acme SSO", autoProvision: false },
    });
    expect(providers.patchBodies[0]?.body).not.toHaveProperty("clientSecret");
    expect(providers.patchBodies[0]?.body).not.toHaveProperty("slug");
    expect(await rowOf("Acme SSO")).toBeDefined();
  });

  test("the e-mail linking and auto-provision switches reflect the API and explain themselves", async () => {
    const { user } = renderProviders();
    const row = await rowOf("GitHub");
    await user.click(within(row).getByRole("button", { name: "Edit GitHub" }));
    const form = await screen.findByTestId("provider-form");

    expect(
      within(form).getByTestId("provider-flag-email-linking").getAttribute("aria-checked"),
    ).toBe("false");
    expect(
      within(form).getByTestId("provider-flag-auto-provision").getAttribute("aria-checked"),
    ).toBe("false");
    expect(
      within(form).getByText(/only when the provider states the e-mail is verified/),
    ).toBeDefined();
    expect(within(form).getByText(/connect this provider manually/)).toBeDefined();
    expect(within(form).queryByTestId("provider-flag-enabled")).toBeNull();
  });

  test("clearing a secret needs the explicit none authentication method and sends null", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    const row = await rowOf("Google");
    await user.click(within(row).getByRole("button", { name: "Edit Google" }));
    const form = await screen.findByTestId("provider-form");

    expect(within(form).queryByLabelText("Remove the stored client secret")).toBeNull();
    await chooseOption(
      user,
      within(form).getByLabelText("Token endpoint authentication"),
      "None (public client)",
    );
    await user.click(await within(form).findByLabelText("Remove the stored client secret"));
    await user.click(within(form).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(providers.patchBodies).toHaveLength(1);
    });
    expect(providers.patchBodies[0]?.body).toEqual({ tokenAuthMethod: "none", clientSecret: null });
  });

  test("the redirect URI is shown read-only inside the edit form", async () => {
    const { user } = renderProviders();
    const row = await rowOf("Google");
    await user.click(within(row).getByRole("button", { name: "Edit Google" }));
    const form = await screen.findByTestId("provider-form");

    const redirect = within(form).getByTestId("provider-redirect-uri");
    expect(redirect.textContent).toContain(
      "https://palmr.example.test/api/v1/auth/providers/google/callback",
    );
    expect(within(redirect).queryByRole("textbox")).toBeNull();
  });
});

describe("component_provider_test_and_state", () => {
  test("a successful test shows the checks and refreshes provider state and the password-login preflight", async () => {
    const { user, providers } = renderProviders();
    const row = await rowOf("Company SSO");
    const before = { ...providers.calls };

    await user.click(within(row).getByRole("button", { name: "Test Company SSO" }));

    const checks = await within(await rowOf("Company SSO")).findByTestId("provider-checks");
    expect(within(checks).getAllByText("Passed")).toHaveLength(2);
    await waitFor(() => {
      expect(
        within(rowBySlug("authentik"))
          .getByTestId("provider-validation")
          .getAttribute("data-state"),
      ).toBe("validated");
    });
    expect(providers.testedIds).toEqual([AUTHENTIK_ID]);
    await waitFor(() => {
      expect(providers.calls.providers).toBeGreaterThan(before.providers);
      expect(providers.calls.passwordLogin).toBeGreaterThan(before.passwordLogin);
    });
  });

  test("a failed test clears stale green validation, lists the failing checks and refreshes persisted state", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      providers: {
        tests: [
          {
            kind: "checks-failed",
            checks: [
              { name: "discovery", ok: true },
              { name: "jwks", ok: false, detail: "upstream_error" },
            ],
          },
        ],
      },
    });
    const google = await rowOf("Google");
    expect(within(google).getByTestId("provider-validation").getAttribute("data-state")).toBe(
      "validated",
    );
    const before = { ...providers.calls };

    await user.click(within(google).getByRole("button", { name: "Test Google" }));

    await waitFor(() => {
      expect(
        within(rowBySlug("google")).getByTestId("provider-validation").getAttribute("data-state"),
      ).toBe("failed");
    });
    const row = await rowOf("Google");
    expect(within(row).queryByText(/^Tested /)).toBeNull();
    const checks = within(row).getByTestId("provider-checks");
    expect(within(checks).getByText("Signing keys")).toBeDefined();
    expect(within(checks).getByText("upstream_error")).toBeDefined();
    expect(within(checks).getByText("Failed")).toBeDefined();
    await waitFor(() => {
      expect(providers.calls.providers).toBeGreaterThan(before.providers);
      expect(providers.calls.passwordLogin).toBeGreaterThan(before.passwordLogin);
    });
    expect(providers.providers.find((p) => p.id === GOOGLE_ID)?.validatedAt).toBeNull();
  });

  test("an unexpected test error is mapped by code without inventing a validation result", async () => {
    const { user } = renderProviders("/admin/providers", {
      providers: { tests: [{ kind: "error", code: "DATABASE_BUSY", status: 503 }] },
    });
    const google = await rowOf("Google");

    await user.click(within(google).getByRole("button", { name: "Test Google" }));

    expect(await screen.findByTestId("provider-test-failure")).toBeDefined();
    expect(
      within(await rowOf("Google"))
        .getByTestId("provider-validation")
        .getAttribute("data-state"),
    ).toBe("validated");
  });
});

describe("component_provider_reorder", () => {
  test("move buttons give a keyboard path, persist through the order endpoint and refresh the login order", async () => {
    const { user, providers } = renderProviders();
    await rows();
    const bootstrap = countBootstrap();
    const bootstrapBefore = bootstrap.bootstrap;

    await user.click(screen.getByRole("button", { name: "Move Google down" }));

    await waitFor(() => {
      expect(providers.orderBodies).toEqual([[AUTHENTIK_ID, GOOGLE_ID, GITHUB_ID]]);
    });
    await waitFor(() => {
      expect(
        screen.getAllByTestId("provider-row").map((row) => row.getAttribute("data-provider-slug")),
      ).toEqual(["authentik", "google", "github"]);
    });
    expect(screen.getByText("Google moved to position 2 of 3.")).toBeDefined();
    await waitFor(() => {
      expect(bootstrap.bootstrap).toBeGreaterThan(bootstrapBefore);
    });
  });

  test("the first row cannot move up and the last cannot move down", async () => {
    renderProviders();
    await rows();

    expect(screen.getByRole("button", { name: "Move Google up" }).hasAttribute("disabled")).toBe(
      true,
    );
    expect(screen.getByRole("button", { name: "Move GitHub down" }).hasAttribute("disabled")).toBe(
      true,
    );
  });

  test("dragging a handle onto another row reorders through the same endpoint", async () => {
    const { providers } = renderProviders();
    const all = await rows();
    const handle = within(must(all[0])).getByTestId("provider-drag-handle");
    const target = must(all[2]);
    const dataTransfer = {
      effectAllowed: "",
      dropEffect: "",
      setData: vi.fn(),
      setDragImage: vi.fn(),
    };

    await act(async () => {
      fireEvent.dragStart(handle, { dataTransfer });
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.dragOver(target, { dataTransfer });
      await Promise.resolve();
    });
    await act(async () => {
      fireEvent.drop(target, { dataTransfer });
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(providers.orderBodies).toEqual([[AUTHENTIK_ID, GITHUB_ID, GOOGLE_ID]]);
    });
  });

  test("a failed reorder restores the server order and shows the mapped error", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      providers: { failures: { order: () => errorEnvelope("VALIDATION_ERROR", 422, "req-order") } },
    });
    await rows();

    await user.click(screen.getByRole("button", { name: "Move Google down" }));

    expect(await screen.findByText(/Some of the information entered isn't valid/)).toBeDefined();
    await waitFor(() => {
      expect(
        screen.getAllByTestId("provider-row").map((row) => row.getAttribute("data-provider-slug")),
      ).toEqual(["google", "authentik", "github"]);
    });
    expect(providers.orderBodies).toHaveLength(1);
  });
});

describe("component_provider_enable_and_delete", () => {
  test("the switch disables a provider with a PATCH and refreshes the login state", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    const row = await rowOf("Google");
    const bootstrap = countBootstrap();
    const before = bootstrap.bootstrap;

    await user.click(within(row).getByRole("switch", { name: "Enable Google" }));

    await waitFor(() => {
      expect(providers.patchBodies).toEqual([{ id: GOOGLE_ID, body: { enabled: false } }]);
    });
    await waitFor(() => {
      expect(
        within(rowBySlug("google")).getByTestId("provider-enabled").getAttribute("data-enabled"),
      ).toBe("false");
    });
    await waitFor(() => {
      expect(bootstrap.bootstrap).toBeGreaterThan(before);
    });
  });

  test("a refusal that would remove the last admin path is shown by code and the state is unchanged", async () => {
    const { user } = renderProviders("/admin/providers", {
      providers: {
        failures: {
          update: () => errorEnvelope("PASSWORD_LOGIN_DISABLE_UNSAFE", 409, "req-unsafe"),
        },
      },
    });
    const row = await rowOf("Google");

    await user.click(within(row).getByRole("switch", { name: "Enable Google" }));

    expect(await screen.findByText(/no safe administrator sign-in path/)).toBeDefined();
    expect(
      within(await rowOf("Google"))
        .getByTestId("provider-enabled")
        .getAttribute("data-enabled"),
    ).toBe("true");
  });

  test("deleting needs a destructive confirmation, then removes the row", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    const row = await rowOf("Company SSO");

    await user.click(within(row).getByRole("button", { name: "Delete Company SSO" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Company SSO?" });
    expect(within(dialog).getByText(/can't be undone/)).toBeDefined();
    await user.click(within(dialog).getByRole("button", { name: "Delete provider" }));

    await waitFor(() => {
      expect(providers.deleted).toEqual([AUTHENTIK_ID]);
    });
    await waitFor(() => {
      expect(screen.queryByText("Company SSO")).toBeNull();
    });
    expect(screen.getByText("Provider deleted.")).toBeDefined();
  });

  test("a provider with links is refused by the server and the dialog warns about the links up front", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: false },
    });
    const row = await rowOf("Google");

    await user.click(within(row).getByRole("button", { name: "Delete Google" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Google?" });
    expect(within(dialog).getByText(/4 linked accounts still use this provider/)).toBeDefined();
    await user.click(within(dialog).getByRole("button", { name: "Delete provider" }));

    expect(await screen.findByText(/still has linked identities/)).toBeDefined();
    expect(providers.deleted).toEqual([]);
    expect(await rowOf("Google")).toBeDefined();
  });

  test("deletion goes through recent auth and replays exactly once", async () => {
    const { user, providers } = renderProviders("/admin/providers", {
      admin: { recentAuth: true },
    });
    const row = await rowOf("Company SSO");

    await user.click(within(row).getByRole("button", { name: "Delete Company SSO" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Company SSO?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete provider" }));
    await confirmRecentAuth(user);

    await waitFor(() => {
      expect(providers.deleted).toEqual([AUTHENTIK_ID]);
    });
    expect(recentAuthStore.getState().challenge).toBeNull();
  });
});

describe("component_provider_global_toggle", () => {
  test("the global switch writes authProvidersEnabled and refreshes the public login state", async () => {
    const { user, state } = renderProviders("/admin/providers", { admin: { recentAuth: false } });
    const toggle = await screen.findByRole("switch", { name: "Allow external sign-in" });
    const bootstrap = countBootstrap();
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    const before = bootstrap.bootstrap;

    await user.click(toggle);

    await waitFor(() => {
      expect(state.settingsPatches.security).toEqual([{ authProvidersEnabled: false }]);
    });
    expect(await screen.findByText(/External sign-in is off/)).toBeDefined();
    await waitFor(() => {
      expect(bootstrap.bootstrap).toBeGreaterThan(before);
    });
  });
});
