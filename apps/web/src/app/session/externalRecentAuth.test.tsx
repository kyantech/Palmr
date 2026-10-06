import { useMutation } from "@tanstack/react-query";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { useState } from "react";
import { type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { recentAuthStore } from "../../features/auth";
import { apiFetch } from "../../shared/api/apiFetch";
import { qk } from "../../shared/api/query-keys";
import { bootHandlers, errorEnvelope, ME_URL, meFixture } from "../../test/bootFixtures";
import {
  dispatchWindowMessage,
  type FakePopup,
  type PopupHarness,
  stubWindowOpen,
} from "../../test/fakePopup";
import { renderSession, resetSessionHarness, stubMatchMedia } from "../../test/renderSession";
import { server } from "../../test/server";
import { authenticatedRoutes } from "../guards/chain";

const ACTION_URL = "*/api/v1/profile/password";
const REAUTH_URL = "*/api/v1/auth/reauthenticate";
const IDP_URL = "https://idp.example.test/authorize?prompt=login&max_age=0&state=server-issued";
const BEFORE = "2026-09-28T00:00:00Z";
const AFTER = "2026-09-28T12:05:00Z";
const SERVER_PROSE = "idp internals: assertion replayed for ada@example.test";

function SensitiveAction() {
  const [draft, setDraft] = useState("");
  const run = useMutation({
    mutationKey: ["sensitive"],
    mutationFn: (body: { newPassword: string }) => apiFetch("post", "/profile/password", { body }),
  });
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        run.mutate({ newPassword: draft });
      }}
    >
      <label>
        Draft value
        <input
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
          }}
        />
      </label>
      <button type="submit">Run sensitive action</button>
      <output data-testid="action-status">{run.status}</output>
    </form>
  );
}

function Elsewhere() {
  const { pathname, search } = useLocation();
  return <output data-testid="routed">{`${pathname}${search}`}</output>;
}

const routes: RouteObject[] = [
  ...authenticatedRoutes([
    { path: "/settings/sensitive", element: <SensitiveAction /> },
    { path: "/overview", element: <Elsewhere /> },
  ]),
  { path: "*", element: <Elsewhere /> },
];

interface ServerState {
  actionBodies: unknown[];
  reauthBodies: unknown[];
  events: string[];
  meCalls: number;
  recentAuthUntil: string;
}

interface ServerOptions {
  reauth?: (state: ServerState) => Response | Promise<Response>;
  meDelayMs?: number;
}

function installServer({ reauth, meDelayMs = 0 }: ServerOptions = {}): ServerState {
  const me = meFixture({ capabilities: { hasLocalPassword: false }, recentAuthUntil: BEFORE });
  const state: ServerState = {
    actionBodies: [],
    reauthBodies: [],
    events: [],
    meCalls: 0,
    recentAuthUntil: BEFORE,
  };
  const { handlers } = bootHandlers({ me });
  server.use(
    http.get(ME_URL, async () => {
      state.meCalls += 1;
      await delay(meDelayMs);
      return HttpResponse.json({
        ...me,
        session: { ...me.session, recentAuthUntil: state.recentAuthUntil },
      });
    }),
    ...handlers.slice(0, 1),
    http.post(ACTION_URL, async ({ request }) => {
      state.actionBodies.push(await request.json());
      state.events.push("action");
      return state.actionBodies.length === 1
        ? errorEnvelope("AUTH_RECENT_AUTH_REQUIRED", 403, "req-recent", { message: SERVER_PROSE })
        : new HttpResponse(null, { status: 204 });
    }),
    http.post(REAUTH_URL, async ({ request }) => {
      state.reauthBodies.push(await request.json());
      state.events.push("reauth");
      return (
        reauth?.(state) ??
        HttpResponse.json({ accepted: true, externalReauthUrl: IDP_URL }, { status: 202 })
      );
    }),
  );
  return state;
}

