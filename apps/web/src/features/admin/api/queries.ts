import { keepPreviousData, queryOptions, useQuery } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { type AdminSettingsGroup, qk } from "../../../shared/api/query-keys";
import type {
  GeneralSettings,
  PublicLinkSettings,
  QuotaSettings,
  SecuritySettings,
  SmtpSettings,
} from "../types";
import {
  type InvitesListParams,
  invitesRequestQuery,
  type UsersListParams,
  usersRequestQuery,
} from "./params";

export const USER_SESSIONS_PAGE_SIZE = 50;
export const PROVIDERS_PAGE_SIZE = 100;

export function usersQueryOptions(params: UsersListParams) {
  return queryOptions({
    queryKey: qk.admin.users({ ...params }),
    queryFn: ({ signal }) =>
      apiFetch("get", "/admin/users", { query: usersRequestQuery(params), signal }),
  });
}

export function useUsers(params: UsersListParams) {
  return useQuery({ ...usersQueryOptions(params), placeholderData: keepPreviousData });
}

export function userQueryOptions(userId: string) {
  return queryOptions({
    queryKey: qk.admin.user(userId),
    queryFn: ({ signal }) => apiFetch("get", "/admin/users/{id}", { path: { id: userId }, signal }),
  });
}

export function useUser(userId: string) {
  return useQuery({ ...userQueryOptions(userId), refetchOnWindowFocus: false });
}

export function userSessionsQueryOptions(userId: string) {
  return queryOptions({
    queryKey: qk.admin.userSessions(userId),
    queryFn: ({ signal }) =>
      apiFetch("get", "/admin/users/{userId}/sessions", {
        path: { userId },
        query: { sort: "lastSeenAt:desc", limit: USER_SESSIONS_PAGE_SIZE },
        signal,
      }),
  });
}

export function useUserSessions(userId: string) {
  return useQuery(userSessionsQueryOptions(userId));
}

export function invitesQueryOptions(params: InvitesListParams) {
  return queryOptions({
    queryKey: qk.admin.invites({ ...params }),
    queryFn: ({ signal }) =>
      apiFetch("get", "/admin/invites", { query: invitesRequestQuery(params), signal }),
  });
}

export function useInvites(params: InvitesListParams) {
  return useQuery({ ...invitesQueryOptions(params), placeholderData: keepPreviousData });
}

export interface SettingsByGroup {
  general: GeneralSettings;
  security: SecuritySettings;
  quotas: QuotaSettings;
  "public-links": PublicLinkSettings;
  smtp: SmtpSettings;
}

type SettingsFetchers = {
  [Group in AdminSettingsGroup]: (signal: AbortSignal) => Promise<SettingsByGroup[Group]>;
};

const SETTINGS_FETCHERS: SettingsFetchers = {
  general: (signal) => apiFetch("get", "/admin/settings/general", { signal }),
  security: (signal) => apiFetch("get", "/admin/settings/security", { signal }),
  quotas: (signal) => apiFetch("get", "/admin/settings/quotas", { signal }),
  "public-links": (signal) => apiFetch("get", "/admin/settings/public-links", { signal }),
  smtp: (signal) => apiFetch("get", "/admin/settings/smtp", { signal }),
};

export function settingsQueryOptions<Group extends AdminSettingsGroup>(group: Group) {
  const fetcher = SETTINGS_FETCHERS[group];
  return queryOptions({
    queryKey: qk.admin.settings(group),
    queryFn: ({ signal }): Promise<SettingsByGroup[Group]> => fetcher(signal),
    refetchOnWindowFocus: false,
  });
}

export function useSettings<Group extends AdminSettingsGroup>(group: Group) {
  return useQuery(settingsQueryOptions(group));
}

export function providersQueryOptions() {
  return queryOptions({
    queryKey: qk.admin.providers(),
    queryFn: ({ signal }) =>
      apiFetch("get", "/admin/providers", { query: { limit: PROVIDERS_PAGE_SIZE }, signal }),
    refetchOnWindowFocus: false,
  });
}

export function useProviders() {
  return useQuery(providersQueryOptions());
}

export function providerPresetsQueryOptions() {
  return queryOptions({
    queryKey: qk.admin.providerPresets(),
    queryFn: ({ signal }) => apiFetch("get", "/admin/providers/presets", { signal }),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

export function useProviderPresets(enabled: boolean) {
  return useQuery({ ...providerPresetsQueryOptions(), enabled });
}

export function passwordLoginQueryOptions() {
  return queryOptions({
    queryKey: qk.admin.passwordLogin(),
    queryFn: ({ signal }) => apiFetch("get", "/admin/auth/password-login", { signal }),
    refetchOnWindowFocus: false,
  });
}

export function usePasswordLogin() {
  return useQuery(passwordLoginQueryOptions());
}
