import {
  type InfiniteData,
  type QueryClient,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type { components } from "../../../shared/api/schema";
import { isApiErrorCode } from "../../../shared/errors";
import {
  type ExternalReauthStart,
  externalNavigation,
  externalReauthChannelOrFail,
  providerUrlOrFail,
} from "../externalNavigation";
import { beginMfaChallenge, clearMfaChallenge, mfaChallengeStore, setLoginNotice } from "../store";

export type ReauthenticateRequest = components["schemas"]["ReauthenticateRequest"];
export type LoginRequest = components["schemas"]["LoginRequest"];
export type AcceptInviteRequest = components["schemas"]["AcceptInviteRequest"];
export type Enrollment = components["schemas"]["EnrollmentResponse"];
export type BackupCodes = components["schemas"]["BackupCodesResponse"];
type TrustedDeviceList = components["schemas"]["TrustedDeviceList"];

export type LoginOutcome = "signedIn" | "mfaRequired";
export type SecondFactorOutcome = "signedIn" | "challengeMissing";

export interface SecondFactorSubmission {
  code: string;
  rememberDevice: boolean;
}

export interface EnrollmentVerification {
  enrollmentId: string;
  code: string;
}

function refreshMe(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.me.current(), exact: true });
}

function refreshTwoFactor(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.me.twoFactor(), exact: true });
}

function refreshTrustedDevices(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.me.trustedDevices() });
}

function removeTrustedDevices(client: QueryClient, keep: (id: string) => boolean) {
  client.setQueryData<InfiniteData<TrustedDeviceList, string | null>>(
    qk.me.trustedDevices(),
    (data) =>
      data === undefined
        ? data
        : {
            ...data,
            pages: data.pages.map((page) => ({
              ...page,
              items: page.items.filter((item) => keep(item.id)),
            })),
          },
  );
}

export function useReauthenticate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "reauthenticate"],
    mutationFn: (body: ReauthenticateRequest) => apiFetch("post", "/auth/reauthenticate", { body }),
    retry: false,
    gcTime: 0,
    onSuccess: () => refreshMe(queryClient),
  });
}

export interface ExternalLoginStart {
  slug: string;
  returnTo: string | null;
}

export function useStartExternalLogin() {
  return useMutation({
    mutationKey: ["auth", "providers", "authorize"],
    mutationFn: async ({ slug, returnTo }: ExternalLoginStart) => {
      const response = await apiFetch("post", "/auth/providers/{slug}/authorize", {
        path: { slug },
        body: { purpose: "login", ...(returnTo === null ? {} : { returnTo }) },
      });
      return providerUrlOrFail(response.authorizationUrl, {
        method: "POST",
        path: "/auth/providers/{slug}/authorize",
      });
    },
    retry: false,
    gcTime: 0,
    onSuccess: (url) => {
      externalNavigation.assign(url);
    },
  });
}

export function useStartIdentityLink() {
  return useMutation({
    mutationKey: ["me", "identity-links", "start"],
    mutationFn: async ({ slug }: { slug: string }) => {
      const response = await apiFetch("post", "/auth/providers/{slug}/link", { path: { slug } });
      return providerUrlOrFail(response.authorizationUrl, {
        method: "POST",
        path: "/auth/providers/{slug}/link",
      });
    },
    retry: false,
    gcTime: 0,
    onSuccess: (url) => {
      externalNavigation.assign(url);
    },
  });
}

export function useUnlinkIdentity(onUnlinked: () => Promise<void>) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "identity-links", "unlink"],
    mutationFn: ({ id }: { id: string }) =>
      apiFetch("delete", "/identity-links/{id}", { path: { id } }),
    retry: false,
    onSuccess: async () => {
      setLoginNotice("identityUnlinked");
      await onUnlinked();
    },
    onError: (error) => {
      if (!isApiErrorCode(error, "AUTH_RECENT_AUTH_REQUIRED")) {
        void client.invalidateQueries({ queryKey: qk.me.identityLinks() });
      }
    },
  });
}

export function useStartExternalReauthentication() {
  return useMutation({
    mutationKey: ["auth", "reauthenticate", "external"],
    mutationFn: async (): Promise<ExternalReauthStart> => {
      const response = await apiFetch("post", "/auth/reauthenticate", { body: {} });
      const request = { method: "POST", path: "/auth/reauthenticate" } as const;
      const url = providerUrlOrFail(response?.externalReauthUrl, request);
      return {
        url,
        channel: externalReauthChannelOrFail(response?.externalReauthChannel, request),
      };
    },
    retry: false,
    gcTime: 0,
  });
}

export function useLogin() {
  return useMutation({
    mutationKey: ["auth", "login"],
    mutationFn: async (body: LoginRequest): Promise<LoginOutcome> => {
      clearMfaChallenge();
      try {
        await apiFetch("post", "/auth/login", { body });
        return "signedIn";
      } catch (error) {
        if (isApiErrorCode(error, "AUTH_2FA_REQUIRED") && beginMfaChallenge(error.details)) {
          return "mfaRequired";
        }
        throw error;
      }
    },
    retry: false,
    gcTime: 0,
  });
}

