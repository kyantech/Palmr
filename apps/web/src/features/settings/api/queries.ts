import {
  type QueryClient,
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type { Me, Preferences, SessionPage } from "../types";

export const SESSIONS_PAGE_SIZE = 50;

function cachedMe(client: QueryClient): Me | undefined {
  return client.getQueryData<Me | null>(qk.me.current()) ?? undefined;
}

function cachedMeUpdatedAt(client: QueryClient): number | undefined {
  return client.getQueryState(qk.me.current())?.dataUpdatedAt;
}

export function useProfile() {
  const client = useQueryClient();
  return useQuery({
    queryKey: qk.me.profile(),
    queryFn: ({ signal }) => apiFetch("get", "/profile", { signal }),
    initialData: () => cachedMe(client)?.user,
    initialDataUpdatedAt: () => cachedMeUpdatedAt(client),
    refetchOnWindowFocus: false,
  });
}

export function usePreferences() {
  const client = useQueryClient();
  return useQuery({
    queryKey: qk.me.preferences(),
    queryFn: ({ signal }) => apiFetch("get", "/profile/preferences", { signal }),
    initialData: (): Preferences | undefined => {
      const user = cachedMe(client)?.user;
      return user === undefined
        ? undefined
        : { locale: user.locale, theme: user.theme, accent: user.accent };
    },
    initialDataUpdatedAt: () => cachedMeUpdatedAt(client),
    refetchOnWindowFocus: false,
  });
}

export function useSessions() {
  return useInfiniteQuery({
    queryKey: qk.me.sessions(),
    queryFn: ({ pageParam, signal }): Promise<SessionPage> =>
      apiFetch("get", "/sessions", {
        query: {
          limit: SESSIONS_PAGE_SIZE,
          ...(pageParam === null ? {} : { cursor: pageParam }),
        },
        signal,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
  });
}

export function useEffectiveSettings() {
  return useQuery({
    queryKey: qk.me.effectiveSettings(),
    queryFn: ({ signal }) => apiFetch("get", "/settings/effective", { signal }),
    refetchOnWindowFocus: false,
  });
}
