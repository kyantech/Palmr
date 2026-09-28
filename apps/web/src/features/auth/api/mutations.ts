import { useMutation, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";

export type ReauthenticateRequest = components["schemas"]["ReauthenticateRequest"];

export function useReauthenticate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "reauthenticate"],
    mutationFn: (body: ReauthenticateRequest) => apiFetch("post", "/auth/reauthenticate", { body }),
    retry: false,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: qk.me.current(), exact: true }),
  });
}