export function useLoginSecondFactor() {
  return useMutation({
    mutationKey: ["auth", "login", "second-factor"],
    mutationFn: async ({
      code,
      rememberDevice,
    }: SecondFactorSubmission): Promise<SecondFactorOutcome> => {
      const { challenge } = mfaChallengeStore.getState();
      if (challenge === null) {
        return "challengeMissing";
      }
      await apiFetch("post", "/auth/login/totp", {
        body: {
          mfaToken: challenge.mfaToken,
          code,
          rememberDevice: rememberDevice && challenge.trustedDeviceOffered,
        },
      });
      return "signedIn";
    },
    retry: false,
    gcTime: 0,
  });
}

export function useLogout() {
  return useMutation({
    mutationKey: ["auth", "logout"],
    mutationFn: () => apiFetch("post", "/auth/logout"),
    retry: false,
  });
}

export function useForgotPassword() {
  return useMutation({
    mutationKey: ["auth", "password", "forgot"],
    mutationFn: ({ identifier }: { identifier: string }) =>
      apiFetch("post", "/auth/password/forgot", { body: { identifier } }),
    retry: false,
    gcTime: 0,
  });
}

export function useResetPassword(token: string) {
  return useMutation({
    mutationKey: ["auth", "password", "reset"],
    mutationFn: ({ newPassword }: { newPassword: string }) =>
      apiFetch("post", "/auth/password/reset", { body: { token, newPassword } }),
    retry: false,
    gcTime: 0,
  });
}

export function useVerifyEmail(token: string) {
  return useMutation({
    mutationKey: ["auth", "email", "verify"],
    mutationFn: () => apiFetch("post", "/auth/email/verify", { body: { token } }),
    retry: false,
    gcTime: 0,
  });
}

export function useAcceptInvite(token: string) {
  return useMutation({
    mutationKey: ["auth", "invite", "accept"],
    mutationFn: ({ firstName, lastName, username, password, locale }: AcceptInviteRequest) =>
      apiFetch("post", "/public/invites/{token}/accept", {
        path: { token },
        body: { firstName, lastName, username, password, locale },
      }),
    retry: false,
    gcTime: 0,
  });
}

export function useForcedPasswordChange() {
  return useMutation({
    mutationKey: ["auth", "password", "forced-change"],
    mutationFn: ({ newPassword }: { newPassword: string }) =>
      apiFetch("post", "/profile/password", { body: { newPassword } }),
    retry: false,
    gcTime: 0,
  });
}

export function useStartEnrollment(deliver: (enrollment: Enrollment) => void) {
  return useMutation({
    mutationKey: ["auth", "two-factor", "enroll"],
    mutationFn: async () => {
      deliver(await apiFetch("post", "/auth/2fa/enroll"));
    },
    retry: false,
    gcTime: 0,
  });
}

export function useVerifyEnrollment(deliver: (codes: BackupCodes) => void) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "two-factor", "verify"],
    mutationFn: async ({ enrollmentId, code }: EnrollmentVerification) => {
      deliver(await apiFetch("post", "/auth/2fa/enroll/verify", { body: { enrollmentId, code } }));
    },
    retry: false,
    gcTime: 0,
    onSuccess: () =>
      Promise.all([
        refreshTwoFactor(client),
        refreshTrustedDevices(client),
        client.invalidateQueries({ queryKey: qk.me.sessions() }),
      ]),
  });
}

export function useRegenerateBackupCodes(deliver: (codes: BackupCodes) => void) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "two-factor", "backup-codes"],
    mutationFn: async () => {
      deliver(await apiFetch("post", "/auth/2fa/backup-codes/regenerate"));
    },
    retry: false,
    gcTime: 0,
    onSuccess: () => refreshTwoFactor(client),
  });
}

export function useDisableTwoFactor(onDisabled: () => Promise<void>) {
  return useMutation({
    mutationKey: ["auth", "two-factor", "disable"],
    mutationFn: () => apiFetch("post", "/auth/2fa/disable"),
    retry: false,
    onSuccess: () => onDisabled(),
  });
}

export function useRevokeTrustedDevice() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "trusted-devices", "revoke"],
    mutationFn: ({ id }: { id: string }) =>
      apiFetch("delete", "/auth/trusted-devices/{id}", { path: { id } }),
    retry: false,
    onSuccess: (_result, { id }) => {
      removeTrustedDevices(client, (item) => item !== id);
    },
    onSettled: () => refreshTrustedDevices(client),
  });
}

export function useRevokeAllTrustedDevices() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["auth", "trusted-devices", "revoke-all"],
    mutationFn: () => apiFetch("delete", "/auth/trusted-devices"),
    retry: false,
    onSuccess: () => {
      removeTrustedDevices(client, () => false);
    },
    onSettled: () => refreshTrustedDevices(client),
  });
}
