import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { defaultSettings } from "../../../test/adminServer";
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

async function openSmtp(options: Parameters<typeof renderAdmin>[1] = {}) {
  const harness = renderAdmin("/admin/smtp", options);
  await screen.findByTestId("smtp-settings");
  return harness;
}

const form = () => screen.getByTestId("smtp-settings");
const testPanel = () => screen.getByTestId("smtp-test");
const input = (label: string) => within(form()).getByLabelText<HTMLInputElement>(label);

describe("smtp settings", () => {
  test("renders the saved configuration and never a password", async () => {
    await openSmtp();

    expect(input("Host").value).toBe("smtp.example.test");
    expect(input("Port").value).toBe("587");
    expect(
      within(form()).getByRole("combobox", { name: "Connection security" }).closest(".ant-select")
        ?.textContent,
    ).toContain("STARTTLS");
    expect(input("Username").value).toBe("mailer");
    expect(input("Sender name").value).toBe("Palmr");
    expect(input("Sender address").value).toBe("palmr@example.test");
    expect(
      within(form()).getByRole("switch", { name: "Send e-mail through this server" }),
    ).toHaveProperty("ariaChecked", "true");
    expect(
      within(form()).getByRole("switch", { name: "Allow a self-signed certificate" }),
    ).toHaveProperty("ariaChecked", "false");
    expect(
      within(form()).getByRole("switch", { name: "The server doesn't require authentication" }),
    ).toHaveProperty("ariaChecked", "false");

    const password = input("Password");
    expect(password.value).toBe("");
    expect(password.getAttribute("type")).toBe("password");
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Password configured",
    );
  });

  test("an untouched password field omits the password from the PATCH", async () => {
    const { state, user } = await openSmtp();

    const host = input("Host");
    await user.clear(host);
    await user.type(host, "mail.example.test");
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    expect(await within(form()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.smtp).toEqual([{ host: "mail.example.test" }]);
    expect(state.settingsPatches.smtp?.[0]).not.toHaveProperty("password");
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Password configured",
    );
  });

  test("an entered password replaces the saved one and the field is blank again afterwards", async () => {
    const { state, user } = await openSmtp();

    await user.type(input("Password"), "hunter2-new");
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Will be replaced when you save",
    );
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    expect(await within(form()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.smtp).toEqual([{ password: "hunter2-new" }]);
    await waitFor(() => {
      expect(input("Password").value).toBe("");
    });
    expect(screen.queryByDisplayValue("hunter2-new")).toBeNull();
    expect(document.body.innerHTML).not.toContain("hunter2-new");
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Password configured",
    );
  });

  test("Clear is an explicit action that sends password null, and can be undone before saving", async () => {
    const { state, user } = await openSmtp();

    await user.click(within(form()).getByRole("button", { name: "Clear saved password" }));
    const dialog = await findDialog("Clear the saved password?");
    expect(state.settingsPatches.smtp).toBeUndefined();
    await user.click(within(dialog).getByRole("button", { name: "Clear password" }));
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Will be removed when you save",
    );

    await user.click(within(form()).getByRole("button", { name: "Keep saved password" }));
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Password configured",
    );
    expect(within(form()).getByRole("button", { name: "Save changes" })).toHaveProperty(
      "disabled",
      true,
    );

    await user.click(within(form()).getByRole("button", { name: "Clear saved password" }));
    const again = await findDialog("Clear the saved password?");
    await user.click(within(again).getByRole("button", { name: "Clear password" }));
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    expect(await within(form()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.smtp).toEqual([{ password: null }]);
    await waitFor(() => {
      expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
        "No password saved",
      );
    });
  });

  test("an empty password field alone never clears the saved password", async () => {
    const { state, user } = await openSmtp();

    const password = input("Password");
    await user.type(password, "abc");
    await user.clear(password);
    const save = within(form()).getByRole("button", { name: "Save changes" });
    await user.click(save);

    expect(state.settingsPatches.smtp).toBeUndefined();
    expect(within(form()).getByTestId("smtp-password-state").textContent).toBe(
      "Password configured",
    );
  });

  test("typing a new password after choosing Clear cancels the clear", async () => {
    const { state, user } = await openSmtp();

    await user.click(within(form()).getByRole("button", { name: "Clear saved password" }));
    const dialog = await findDialog("Clear the saved password?");
    await user.click(within(dialog).getByRole("button", { name: "Clear password" }));
    await user.type(input("Password"), "fresh-secret");
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(state.settingsPatches.smtp).toEqual([{ password: "fresh-secret" }]);
    });
  });

  test("clearing optional text fields sends explicit nulls and no-auth is its own flag", async () => {
    const { state, user } = await openSmtp();

    await user.clear(input("Sender name"));
    await user.click(
      within(form()).getByRole("switch", { name: "The server doesn't require authentication" }),
    );
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(state.settingsPatches.smtp).toEqual([{ fromName: null, noAuth: true }]);
    });
  });

  test("a requiredWhenEnabled rejection lands on the field named by details.key", async () => {
    const { user } = await openSmtp({
      failures: {
        "patch-smtp": () =>
          errorEnvelope("SETTING_VALUE_INVALID", 422, "req-smtp-required", {
            details: { key: "fromEmail", requiredWhenEnabled: true },
          }),
      },
    });

    await user.clear(input("Sender address"));
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));

    expect(
      await within(form()).findByText("Required while this feature is enabled."),
    ).toBeDefined();
    expect(input("Sender address").getAttribute("aria-invalid")).toBe("true");
  });

  test("a blocked save replays after recent authentication without losing the typed password", async () => {
    const { state, user } = await openSmtp({ recentAuth: true });

    await user.type(input("Password"), "replay-secret");
    await user.click(within(form()).getByRole("button", { name: "Save changes" }));
    const challenge = await findDialog("Confirm it's you");
    expect(input("Password").value).toBe("replay-secret");
    await user.type(within(challenge).getByLabelText("Password"), "correct horse");
    await user.click(within(challenge).getByRole("button", { name: "Confirm" }));

    expect(await within(form()).findByText("Settings saved.")).toBeDefined();
    expect(state.settingsPatches.smtp).toEqual([
      { password: "replay-secret" },
      { password: "replay-secret" },
    ]);
    await expectNoDialog();
  });
});

