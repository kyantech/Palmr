import { useMutation, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";

export type ReauthenticateRequest = components["schemas"]["ReauthenticateRequest"];
export type LoginRequest = components["schemas"]["LoginRequest"];

export function useReauthenticate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "reauthenticate"],
    mutationFn: (body: ReauthenticateRequest) => apiFetch("post", "/auth/reauthenticate", { body }),
    retry: false,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: qk.me.current(), exact: true }),
  });
}

export function useLogin() {
  return useMutation({
    mutationKey: ["auth", "login"],
    mutationFn: (body: LoginRequest) => apiFetch("post", "/auth/login", { body }),
    retry: false,
  });
}

export function useLogout() {
  return useMutation({
    mutationKey: ["auth", "logout"],
    mutationFn: () => apiFetch("post", "/auth/logout"),
    retry: false,
  });
}
