import { afterEach, expect, test, vi } from "vitest";
import {
  discardRecentAuthChallenge,
  openRecentAuthChallenge,
  recentAuthStore,
  takeRecentAuthReplay,
} from "./store";

afterEach(() => {
  discardRecentAuthChallenge();
});

test("unit_recent_auth_replay_is_taken_at_most_once", async () => {
  const replay = vi.fn(() => Promise.resolve("done"));
  const id = openRecentAuthChallenge({ originPath: "/settings", requestId: "req-1", replay });

  const taken = takeRecentAuthReplay(id);
  expect(recentAuthStore.getState().challenge).toBeNull();
  expect(takeRecentAuthReplay(id)).toBeNull();
  await taken?.();

  expect(replay).toHaveBeenCalledTimes(1);
});

test("unit_recent_auth_stale_ids_cannot_take_or_discard_a_newer_challenge", () => {
  const first = openRecentAuthChallenge({
    originPath: "/a",
    requestId: null,
    replay: () => Promise.resolve(),
  });
  const second = openRecentAuthChallenge({
    originPath: "/b",
    requestId: null,
    replay: () => Promise.resolve(),
  });

  expect(takeRecentAuthReplay(first)).toBeNull();
  discardRecentAuthChallenge(first);
  expect(recentAuthStore.getState().challenge?.id).toBe(second);

  discardRecentAuthChallenge();
  expect(recentAuthStore.getState().challenge).toBeNull();
});

test("unit_recent_auth_store_is_memory_only", () => {
  const setItem = vi.spyOn(Storage.prototype, "setItem");
  openRecentAuthChallenge({
    originPath: "/settings",
    requestId: null,
    replay: () => Promise.resolve(),
  });

  expect(setItem).not.toHaveBeenCalled();
  expect(Object.keys(recentAuthStore)).not.toContain("persist");
  setItem.mockRestore();
});
