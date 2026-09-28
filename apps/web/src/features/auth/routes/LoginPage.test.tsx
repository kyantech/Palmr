import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { errorEnvelope } from "../../../test/bootFixtures";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { LoginPage, type LoginPageProps } from "./LoginPage";

const LOGIN_URL = "*/api/v1/auth/login";
const SERVER_PROSE = "argon2 verify failed for user ada: account is locked until 12:10";

async function renderLogin(props: Partial<LoginPageProps> = {}) {
  const onSignedIn = vi.fn(() => Promise.resolve());
  await renderFeature(
    <LoginPage
      appName="Acme Files"
      passwordLoginEnabled
      providers={[]}
      onSignedIn={onSignedIn}
      {...props}
    />,
  );
  await screen.findByRole("heading", { level: 1, name: "Sign in" });
  return { onSignedIn, user: userEvent.setup() };
}

function captureLogin(respond: () => Response) {
  const bodies: unknown[] = [];
  server.use(
    http.post(LOGIN_URL, async ({ request }) => {
      bodies.push(await request.json());
      return respond();
    }),
  );
  return bodies;
}

async function signIn(user: ReturnType<typeof userEvent.setup>, identifier = "Ada@Example.test") {
  await user.type(screen.getByLabelText("E-mail or username"), identifier);
  await user.type(screen.getByLabelText("Password"), "hunter2-password");
  await user.click(screen.getByRole("button", { name: "Sign in" }));
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("component_login_maps_error_codes", () => {
  test.each([
    [
      "AUTH_INVALID_CREDENTIALS",
      401,
      {},
      "Incorrect sign-in details. Check them and try again.",
      false,
    ],
    [
      "AUTH_LOCKED",
      429,
      { "Retry-After": "600" },
      "Sign-in is temporarily locked after too many failed attempts. Try again later.",
      false,
    ],
    [
      "AUTH_PASSWORD_LOGIN_DISABLED",
      403,
      {},
      "Password sign-in is disabled on this instance.",
      false,
    ],
    [
      "VALIDATION_ERROR",
      422,
      {},
      "Some of the information entered isn't valid. Review it and try again.",
      false,
    ],
    [
      "INTERNAL_ERROR",
      500,
      {},
      "Palmr ran into an unexpected problem. Try again, and contact your administrator with the request ID if it keeps happening.",
      true,
    ],
    [
      "AUTH_2FA_REQUIRED",
      401,
      {},
      "Palmr couldn't complete this action. If the problem continues, contact your administrator and include the details below.",
      true,
    ],
  ])("%s renders its mapped message", async (code, status, headers, text, showsRequestId) => {
    const bodies = captureLogin(() =>
      errorEnvelope(code, status, "req-login", { message: SERVER_PROSE, headers }),
    );
    const { user, onSignedIn } = await renderLogin();

    await signIn(user);

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(text)).toBeDefined();
    expect(alert.textContent.includes("Request ID: req-login")).toBe(showsRequestId);
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
    expect(bodies).toEqual([{ identifier: "Ada@Example.test", password: "hunter2-password" }]);
    expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe("");
    expect(screen.getByLabelText<HTMLInputElement>("E-mail or username").value).toBe(
      "Ada@Example.test",
    );
    expect(onSignedIn).not.toHaveBeenCalled();
  });

  test("wrong identifier and wrong password are indistinguishable", async () => {
    captureLogin(() => errorEnvelope("AUTH_INVALID_CREDENTIALS", 401, "req-a"));
    const { user } = await renderLogin();
    await signIn(user, "nobody");
    const unknown = (await screen.findByRole("alert")).textContent;

    captureLogin(() => errorEnvelope("AUTH_INVALID_CREDENTIALS", 401, "req-b"));
    await user.type(screen.getByLabelText("Password"), "wrong");
    await user.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => {
      expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe("");
    });

    expect(screen.getByRole("alert").textContent).toBe(unknown);
    expect(unknown).not.toMatch(/nobody|exist|unknown|not found/i);
  });

  test("RATE_LIMITED blocks submission for Retry-After and never resubmits by itself", async () => {
    const bodies = captureLogin(() =>
      errorEnvelope("RATE_LIMITED", 429, "req-429", { headers: { "Retry-After": "30" } }),
    );
    const { user } = await renderLogin();

    await signIn(user);

    expect(
      await screen.findByText("Too many attempts. Wait a moment and try again."),
    ).toBeDefined();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Sign in" }).disabled).toBe(true);
    await user.type(screen.getByLabelText("Password"), "again{Enter}");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(bodies).toHaveLength(1);
  });

  test("empty fields are reported inline without a request", async () => {
    const bodies = captureLogin(() => new HttpResponse(null, { status: 500 }));
    const { user } = await renderLogin();

    await user.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByText("Enter your e-mail or username.")).toBeDefined();
    expect(screen.getByText("Enter your password.")).toBeDefined();
    expect(screen.getByLabelText("E-mail or username").getAttribute("aria-invalid")).toBe("true");
    expect(bodies).toEqual([]);
  });
});

