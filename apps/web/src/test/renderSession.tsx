import type { QueryClient } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { ConfigProvider } from "antd";
import { createMemoryRouter, type RouteObject } from "react-router";
import { vi } from "vitest";
import { AppTree } from "../app/AppTree";
import { createI18n } from "../app/i18n/i18n";
import { routerContextFor } from "../app/router/routeContext";
import { createSessionCoordinator } from "../app/session/sessionCoordinator";
import { discardRecentAuthChallenge } from "../features/auth";
import { createQueryClient } from "../shared/api/queryClient";

export interface RenderSessionOptions {
  routes: RouteObject[];
  initialEntries: string[];
  seed?: (queryClient: QueryClient) => void;
}

const disconnects: (() => void)[] = [];

export function stubMatchMedia() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
}

export function renderSession({ routes, initialEntries, seed }: RenderSessionOptions) {
  const session = createSessionCoordinator();
  const handleError = session.handleError;
  const errorEvents: Parameters<typeof handleError>[0][] = [];
  const queryClient = createQueryClient({
    onError: (event) => {
      errorEvents.push(event);
      handleError(event);
    },
  });
  seed?.(queryClient);
  const router = createMemoryRouter(routes, {
    initialEntries,
    getContext: routerContextFor(queryClient),
  });
  disconnects.push(session.connect(router));

  const locations: string[] = [];
  const describe = () => `${router.state.location.pathname}${router.state.location.search}`;
  let last = describe();
  locations.push(last);
  router.subscribe(() => {
    const current = describe();
    if (current !== last) {
      last = current;
      locations.push(current);
    }
  });

  const view = render(
    <ConfigProvider theme={{ token: { motion: false } }}>
      <AppTree
        queryClient={queryClient}
        i18n={createI18n()}
        router={router}
        cspNonce={undefined}
        browserLanguages={[]}
      />
    </ConfigProvider>,
  );
  return { session, queryClient, router, locations, errorEvents, view };
}

export function resetSessionHarness() {
  for (const disconnect of disconnects.splice(0)) {
    disconnect();
  }
  discardRecentAuthChallenge();
  vi.unstubAllGlobals();
}
