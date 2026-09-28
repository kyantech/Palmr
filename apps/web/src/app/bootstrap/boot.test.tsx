import { focusManager, QueryClient } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { theme } from "antd";
import { http, HttpResponse } from "msw";
import { useTranslation } from "react-i18next";
import { createMemoryRouter, type RouteObject, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { createQueryClient } from "../../shared/api/queryClient";
import { qk } from "../../shared/api/query-keys";
import {
  BOOTSTRAP_URL,
  bootHandlers,
  bootstrapFixture,
  errorEnvelope,
  ME_URL,
  meFixture,
} from "../../test/bootFixtures";
import { server } from "../../test/server";
import { AppTree } from "../AppTree";
import { createI18n } from "../i18n/i18n";
import { RootRedirect } from "../router/RootRedirect";
import { composeTheme } from "../theme/appearance";
import { type BootState, useBootState } from "./bootState";
import { bootPresentation } from "./presentation";

interface Observation {
  path: string;
  setupCompleted: boolean;
  signedIn: boolean;
  lang: string;
  i18n: string;
  theme: string | undefined;
  colorPrimary: string;
}

function observingRoutes(observations: Observation[]): RouteObject[] {
  function Probe() {
    const { bootstrap, me } = useBootState();
    const { pathname } = useLocation();
    const { token } = theme.useToken();
    const { i18n } = useTranslation();
    observations.push({
      path: pathname,
      setupCompleted: bootstrap.setupCompleted,
      signedIn: me !== null,
      lang: document.documentElement.lang,
      i18n: i18n.language,
      theme: document.documentElement.dataset.theme,
      colorPrimary: token.colorPrimary,
    });
    return <output data-testid="routed">{pathname}</output>;
  }
  return [
    { path: "/", element: <RootRedirect /> },
    { path: "*", element: <Probe /> },
  ];
}

function quietQueryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

function renderApp({
  queryClient = createQueryClient(),
  initialEntries = ["/"],
  browserLanguages = [] as readonly string[],
} = {}) {
  const observations: Observation[] = [];
  const router = createMemoryRouter(observingRoutes(observations), { initialEntries });
  const visited: string[] = [router.state.location.pathname];
  router.subscribe((state) => {
    visited.push(state.location.pathname);
  });
  const view = render(
    <AppTree
      queryClient={queryClient}
      i18n={createI18n()}
      router={router}
      cspNonce={undefined}
      browserLanguages={browserLanguages}
    />,
  );
  return { observations, router, visited, view, queryClient };
}

async function routedPath() {
  return (await screen.findByTestId("routed", {}, { timeout: 3000 })).textContent;
}

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
  document.documentElement.removeAttribute("lang");
  delete document.documentElement.dataset.theme;
});

describe("component_boot_gate_waits_for_bootstrap_and_me", () => {
  test("the router does not mount until bootstrap and /me have both resolved", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture(), latencyMs: 40 });
    server.use(...handlers);

    const { observations, visited } = renderApp();

    expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
    expect(screen.getByText("Loading Palmr")).toBeDefined();
    expect(screen.queryByTestId("routed")).toBeNull();

    expect(await routedPath()).toBe("/overview");
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
    expect(observations.every((seen) => seen.signedIn && seen.setupCompleted)).toBe(true);
    expect(visited).not.toContain("/login");
    expect(visited).toEqual(["/", "/overview"]);
    expect(screen.queryByText("Loading Palmr")).toBeNull();
  });

  test("setup incomplete: /me is never requested and routing goes straight to /setup", async () => {
    const { calls, handlers } = bootHandlers({
      bootstrap: bootstrapFixture({ setupCompleted: false }),
      me: meFixture({ role: "admin" }),
    });
    server.use(...handlers);

    const { observations, visited } = renderApp();

    expect(await routedPath()).toBe("/setup");
    expect(calls).toEqual({ bootstrap: 1, me: 0 });
    expect(visited).toEqual(["/", "/setup"]);
    expect(visited).not.toContain("/overview");
    expect(observations.every((seen) => !seen.signedIn && !seen.setupCompleted)).toBe(true);
  });
});

