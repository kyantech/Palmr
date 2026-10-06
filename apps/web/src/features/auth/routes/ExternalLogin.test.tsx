import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { errorEnvelope } from "../../../test/bootFixtures";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { externalNavigation } from "../externalNavigation";
import { LoginPage, type LoginPageProps } from "./LoginPage";

const AUTHORIZE_URL = "*/api/v1/auth/providers/:slug/authorize";
const PROVIDER_LIST_URL = "*/api/v1/auth/providers";
const SERVER_URL = "https://idp.example.test/authorize?response_type=code&state=server-issued";
const SERVER_PROSE = "upstream said: user ada is not allowed because of rule 7";

const PROVIDERS = [
  { slug: "authentik", displayName: "Company SSO", iconKey: "authentik", sortOrder: 1 },
  { slug: "github", displayName: "GitHub", iconKey: "github", sortOrder: 2 },
];

async function renderLogin(props: Partial<LoginPageProps> = {}) {
  await renderFeature(
    <LoginPage
      appName="Acme Files"
      passwordLoginEnabled
      providers={PROVIDERS}
      onSignedIn={() => Promise.resolve()}
      onMfaRequired={() => undefined}
      {...props}
    />,
  );
  await screen.findByRole("heading", { level: 1, name: "Sign in" });
  return userEvent.setup();
}

interface Captured {
  bodies: unknown[];
  slugs: string[];
}

function captureAuthorize(respond: () => Response | Promise<Response>): Captured {
  const captured: Captured = { bodies: [], slugs: [] };
  server.use(
    http.post(AUTHORIZE_URL, async ({ request, params }) => {
      captured.bodies.push(await request.json());
      captured.slugs.push(String(params.slug));
      return respond();
    }),
  );
  return captured;
}

