import { useQuery } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";

export const setupKeys = {
  status: () => ["setup", "status"] as const,
};

export function useSetupStatus() {
  return useQuery({
    queryKey: setupKeys.status(),
    queryFn: () => apiFetch("get", "/setup/status"),
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
}