let popups: PopupHarness;

async function startChallenge(options: { blocked?: boolean } = {}) {
  popups = stubWindowOpen(options);
  const harness = renderSession({ routes, initialEntries: ["/settings/sensitive"] });
  const user = userEvent.setup({ delay: null });
  await user.type(await screen.findByLabelText("Draft value"), "keep me");
  await user.click(screen.getByRole("button", { name: "Run sensitive action" }));
  const dialog = await screen.findByRole("dialog", { name: "Confirm it's you" });
  return { ...harness, user, dialog };
}

function continueButton(dialog: HTMLElement) {
  return within(dialog).getByRole("button", { name: /Continue with your sign-in provider/ });
}

function popup(index = 0): FakePopup {
  const found = popups.popups[index];
  if (found === undefined) {
    throw new Error(`popup ${String(index)} was not opened`);
  }
  return found;
}

const SUCCESS = { type: "palmr:external-reauth", status: "success" };

async function settle(ms = 60) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

function expectDraftIntact() {
  expect(screen.getByLabelText<HTMLInputElement>("Draft value").value).toBe("keep me");
}

beforeEach(() => {
  stubMatchMedia();
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  popups.restore();
  resetSessionHarness();
  vi.restoreAllMocks();
});

describe("component_external_recent_auth_popup_start", () => {
  test("the popup opens synchronously in the click, before the reauthenticate request exists", async () => {
    const state = installServer({
      reauth: async () => {
        await delay(60);
        return HttpResponse.json({ accepted: true, externalReauthUrl: IDP_URL }, { status: 202 });
      },
    });
    const { user, dialog } = await startChallenge();
    const openedBeforeRequest: string[][] = [];
    popups.open.mockImplementation(((url?: string) => {
      openedBeforeRequest.push([...state.events]);
      const placeholder: FakePopup = {
        closed: false,
        location: { href: String(url) },
        close: vi.fn(() => {
          placeholder.closed = true;
        }),
        postMessage: vi.fn(),
      };
      popups.popups.push(placeholder);
      return placeholder;
    }) as never);

    await user.click(continueButton(dialog));

    expect(popups.open).toHaveBeenCalledTimes(1);
    expect(popups.open.mock.calls[0]?.[0]).toBe("about:blank");
    expect(openedBeforeRequest).toEqual([["action"]]);
    await waitFor(() => {
      expect(popup().location.href).toBe(IDP_URL);
    });
    expect(state.reauthBodies).toEqual([{}]);
    expect(state.events).toEqual(["action", "reauth"]);
  });

  test("only the server-returned URL is ever loaded into the popup and Palmr TOTP is never asked for", async () => {
    installServer();
    const { user, dialog } = await startChallenge();

    expect(within(dialog).queryByLabelText("Authentication code")).toBeNull();
    expect(within(dialog).queryByLabelText("Password")).toBeNull();
    await user.click(continueButton(dialog));

    await waitFor(() => {
      expect(popup().location.href).toBe(IDP_URL);
    });
    expect(popups.locationAtOpen).toEqual(["about:blank"]);
    expect(within(dialog).getByTestId("recent-auth-external-progress")).toBeDefined();
  });

  test("a blocked popup keeps the challenge, explains it, makes no request and never retries by itself", async () => {
    const state = installServer();
    const { user, dialog } = await startChallenge({ blocked: true });

    await user.click(continueButton(dialog));

    const notice = await within(dialog).findByTestId("recent-auth-external-notice");
    expect(notice.getAttribute("data-notice")).toBe("popupBlocked");
    expect(within(notice).getByText(/Allow pop-ups for this site/)).toBeDefined();
    await settle(700);
    expect(popups.open).toHaveBeenCalledTimes(1);
    expect(state.reauthBodies).toHaveLength(0);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
    expect(state.actionBodies).toHaveLength(1);
    expectDraftIntact();

    await user.click(continueButton(dialog));
    expect(popups.open).toHaveBeenCalledTimes(2);
  });

  test("a failed reauthenticate request closes the placeholder, shows the mapped error and keeps the challenge", async () => {
    const state = installServer({
      reauth: () =>
        errorEnvelope("PROVIDER_DISABLED", 403, "req-disabled", { message: SERVER_PROSE }),
    });
    const { user, dialog } = await startChallenge();

    await user.click(continueButton(dialog));

    expect(await within(dialog).findByText("This identity provider is turned off.")).toBeDefined();
    expect(screen.queryByText(SERVER_PROSE)).toBeNull();
    expect(popup().close).toHaveBeenCalledTimes(1);
    expect(popup().location.href).toBe("about:blank");
    expect(recentAuthStore.getState().challenge).not.toBeNull();
    expect(state.actionBodies).toHaveLength(1);
    expectDraftIntact();
    expect(continueButton(dialog).hasAttribute("disabled")).toBe(false);
  });

  test("a non-http(s) externalReauthUrl is refused and the placeholder is closed", async () => {
    installServer({
      reauth: () =>
        HttpResponse.json(
          { accepted: true, externalReauthUrl: "javascript:alert(1)" },
          { status: 202 },
        ),
    });
    const { user, dialog } = await startChallenge();

    await user.click(continueButton(dialog));

    expect(await within(dialog).findByRole("alert")).toBeDefined();
    expect(popup().location.href).toBe("about:blank");
    expect(popup().close).toHaveBeenCalled();
  });
});

