import type { QueryClient } from "@tanstack/react-query";
import { qk } from "../../shared/api/query-keys";
import { currentSessionQueryOptions } from "../bootstrap/queries";

export async function refetchBootstrap(client: QueryClient): Promise<void> {
  await client.refetchQueries({ queryKey: qk.bootstrap(), exact: true });
}

export async function reconcileSignedIn(client: QueryClient): Promise<void> {
  await client.query({ ...currentSessionQueryOptions(), staleTime: 0 });
}

// Every transition that changes the signed-in state reconciles both halves of
// the boot payload: `/auth/me` remains the session authority, while
// `/bootstrap` is the public instance/login configuration it was always a
// separate concern. Bootstrap carries no session state, but it is still
// refreshed at the documented lifecycle boundaries so branding, login methods
// and setup state stay in step.
export async function reconcileAuthTransition(client: QueryClient): Promise<void> {
  try {
    await reconcileSignedIn(client);
  } finally {
    await refetchBootstrap(client);
  }
}

// The session is read before the bootstrap flips `setupCompleted`, so the
// moment BootstrapProvider enables its session query the answer is already
// cached and BootGate never falls back to its loading screen.
export async function reconcileSetupFinished(client: QueryClient): Promise<void> {
  await reconcileAuthTransition(client);
}
