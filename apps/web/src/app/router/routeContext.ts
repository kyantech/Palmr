import type { QueryClient } from "@tanstack/react-query";
import { createContext, RouterContextProvider } from "react-router";

export const queryClientContext = createContext<QueryClient | null>(null);

export function routerContextFor(queryClient: QueryClient) {
  return () => {
    const provider = new RouterContextProvider();
    provider.set(queryClientContext, queryClient);
    return provider;
  };
}
