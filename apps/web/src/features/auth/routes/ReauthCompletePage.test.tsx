import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import { ReauthCompletePage } from "./ReauthCompletePage";

interface FakeOpener {
  closed: boolean;
  postMessage: ReturnType<typeof vi.fn>;
}

function setOpener(opener: FakeOpener | null) {
  Object.defineProperty(window, "opener", { value: opener, configurable: true, writable: true });
}

async function renderCompletion(search: string) {
  const onContinue = vi.fn();
  await renderFeature(<ReauthCompletePage search={search} onContinue={onContinue} />);
  await screen.findByTestId("reauth-complete");
  return { onContinue, user: userEvent.setup() };
}

let close: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  stubMatchMedia();
  close = vi.spyOn(window, "close").mockImplementation(() => undefined);
});

afterEach(() => {
  setOpener(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("component_reauth_complete_with_opener", () => {
  test("success posts exactly the minimal success message to the same origin and closes the popup", async () => {
    const opener: FakeOpener = { closed: false, postMessage: vi.fn() };
    setOpener(opener);

    await renderCompletion("?status=success");

    expect(opener.postMessage).toHaveBeenCalledTimes(1);
    expect(opener.postMessage).toHaveBeenCalledWith(
      { type: "palmr:external-reauth", status: "success" },
      window.location.origin,
    );
    expect(opener.postMessage.mock.calls[0]?.[1]).not.toBe("*");
    expect(close).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("reauth-complete").getAttribute("data-outcome")).toBe("success");
  });

  test("an error posts the code and the callback request id and shows them in the popup", async () => {
    const opener: FakeOpener = { closed: false, postMessage: vi.fn() };
    setOpener(opener);

    await renderCompletion("?status=error&error=PROVIDER_AUTH_DENIED&requestId=req-8c1");

    expect(opener.postMessage).toHaveBeenCalledWith(
      {
        type: "palmr:external-reauth",
        status: "error",
        error: "PROVIDER_AUTH_DENIED",
        requestId: "req-8c1",
      },
      window.location.origin,
    );
    expect(close).toHaveBeenCalledTimes(1);
    const page = screen.getByTestId("reauth-complete");
    expect(
      within(page).getByText("The identity provider denied the sign-in request."),
    ).toBeDefined();
    expect(within(page).getByText("Request ID: req-8c1")).toBeDefined();
  });

  test("a message is sent once even when React replays the effect", async () => {
    const opener: FakeOpener = { closed: false, postMessage: vi.fn() };
    setOpener(opener);

    await renderFeature(
      <StrictMode>
        <ReauthCompletePage search="?status=success" onContinue={() => undefined} />
      </StrictMode>,
    );
    await screen.findByTestId("reauth-complete");

    expect(opener.postMessage).toHaveBeenCalledTimes(1);
  });

  test("a malformed landing sends nothing and offers the way back", async () => {
    const opener: FakeOpener = { closed: false, postMessage: vi.fn() };
    setOpener(opener);

    await renderCompletion("?status=error&error=%3Cscript%3E");

    expect(opener.postMessage).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByTestId("reauth-complete").getAttribute("data-outcome")).toBe("invalid");
    expect(screen.getByRole("button", { name: "Continue to Palmr" })).toBeDefined();
  });

  test("it never carries credentials, user data or tokens in the message", async () => {
    const opener: FakeOpener = { closed: false, postMessage: vi.fn() };
    setOpener(opener);

    await renderCompletion("?status=success&code=auth-code&state=oauth-state&token=t");

    const call = opener.postMessage.mock.calls[0] as unknown[] | undefined;
    expect(Object.keys(call?.[0] as object).sort()).toEqual(["status", "type"]);
  });
});

describe("component_reauth_complete_without_opener", () => {
  test("success renders the localized outcome with a safe way back and does not crash", async () => {
    setOpener(null);

    const { onContinue, user } = await renderCompletion("?status=success");

    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { level: 1, name: "Identity confirmed" })).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Continue to Palmr" }));
    expect(onContinue).toHaveBeenCalledTimes(1);
  });

  test("an error shows its code message and request id, not an authentication verdict", async () => {
    setOpener(null);

    await renderCompletion("?status=error&error=AUTH_RECENT_AUTH_REQUIRED&requestId=req-5");

    expect(
      screen.getByRole("heading", { level: 1, name: "Couldn't confirm your identity" }),
    ).toBeDefined();
    expect(screen.getByText("Request ID: req-5")).toBeDefined();
    expect(screen.getByRole("button", { name: "Continue to Palmr" })).toBeDefined();
  });

  test("a closed opener is treated like no opener", async () => {
    const opener: FakeOpener = { closed: true, postMessage: vi.fn() };
    setOpener(opener);

    await renderCompletion("?status=success");

    expect(opener.postMessage).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Continue to Palmr" })).toBeDefined();
  });
});
