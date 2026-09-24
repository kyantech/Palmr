import { expect, test } from "vitest";
import { createQueryClient } from "./queryClient";

test("query defaults never retry blindly", () => {
  const client = createQueryClient();
  const { queries, mutations } = client.getDefaultOptions();

  expect(queries?.retry).toBe(false);
  expect(mutations?.retry).toBe(false);
  expect(queries?.staleTime).toBe(30_000);
  expect(queries?.gcTime).toBe(300_000);
  expect(queries?.refetchOnReconnect).toBe(true);
});

test("each call creates an independent client", () => {
  expect(createQueryClient()).not.toBe(createQueryClient());
});
