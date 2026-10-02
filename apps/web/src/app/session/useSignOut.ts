import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { useLogout } from "../../features/auth";
import { qk } from "../../shared/api/query-keys";
import { reconcileSignedIn, refetchBootstrap } from "./authReconciliation";
import { purgeSignedOutState, signOutLocally } from "./sessionCoordinator";

export async function endLocalSession(client: QueryClient): Promise<void> {
  signOutLocally(client);
  await refetchBootstrap(client);
}

export async function reconcileSessionRevoked(client: QueryClient): Promise<void> {
  const wasSignedIn = client.getQueryData(qk.me.current()) != null;
  try {
    await reconcileSignedIn(client);
    if (wasSignedIn && client.getQueryData(qk.me.current()) === null) {
      purgeSignedOutState(client);
    }
  } finally {
    await refetchBootstrap(client);
  }
}

export function useSignOut() {
  const client = useQueryClient();
  const { mutateAsync, isPending, error } = useLogout();
  const signOut = useCallback(async () => {
    await mutateAsync();
    await endLocalSession(client);
  }, [client, mutateAsync]);
  return { signOut, isPending, error };
}
