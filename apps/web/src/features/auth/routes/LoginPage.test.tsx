import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { errorEnvelope } from "../../../test/bootFixtures";
import { clearLoginNotice, clearMfaChallenge, mfaChallengeStore, setLoginNotice } from "../store";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { LoginPage, type LoginPageProps } from "./LoginPage";

const LOGIN_URL = "*/api/v1/auth/login";
const FORGOT_URL = "*/api/v1/auth/password/forgot";
const MFA_TOKEN = "mfa-token-7f3c9a1e-only-in-memory";
const SERVER_PROSE = "argon2 verify failed for user ada: account is locked until 12:10";

async function renderLogin(props: Partial<LoginPageProps> = {}) {
  const onSignedIn = vi.fn(() => Promise.resolve());
  const onMfaRequired = vi.fn();
  await renderFeature(
    <LoginPage
      appName="Acme Files"
      passwordLoginEnabled
      providers={[]}
      onSignedIn={onSignedIn}
      onMfaRequired={onMfaRequired}
      {...props}
    />,
  );
  await screen.findByRole("heading", {
    level: 1,
    name: props.initialMode === "forgot" ? "Reset your password" : "Sign in",
  });
  return { onSignedIn, onMfaRequired, user: userEvent.setup() };
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
  clearMfaChallenge();
  clearLoginNotice();
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
    expect(actions.map((button) => button.textContent)).toEqual(["Forgot password?", "Sign in"]);
  });

  test("renders exactly the bootstrap providers in the server-provided order, after the password form", async () => {
    await renderLogin({ providers });

    const area = screen.getByTestId("login-providers");
    expect(
      within(area)
        .getAllByRole("button")
        .map((button) => button.textContent),
    ).toEqual(["Continue with Zeta SSO", "Continue with GitHub"]);
    expect(screen.getByText("or")).toBeDefined();
    expect(screen.queryByRole("link")).toBeNull();
  });

  test("each provider shows a bundled icon chosen by iconKey and falls back for unknown keys", async () => {
    await renderLogin({ providers });

    const area = screen.getByTestId("login-providers");
    const github = within(area).getByRole("button", { name: "Continue with GitHub" });
    const unknown = within(area).getByRole("button", { name: "Continue with Zeta SSO" });
    expect(github.getAttribute("data-icon-key")).toBe("github");
    expect(unknown.getAttribute("data-icon-key")).toBe("oidc");
    for (const button of [github, unknown]) {
      expect(button.querySelectorAll("svg[aria-hidden='true']")).toHaveLength(1);
      expect(button.querySelector("img, [style*='url(']")).toBeNull();
    }
    expect(github.innerHTML).not.toBe(unknown.innerHTML);
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

function mfaChallengeResponse() {
  return HttpResponse.json(
    {
      error: {
        code: "AUTH_2FA_REQUIRED",
        message: "second factor required",
        requestId: "req-mfa",
        details: {
          mfaToken: MFA_TOKEN,
          expiresAt: new Date(Date.now() + 300_000).toISOString(),
          methods: ["totp", "backup_code"],
          trustedDeviceOffered: true,
        },
      },
    },
    { status: 401, headers: { "X-Request-Id": "req-mfa" } },
  );
}

describe("component_login_hands_mfa_challenge_to_memory", () => {
  test("AUTH_2FA_REQUIRED stores the challenge in memory and asks for the 2FA step instead of failing", async () => {
    const bodies = captureLogin(mfaChallengeResponse);
    const { user, onSignedIn, onMfaRequired } = await renderLogin();

    await signIn(user);

    await waitFor(() => {
      expect(onMfaRequired).toHaveBeenCalledTimes(1);
    });
    expect(onSignedIn).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(bodies).toEqual([{ identifier: "Ada@Example.test", password: "hunter2-password" }]);
    expect(mfaChallengeStore.getState().challenge).toMatchObject({
      mfaToken: MFA_TOKEN,
      methods: ["totp", "backup_code"],
      trustedDeviceOffered: true,
    });
    expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe("");
    expect(document.body.innerHTML).not.toContain(MFA_TOKEN);
  });

  test("a new password attempt replaces any earlier challenge before the request is sent", async () => {
    mfaChallengeStore.setState({
      challenge: {
        mfaToken: "stale-token",
        expiresAt: "2026-09-28T00:05:00Z",
        methods: ["totp"],
        trustedDeviceOffered: false,
        deadline: Date.now() + 60_000,
      },
    });
    let seenDuringRequest: unknown = "unset";
    server.use(
      http.post(LOGIN_URL, () => {
        seenDuringRequest = mfaChallengeStore.getState().challenge;
        return errorEnvelope("AUTH_INVALID_CREDENTIALS", 401, "req-bad");
      }),
    );
    const { user } = await renderLogin();

    await signIn(user);

    expect(await screen.findByRole("alert")).toBeDefined();
    expect(seenDuringRequest).toBeNull();
    expect(mfaChallengeStore.getState().challenge).toBeNull();
  });

  test("a notice from a finished flow is shown once and cleared by the next attempt", async () => {
    setLoginNotice("passwordReset");
    captureLogin(() => errorEnvelope("AUTH_INVALID_CREDENTIALS", 401, "req-bad"));
    const { user } = await renderLogin();

    expect(screen.getByTestId("login-notice").textContent).toContain("Your password was reset");
    await signIn(user);

    await waitFor(() => {
      expect(screen.queryByTestId("login-notice")).toBeNull();
    });
  });
});

describe("component_forgot_password_is_non_enumerating", () => {
  async function requestReset(respond: () => Response, identifier: string) {
    const bodies = captureForgot(respond);
    const { user } = await renderLogin();
    await user.click(screen.getByRole("button", { name: "Forgot password?" }));
    await screen.findByRole("heading", { level: 1, name: "Reset your password" });
    await user.type(screen.getByLabelText("E-mail or username"), identifier);
    await user.click(screen.getByRole("button", { name: "Send reset instructions" }));
    return { bodies, user };
  }

  function captureForgot(respond: () => Response) {
    const bodies: unknown[] = [];
    server.use(
      http.post(FORGOT_URL, async ({ request }) => {
        bodies.push(await request.json());
        return respond();
      }),
    );
    return bodies;
  }

  test("the request body is only the identifier and the answer is the same generic state for any input", async () => {
    const accepted = () => HttpResponse.json({ accepted: true }, { status: 202 });
    const first = await requestReset(accepted, "ada@example.test");
    const shown = (await screen.findByTestId("forgot-password-sent")).closest("div")?.textContent;
    const heading = screen.getByRole("heading", { level: 1 }).textContent;
    expect(first.bodies).toEqual([{ identifier: "ada@example.test" }]);
    expect(heading).toBe("Request received");
    expect(document.body.textContent).toContain(
      "If an eligible account matches the information you entered",
    );
    expect(document.body.textContent).not.toMatch(/ada@example\.test|we sent|was sent to/i);

    await first.user.click(screen.getByRole("button", { name: "Back to sign in" }));
    await screen.findByRole("heading", { level: 1, name: "Sign in" });
    await first.user.click(screen.getByRole("button", { name: "Forgot password?" }));
    await first.user.type(screen.getByLabelText("E-mail or username"), "nobody-at-all");
    await first.user.click(screen.getByRole("button", { name: "Send reset instructions" }));

    expect((await screen.findByTestId("forgot-password-sent")).closest("div")?.textContent).toBe(
      shown,
    );
    expect(first.bodies).toEqual([
      { identifier: "ada@example.test" },
      { identifier: "nobody-at-all" },
    ]);
    expect(Object.keys(first.bodies[1] as object)).toEqual(["identifier"]);
  });

  test("FEATURE_UNAVAILABLE_SMTP is the one instance-wide failure it reports", async () => {
    await requestReset(
      () => errorEnvelope("FEATURE_UNAVAILABLE_SMTP", 409, "req-smtp", { message: SERVER_PROSE }),
      "ada",
    );

    expect(
      await screen.findByText(
        "E-mail isn't configured on this instance, so this action isn't available. Contact your administrator.",
      ),
    ).toBeDefined();
    expect(screen.queryByTestId("forgot-password-sent")).toBeNull();
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
  });

  test("RATE_LIMITED blocks resubmission without auto-retrying", async () => {
    const { bodies } = await requestReset(
      () => errorEnvelope("RATE_LIMITED", 429, "req-429", { headers: { "Retry-After": "30" } }),
      "ada",
    );

    expect(
      await screen.findByText("Too many attempts. Wait a moment and try again."),
    ).toBeDefined();
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Send reset instructions" }).disabled,
    ).toBe(true);
    expect(bodies).toHaveLength(1);
  });

  test("the reset page can open the login experience directly in forgot mode", async () => {
    await renderLogin({ initialMode: "forgot" });

    expect(screen.getByLabelText("E-mail or username")).toBeDefined();
    expect(screen.queryByLabelText("Password")).toBeNull();
  });
});
