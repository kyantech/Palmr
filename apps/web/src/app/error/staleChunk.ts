import { ApiError } from "../../shared/errors";

const CHUNK_LOAD_FAILURES = [
  /Failed to fetch dynamically imported module/i,
  /error loading dynamically imported module/i,
  /Importing a module script failed/i,
  /Unable to preload CSS for /i,
];

const MAX_CAUSE_DEPTH = 4;

export const STALE_CHUNK_GUARD_KEY = "palmr.staleChunkReloadAt";
export const STALE_CHUNK_GUARD_WINDOW_MS = 30_000;

export type StaleChunkOutcome = "notStale" | "reloading" | "exhausted";

export function isStaleChunkError(error: unknown): boolean {
  let current: unknown = error;
  for (let depth = 0; depth <= MAX_CAUSE_DEPTH && current instanceof Error; depth += 1) {
    if (current instanceof ApiError) {
      return false;
    }
    const { message } = current;
    if (CHUNK_LOAD_FAILURES.some((pattern) => pattern.test(message))) {
      return true;
    }
    current = current.cause;
  }
  return false;
}

export interface RecoveryEnvironment {
  storage: () => Pick<Storage, "getItem" | "setItem">;
  now: () => number;
  reload: () => void;
}

export interface StaleChunkRecovery {
  recover: (error: unknown) => StaleChunkOutcome;
  reload: () => void;
}

export const browserRecoveryEnvironment: RecoveryEnvironment = {
  storage: () => window.sessionStorage,
  now: () => Date.now(),
  reload: () => {
    window.location.reload();
  },
};

export function createStaleChunkRecovery(
  environment: RecoveryEnvironment = browserRecoveryEnvironment,
): StaleChunkRecovery {
  const outcomes = new WeakMap<object, StaleChunkOutcome>();

  const claimReload = (): boolean => {
    try {
      const storage = environment.storage();
      const now = environment.now();
      const last = Number(storage.getItem(STALE_CHUNK_GUARD_KEY));
      const elapsed = now - last;
      if (last > 0 && elapsed >= 0 && elapsed < STALE_CHUNK_GUARD_WINDOW_MS) {
        return false;
      }
      storage.setItem(STALE_CHUNK_GUARD_KEY, String(now));
      return true;
    } catch {
      return false;
    }
  };

  return {
    recover: (error) => {
      if (!isStaleChunkError(error)) {
        return "notStale";
      }
      const key = error as object;
      const known = outcomes.get(key);
      if (known) {
        return known;
      }
      const outcome = claimReload() ? "reloading" : "exhausted";
      outcomes.set(key, outcome);
      if (outcome === "reloading") {
        environment.reload();
      }
      return outcome;
    },
    reload: environment.reload,
  };
}

export const staleChunkRecovery = createStaleChunkRecovery();

export function requestIdOf(error: unknown): string | null {
  return error instanceof ApiError ? error.requestId : null;
}
