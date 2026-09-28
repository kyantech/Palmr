import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { errorEnvelope } from "../../../test/bootFixtures";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { SetupPage } from "./SetupPage";

const STATUS_URL = "*/api/v1/setup/status";
const SETUP_URL = "*/api/v1/setup";
const LOCALES = ["de-DE", "en-US", "pt-BR"];
const SERVER_PROSE = "UNIQUE constraint failed: users.username_normalized";

const ADMIN = {
  firstName: "Ada",
  lastName: "Lovelace",
  username: "ada",
  email: "ada@example.test",
  password: "correct horse battery",
};

interface StatusBody {
  setupCompleted: boolean;
  passwordMinLength?: number;
}

function serveStatus(body: StatusBody = { setupCompleted: false, passwordMinLength: 8 }) {
  server.use(http.get(STATUS_URL, () => HttpResponse.json(body)));
}

function captureSetup(respond: () => Response) {
  const bodies: unknown[] = [];
  server.use(
    http.post(SETUP_URL, async ({ request }) => {
      bodies.push(await request.json());
      return respond();
    }),
  );
  return bodies;
}

async function renderSetup({
  defaultLocale = "en-US",
  onSetupFinished = vi.fn(() => Promise.resolve()),
}: { defaultLocale?: string; onSetupFinished?: () => Promise<void> } = {}) {
  await renderFeature(
    <SetupPage
      defaultLocale={defaultLocale}
      supportedLocales={LOCALES}
      onSetupFinished={onSetupFinished}
    />,
  );
  await screen.findByRole("heading", { level: 1, name: "Set up your instance" });
  return { onSetupFinished, user: userEvent.setup() };
}

async function fillAdmin(user: ReturnType<typeof userEvent.setup>, values = ADMIN) {
  await user.type(screen.getByLabelText("First name"), values.firstName);
  await user.type(screen.getByLabelText("Last name"), values.lastName);
  await user.type(screen.getByLabelText("Username"), values.username);
  await user.type(screen.getByLabelText("E-mail"), values.email);
  await user.type(screen.getByLabelText("Password"), values.password);
}

async function submitButton() {
  const button = screen.getByRole<HTMLButtonElement>("button", { name: "Complete setup" });
  await waitFor(() => {
    expect(button.disabled).toBe(false);
  });
  return button;
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("component_setup_form_prefills_palmr", () => {
  test("appName is pre-filled with exactly Palmr and the form carries only the setup fields", async () => {
    serveStatus();
    await renderSetup();

    expect(screen.getByLabelText<HTMLInputElement>("Instance name").value).toBe("Palmr");
    for (const label of ["Language", "First name", "Last name", "Username", "E-mail", "Password"]) {
      expect(screen.getByLabelText(label)).toBeDefined();
    }
    expect(document.querySelectorAll("form input")).toHaveLength(7);
    expect(screen.queryByLabelText(/smtp|description|token|confirm/i)).toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
  });

  test("the locale defaults to the bootstrap suggestion and the request uses the accepted shape", async () => {
    serveStatus();
    const bodies = captureSetup(() =>
      HttpResponse.json(
        {
          user: { id: "u1", username: "ada", email: "ada@example.test", role: "admin" },
          mustChangePassword: false,
        },
        { status: 201 },
      ),
    );
    const { user, onSetupFinished } = await renderSetup({ defaultLocale: "pt-BR" });

    expect(screen.getByText("Português (Brasil)")).toBeDefined();
    await fillAdmin(user);
    await user.click(await submitButton());

    await waitFor(() => {
      expect(onSetupFinished).toHaveBeenCalledTimes(1);
    });
    expect(bodies).toEqual([{ appName: "Palmr", ...ADMIN, locale: "pt-BR" }]);
  });
});

describe("component_setup_password_policy_from_setup_status", () => {
  test("the hint and the client check follow passwordMinLength from /setup/status", async () => {
    server.use(
      http.get(STATUS_URL, async () => {
        await delay(30);
        return HttpResponse.json({ setupCompleted: false, passwordMinLength: 12 });
      }),
    );
    const bodies = captureSetup(() => new HttpResponse(null, { status: 500 }));
    const { user } = await renderSetup();

    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Complete setup" }).disabled).toBe(
      true,
    );
    expect(await screen.findByText("Use at least 12 characters.")).toBeDefined();

    await fillAdmin(user, { ...ADMIN, password: "short-pass" });
    await user.click(await submitButton());

    const password = screen.getByLabelText("Password");
    await waitFor(() => {
      expect(password.getAttribute("aria-invalid")).toBe("true");
    });
    expect(screen.getAllByText("Use at least 12 characters.")).toHaveLength(2);
    expect(bodies).toEqual([]);
  });

  test("a failed status request shows the generic mapped error with its request id and can be retried", async () => {
    let calls = 0;
    server.use(
      http.get(STATUS_URL, () => {
        calls += 1;
        return calls === 1
          ? errorEnvelope("SETUP_STATUS_FROM_THE_FUTURE", 500, "req-status")
          : HttpResponse.json({ setupCompleted: false, passwordMinLength: 10 });
      }),
    );
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user } = await renderSetup();

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Palmr couldn't complete this action.");
    expect(alert.textContent).toContain("Error code: SETUP_STATUS_FROM_THE_FUTURE");
    expect(alert.textContent).toContain("Request ID: req-status");
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByText("Use at least 10 characters.")).toBeDefined();
    await submitButton();
  });

  test("a status that already reports completion reconciles instead of offering the form", async () => {
    serveStatus({ setupCompleted: true });
    const { onSetupFinished } = await renderSetup();

    await waitFor(() => {
      expect(onSetupFinished).toHaveBeenCalledTimes(1);
    });
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Complete setup" }).disabled).toBe(
      true,
    );
  });
});

