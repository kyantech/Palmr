import { type QueryClient, useMutation, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { type AdminSettingsGroup, qk } from "../../../shared/api/query-keys";
import { type ApiError, detailChecks, isApiErrorCode } from "../../../shared/errors";
import type {
  CreatedInvite,
  CreateInviteRequest,
  CreateProviderRequest,
  CreateUserRequest,
  GeneralPatch,
  PasswordLoginRequest,
  PasswordReset,
  Provider,
  ProviderPage,
  PublicLinkPatch,
  QuotaOverrideRequest,
  QuotaPatch,
  SecurityPatch,
  SmtpPatch,
  SmtpTestRequest,
  UpdateProviderRequest,
  UpdateUserRequest,
  UserRole,
} from "../types";
import type { SettingsByGroup } from "./queries";

function refreshUser(client: QueryClient, userId: string) {
  return Promise.all([
    client.invalidateQueries({ queryKey: qk.admin.user(userId) }),
    client.invalidateQueries({ queryKey: qk.admin.usersAll() }),
  ]);
}

function refreshUserDetail(client: QueryClient, userId: string) {
  return client.invalidateQueries({ queryKey: qk.admin.user(userId) });
}

function refreshInvites(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.admin.invitesAll() });
}

export interface NewUser {
  body: CreateUserRequest;
  idempotencyKey: string;
}

export function useCreateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "users", "create"],
    mutationFn: ({ body, idempotencyKey }: NewUser) =>
      apiFetch("post", "/admin/users", {
        body,
        headers: { "Idempotency-Key": idempotencyKey },
      }),
    gcTime: 0,
    onSuccess: () => client.invalidateQueries({ queryKey: qk.admin.usersAll() }),
  });
}

export interface UserEdit {
  userId: string;
  body: UpdateUserRequest;
}

export function useUpdateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "update"],
    mutationFn: ({ userId, body }: UserEdit) =>
      apiFetch("patch", "/admin/users/{id}", { path: { id: userId }, body }),
    onSuccess: (_user, { userId }) => refreshUser(client, userId),
  });
}

export interface RoleChange {
  userId: string;
  role: UserRole;
}

export function useChangeRole() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "role"],
    mutationFn: ({ userId, role }: RoleChange) =>
      apiFetch("put", "/admin/users/{id}/role", { path: { id: userId }, body: { role } }),
    onSuccess: (_user, { userId }) => refreshUser(client, userId),
  });
}

export function useActivateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "activate"],
    mutationFn: (userId: string) =>
      apiFetch("post", "/admin/users/{id}/activate", { path: { id: userId } }),
    onSuccess: (_user, userId) => refreshUser(client, userId),
  });
}

export function useDeactivateUser() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "deactivate"],
    mutationFn: (userId: string) =>
      apiFetch("post", "/admin/users/{id}/deactivate", { path: { id: userId } }),
    onSuccess: (_user, userId) => refreshUser(client, userId),
  });
}

export function useUnlockUser() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "unlock"],
    mutationFn: (userId: string) =>
      apiFetch("post", "/admin/users/{id}/unlock", { path: { id: userId } }),
    onSuccess: (_result, userId) => refreshUser(client, userId),
  });
}

export function useRevokeUserSessions() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "sessions", "revoke"],
    mutationFn: (userId: string) =>
      apiFetch("delete", "/admin/users/{userId}/sessions", { path: { userId } }),
    onSuccess: (_result, userId) => refreshUserDetail(client, userId),
  });
}

export function useResetUserPassword(onTemporaryPassword: (temporaryPassword: string) => void) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "password-reset"],
    mutationFn: async (userId: string) => {
      const reset: PasswordReset = await apiFetch("post", "/admin/users/{id}/password-reset", {
        path: { id: userId },
      });
      onTemporaryPassword(reset.temporaryPassword);
    },
    gcTime: 0,
    onSuccess: (_result, userId) => refreshUser(client, userId),
  });
}

export interface QuotaOverrideChange {
  userId: string;
  body: QuotaOverrideRequest;
}

export function useSetUserQuota() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "quota"],
    mutationFn: ({ userId, body }: QuotaOverrideChange) =>
      apiFetch("put", "/admin/users/{id}/quota", { path: { id: userId }, body }),
    onSuccess: (_quota, { userId }) => refreshUser(client, userId),
  });
}

export interface EmailChange {
  userId: string;
  email: string;
}

