import {
  type Mutation,
  MutationCache,
  type Query,
  QueryCache,
  QueryClient,
} from "@tanstack/react-query";
import { mutationOperationOf, type RandomSource, retryOptions } from "./retryPolicy";

export type QueryErrorSource = "query" | "mutation";

export type GlobalQueryError =
  | { source: "query"; error: unknown; client: QueryClient; query: Query<unknown, unknown> }
  | {
      source: "mutation";
      error: unknown;
      client: QueryClient;
      mutation: Mutation<unknown, unknown>;
    };

export interface QueryClientOptions {
  onError?: (event: GlobalQueryError) => void;
  random?: RandomSource;
}

export function createQueryClient({ onError, random }: QueryClientOptions = {}): QueryClient {
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => onError?.({ source: "query", error, client, query }),
    }),
    mutationCache: new MutationCache({
      onError: (error, _variables, _context, mutation) =>
        onError?.({ source: "mutation", error, client, mutation }),
    }),
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
  return client;
}