describe("component_boot_gate_401_me_is_anonymous", () => {
  test("AUTH_REQUIRED from /me resolves to me = null without an error or a retry", async () => {
    const { calls, handlers } = bootHandlers({ me: null });
    server.use(...handlers);

    const { observations, visited, queryClient } = renderApp();

    expect(await routedPath()).toBe("/login");
    expect(calls).toEqual({ bootstrap: 1, me: 1 });
    expect(visited).toEqual(["/", "/login"]);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(observations.every((seen) => !seen.signedIn)).toBe(true);
    expect(queryClient.getQueryData(qk.me.current())).toBeNull();
    expect(queryClient.getQueryState(qk.me.current())?.status).toBe("success");
  });
});

describe("component_boot_gate_non_auth_me_error_is_not_anonymous", () => {
  test.each([
    ["SERVICE_UNAVAILABLE", () => errorEnvelope("SERVICE_UNAVAILABLE", 503, "req-503"), "req-503"],
    ["INTERNAL_ERROR", () => errorEnvelope("INTERNAL_ERROR", 500, "req-500"), "req-500"],
    ["CLIENT_NETWORK_ERROR", () => HttpResponse.error(), null],
  ])(
    "%s from /me shows a retryable boot failure, never /login",
    async (_code, reply, requestId) => {
      const { calls, handlers } = bootHandlers({ me: meFixture() });
      server.use(...handlers);
      let failing = true;
      let meCalls = 0;
      server.use(
        http.get(ME_URL, () => {
          meCalls += 1;
          return failing ? reply() : HttpResponse.json(meFixture());
        }),
      );

      const { visited } = renderApp({ queryClient: quietQueryClient() });

      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toContain("Palmr could not start");
      expect(alert.textContent).not.toContain(_code);
      if (requestId === null) {
        expect(alert.textContent).not.toContain("Request ID");
      } else {
        expect(alert.textContent).toContain(`Request ID: ${requestId}`);
      }
      expect(screen.queryByTestId("routed")).toBeNull();
      expect(visited).toEqual(["/"]);

      failing = false;
      await userEvent.click(screen.getByRole("button", { name: "Retry" }));

      expect(await routedPath()).toBe("/overview");
      expect(meCalls).toBe(2);
      expect(calls.bootstrap).toBe(1);
      expect(visited).not.toContain("/login");
    },
  );

  test("a bootstrap failure is retryable and retry reruns bootstrap only", async () => {
    let failing = true;
    let bootstrapCalls = 0;
    const { calls, handlers } = bootHandlers({ me: null });
    server.use(...handlers);
    server.use(
      http.get(BOOTSTRAP_URL, () => {
        bootstrapCalls += 1;
        return failing
          ? errorEnvelope("INTERNAL_ERROR", 500, "req-boot")
          : HttpResponse.json(bootstrapFixture());
      }),
    );

    renderApp({ queryClient: quietQueryClient() });

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Request ID: req-boot");
    expect(calls.me).toBe(0);

    failing = false;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await routedPath()).toBe("/login");
    expect(bootstrapCalls).toBe(2);
    expect(calls.me).toBe(1);
  });

  test("a divergent supportedLocales list is not silently accepted", async () => {
    const { handlers } = bootHandlers({
      bootstrap: bootstrapFixture({ supportedLocales: ["en-US", "xx-XX"] }),
    });
    server.use(...handlers);

    renderApp({ queryClient: quietQueryClient() });

    expect((await screen.findByRole("alert")).textContent).toContain("Palmr could not start");
    expect(screen.queryByTestId("routed")).toBeNull();
  });
});