describe("component_external_recent_auth_completion", () => {
  async function waitForPopupNavigation(
    dialog: HTMLElement,
    user: ReturnType<typeof userEvent.setup>,
  ) {
    await user.click(continueButton(dialog));
    await waitFor(() => {
      expect(popup().location.href).toBe(IDP_URL);
    });
  }

  test("a validated success message re-reads the server session and then replays the mutation exactly once", async () => {
    const state = installServer();
    const { user, dialog, queryClient } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    const meCallsBefore = state.meCalls;

    state.recentAuthUntil = AFTER;
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(screen.getByTestId("action-status").textContent).toBe("success");
    });
    expect(state.meCalls).toBe(meCallsBefore + 1);
    expect(queryClient.getQueryData(qk.me.current())).toMatchObject({
      session: { recentAuthUntil: AFTER },
    });
    expect(state.actionBodies).toEqual([{ newPassword: "keep me" }, { newPassword: "keep me" }]);
    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(popup().close).toHaveBeenCalled();
    await settle();
    expect(state.actionBodies).toHaveLength(2);
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Confirm it's you" })).toBeNull();
    });
  });

  test("the success message alone replays nothing before the server session has been re-read", async () => {
    const state = installServer({ meDelayMs: 150 });
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    const meCallsBefore = state.meCalls;

    state.recentAuthUntil = AFTER;
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await settle(30);

    expect(state.meCalls).toBe(meCallsBefore + 1);
    expect(state.actionBodies).toHaveLength(1);
    expect(within(dialog).getByTestId("recent-auth-external-progress")).toBeDefined();
    await waitFor(() => {
      expect(state.actionBodies).toHaveLength(2);
    });
  });

  test("a success message the server does not back with a fresh window replays nothing", async () => {
    const state = installServer();
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);

    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });

    const alert = await within(dialog).findByTestId("recent-auth-external-unconfirmed");
    expect(within(alert).getByText(/didn't confirm a fresh sign-in/)).toBeDefined();
    await settle();
    expect(state.actionBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
    expectDraftIntact();
    expect(continueButton(dialog).hasAttribute("disabled")).toBe(false);
  });

  test("an error message shows the localized code and request id, keeps the challenge and replays nothing", async () => {
    const state = installServer();
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    const meCallsBefore = state.meCalls;

    await act(async () => {
      dispatchWindowMessage(
        {
          type: "palmr:external-reauth",
          status: "error",
          error: "PROVIDER_AUTH_DENIED",
          requestId: "req-cb-42",
        },
        { source: popup() },
      );
      await Promise.resolve();
    });

    expect(
      await within(dialog).findByText("The identity provider denied the sign-in request."),
    ).toBeDefined();
    expect(within(dialog).getByText("Request ID: req-cb-42")).toBeDefined();
    expect(popup().close).toHaveBeenCalled();
    expect(state.meCalls).toBe(meCallsBefore);
    expect(state.actionBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
    expectDraftIntact();
  });

  test("an error that names AUTH_RECENT_AUTH_REQUIRED reads as an unconfirmed sign-in, not a password prompt", async () => {
    installServer();
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);

    await act(async () => {
      dispatchWindowMessage(
        {
          type: "palmr:external-reauth",
          status: "error",
          error: "AUTH_RECENT_AUTH_REQUIRED",
          requestId: "req-cb-9",
        },
        { source: popup() },
      );
      await Promise.resolve();
    });

    const alert = await within(dialog).findByTestId("recent-auth-external-unconfirmed");
    expect(within(alert).getByText("Request ID: req-cb-9")).toBeDefined();
    expect(within(dialog).queryByText("Confirm your password to continue.")).toBeNull();
  });

  test("a message from the wrong origin is ignored", async () => {
    const state = installServer();
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    state.recentAuthUntil = AFTER;
    const meCallsBefore = state.meCalls;

    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup(), origin: "https://evil.example" });
      await Promise.resolve();
    });
    await settle();

    expect(state.meCalls).toBe(meCallsBefore);
    expect(state.actionBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
  });

  test("a message whose source is not the exact popup is ignored, even from the same origin", async () => {
    const state = installServer();
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    state.recentAuthUntil = AFTER;
    const meCallsBefore = state.meCalls;
    const otherTab = { closed: false } as unknown as Window;

    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: otherTab });
      dispatchWindowMessage(SUCCESS, { source: window });
      dispatchWindowMessage(SUCCESS, { source: null });
      await Promise.resolve();
    });
    await settle();

    expect(state.meCalls).toBe(meCallsBefore);
    expect(state.actionBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
  });

  test.each([
    ["a bare string", "success"],
    ["no type", { status: "success" }],
    ["an extra key", { type: "palmr:external-reauth", status: "success", token: "x" }],
    ["an unknown status", { type: "palmr:external-reauth", status: "done" }],
    ["a loose truthy object", { type: "palmr:external-reauth", status: "success", ok: true }],
  ])(
    "a malformed message (%s) is ignored and a valid one still works afterwards",
    async (_label, data) => {
      const state = installServer();
      const { user, dialog } = await startChallenge();
      await waitForPopupNavigation(dialog, user);
      state.recentAuthUntil = AFTER;
      const meCallsBefore = state.meCalls;

      await act(async () => {
        dispatchWindowMessage(data, { source: popup() });
        await Promise.resolve();
      });
      await settle();
      expect(state.meCalls).toBe(meCallsBefore);
      expect(state.actionBodies).toHaveLength(1);

      await act(async () => {
        dispatchWindowMessage(SUCCESS, { source: popup() });
        await Promise.resolve();
      });
      await waitFor(() => {
        expect(state.actionBodies).toHaveLength(2);
      });
    },
  );

  test("duplicate success messages cannot replay twice", async () => {
    const state = installServer({ meDelayMs: 40 });
    const { user, dialog } = await startChallenge();
    await waitForPopupNavigation(dialog, user);
    const meCallsBefore = state.meCalls;
    state.recentAuthUntil = AFTER;

    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      dispatchWindowMessage(SUCCESS, { source: popup() });
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(screen.getByTestId("action-status").textContent).toBe("success");
    });
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await settle(120);

    expect(state.actionBodies).toHaveLength(2);
    expect(state.meCalls).toBe(meCallsBefore + 1);
  });
});

