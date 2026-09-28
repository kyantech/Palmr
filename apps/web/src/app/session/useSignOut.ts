import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { useLogout } from "../../features/auth";
import { refetchBootstrap } from "./authReconciliation";
import { signOutLocally } from "./sessionCoordinator";

export async function endLocalSession(client: QueryClient): Promise<void> {
  signOutLocally(client);
  await refetchBootstrap(client);
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
