import { afterEach, expect, test, vi } from "vitest";
import { ApiError, type ErrorCode } from "../errors";
import { createQueryClient } from "./queryClient";

function apiError(code: ErrorCode): ApiError {
  return new ApiError({
    code,
    status: 0,
    requestId: null,
    details: {},
    request: { method: "GET", path: "/probe" },
    serverMessage: code,
  });
}

afterEach(() => {
  vi.useRealTimers();
});

test("query defaults follow the accepted policy", () => {
  const client = createQueryClient();
  const { queries } = client.getDefaultOptions();

  expect(queries?.staleTime).toBe(30_000);
  expect(queries?.gcTime).toBe(300_000);
  expect(queries?.refetchOnReconnect).toBe(true);
  expect(queries?.refetchOnWindowFocus).toBeUndefined();
  expect(queries?.structuralSharing).toBeUndefined();
});

test("default retry never retries blindly", () => {
  const { queries, mutations } = createQueryClient().getDefaultOptions();

  expect(typeof queries?.retry).toBe("function");
  expect(typeof mutations?.retry).toBe("function");
  const queryRetry = queries?.retry as (count: number, error: unknown) => boolean;
  const mutationRetry = mutations?.retry as (count: number, error: unknown) => boolean;
  expect(queryRetry(0, new Error("render bug"))).toBe(false);
  expect(queryRetry(0, apiError("AUTH_REQUIRED"))).toBe(false);
  expect(mutationRetry(0, apiError("CLIENT_NETWORK_ERROR"))).toBe(false);
});

test("each call creates an independent client", () => {
  expect(createQueryClient()).not.toBe(createQueryClient());
});

test("the global error seam reports query and mutation failures without acting on them", async () => {
  vi.useFakeTimers();
  const onError = vi.fn();
  const client = createQueryClient({ onError });
  const failure = apiError("VALIDATION_ERROR");

  await client
    .query({ queryKey: ["probe"], queryFn: () => Promise.reject(failure) })
    .catch(() => undefined);
  await client
    .getMutationCache()
    .build(client, { mutationFn: () => Promise.reject(failure) })
    .execute(undefined)
    .catch(() => undefined);

  expect(onError).toHaveBeenNthCalledWith(1, failure, "query");
  expect(onError).toHaveBeenNthCalledWith(2, failure, "mutation");
});
