import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { errorEnvelope, meFixture } from "../../../test/bootFixtures";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { server } from "../../../test/server";
import { ProfileForm } from "./ProfileForm";

const PROFILE_URL = "*/api/v1/profile";
const PROFILE = meFixture().user;

function capturePatch(respond: (body: Record<string, unknown>) => Response) {
  const bodies: Record<string, unknown>[] = [];
  server.use(
    http.patch(PROFILE_URL, async ({ request }) => {
      const body = (await request.json()) as Record<string, unknown>;
      bodies.push(body);
      return respond(body);
    }),
  );
  return bodies;
}

async function renderProfile() {
  await renderFeature(<ProfileForm profile={PROFILE} />);
  await screen.findByRole("heading", { level: 2, name: "Profile" });
  return userEvent.setup();
}

beforeEach(() => {
  stubMatchMedia();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("component_profile_fields_limited", () => {
  test("the self-service form can only express first and last name", async () => {
    await renderProfile();

    const inputs = screen.getAllByRole("textbox");
    expect(inputs.map((input) => input.getAttribute("autocomplete"))).toEqual([
      "given-name",
      "family-name",
    ]);
    expect(screen.getByLabelText("First name")).toHaveProperty("value", "Ada");
    expect(screen.getByLabelText("Last name")).toHaveProperty("value", "Lovelace");
    for (const label of [/e-?mail/i, /username/i, /role/i]) {
      expect(screen.queryByLabelText(label)).toBeNull();
      expect(screen.queryByRole("textbox", { name: label })).toBeNull();
      expect(screen.queryByRole("combobox", { name: label })).toBeNull();
    }
    for (const value of [PROFILE.email, PROFILE.username, PROFILE.role]) {
      expect(screen.queryByDisplayValue(value)).toBeNull();
    }
    expect(screen.getByRole("button", { name: "Save changes" })).toHaveProperty("disabled", true);
  });

  test("the PATCH body carries exactly firstName and lastName", async () => {
    const bodies = capturePatch((body) => HttpResponse.json({ ...PROFILE, ...body }));
    const user = await renderProfile();

    await user.clear(screen.getByLabelText("First name"));
    await user.type(screen.getByLabelText("First name"), "  Grace ");
    await user.clear(screen.getByLabelText("Last name"));
    await user.type(screen.getByLabelText("Last name"), "Hopper");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText("Profile saved.")).toBeDefined();
    expect(bodies).toEqual([{ firstName: "Grace", lastName: "Hopper" }]);
    expect(Object.keys(bodies[0] ?? {}).sort()).toEqual(["firstName", "lastName"]);
  });

  test("the server's version of the saved name becomes the form baseline", async () => {
    capturePatch(() => HttpResponse.json({ ...PROFILE, firstName: "Grace", lastName: "Hopper" }));
    const user = await renderProfile();

    await user.clear(screen.getByLabelText("First name"));
    await user.type(screen.getByLabelText("First name"), "grace");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(screen.getByLabelText("First name")).toHaveProperty("value", "Grace");
    });
    expect(screen.getByLabelText("Last name")).toHaveProperty("value", "Hopper");
    expect(screen.getByRole("button", { name: "Save changes" })).toHaveProperty("disabled", true);
  });

  test("client validation blocks empty names without a request", async () => {
    const bodies = capturePatch(() => HttpResponse.json(PROFILE));
    const user = await renderProfile();

    await user.clear(screen.getByLabelText("Last name"));
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText("This field is required.")).toBeDefined();
    expect(bodies).toEqual([]);
  });

  test("VALIDATION_ERROR field details map onto the named field, never the server prose", async () => {
    capturePatch(() =>
      HttpResponse.json(
        {
          error: {
            code: "VALIDATION_ERROR",
            message: "display_text rejected lastName",
            requestId: "req-profile",
            details: { fields: ["lastName"] },
          },
        },
        { status: 422, headers: { "X-Request-Id": "req-profile" } },
      ),
    );
    const user = await renderProfile();

    await user.type(screen.getByLabelText("Last name"), "!");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText("Enter a valid name.")).toBeDefined();
    expect(screen.getByLabelText("Last name").getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByLabelText("First name").getAttribute("aria-invalid")).toBe("false");
    expect(screen.queryByText(/display_text/)).toBeNull();
  });

  test("an unexpected failure shows the mapped error with its request id", async () => {
    capturePatch(() =>
      errorEnvelope("INTERNAL_ERROR", 500, "req-boom", { message: "database exploded" }),
    );
    const user = await renderProfile();

    await user.type(screen.getByLabelText("First name"), "x");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    const alert = await screen.findByRole("alert");
    expect(alert.querySelector("[data-request-id='req-boom']")).not.toBeNull();
    expect(screen.queryByText(/database exploded/)).toBeNull();
  });
});