describe("component_external_recent_auth_lifecycle", () => {
  async function engage() {
    const state = installServer();
    const harness = await startChallenge();
    await harness.user.click(continueButton(harness.dialog));
    await waitFor(() => {
      expect(popup().location.href).toBe(IDP_URL);
    });
    return { state, ...harness };
  }

  test("cancelling the modal closes the popup, discards the challenge and a late message replays nothing", async () => {
    const { state, user, dialog } = await engage();
    state.recentAuthUntil = AFTER;
    const meCallsBefore = state.meCalls;

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(popup().close).toHaveBeenCalled();
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await settle();
    expect(state.actionBodies).toHaveLength(1);
    expect(state.meCalls).toBe(meCallsBefore);
    expectDraftIntact();
  });

  test("closing the popup by hand returns to idle without replaying, and the user can start again", async () => {
    const { state, user, dialog } = await engage();

    popup().closed = true;

    const notice = await within(dialog).findByTestId("recent-auth-external-notice", undefined, {
      timeout: 4000,
    });
    expect(notice.getAttribute("data-notice")).toBe("popupClosed");
    expect(state.actionBodies).toHaveLength(1);
    expect(recentAuthStore.getState().challenge).not.toBeNull();
    expectDraftIntact();

    await user.click(continueButton(dialog));
    expect(popups.open).toHaveBeenCalledTimes(2);
  });

  test("a success message that arrives right as the popup closes itself still counts", async () => {
    const { state, dialog } = await engage();
    state.recentAuthUntil = AFTER;

    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      popup().closed = true;
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(state.actionBodies).toHaveLength(2);
    });
    expect(within(dialog).queryByTestId("recent-auth-external-notice")).toBeNull();
  });

  test("leaving the originating route discards the challenge and closes the popup", async () => {
    const { router } = await engage();

    await act(async () => {
      await router.navigate("/overview");
    });

    expect(recentAuthStore.getState().challenge).toBeNull();
    expect(popup().close).toHaveBeenCalled();
  });

  test("session loss destroys the challenge and its popup", async () => {
    const { state, queryClient } = await engage();

    await act(async () => {
      queryClient.setQueryData(qk.me.current(), null);
      recentAuthStore.setState({ challenge: null });
      await Promise.resolve();
    });

    expect(popup().close).toHaveBeenCalled();
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await settle();
    expect(state.actionBodies).toHaveLength(1);
  });
});