describe("component_boot_state_drives_presentation_preferences", () => {
  test("an authenticated user's saved locale, theme and accent apply on the first routed paint", async () => {
    const { handlers } = bootHandlers({
      bootstrap: bootstrapFixture({ defaultLocale: "de-DE", primaryColor: "#aa3300" }),
      me: meFixture({ locale: "pt-BR", theme: "dark", accent: "violet" }),
      latencyMs: 20,
    });
    server.use(...handlers);

    const { observations } = renderApp({ browserLanguages: ["fr-FR"] });
    const darkVioletPrimary = theme.getDesignToken(
      composeTheme("dark", { accent: "violet", primaryColor: "#aa3300" }),
    ).colorPrimary;
    expect(darkVioletPrimary).not.toBe(
      theme.getDesignToken(composeTheme("dark", { accent: "default", primaryColor: "#aa3300" }))
        .colorPrimary,
    );

    expect(await routedPath()).toBe("/overview");
    expect(observations.length).toBeGreaterThan(0);
    for (const seen of observations) {
      expect(seen).toMatchObject({
        lang: "pt-BR",
        i18n: "pt-BR",
        theme: "dark",
        colorPrimary: darkVioletPrimary,
      });
    }
  });

  test("an anonymous visitor gets browser locale, system appearance and the instance colour", async () => {
    const { handlers } = bootHandlers({
      bootstrap: bootstrapFixture({ defaultLocale: "de-DE", primaryColor: "#aa3300" }),
      me: null,
    });
    server.use(...handlers);

    const { observations } = renderApp({ browserLanguages: ["ja-JP"] });

    expect(await routedPath()).toBe("/login");
    for (const seen of observations) {
      expect(seen).toMatchObject({ lang: "ja-JP", theme: "light", colorPrimary: "#aa3300" });
    }
  });

  test("with no browser match an anonymous visitor falls back to the instance default", () => {
    const state: BootState = {
      bootstrap: bootstrapFixture({ defaultLocale: "de-DE", primaryColor: "#aa3300" }),
      me: null,
    };

    expect(bootPresentation(state, ["xx-YY"])).toEqual({
      locale: "de-DE",
      appearance: { preference: "system", accent: "default", primaryColor: "#aa3300" },
    });
  });

  test("the authenticated locale beats browser and instance default; unknown values fall back safely", () => {
    const state: BootState = {
      bootstrap: bootstrapFixture({ defaultLocale: "de-DE" }),
      me: meFixture({ locale: "ko-KR", theme: "neon", accent: "plaid" }),
    };

    expect(bootPresentation(state, ["fr-FR"])).toEqual({
      locale: "ko-KR",
      appearance: { preference: "system", accent: "default", primaryColor: "#1668dc" },
    });
  });
});

describe("bootstrap query semantics", () => {
  afterEach(() => {
    focusManager.setFocused(undefined);
  });

  test("bootstrap is never stale, never collected and never refetched on focus", async () => {
    const { calls, handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);

    const { queryClient, view } = renderApp();
    await routedPath();

    const query = queryClient.getQueryCache().find({ queryKey: qk.bootstrap() });
    const observed = query?.observers[0]?.options;
    expect(observed?.staleTime).toBe(Infinity);
    expect(query?.options.gcTime).toBe(Infinity);
    expect(observed?.refetchOnWindowFocus).toBe(false);
    expect(query?.isStale()).toBe(false);

    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
      window.dispatchEvent(new Event("visibilitychange"));
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(calls.bootstrap).toBe(1);

    vi.useFakeTimers();
    try {
      view.unmount();
      vi.advanceTimersByTime(24 * 60 * 60 * 1000);
      expect(queryClient.getQueryData(qk.bootstrap())).toBeDefined();
    } finally {
      vi.useRealTimers();
    }
  });

  test("the boot state is a projection of the query cache, not a copy", async () => {
    const { handlers } = bootHandlers({ me: meFixture() });
    server.use(...handlers);

    const { queryClient, observations } = renderApp();
    await routedPath();
    expect(observations.at(-1)?.signedIn).toBe(true);

    act(() => {
      queryClient.setQueryData(qk.me.current(), null);
    });

    await waitFor(() => {
      expect(observations.at(-1)?.signedIn).toBe(false);
    });
  });
});
