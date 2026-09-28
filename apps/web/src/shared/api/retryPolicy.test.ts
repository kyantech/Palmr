import { afterEach, describe, expect, test, vi } from "vitest";
import { ApiError, type ErrorCode } from "../errors";
import { createQueryClient } from "./queryClient";
import {
  idempotentMutationRetry,
  RETRY_AFTER_CAP_MS,
  RETRY_CLASS,
  retryClassOf,
  retryDelay,
  shouldRetry,
} from "./retryPolicy";

interface ErrorShape {
  code: ErrorCode;
  status?: number;
  method?: string;
  retryAfterSeconds?: number | null;
}

function apiError({ code, status = 0, method = "GET", retryAfterSeconds = null }: ErrorShape) {
  return new ApiError({
    code,
    status,
    requestId: null,
    details: {},
    request: { method, path: "/probe" },
    serverMessage: `message mentioning quota, offline and retry for ${code}`,
    retryAfterSeconds,
  });
}

const midpoint = () => 0.5;

async function countQueryAttempts(error: ApiError): Promise<number> {
  const client = createQueryClient({ random: midpoint });
  const queryFn = vi.fn(() => Promise.reject(error));
  const settled = client.query({ queryKey: ["probe"], queryFn }).catch(() => undefined);
  await vi.runAllTimersAsync();
  await settled;
  return queryFn.mock.calls.length;
}

async function countMutationAttempts(error: ApiError, options = {}): Promise<number> {
  const client = createQueryClient({ random: midpoint });
  const mutationFn = vi.fn(() => Promise.reject(error));
  const settled = client
    .getMutationCache()
    .build(client, { mutationFn, ...options })
    .execute(undefined)
    .catch(() => undefined);
  await vi.runAllTimersAsync();
  await settled;
  return mutationFn.mock.calls.length;
}

afterEach(() => {
  vi.useRealTimers();
});

test("unit_retry_policy_by_error_class", async () => {
  vi.useFakeTimers();

  expect(await countQueryAttempts(apiError({ code: "CLIENT_NETWORK_ERROR" }))).toBe(4);
  expect(await countQueryAttempts(apiError({ code: "INTERNAL_ERROR", status: 500 }))).toBe(3);
  expect(
    await countQueryAttempts(apiError({ code: "RATE_LIMITED", status: 429, retryAfterSeconds: 2 })),
  ).toBe(2);
  expect(await countQueryAttempts(apiError({ code: "AUTH_REQUIRED", status: 401 }))).toBe(1);
  expect(
    await countQueryAttempts(apiError({ code: "AUTH_RECENT_AUTH_REQUIRED", status: 403 })),
  ).toBe(1);
  expect(await countQueryAttempts(apiError({ code: "VALIDATION_ERROR", status: 422 }))).toBe(1);
  expect(
    await countQueryAttempts(apiError({ code: "QUOTA_EXCEEDED" as ErrorCode, status: 507 })),
  ).toBe(1);

  const postNetwork = apiError({ code: "CLIENT_NETWORK_ERROR", method: "POST" });
  expect(await countMutationAttempts(postNetwork)).toBe(1);
  expect(await countMutationAttempts(postNetwork, idempotentMutationRetry)).toBe(4);
  expect(
    await countMutationAttempts(apiError({ code: "CLIENT_NETWORK_ERROR", method: "PUT" })),
  ).toBe(4);
  expect(
    await countMutationAttempts(apiError({ code: "CLIENT_NETWORK_ERROR", method: "DELETE" })),
  ).toBe(4);
});

