import { describe, expect, test } from "vitest";
import { ApiError } from "../../shared/errors";
import { memoryEnvironment } from "../../test/staleChunkEnvironment";
import {
  browserRecoveryEnvironment,
  createStaleChunkRecovery,
  isStaleChunkError,
  STALE_CHUNK_GUARD_KEY,
  STALE_CHUNK_GUARD_WINDOW_MS,
} from "./staleChunk";

const chunkError = () =>
  new TypeError(
    "Failed to fetch dynamically imported module: https://palmr.test/assets/Files-3f9a.js",
  );

describe("stale chunk detection", () => {
  test.each([
    "Failed to fetch dynamically imported module: https://palmr.test/assets/a-1.js",
    "error loading dynamically imported module: https://palmr.test/assets/a-1.js",
    "Importing a module script failed.",
    "Unable to preload CSS for /assets/a-1.css",
  ])("recognises %j", (message) => {
    expect(isStaleChunkError(new TypeError(message))).toBe(true);
    expect(isStaleChunkError(new Error("route failed", { cause: new TypeError(message) }))).toBe(
      true,
    );
  });

  test("render errors, API errors and non-errors are not stale chunks", () => {
    expect(isStaleChunkError(new TypeError("Cannot read properties of undefined"))).toBe(false);
    expect(isStaleChunkError(new Error("Failed to fetch"))).toBe(false);
    expect(isStaleChunkError("Failed to fetch dynamically imported module")).toBe(false);
    expect(isStaleChunkError(null)).toBe(false);
    expect(
      isStaleChunkError(
        new ApiError({
          code: "CLIENT_NETWORK_ERROR",
          status: 0,
          requestId: null,
          details: {},
          request: { method: "GET", path: "/x" },
          serverMessage: "Failed to fetch dynamically imported module",
        }),
      ),
    ).toBe(false);
  });
});

describe("one guarded reload", () => {
  test("the first stale chunk reloads once and records the attempt", () => {
    const memory = memoryEnvironment();
    const recovery = createStaleChunkRecovery(memory.environment);

    expect(recovery.recover(chunkError())).toBe("reloading");
    expect(memory.reload).toHaveBeenCalledTimes(1);
    expect(memory.store.get(STALE_CHUNK_GUARD_KEY)).toBe("1700000000000");
  });

  test("the same error reported twice reloads once", () => {
    const memory = memoryEnvironment();
    const recovery = createStaleChunkRecovery(memory.environment);
    const error = chunkError();

    expect(recovery.recover(error)).toBe("reloading");
    expect(recovery.recover(error)).toBe("reloading");
    expect(memory.reload).toHaveBeenCalledTimes(1);
  });

  test("a second stale chunk inside the window does not reload again", () => {
    const memory = memoryEnvironment();
    createStaleChunkRecovery(memory.environment).recover(chunkError());
    memory.advance(STALE_CHUNK_GUARD_WINDOW_MS - 1);

    const afterReload = createStaleChunkRecovery(memory.environment);

    expect(afterReload.recover(chunkError())).toBe("exhausted");
    expect(afterReload.recover(chunkError())).toBe("exhausted");
    expect(memory.reload).toHaveBeenCalledTimes(1);
  });

  test("an expired guard permits one future recovery reload", () => {
    const memory = memoryEnvironment();
    createStaleChunkRecovery(memory.environment).recover(chunkError());
    memory.advance(STALE_CHUNK_GUARD_WINDOW_MS);

    const later = createStaleChunkRecovery(memory.environment);

    expect(later.recover(chunkError())).toBe("reloading");
    expect(later.recover(chunkError())).toBe("exhausted");
    expect(memory.reload).toHaveBeenCalledTimes(2);
  });

  test("a normal error never touches the guard or reloads", () => {
    const memory = memoryEnvironment();

    expect(createStaleChunkRecovery(memory.environment).recover(new Error("boom"))).toBe(
      "notStale",
    );
    expect(memory.reload).not.toHaveBeenCalled();
    expect(memory.store.size).toBe(0);
  });

  test("unusable storage means no automatic reload rather than a possible loop", () => {
    const memory = memoryEnvironment();
    const recovery = createStaleChunkRecovery({
      ...memory.environment,
      storage: () => {
        throw new DOMException("denied", "SecurityError");
      },
    });

    expect(recovery.recover(chunkError())).toBe("exhausted");
    expect(memory.reload).not.toHaveBeenCalled();
  });

  test("the browser recovery guards with sessionStorage, never localStorage", () => {
    expect(browserRecoveryEnvironment.storage()).toBe(window.sessionStorage);
    expect(browserRecoveryEnvironment.storage()).not.toBe(window.localStorage);
  });
});
