import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, type MockInstance, test, vi } from "vitest";
import { renderFeature } from "../../../test/renderFeature";
import { stubMatchMedia } from "../../../test/renderSession";
import {
  CHANNEL_ID,
  type ChannelListener,
  listenOnChannel,
  OTHER_CHANNEL_ID,
} from "../../../test/reauthChannel";
import { ReauthCompletePage } from "./ReauthCompletePage";

const listeners: ChannelListener[] = [];

function listen(channelId: string) {
  const listener = listenOnChannel(channelId);
  listeners.push(listener);
  return listener;
}

async function renderCompletion(search: string) {
  const onContinue = vi.fn();
  await renderFeature(<ReauthCompletePage search={search} onContinue={onContinue} />);
  await screen.findByTestId("reauth-complete");
  return { onContinue, user: userEvent.setup() };
}

async function settle(ms = 60) {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

let close: MockInstance<typeof window.close>;

beforeEach(() => {
  stubMatchMedia();
  close = vi.spyOn(window, "close").mockImplementation(() => undefined);
});

afterEach(() => {
  for (const listener of listeners.splice(0)) {
    listener.close();
  }
  vi.restoreAllMocks();
});

describe("component_reauth_complete_broadcasts_on_the_challenge_channel", () => {
  test("success broadcasts exactly the minimal success message on its channel and closes the window", async () => {
    const own = listen(CHANNEL_ID);

    await renderCompletion(`?status=success&channel=${CHANNEL_ID}`);

    await waitFor(() => {
      expect(own.messages).toEqual([{ type: "palmr:external-reauth", status: "success" }]);
    });
    expect(close).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("reauth-complete").getAttribute("data-outcome")).toBe("success");
    expect(screen.getByTestId("reauth-complete").getAttribute("data-reporting")).toBe("true");
  });

  test("an error broadcasts the code and the callback request id and shows them in the window", async () => {
    const own = listen(CHANNEL_ID);

    await renderCompletion(
      `?status=error&error=PROVIDER_AUTH_DENIED&requestId=req-8c1&channel=${CHANNEL_ID}`,
    );

    await waitFor(() => {
      expect(own.messages).toEqual([
        {
          type: "palmr:external-reauth",
          status: "error",
          error: "PROVIDER_AUTH_DENIED",
          requestId: "req-8c1",
        },
      ]);
    });
    const page = screen.getByTestId("reauth-complete");
    expect(
      within(page).getByText("The identity provider denied the sign-in request."),
    ).toBeDefined();
    expect(within(page).getByText("Request ID: req-8c1")).toBeDefined();
  });

  test("a message reaches only the challenge's own channel, never another one", async () => {
    const own = listen(CHANNEL_ID);
    const other = listen(OTHER_CHANNEL_ID);

    await renderCompletion(`?status=success&channel=${CHANNEL_ID}`);
    await waitFor(() => {
      expect(own.messages).toHaveLength(1);
    });
    await settle();

    expect(other.messages).toEqual([]);
  });

  test("a message is sent once even when React replays the effect", async () => {
    const own = listen(CHANNEL_ID);

    await renderFeature(
      <StrictMode>
        <ReauthCompletePage
          search={`?status=success&channel=${CHANNEL_ID}`}
          onContinue={() => undefined}
        />
      </StrictMode>,
    );
    await screen.findByTestId("reauth-complete");
    await waitFor(() => {
      expect(own.messages).toHaveLength(1);
    });
    await settle();

    expect(own.messages).toHaveLength(1);
  });

  test("it never carries credentials, user data or tokens in the message", async () => {
    const own = listen(CHANNEL_ID);

    await renderCompletion(
      `?status=success&channel=${CHANNEL_ID}&code=auth-code&state=oauth-state&token=t`,
    );

    await waitFor(() => {
      expect(own.messages).toHaveLength(1);
    });
    expect(Object.keys(own.messages[0] as object).sort()).toEqual(["status", "type"]);
    expect(JSON.stringify(own.messages)).not.toContain(CHANNEL_ID);
  });

  test("it does not depend on window.opener", async () => {
    Object.defineProperty(window, "opener", { value: null, configurable: true, writable: true });
    const own = listen(CHANNEL_ID);

    await renderCompletion(`?status=success&channel=${CHANNEL_ID}`);

    await waitFor(() => {
      expect(own.messages).toHaveLength(1);
    });
  });
});

describe("component_reauth_complete_fallback_view", () => {
  test("it stays readable with a safe way back when the window cannot close itself", async () => {
    const own = listen(CHANNEL_ID);
    close.mockImplementation(() => {
      throw new Error("not script-closable");
    });

    const { onContinue, user } = await renderCompletion(`?status=success&channel=${CHANNEL_ID}`);

    await waitFor(() => {
      expect(own.messages).toHaveLength(1);
    });
    expect(screen.getByRole("heading", { level: 1, name: "Identity confirmed" })).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Continue to Palmr" }));
    expect(onContinue).toHaveBeenCalledTimes(1);
  });

  test("an error shows its code message and request id, not an authentication verdict", async () => {
    await renderCompletion(
      `?status=error&error=AUTH_RECENT_AUTH_REQUIRED&requestId=req-5&channel=${CHANNEL_ID}`,
    );

    expect(
      screen.getByRole("heading", { level: 1, name: "Couldn't confirm your identity" }),
    ).toBeDefined();
    expect(screen.getByText("Request ID: req-5")).toBeDefined();
    expect(screen.getByRole("button", { name: "Continue to Palmr" })).toBeDefined();
  });

  test.each([
    ["no channel", "?status=success"],
    ["a malformed channel", "?status=success&channel=short"],
    ["a malformed error code", `?status=error&error=%3Cscript%3E&channel=${CHANNEL_ID}`],
    ["an unknown status", `?status=done&channel=${CHANNEL_ID}`],
  ])(
    "%s broadcasts nothing, keeps the window open and offers the way back",
    async (_label, search) => {
      const own = listen(CHANNEL_ID);

      await renderCompletion(search);
      await settle();

      expect(own.messages).toEqual([]);
      expect(close).not.toHaveBeenCalled();
      expect(screen.getByTestId("reauth-complete").getAttribute("data-outcome")).toBe("invalid");
      expect(screen.getByTestId("reauth-complete").getAttribute("data-reporting")).toBe("false");
      expect(screen.getByRole("button", { name: "Continue to Palmr" })).toBeDefined();
    },
  );
});