export function useStartEmailChange() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "email", "start"],
    mutationFn: ({ userId, email }: EmailChange) =>
      apiFetch("post", "/admin/users/{id}/email", { path: { id: userId }, body: { email } }),
    onSuccess: (_result, { userId }) => refreshUser(client, userId),
  });
}

export function useResendEmailChange() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "email", "resend"],
    mutationFn: (userId: string) =>
      apiFetch("post", "/admin/users/{id}/email/resend", { path: { id: userId } }),
    onSuccess: (_result, userId) => refreshUser(client, userId),
  });
}

export function useCancelEmailChange() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "user", "email", "cancel"],
    mutationFn: (userId: string) =>
      apiFetch("delete", "/admin/users/{id}/email", { path: { id: userId } }),
    onSuccess: (_result, userId) => refreshUser(client, userId),
  });
}

export interface NewInvite {
  body: CreateInviteRequest;
  idempotencyKey: string;
}

export function useCreateInvite(onCreated: (invite: CreatedInvite) => void) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "invites", "create"],
    mutationFn: async ({ body, idempotencyKey }: NewInvite) => {
      const invite = await apiFetch("post", "/admin/invites", {
        body,
        headers: { "Idempotency-Key": idempotencyKey },
      });
      onCreated(invite);
    },
    gcTime: 0,
    onSuccess: () => refreshInvites(client),
  });
}

export function useResendInvite() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "invites", "resend"],
    mutationFn: (inviteId: string) =>
      apiFetch("post", "/admin/invites/{id}/resend", { path: { id: inviteId } }),
    onSuccess: () => refreshInvites(client),
  });
}

export function useRevokeInvite() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "invites", "revoke"],
    mutationFn: (inviteId: string) =>
      apiFetch("delete", "/admin/invites/{id}", { path: { id: inviteId } }),
    onSuccess: () => refreshInvites(client),
  });
}

export interface PatchByGroup {
  general: GeneralPatch;
  security: SecurityPatch;
  quotas: QuotaPatch;
  "public-links": PublicLinkPatch;
  smtp: SmtpPatch;
}

function settingsEffects(client: QueryClient, group: AdminSettingsGroup) {
  switch (group) {
    case "general":
      return [client.invalidateQueries({ queryKey: qk.bootstrap(), exact: true })];
    case "quotas":
      return [
        client.invalidateQueries({ queryKey: qk.me.effectiveSettings(), exact: true }),
        client.invalidateQueries({ queryKey: qk.admin.usersAll() }),
        client.invalidateQueries({ queryKey: qk.admin.userAll() }),
      ];
    case "security":
      return [
        client.invalidateQueries({ queryKey: qk.me.effectiveSettings(), exact: true }),
        client.invalidateQueries({ queryKey: qk.admin.passwordLogin() }),
        refetchBootstrap(client),
      ];
    case "public-links":
    case "smtp":
      return [client.invalidateQueries({ queryKey: qk.me.effectiveSettings(), exact: true })];
  }
}

function useSettingsMutation<Group extends AdminSettingsGroup>(
  group: Group,
  send: (body: PatchByGroup[Group]) => Promise<SettingsByGroup[Group]>,
  secret = false,
) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "settings", group, "update"],
    mutationFn: send,
    ...(secret ? { gcTime: 0 } : {}),
    onSuccess: async (settings) => {
      client.setQueryData(qk.admin.settings(group), settings);
      await Promise.all([
        client.invalidateQueries({ queryKey: qk.admin.settings(group), exact: true }),
        ...settingsEffects(client, group),
      ]);
    },
  });
}

export function useUpdateGeneralSettings() {
  return useSettingsMutation("general", (body) =>
    apiFetch("patch", "/admin/settings/general", { body }),
  );
}

export function useUpdateSecuritySettings() {
  return useSettingsMutation("security", (body) =>
    apiFetch("patch", "/admin/settings/security", { body }),
  );
}

export function useUpdateQuotaSettings() {
  return useSettingsMutation("quotas", (body) =>
    apiFetch("patch", "/admin/settings/quotas", { body }),
  );
}

export function useUpdatePublicLinkSettings() {
  return useSettingsMutation("public-links", (body) =>
    apiFetch("patch", "/admin/settings/public-links", { body }),
  );
}

export function useUpdateSmtpSettings() {
  return useSettingsMutation(
    "smtp",
    (body) => apiFetch("patch", "/admin/settings/smtp", { body }),
    true,
  );
}