let assign: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  stubMatchMedia();
  assign = vi.spyOn(externalNavigation, "assign").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("component_login_external_start", () => {
  test("provider buttons come from the bootstrap props and no provider list is requested", async () => {
    const listed = vi.fn();
    server.use(
      http.get(PROVIDER_LIST_URL, () => {
        listed();
        return HttpResponse.json({ providers: [] });
      }),
    );
    await renderLogin();

    const buttons = within(screen.getByTestId("login-providers")).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      "Continue with Company SSO",
      "Continue with GitHub",
    ]);
    expect(listed).not.toHaveBeenCalled();
  });

  test("clicking a provider starts a login at the server and navigates to the URL it returns", async () => {
    const captured = captureAuthorize(() => HttpResponse.json({ authorizationUrl: SERVER_URL }));
    const user = await renderLogin();

    await user.click(screen.getByRole("button", { name: "Continue with GitHub" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledTimes(1);
    });
    expect(assign).toHaveBeenCalledWith(SERVER_URL);
    expect(captured.slugs).toEqual(["github"]);
    expect(captured.bodies).toEqual([{ purpose: "login" }]);
  });

  test("the sanitized next path is sent as returnTo and nothing else about OAuth is generated client-side", async () => {
    const captured = captureAuthorize(() => HttpResponse.json({ authorizationUrl: SERVER_URL }));
    const user = await renderLogin({ returnTo: "/settings/security?tab=1" });

    await user.click(screen.getByRole("button", { name: "Continue with Company SSO" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledTimes(1);
    });
    expect(captured.bodies).toEqual([{ purpose: "login", returnTo: "/settings/security?tab=1" }]);
    const body = JSON.stringify(captured.bodies);
    for (const forbidden of ["state", "nonce", "code_challenge", "codeChallenge", "redirect"]) {
      expect(body).not.toContain(forbidden);
    }
  });

  test("a pending start cannot be repeated and the other providers are held back", async () => {
    const captured = captureAuthorize(async () => {
      await delay(80);
      return HttpResponse.json({ authorizationUrl: SERVER_URL });
    });
    const user = await renderLogin();

    const github = screen.getByRole("button", { name: /Continue with GitHub/ });
    await user.dblClick(github);
    await user.click(screen.getByRole("button", { name: "Continue with Company SSO" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledTimes(1);
    });
    expect(captured.slugs).toEqual(["github"]);
    const area = screen.getByTestId("login-providers");
    for (const button of within(area).getAllByRole("button")) {
      expect(button.hasAttribute("disabled") || button.classList.contains("ant-btn-loading")).toBe(
        true,
      );
    }
    await user.click(within(area).getByRole("button", { name: /Continue with GitHub/ }));
    expect(captured.slugs).toEqual(["github"]);
  });

  test("a refusal is shown by its code and nothing is navigated", async () => {
    captureAuthorize(() =>
      errorEnvelope("PROVIDER_DISABLED", 403, "req-disabled", { message: SERVER_PROSE }),
    );
    const user = await renderLogin();

    await user.click(screen.getByRole("button", { name: "Continue with GitHub" }));

    expect(await screen.findByText("This identity provider is turned off.")).toBeDefined();
    expect(screen.queryByText(SERVER_PROSE)).toBeNull();
    expect(assign).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "Continue with GitHub" }).hasAttribute("disabled"),
    ).toBe(false);
  });

  test.each(["javascript:alert(1)", "data:text/html,boom", "//evil.example/authorize", ""])(
    "a non-http(s) authorization URL (%s) is refused and never navigated to",
    async (authorizationUrl) => {
      captureAuthorize(() => HttpResponse.json({ authorizationUrl }));
      const user = await renderLogin();

      await user.click(screen.getByRole("button", { name: "Continue with GitHub" }));

      expect(
        await screen.findByText(/The server sent a response Palmr didn't expect/),
      ).toBeDefined();
      expect(assign).not.toHaveBeenCalled();
    },
  );

  test("with password login disabled the provider buttons still start a login", async () => {
    captureAuthorize(() => HttpResponse.json({ authorizationUrl: SERVER_URL }));
    const user = await renderLogin({ passwordLoginEnabled: false });

    expect(screen.queryByLabelText("Password")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Continue with Company SSO" }));

    await waitFor(() => {
      expect(assign).toHaveBeenCalledWith(SERVER_URL);
    });
  });
});

describe("component_login_callback_error", () => {
  test("a known code renders its localized message and the real request id, never any provider text", async () => {
    await renderLogin({
      callbackError: { code: "PROVIDER_EMAIL_UNVERIFIED", requestId: "req-7f3a" },
    });

    const alert = screen.getByTestId("login-callback-error");
    expect(
      within(alert).getByText(/The provider didn't confirm your e-mail address/),
    ).toBeDefined();
    expect(within(alert).getByText("Request ID: req-7f3a")).toBeDefined();
    expect(alert.querySelector("[data-error-code]")).toBeNull();
  });

  test("an unknown code uses the generic message with the raw code as diagnostics", async () => {
    await renderLogin({ callbackError: { code: "PROVIDER_FUTURE_CODE", requestId: "req-9" } });

    const alert = screen.getByTestId("login-callback-error");
    expect(within(alert).getByText(/Palmr couldn't complete this action/)).toBeDefined();
    expect(within(alert).getByText("Error code: PROVIDER_FUTURE_CODE")).toBeDefined();
    expect(within(alert).getByText("Request ID: req-9")).toBeDefined();
  });

  test("a missing request id renders the message without a request id line", async () => {
    await renderLogin({ callbackError: { code: "PROVIDER_STATE_INVALID", requestId: null } });

    const alert = screen.getByTestId("login-callback-error");
    expect(within(alert).getByText(/expired or was already used/)).toBeDefined();
    expect(within(alert).queryByText(/Request ID/)).toBeNull();
  });

  test("no callback error renders no callback surface", async () => {
    await renderLogin();

    expect(screen.queryByTestId("login-callback-error")).toBeNull();
  });
});
