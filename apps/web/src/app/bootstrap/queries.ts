import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "../../shared/api/apiFetch";
import { qk } from "../../shared/api/query-keys";
import type { components } from "../../shared/api/schema";
import { ApiError } from "../../shared/errors";
import { assertSupportedLocales } from "./localeContract";

export type Bootstrap = components["schemas"]["Bootstrap"];
export type Me = components["schemas"]["MeResponse"];
export type Restriction = NonNullable<Me["restriction"]>;

async function fetchBootstrap(): Promise<Bootstrap> {
  const bootstrap = await apiFetch("get", "/bootstrap");
  assertSupportedLocales(bootstrap.supportedLocales);
  return bootstrap;
}

async function fetchCurrentSession(): Promise<Me | null> {
  try {
    return await apiFetch("get", "/auth/me");
  } catch (error) {
    if (error instanceof ApiError && error.code === "AUTH_REQUIRED") {
      return null;
    }
    throw error;
  }
}

export function bootstrapQueryOptions() {
  return queryOptions({
    queryKey: qk.bootstrap(),
    queryFn: fetchBootstrap,
    staleTime: Infinity,
    gcTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

export function currentSessionQueryOptions() {
  return queryOptions({
    queryKey: qk.me.current(),
    queryFn: fetchCurrentSession,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}