describe("component_external_recent_auth_replay_is_memory_only", () => {
  test("no storage, cookie, IndexedDB or URL carries the replay or any credential", async () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    const cookie = vi.spyOn(document, "cookie", "set");
    const openDb = vi.fn();
    vi.stubGlobal("indexedDB", { open: openDb });
    const state = installServer();
    const { user, dialog, router } = await startChallenge();
    const locationAtStart = `${router.state.location.pathname}${router.state.location.search}`;

    await user.click(continueButton(dialog));
    await waitFor(() => {
      expect(popup().location.href).toBe(IDP_URL);
    });
    state.recentAuthUntil = AFTER;
    await act(async () => {
      dispatchWindowMessage(SUCCESS, { source: popup() });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(screen.getByTestId("action-status").textContent).toBe("success");
    });

    expect(setItem).not.toHaveBeenCalled();
    expect(cookie).not.toHaveBeenCalled();
    expect(openDb).not.toHaveBeenCalled();
    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
    expect(`${router.state.location.pathname}${router.state.location.search}`).toBe(
      locationAtStart,
    );
    expect(window.location.search).toBe("");
    expect(window.location.hash).toBe("");
    expect(popup().location.href).not.toContain("keep%20me");
    expect(JSON.stringify(recentAuthStore.getState())).not.toContain("keep me");
    vi.unstubAllGlobals();
  });
});