describe("component_setup_maps_error_codes", () => {
  test.each([
    ["USER_USERNAME_TAKEN", 409, "Username", "This username is already in use."],
    ["USER_EMAIL_TAKEN", 409, "E-mail", "This e-mail address is already in use."],
    [
      "PASSWORD_POLICY_VIOLATION",
      422,
      "Password",
      "The password doesn't meet this instance's requirements.",
    ],
  ])("%s marks the %s field with the mapped message", async (code, status, label, text) => {
    serveStatus();
    captureSetup(() => errorEnvelope(code, status, "req-field", { message: SERVER_PROSE }));
    const { user, onSetupFinished } = await renderSetup();

    await fillAdmin(user);
    await user.click(await submitButton());

    const field = screen.getByLabelText(label);
    await waitFor(() => {
      expect(field.getAttribute("aria-invalid")).toBe("true");
    });
    const help = document.getElementById(
      field.getAttribute("aria-describedby")?.split(" ")[0] ?? "",
    );
    expect(help?.textContent).toBe(text);
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
    expect(onSetupFinished).not.toHaveBeenCalled();
  });

  test("VALIDATION_ERROR marks every field named in details.fields", async () => {
    serveStatus();
    captureSetup(() =>
      HttpResponse.json(
        {
          error: {
            code: "VALIDATION_ERROR",
            message: SERVER_PROSE,
            requestId: "req-422",
            details: { fields: ["email", "locale", "notAField"] },
          },
        },
        { status: 422, headers: { "X-Request-Id": "req-422" } },
      ),
    );
    const { user } = await renderSetup();

    await fillAdmin(user);
    await user.click(await submitButton());

    await waitFor(() => {
      expect(screen.getByLabelText("E-mail").getAttribute("aria-invalid")).toBe("true");
    });
    expect(screen.getByLabelText("Language").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByLabelText("Username").getAttribute("aria-invalid")).toBe("false");
    expect(screen.getAllByText("Check this value.")).toHaveLength(2);
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
  });

  test("an unexpected failure shows the generic mapped alert with the request id and keeps the input", async () => {
    serveStatus();
    captureSetup(() => errorEnvelope("INTERNAL_ERROR", 500, "req-boom", { message: SERVER_PROSE }));
    const { user } = await renderSetup();

    await fillAdmin(user);
    await user.click(await submitButton());

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Palmr ran into an unexpected problem/)).toBeDefined();
    expect(alert.textContent).toContain("Request ID: req-boom");
    expect(document.body.textContent).not.toContain(SERVER_PROSE);
    expect(screen.getByLabelText<HTMLInputElement>("Password").value).toBe(ADMIN.password);
  });

  test("SETUP_ALREADY_COMPLETED hands control back to routing instead of retrying setup", async () => {
    serveStatus();
    const bodies = captureSetup(() => errorEnvelope("SETUP_ALREADY_COMPLETED", 409, "req-409"));
    const { user, onSetupFinished } = await renderSetup();

    await fillAdmin(user);
    await user.click(await submitButton());

    await waitFor(() => {
      expect(onSetupFinished).toHaveBeenCalledTimes(1);
    });
    expect(await screen.findByText("This Palmr instance has already been set up.")).toBeDefined();
    expect(bodies).toHaveLength(1);
  });
});
