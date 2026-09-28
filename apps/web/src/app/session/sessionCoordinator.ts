import type { QueryClient } from "@tanstack/react-query";
import type { RouterProviderProps } from "react-router/dom";
import {
  discardRecentAuthChallenge,
  openRecentAuthChallenge,
  recentAuthStore,
} from "../../features/auth";
import { isPublicRequest } from "../../shared/api/publicRequests";
import { isAuthenticatedQueryKey, qk } from "../../shared/api/query-keys";
import type { GlobalQueryError } from "../../shared/api/queryClient";
import { ApiError } from "../../shared/errors";
import { loginPathWithNext } from "../router/next";
import { PATHS } from "../router/paths";

export type SessionRouter = Pick<RouterProviderProps["router"], "navigate" | "state" | "subscribe">;

export interface SessionCoordinator {
  handleError: (event: GlobalQueryError) => void;
  connect: (router: SessionRouter) => () => void;
}

const AUTH_ROUTES: readonly string[] = [PATHS.login, PATHS.setup];

function isAuthRoute(pathname: string): boolean {
  return AUTH_ROUTES.some((route) => pathname === route || pathname.startsWith(`${route}/`));
}

const authenticatedQueries = {
  predicate: (query: { queryKey: readonly unknown[] }) => isAuthenticatedQueryKey(query.queryKey),
};

export function createSessionCoordinator(): SessionCoordinator {
  let router: SessionRouter | null = null;

  function navigateToLogin() {
    if (router === null) {
      return;
    }
    const { pathname, search } = router.state.location;
    if (pathname === PATHS.login) {
      return;
    }
    const target = isAuthRoute(pathname) ? PATHS.login : loginPathWithNext(pathname, search);
    void router.navigate(target, { replace: true });
  }

  function purgeAuthenticatedQueries(client: QueryClient) {
    const cache = client.getQueryCache();
    const signedOut = () => client.getQueryData(qk.me.current()) === null;
    const hasActive = () => cache.findAll({ ...authenticatedQueries, type: "active" }).length > 0;
    void client.cancelQueries(authenticatedQueries);
    client.removeQueries({ ...authenticatedQueries, type: "inactive" });
    if (!hasActive()) {
      return;
    }
    const unsubscribe = cache.subscribe((event) => {
      if (!signedOut()) {
        unsubscribe();
        return;
      }
      if (event.type !== "observerRemoved" || !authenticatedQueries.predicate(event.query)) {
        return;
      }
      if (event.query.getObserversCount() === 0) {
        cache.remove(event.query);
      }
      if (!hasActive()) {
        unsubscribe();
      }
    });
  }

  function reconcileSignedOut(client: QueryClient) {
    discardRecentAuthChallenge();
    if (client.getQueryData(qk.me.current()) === null) {
      return;
    }
    client.setQueryData(qk.me.current(), null);
    purgeAuthenticatedQueries(client);
    navigateToLogin();
  }

  function challengeRecentAuth(event: Extract<GlobalQueryError, { source: "mutation" }>) {
    const { error, client, mutation } = event;
    if (router === null || !(error instanceof ApiError)) {
      return;
    }
    if (client.getQueryData(qk.me.current()) == null) {
      return;
    }
    const variables = mutation.state.variables;
    openRecentAuthChallenge({
      originPath: router.state.location.pathname,
      requestId: error.requestId,
      replay: () => mutation.execute(variables),
    });
  }

  return {
    handleError(event) {
      const { error } = event;
      if (!(error instanceof ApiError)) {
        return;
      }
      if (error.code === "AUTH_REQUIRED" && !isPublicRequest(error.request)) {
        reconcileSignedOut(event.client);
      } else if (error.code === "AUTH_RECENT_AUTH_REQUIRED" && event.source === "mutation") {
        challengeRecentAuth(event);
      }
    },

    connect(target) {
      router = target;
      const unsubscribe = target.subscribe((state) => {
        const { challenge } = recentAuthStore.getState();
        if (challenge !== null && state.location.pathname !== challenge.originPath) {
          discardRecentAuthChallenge(challenge.id);
        }
      });
      return () => {
        unsubscribe();
        if (router === target) {
          router = null;
        }
      };
    },
  };
}
