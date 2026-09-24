import { QueryClient } from "@tanstack/react-query";

export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 30_000,
        gcTime: 5 * 60_000,
        refetchOnReconnect: true,
        retry: false,
      },
      mutations: {
        retry: false,
      },
    },
  });
}
