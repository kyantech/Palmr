import {
  type InfiniteData,
  type QueryClient,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { apiFetch } from "../../../shared/api/apiFetch";
import { qk } from "../../../shared/api/query-keys";
import type {
  Me,
  PasswordChange,
  PreferenceChange,
  Preferences,
  Profile,
  ProfileChange,
  SessionItem,
  SessionPage,
} from "../types";

function refreshMe(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.me.current(), exact: true });
}

function refreshSessions(client: QueryClient) {
  return client.invalidateQueries({ queryKey: qk.me.sessions() });
}

function patchMe(client: QueryClient, patch: (user: Profile) => Profile) {
  client.setQueryData<Me | null>(qk.me.current(), (me) =>
    me == null ? me : { ...me, user: patch(me.user) },
  );
}

export function useUpdateProfile() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "profile", "update"],
    mutationFn: ({ firstName, lastName }: ProfileChange) =>
      apiFetch("patch", "/profile", { body: { firstName, lastName } }),
    retry: false,
    onSuccess: async (profile) => {
      client.setQueryData(qk.me.profile(), profile);
      patchMe(client, () => profile);
      await refreshMe(client);
    },
  });
}

function applyPreferences(client: QueryClient, change: Partial<Preferences>) {
  client.setQueryData<Preferences>(qk.me.preferences(), (current) =>
    current === undefined ? current : { ...current, ...change },
  );
  patchMe(client, (user) => ({ ...user, ...change }));
}

interface PreferencesSnapshot {
  preferences: Preferences | undefined;
  me: Me | null | undefined;
}

export function useUpdatePreferences() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "preferences", "update"],
    mutationFn: (change: PreferenceChange) =>
      apiFetch("patch", "/profile/preferences", { body: change }),
    retry: false,
    onMutate: async (change): Promise<PreferencesSnapshot> => {
      await Promise.all([
        client.cancelQueries({ queryKey: qk.me.preferences(), exact: true }),
        client.cancelQueries({ queryKey: qk.me.current(), exact: true }),
      ]);
      const snapshot = {
        preferences: client.getQueryData<Preferences>(qk.me.preferences()),
        me: client.getQueryData<Me | null>(qk.me.current()),
      };
      applyPreferences(client, change);
      return snapshot;
    },
    onError: (_error, _change, snapshot) => {
      if (snapshot === undefined) {
        return;
      }
      client.setQueryData(qk.me.preferences(), snapshot.preferences);
      client.setQueryData(qk.me.current(), snapshot.me);
    },
    onSuccess: (preferences) => {
      applyPreferences(client, preferences);
    },
    onSettled: () =>
      Promise.all([
        client.invalidateQueries({ queryKey: qk.me.preferences(), exact: true }),
        refreshMe(client),
      ]),
  });
}

export function useChangePassword() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "password", "change"],
    mutationFn: ({ currentPassword, newPassword }: PasswordChange) =>
      apiFetch("post", "/profile/password", { body: { currentPassword, newPassword } }),
    retry: false,
    onSuccess: () => Promise.all([refreshMe(client), refreshSessions(client)]),
  });
}

function removeSession(client: QueryClient, id: string) {
  client.setQueryData<InfiniteData<SessionPage, string | null>>(qk.me.sessions(), (data) =>
    data === undefined
      ? data
      : {
          ...data,
          pages: data.pages.map((page) => ({
            ...page,
            items: page.items.filter((item) => item.id !== id),
          })),
        },
  );
}

export function useRevokeSession(onCurrentSessionEnded: () => Promise<void>) {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "sessions", "revoke"],
    mutationFn: ({ id }: Pick<SessionItem, "id" | "isCurrent">) =>
      apiFetch("delete", "/sessions/{id}", { path: { id } }),
    onSuccess: async (_result, { id, isCurrent }) => {
      if (isCurrent) {
        await onCurrentSessionEnded();
        return;
      }
      removeSession(client, id);
      await refreshSessions(client);
    },
    onError: () => refreshSessions(client),
  });
}

export function useRevokeOtherSessions() {
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["me", "sessions", "revoke-others"],
    mutationFn: () => apiFetch("delete", "/sessions"),
    onSuccess: () => refreshSessions(client),
  });
}
