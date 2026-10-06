import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";

export type TwoFactorStatus = components["schemas"]["TwoFactorStatus"];
export type TrustedDeviceItem = components["schemas"]["TrustedDeviceItem"];
export type TrustedDeviceList = components["schemas"]["TrustedDeviceList"];
export type ResetCheck = components["schemas"]["ResetCheckResponse"];
export type InviteLookup = components["schemas"]["InviteLookupResponse"];
export type IdentityLinkItem = components["schemas"]["IdentityLinkItem"];
export type IdentityLinkPage = components["schemas"]["Page_IdentityLinkItem"];

export const IDENTITY_LINKS_PAGE_SIZE = 50;

export const TRUSTED_DEVICES_PAGE_SIZE = 50;

const ONE_SHOT = {
  gcTime: 0,
  staleTime: Infinity,
  retry: false,
  refetchOnWindowFocus: false,
  refetchOnReconnect: false,
} as const;

export function useTwoFactorStatus() {
  return useQuery({
    queryKey: qk.me.twoFactor(),
    queryFn: ({ signal }) => apiFetch("get", "/auth/2fa", { signal }),
    refetchOnWindowFocus: false,
  });
}

export function useTrustedDevices() {
  return useInfiniteQuery({
    queryKey: qk.me.trustedDevices(),
    queryFn: ({ pageParam, signal }): Promise<TrustedDeviceList> =>
      apiFetch("get", "/auth/trusted-devices", {
        query: {
          limit: TRUSTED_DEVICES_PAGE_SIZE,
          ...(pageParam === null ? {} : { cursor: pageParam }),
        },
        signal,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
    refetchOnWindowFocus: false,
  });
}

export function useIdentityLinks() {
  return useInfiniteQuery({
    queryKey: qk.me.identityLinks(),
    queryFn: ({ pageParam, signal }): Promise<IdentityLinkPage> =>
      apiFetch("get", "/identity-links", {
        query: {
          limit: IDENTITY_LINKS_PAGE_SIZE,
          ...(pageParam === null ? {} : { cursor: pageParam }),
        },
        signal,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.nextCursor,
    refetchOnWindowFocus: false,
  });
}

export function useResetTokenCheck(instance: string, token: string) {
  return useQuery({
    queryKey: qk.public.passwordReset(instance),
    queryFn: ({ signal }) =>
      apiFetch("post", "/auth/password/reset/check", { body: { token }, signal }),
    ...ONE_SHOT,
  });
}

export function useInviteLookup(instance: string, token: string) {
  return useQuery({
    queryKey: qk.public.invite(instance),
    queryFn: ({ signal }) =>
      apiFetch("get", "/public/invites/{token}", { path: { token }, signal }),
    ...ONE_SHOT,
  });
}