describe("retry classification", () => {
  test("classes are chosen by code, never by status or message", () => {
    expect(retryClassOf(apiError({ code: "STORAGE_FULL", status: 507 }))).toBe("serverFault");
    expect(retryClassOf(apiError({ code: "QUOTA_EXCEEDED" as ErrorCode, status: 507 }))).toBe(
      "never",
    );
    expect(retryClassOf(apiError({ code: "VALIDATION_ERROR", status: 503 }))).toBe("never");
    expect(retryClassOf(apiError({ code: "SERVICE_UNAVAILABLE", status: 200 }))).toBe(
      "serverFault",
    );
  });

  test("the transient network codes share one class", () => {
    for (const code of [
      "CLIENT_OFFLINE",
      "CLIENT_NETWORK_ERROR",
      "CLIENT_PROXY_BAD_GATEWAY",
      "CLIENT_PROXY_TIMEOUT",
    ] as const) {
      expect(RETRY_CLASS[code]).toBe("transient");
    }
  });

  test("authorization, validation and policy codes never retry", () => {
    for (const code of [
      "FORBIDDEN",
      "NOT_FOUND",
      "VALIDATION_ERROR",
      "AUTH_REQUIRED",
      "AUTH_RECENT_AUTH_REQUIRED",
      "CLIENT_ABORTED",
    ] as const) {
      expect(shouldRetry("query", 0, apiError({ code }))).toBe(false);
    }
  });

  test("codes unknown to this build and non-API errors never retry", () => {
    expect(retryClassOf(apiError({ code: "GONE" as ErrorCode, status: 410 }))).toBe("never");
    expect(retryClassOf(apiError({ code: "toString" as ErrorCode }))).toBe("never");
    expect(retryClassOf(new TypeError("Failed to fetch"))).toBe("never");
    expect(shouldRetry("query", 0, "offline")).toBe(false);
  });

  test("server faults are not retried by mutations, even idempotent ones", () => {
    const fault = apiError({ code: "INTERNAL_ERROR", method: "PUT" });

    expect(shouldRetry("query", 1, fault)).toBe(true);
    expect(shouldRetry("query", 2, fault)).toBe(false);
    expect(shouldRetry("idempotentMutation", 0, fault)).toBe(false);
    expect(shouldRetry("mutation", 0, fault)).toBe(false);
  });

  test("a rate-limited mutation gets the single documented retry", () => {
    const limited = apiError({ code: "RATE_LIMITED", method: "POST", retryAfterSeconds: 1 });

    expect(shouldRetry("mutation", 0, limited)).toBe(true);
    expect(shouldRetry("mutation", 1, limited)).toBe(false);
  });
});

describe("retry delay", () => {
  test("transient backoff doubles from one second and caps at fifteen", () => {
    const error = apiError({ code: "CLIENT_OFFLINE" });

    expect([0, 1, 2, 3, 4, 10].map((n) => retryDelay(n, error, midpoint))).toEqual([
      1_000, 2_000, 4_000, 8_000, 15_000, 15_000,
    ]);
  });

  test("jitter stays within plus or minus twenty percent", () => {
    const error = apiError({ code: "CLIENT_OFFLINE" });

    expect(retryDelay(1, error, () => 0)).toBe(1_600);
    expect(retryDelay(1, error, () => 1)).toBe(2_400);
    expect(retryDelay(4, error, () => 1)).toBe(18_000);
  });

  test("Retry-After is honoured and capped at sixty seconds", () => {
    expect(retryDelay(0, apiError({ code: "RATE_LIMITED", retryAfterSeconds: 7 }), midpoint)).toBe(
      7_000,
    );
    expect(
      retryDelay(0, apiError({ code: "CLIENT_RATE_LIMITED", retryAfterSeconds: 3_600 }), midpoint),
    ).toBe(RETRY_AFTER_CAP_MS);
  });

  test("a rate limit without Retry-After falls back to backoff", () => {
    expect(retryDelay(0, apiError({ code: "CLIENT_RATE_LIMITED" }), midpoint)).toBe(1_000);
  });

  test("the query waits for Retry-After before its single retry", async () => {
    vi.useFakeTimers();
    const client = createQueryClient({ random: midpoint });
    const queryFn = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(apiError({ code: "RATE_LIMITED", retryAfterSeconds: 5 }))
      .mockResolvedValueOnce("ok");

    const result = client.query({ queryKey: ["limited"], queryFn });
    await vi.advanceTimersByTimeAsync(4_999);
    expect(queryFn).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);

    await expect(result).resolves.toBe("ok");
    expect(queryFn).toHaveBeenCalledTimes(2);
  });
});
