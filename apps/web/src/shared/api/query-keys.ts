import type { QueryKey } from "@tanstack/react-query";

export const qk = {
  bootstrap: () => ["bootstrap"] as const,

  me: {
    all: () => ["me"] as const,
    current: () => ["me", "current"] as const,
    profile: () => ["me", "profile"] as const,
    preferences: () => ["me", "preferences"] as const,
    sessions: () => ["me", "sessions"] as const,
    effectiveSettings: () => ["me", "effective-settings"] as const,
    twoFactor: () => ["me", "two-factor"] as const,
    trustedDevices: () => ["me", "trusted-devices"] as const,
  },

  public: {
    all: () => ["public"] as const,
    invite: (instance: string) => ["public", "invite", instance] as const,
    passwordReset: (instance: string) => ["public", "password-reset", instance] as const,
  },
} as const;

const PUBLIC_QUERY_ROOTS: ReadonlySet<unknown> = new Set([qk.bootstrap()[0], qk.public.all()[0]]);

function isCurrentSessionKey(queryKey: QueryKey): boolean {
  const current = qk.me.current();
  return queryKey.length === current.length && current.every((part, i) => queryKey[i] === part);
}

export function isAuthenticatedQueryKey(queryKey: QueryKey): boolean {
  return !PUBLIC_QUERY_ROOTS.has(queryKey[0]) && !isCurrentSessionKey(queryKey);
}
