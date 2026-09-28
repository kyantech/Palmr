import { vi } from "vitest";
import type { RecoveryEnvironment } from "../app/error/staleChunk";

export function memoryEnvironment() {
  const store = new Map<string, string>();
  let now = 1_700_000_000_000;
  const reload = vi.fn();
  return {
    store,
    reload,
    advance(ms: number) {
      now += ms;
    },
    environment: {
      storage: () => ({
        getItem: (key: string) => store.get(key) ?? null,
        setItem: (key: string, value: string) => {
          store.set(key, value);
        },
      }),
      now: () => now,
      reload,
    } satisfies RecoveryEnvironment,
  };
}
