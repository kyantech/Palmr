import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";
import { mutationOperationOf, type RandomSource, retryOptions } from "./retryPolicy";

export type QueryErrorSource = "query" | "mutation";

export interface QueryClientOptions {
  onError?: (error: unknown, source: QueryErrorSource) => void;
  random?: RandomSource;
}

export function createQueryClient({ onError, random }: QueryClientOptions = {}): QueryClient {
  return new QueryClient({
    queryCache: new QueryCache({ onError: (error) => onError?.(error, "query") }),
    mutationCache: new MutationCache({ onError: (error) => onError?.(error, "mutation") }),
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        gcTime: 5 * 60_000,
        refetchOnReconnect: true,
        ...retryOptions("query", random),
      },
      mutations: retryOptions(mutationOperationOf, random),
    },
  });
}
