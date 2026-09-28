import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { type ReactNode, useMemo } from "react";
import { type BootStatus, BootStatusContext } from "./bootState";
import { bootstrapQueryOptions, currentSessionQueryOptions } from "./queries";

type Phase = "loading" | "failed" | "ready";

function phaseOf(query: UseQueryResult): Phase {
  if (query.data !== undefined) {
    return "ready";
  }
  return query.isError && !query.isFetching ? "failed" : "loading";
}

export function BootstrapProvider({ children }: { children: ReactNode }) {
  const bootstrapQuery = useQuery(bootstrapQueryOptions());
  const setupCompleted = bootstrapQuery.data?.setupCompleted === true;
  const meQuery = useQuery({ ...currentSessionQueryOptions(), enabled: setupCompleted });

  const bootstrapPhase = phaseOf(bootstrapQuery);
  const mePhase = setupCompleted ? phaseOf(meQuery) : "ready";
  const { data: bootstrap, error: bootstrapError, refetch: refetchBootstrap } = bootstrapQuery;
  const { data: me, error: meError, refetch: refetchMe } = meQuery;

  const status = useMemo<BootStatus>(() => {
    if (bootstrapPhase === "failed") {
      return { phase: "failed", error: bootstrapError, retry: () => void refetchBootstrap() };
    }
    if (bootstrapPhase === "loading" || bootstrap === undefined) {
      return { phase: "loading" };
    }
    if (!setupCompleted) {
      return { phase: "ready", state: { bootstrap, me: null } };
    }
    if (mePhase === "failed") {
      return { phase: "failed", error: meError, retry: () => void refetchMe() };
    }
    if (mePhase === "loading" || me === undefined) {
      return { phase: "loading" };
    }
    return { phase: "ready", state: { bootstrap, me } };
  }, [
    bootstrap,
    bootstrapError,
    bootstrapPhase,
    me,
    meError,
    mePhase,
    refetchBootstrap,
    refetchMe,
    setupCompleted,
  ]);

  return <BootStatusContext value={status}>{children}</BootStatusContext>;
}
