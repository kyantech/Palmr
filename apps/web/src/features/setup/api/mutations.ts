import { useMutation } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import type { components } from "../../../shared/api/schema";

export type SetupRequest = components["schemas"]["SetupRequest"];

export function useCompleteSetup() {
  return useMutation({
    mutationKey: ["setup", "complete"],
    mutationFn: (body: SetupRequest) => apiFetch("post", "/setup", { body }),
    retry: false,
  });
}