export function useSmtpTest() {
  return useMutation({
    mutationKey: ["admin", "settings", "smtp", "test"],
    mutationFn: (body: SmtpTestRequest) => apiFetch("post", "/admin/settings/smtp/test", { body }),
    gcTime: 0,
    retry: false,
  });
}

function refetchBootstrap(client: QueryClient) {
  return client.refetchQueries({ queryKey: qk.bootstrap(), exact: true });
}

function refreshProviderState(client: QueryClient) {
  return Promise.all([
    client.invalidateQueries({ queryKey: qk.admin.providers() }),
    client.invalidateQueries({ queryKey: qk.admin.passwordLogin() }),
  ]);
}

async function refreshProvidersAndLogin(client: QueryClient) {
  await Promise.all([refreshProviderState(client), refetchBootstrap(client)]);
}

export function useCreateProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "providers", "create"],
    mutationFn: (body: CreateProviderRequest) => apiFetch("post", "/admin/providers", { body }),
    gcTime: 0,
    retry: false,
    onSuccess: () => refreshProvidersAndLogin(client),
  });
}

export interface ProviderEdit {
  id: string;
  body: UpdateProviderRequest;
}

export function useUpdateProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "providers", "update"],
    mutationFn: ({ id, body }: ProviderEdit) =>
      apiFetch("patch", "/admin/providers/{id}", { path: { id }, body }),
    gcTime: 0,
    retry: false,
    onSuccess: () => refreshProvidersAndLogin(client),
    onError: () => refreshProviderState(client),
  });
}

export function useDeleteProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "providers", "delete"],
    mutationFn: (id: string) => apiFetch("delete", "/admin/providers/{id}", { path: { id } }),
    retry: false,
    onSuccess: () => refreshProvidersAndLogin(client),
    onError: () => refreshProviderState(client),
  });
}

export function useReorderProviders() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "providers", "order"],
    mutationFn: (order: string[]) => apiFetch("put", "/admin/providers/order", { body: { order } }),
    retry: false,
    onSuccess: () => refreshProvidersAndLogin(client),
    onError: () => refreshProviderState(client),
  });
}

export function useDiscoverProvider() {
  return useMutation({
    mutationKey: ["admin", "providers", "discover"],
    mutationFn: (issuerUrl: string) =>
      apiFetch("post", "/admin/providers/discover", { body: { issuerUrl } }),
    gcTime: 0,
    retry: false,
  });
}

function patchProvider(client: QueryClient, id: string, patch: Partial<Provider>) {
  client.setQueryData<ProviderPage>(qk.admin.providers(), (page) =>
    page === undefined
      ? page
      : {
          ...page,
          items: page.items.map((item) => (item.id === id ? { ...item, ...patch } : item)),
        },
  );
}

function failureSummary(error: ApiError): string | null {
  const failing = detailChecks(error)
    .filter((check) => !check.ok)
    .map((check) => `${check.name}:${check.detail ?? "failed"}`);
  return failing.length === 0 ? null : failing.join(",").slice(0, 512);
}

export function useTestProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "providers", "test"],
    mutationFn: (id: string) => apiFetch("post", "/admin/providers/{id}/test", { path: { id } }),
    gcTime: 0,
    retry: false,
    onSuccess: (result, id) => {
      patchProvider(client, id, { validatedAt: result.validatedAt, validationError: null });
    },
    onError: (error, id) => {
      if (isApiErrorCode(error, "PROVIDER_VALIDATION_FAILED")) {
        patchProvider(client, id, { validatedAt: null, validationError: failureSummary(error) });
      }
    },
    onSettled: () => refreshProviderState(client),
  });
}

export function useSetPasswordLogin() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["admin", "password-login", "set"],
    mutationFn: (body: PasswordLoginRequest) =>
      apiFetch("put", "/admin/auth/password-login", { body }),
    gcTime: 0,
    retry: false,
    onSuccess: async (state) => {
      client.setQueryData(qk.admin.passwordLogin(), state);
      await Promise.all([
        client.invalidateQueries({ queryKey: qk.admin.passwordLogin() }),
        client.invalidateQueries({ queryKey: qk.admin.providers() }),
        refetchBootstrap(client),
      ]);
    },
    onError: () => client.invalidateQueries({ queryKey: qk.admin.passwordLogin() }),
  });
}
