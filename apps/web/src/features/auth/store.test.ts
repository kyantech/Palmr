import { afterEach, expect, test, vi } from "vitest";
import {
  beginMfaChallenge,
  clearMfaChallenge,
  discardRecentAuthChallenge,
  isMfaChallengeExpired,
  mfaChallengeStore,
  openRecentAuthChallenge,
  recentAuthStore,
  takeRecentAuthReplay,
} from "./store";

afterEach(() => {
  discardRecentAuthChallenge();
  clearMfaChallenge();
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

const NOW = Date.parse("2026-09-28T12:00:00Z");
const EXPIRES = "2026-09-28T12:05:00Z";

test("unit_mfa_challenge_captures_only_the_structured_details", () => {
  expect(
    beginMfaChallenge({
      mfaToken: "raw-token",
      expiresAt: "2026-09-28T12:05:00Z",
      methods: ["totp", "backup_code", "sms"],
      trustedDeviceOffered: true,
    }),
  ).toBe(true);

  expect(mfaChallengeStore.getState().challenge).toEqual({
    mfaToken: "raw-token",
    expiresAt: "2026-09-28T12:05:00Z",
    methods: ["totp", "backup_code"],
    trustedDeviceOffered: true,
    deadline: NOW + 300_000,
  });
});

test("unit_mfa_challenge_rejects_malformed_details_and_clears_any_previous_one", () => {
  beginMfaChallenge({ mfaToken: "old", expiresAt: "2026-09-28T12:05:00Z" });

  expect(beginMfaChallenge({ expiresAt: "2026-09-28T12:05:00Z" })).toBe(false);
  expect(mfaChallengeStore.getState().challenge).toBeNull();
  expect(beginMfaChallenge({ mfaToken: "", expiresAt: "x" })).toBe(false);
});

test("unit_mfa_challenge_is_locally_expired_at_its_server_deadline", () => {
  beginMfaChallenge({ mfaToken: "t", expiresAt: EXPIRES });
  const live = mfaChallengeStore.getState().challenge;
  expect(live?.deadline).toBe(Date.parse(EXPIRES));
  expect(live !== null && isMfaChallengeExpired(live, NOW + 299_999)).toBe(false);
  expect(live !== null && isMfaChallengeExpired(live, NOW + 300_000)).toBe(true);
});

test("unit_mfa_challenge_with_an_unparsable_expiry_is_rejected", () => {
  expect(beginMfaChallenge({ mfaToken: "t", expiresAt: "not a time" })).toBe(false);
  expect(mfaChallengeStore.getState().challenge).toBeNull();
});

test("unit_mfa_challenge_store_is_memory_only", () => {
  const setItem = vi.spyOn(Storage.prototype, "setItem");
  beginMfaChallenge({ mfaToken: "raw-token", expiresAt: "2026-09-28T12:05:00Z" });

  expect(setItem).not.toHaveBeenCalled();
  expect(Object.keys(mfaChallengeStore)).not.toContain("persist");
  expect(document.cookie).not.toContain("raw-token");
  setItem.mockRestore();
});