describe("smtp test", () => {
  test("testing the saved configuration omits useUnsavedSettings", async () => {
    const { state, user } = await openSmtp();

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));

    expect(await within(testPanel()).findByTestId("smtp-test-result")).toBeDefined();
    expect(state.smtpTestBodies).toEqual([{ to: "ada@example.test" }]);
    expect(state.smtpTestBodies[0]).not.toHaveProperty("useUnsavedSettings");
  });

  test("testing the form values sends them without saving and never borrows the saved password", async () => {
    const { state, user } = await openSmtp();

    const host = input("Host");
    await user.clear(host);
    await user.type(host, "draft.example.test");
    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test current form values" }));

    expect(await within(testPanel()).findByTestId("smtp-test-result")).toBeDefined();
    expect(state.smtpTestBodies).toEqual([
      {
        to: "ada@example.test",
        useUnsavedSettings: {
          host: "draft.example.test",
          port: 587,
          security: "starttls",
          fromEmail: "palmr@example.test",
          fromName: "Palmr",
          username: "mailer",
          allowSelfSignedCertificate: false,
          noAuth: false,
        },
      },
    ]);
    expect(state.settingsPatches.smtp).toBeUndefined();
    expect(state.settings.smtp.host).toBe("smtp.example.test");
    expect(within(testPanel()).getByText(/never uses the saved password/)).toBeDefined();
  });

  test("a password entered for an unsaved test is sent for that test only", async () => {
    const { state, user } = await openSmtp();

    await user.type(input("Password"), "only-for-test");
    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test current form values" }));

    await within(testPanel()).findByTestId("smtp-test-result");
    expect(state.smtpTestBodies[0]).toMatchObject({
      useUnsavedSettings: { password: "only-for-test" },
    });
    expect(state.settingsPatches.smtp).toBeUndefined();
  });

  test("a recipient is required and incomplete form values are not sent", async () => {
    const { state, user } = await openSmtp();

    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));
    expect(await within(testPanel()).findByText("This field is required.")).toBeDefined();

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.clear(input("Host"));
    await user.click(within(testPanel()).getByRole("button", { name: "Test current form values" }));
    expect(
      await within(testPanel()).findByText(
        "Enter a host, port and sender address to test the form values.",
      ),
    ).toBeDefined();
    expect(state.smtpTestBodies).toEqual([]);
  });

  test("renders the stage results the server returns, in order", async () => {
    const { user } = await openSmtp();

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));

    const result = await within(testPanel()).findByTestId("smtp-test-result");
    const stages = within(result)
      .getAllByRole("listitem")
      .map((item) => [item.getAttribute("data-stage"), item.getAttribute("data-ok")]);
    expect(stages).toEqual([
      ["connect", "true"],
      ["starttls", "true"],
      ["auth", "true"],
      ["send", "true"],
    ]);
    expect(within(result).getByText("Start TLS")).toBeDefined();
    expect(within(result).getByText("Authenticate")).toBeDefined();
    expect(within(result).getByText("Finished in 120 ms.")).toBeDefined();
    expect(
      within(result).getByText("The test e-mail was sent using the saved settings."),
    ).toBeDefined();
  });

  test("only the stages the server reports are shown", async () => {
    const settings = defaultSettings();
    settings.smtp.security = "none";
    const { user } = await openSmtp({ settings });
    await chooseOption(
      user,
      within(form()).getByRole("combobox", { name: "Connection security" }),
      "Implicit TLS",
    );
    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test current form values" }));

    const result = await within(testPanel()).findByTestId("smtp-test-result");
    expect(within(result).getAllByRole("listitem").length).toBe(4);
  });

  test("SMTP_TEST_FAILED is mapped, shows the failed stage and never the transport text", async () => {
    const { user } = await openSmtp({
      failures: {
        "smtp-test": () =>
          errorEnvelope("SMTP_TEST_FAILED", 502, "req-smtp-failed", {
            message: "535 5.7.8 Authentication failed for mailer@smtp.example.test",
            details: { stage: "auth" },
          }),
      },
    });

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));

    const failure = await within(testPanel()).findByTestId("smtp-test-failure");
    expect(
      within(failure).getByText(
        "The SMTP test failed. Check the connection details and try again.",
      ),
    ).toBeDefined();
    expect(
      within(failure).getByText("Stopped at: Authenticate (testing the saved settings)."),
    ).toBeDefined();
    expect(document.body.textContent).not.toContain("535 5.7.8");
    expect(document.body.textContent).not.toContain("mailer@smtp.example.test");
  });

  test("an unknown stage in the error details is not rendered", async () => {
    const { user } = await openSmtp({
      failures: {
        "smtp-test": () =>
          errorEnvelope("SMTP_TEST_FAILED", 502, "req-smtp-odd", {
            details: { stage: "<b>raw</b>" },
          }),
      },
    });

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));

    const failure = await within(testPanel()).findByTestId("smtp-test-failure");
    expect(within(failure).queryByText(/Stopped at/)).toBeNull();
    expect(failure.textContent).not.toContain("raw");
  });

  test("a blocked test replays after recent authentication", async () => {
    const { state, user } = await openSmtp({ recentAuth: true });

    await user.type(within(testPanel()).getByLabelText("Send the test to"), "ada@example.test");
    await user.click(within(testPanel()).getByRole("button", { name: "Test saved settings" }));
    const challenge = await findDialog("Confirm it's you");
    await user.type(within(challenge).getByLabelText("Password"), "correct horse");
    await user.click(within(challenge).getByRole("button", { name: "Confirm" }));

    expect(await within(testPanel()).findByTestId("smtp-test-result")).toBeDefined();
    expect(state.smtpTestBodies).toHaveLength(2);
  });
});