describe("component_login_success_reconciles", () => {
  test("a successful login hands off to the session reconciliation and nothing else", async () => {
    captureLogin(() =>
      HttpResponse.json({
        user: { id: "u1", username: "ada", role: "admin" },
        mustChangePassword: false,
        mfaEnrollmentRequired: false,
      }),
    );
    const { user, onSignedIn } = await renderLogin();

    await signIn(user);

    await waitFor(() => {
      expect(onSignedIn).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("the identifier field is focused and uses sign-in autocomplete hints", async () => {
    await renderLogin();

    const identifier = screen.getByLabelText("E-mail or username");
    await waitFor(() => {
      expect(document.activeElement).toBe(identifier);
    });
    expect(identifier.getAttribute("autocomplete")).toBe("username");
    expect(screen.getByLabelText("Password").getAttribute("autocomplete")).toBe("current-password");
    expect(screen.getByText("Sign in to continue to Acme Files.")).toBeDefined();
  });
});

describe("component_login_providers_from_bootstrap", () => {
  const providers = [
    { slug: "zeta-oidc", displayName: "Zeta SSO", iconKey: "oidc", sortOrder: 2 },
    { slug: "github", displayName: "GitHub", iconKey: "github", sortOrder: 1 },
  ];

  test("no providers in bootstrap renders no provider area and hardcodes none", async () => {
    await renderLogin();

    expect(screen.queryByTestId("login-providers")).toBeNull();
    expect(screen.queryByText(/Continue with/)).toBeNull();
    expect(screen.queryByText("or")).toBeNull();
    const actions = screen
      .getAllByRole("button")
      .filter((button) => !button.hasAttribute("aria-pressed"));
    expect(actions.map((button) => button.textContent)).toEqual(["Sign in"]);
  });

  test("renders exactly the bootstrap providers in sortOrder, after the password form", async () => {
    await renderLogin({ providers });

    const area = screen.getByTestId("login-providers");
    expect(
      within(area)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Continue with GitHub", "Continue with Zeta SSO"]);
    expect(screen.getByText("or")).toBeDefined();
    expect(screen.queryByRole("link")).toBeNull();
  });

  test("provider selection is a seam for the provider flow and invokes only the supplied handler", async () => {
    const onProviderSelect = vi.fn();
    const { user } = await renderLogin({ providers, onProviderSelect });

    await user.click(screen.getByRole("button", { name: "Continue with GitHub" }));

    expect(onProviderSelect).toHaveBeenCalledWith("github");
  });

  test("with password login disabled only the providers remain", async () => {
    await renderLogin({ passwordLoginEnabled: false, providers });

    expect(screen.queryByLabelText("Password")).toBeNull();
    expect(screen.queryByText("or")).toBeNull();
    expect(screen.getAllByRole("button")).toHaveLength(2);
  });

  test("with neither method available the page explains it instead of rendering an empty form", async () => {
    await renderLogin({ passwordLoginEnabled: false });

    expect(screen.queryByLabelText("Password")).toBeNull();
    expect(screen.getByText("Sign-in is unavailable")).toBeDefined();
  });
});

test("component_login_has_no_registration_surface", async () => {
  await renderLogin({
    providers: [{ slug: "google", displayName: "Google", iconKey: "google", sortOrder: 1 }],
  });

  expect(screen.queryAllByRole("link")).toHaveLength(0);
  expect(document.body.textContent).not.toMatch(/register|sign up|create (an )?account/i);
});
