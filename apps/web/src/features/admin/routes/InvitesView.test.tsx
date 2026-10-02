import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { defaultSettings, inviteFixture } from "../../../test/adminServer";
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

function smtpDisabled() {
  const settings = defaultSettings();
  settings.smtp.enabled = false;
  return settings;
}

describe("invites view", () => {
  test("the Users / Invites selector lives in the URL and lists invites without tokens", async () => {
    const { router, user } = renderAdmin("/admin/users");
    await screen.findByRole("link", { name: "Grace Hopper" });

    await user.click(screen.getByText("Invites", { selector: ".ant-segmented-item-label" }));

    await waitFor(() => {
      expect(router.state.location.search).toBe("?view=invites");
    });
    expect(router.state.location.pathname).toBe("/admin/users");
    expect(await screen.findByText("invitee@example.test")).toBeDefined();
    expect(screen.getByText("used@example.test")).toBeDefined();
    const statuses = screen.getAllByTestId("invite-status").map((tag) => tag.textContent);
    expect(statuses).toEqual(["Pending", "Accepted"]);
    expect(document.body.textContent).not.toMatch(/token/i);
  });

  test("a deep link opens the Invites view and the status filter updates the URL and the request", async () => {
    const { router, state, user } = renderAdmin("/admin/users?view=invites");
    await screen.findByText("invitee@example.test");

    await chooseOption(
      user,
      screen.getByRole("combobox", { name: "Filter invites by status" }),
      "Accepted",
    );

    await waitFor(() => {
      expect(router.state.location.search).toBe("?view=invites&status=accepted");
    });
    await waitFor(() => {
      expect(state.inviteListQueries.at(-1)).toContain("status=accepted");
    });
    expect(await screen.findByText("used@example.test")).toBeDefined();
    await waitFor(() => {
      expect(screen.queryByText("invitee@example.test")).toBeNull();
    });
  });

  test("an invite created without e-mail works when SMTP is unavailable and shows the link once", async () => {
    const { state, queryClient, user } = renderAdmin("/admin/users?view=invites", {
      settings: smtpDisabled(),
    });
    await screen.findByText("invitee@example.test");

    await user.click(screen.getByRole("button", { name: "Invite user" }));
    const dialog = await findDialog("Invite a user");
    const sendEmail = within(dialog).getByRole("switch", { name: "Send the invite by e-mail" });
    expect(sendEmail.getAttribute("aria-checked")).toBe("false");
    expect(
      within(dialog).getByText("E-mail isn't configured, so you'll get a link to share yourself."),
    ).toBeDefined();
    await user.type(within(dialog).getByLabelText("E-mail address"), "new@example.test");
    await user.click(within(dialog).getByRole("button", { name: "Create invite" }));

    await waitFor(() => {
      expect(state.inviteBodies).toHaveLength(1);
    });
    expect(state.inviteBodies[0]).toEqual({
      email: "new@example.test",
      role: "user",
      sendEmail: false,
    });
    const result = await findDialog("Invite link");
    const field = within(result).getByLabelText<HTMLInputElement>("Invite link");
    expect(field.value).toMatch(/^https:\/\/palmr\.example\.test\/invite\/token-/);
    expect(within(result).getByText(/will not be shown again/)).toBeDefined();
    const link = field.value;

    expect(
      JSON.stringify(
        queryClient
          .getQueryCache()
          .getAll()
          .map((query) => query.state.data),
      ),
    ).not.toContain(link);
    expect(
      JSON.stringify(
        queryClient
          .getMutationCache()
          .getAll()
          .map((mutation) => mutation.state.data),
      ),
    ).not.toContain(link);
    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);

    await user.click(within(result).getByRole("button", { name: "Done" }));
    await expectNoDialog();
    expect(document.body.textContent).not.toContain(link);
    expect(screen.getByText("new@example.test")).toBeDefined();
    expect(window.localStorage.length).toBe(0);
  });

  test("the invite link has an explicit Copy action", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const { user } = renderAdmin("/admin/users?view=invites", { settings: smtpDisabled() });
    await screen.findByText("invitee@example.test");
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });

    await user.click(screen.getByRole("button", { name: "Invite user" }));
    const dialog = await findDialog("Invite a user");
    await user.type(within(dialog).getByLabelText("E-mail address"), "copy@example.test");
    await user.click(within(dialog).getByRole("button", { name: "Create invite" }));
    const result = await findDialog("Invite link");
    expect(writeText).not.toHaveBeenCalled();

    await user.click(within(result).getByRole("button", { name: "Copy" }));

    expect(writeText).toHaveBeenCalledTimes(1);
    expect(String(writeText.mock.calls[0]?.[0])).toMatch(
      /^https:\/\/palmr\.example\.test\/invite\//,
    );
    expect(await within(result).findByText("Copied to the clipboard.")).toBeDefined();
  });

  test("sending by e-mail is on by default when SMTP is enabled and a missing SMTP surfaces the stable error", async () => {
    const { state, user } = renderAdmin("/admin/users?view=invites", {
      failures: {
        "create-invite": () => errorEnvelope("FEATURE_UNAVAILABLE_SMTP", 409, "req-smtp-create"),
      },
    });
    await screen.findByText("invitee@example.test");

    await user.click(screen.getByRole("button", { name: "Invite user" }));
    const dialog = await findDialog("Invite a user");
    expect(
      within(dialog)
        .getByRole("switch", { name: "Send the invite by e-mail" })
        .getAttribute("aria-checked"),
    ).toBe("true");
    await user.type(within(dialog).getByLabelText("E-mail address"), "mail@example.test");
    await user.click(within(dialog).getByRole("button", { name: "Create invite" }));

    expect(
      await within(dialog).findByText(/E-mail isn't configured on this instance/),
    ).toBeDefined();
    expect(state.inviteBodies[0]).toMatchObject({ sendEmail: true });
    expect(screen.queryByText("Invite link")).toBeNull();
  });

  test("a duplicate invite address is mapped onto the e-mail field", async () => {
    const { user } = renderAdmin("/admin/users?view=invites", {
      failures: { "create-invite": () => errorEnvelope("USER_EMAIL_TAKEN", 409, "req-taken") },
    });
    await screen.findByText("invitee@example.test");
    await user.click(screen.getByRole("button", { name: "Invite user" }));
    const dialog = await findDialog("Invite a user");
    await user.type(within(dialog).getByLabelText("E-mail address"), "grace@example.test");
    await user.click(within(dialog).getByRole("button", { name: "Create invite" }));

    expect(await within(dialog).findByText("This e-mail address is already in use.")).toBeDefined();
  });

  test("resend needs SMTP and reports the stable error when it is missing", async () => {
    const { user } = renderAdmin("/admin/users?view=invites", { settings: smtpDisabled() });
    await screen.findByText("invitee@example.test");

    await user.click(screen.getByRole("button", { name: "Resend" }));

    expect(await screen.findByText(/E-mail isn't configured on this instance/)).toBeDefined();
  });

  test("resend queues the e-mail and revoke asks for confirmation first", async () => {
    const { state, user } = renderAdmin("/admin/users?view=invites");
    await screen.findByText("invitee@example.test");

    await user.click(screen.getByRole("button", { name: "Resend" }));
    expect(await screen.findByText("The invite e-mail was queued for delivery.")).toBeDefined();
    expect(state.actions).toEqual(["invite-resend"]);

    await user.click(screen.getByRole("button", { name: "Revoke" }));
    const dialog = await findDialog("Revoke this invite?");
    expect(state.actions).toEqual(["invite-resend"]);
    await user.click(within(dialog).getByRole("button", { name: "Revoke invite" }));

    expect(await screen.findByText("The invite was revoked.")).toBeDefined();
    expect(state.actions).toEqual(["invite-resend", "invite-revoke"]);
    await waitFor(() => {
      expect(screen.getAllByTestId("invite-status").map((tag) => tag.textContent)).toEqual([
        "Revoked",
        "Accepted",
      ]);
    });
    expect(screen.queryByRole("button", { name: "Revoke" })).toBeNull();
  });

  test("only pending invites offer Resend and Revoke", async () => {
    renderAdmin("/admin/users?view=invites", {
      invites: [
        inviteFixture({ id: "019a0000-0000-7000-8000-000000000a01", status: "expired" }),
        inviteFixture({
          id: "019a0000-0000-7000-8000-000000000a02",
          email: "late@example.test",
          status: "revoked",
        }),
      ],
    });
    await screen.findAllByTestId("invite-status");

    expect(screen.queryByRole("button", { name: "Resend" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Revoke" })).toBeNull();
  });
});
