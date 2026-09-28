import { createContext, useContext } from "react";
import type { Bootstrap, Me } from "./queries";

export interface BootState {
  bootstrap: Bootstrap;
  me: Me | null;
}

export type BootStatus =
  | { phase: "loading" }
  | { phase: "failed"; error: unknown; retry: () => void }
  | { phase: "ready"; state: BootState };

export const BootStatusContext = createContext<BootStatus | null>(null);

export const BootStateContext = createContext<BootState | null>(null);

export function useBootStatus(): BootStatus {
  const status = useContext(BootStatusContext);
  if (status === null) {
    throw new Error("useBootStatus must be used inside <BootstrapProvider>");
  }
  return status;
}

export function useBootState(): BootState {
  const state = useContext(BootStateContext);
  if (state === null) {
    throw new Error("useBootState must be used below a resolved <BootGate>");
  }
  return state;
}
