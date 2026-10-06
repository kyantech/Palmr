import type { QueryClient } from "@tanstack/react-query";
import type { AdminSettingsGroup } from "../../../shared/api/query-keys";
import { parseInvitesParams, parseUsersParams, parseView } from "./params";
import {
  invitesQueryOptions,
  passwordLoginQueryOptions,
  providersQueryOptions,
  settingsQueryOptions,
  userQueryOptions,
  userSessionsQueryOptions,
  usersQueryOptions,
} from "./queries";

function settle(request: Promise<unknown>): Promise<void> {
  return request.then(
    () => undefined,
    () => undefined,
  );
}

export function primeUsers(client: QueryClient, search: URLSearchParams): Promise<void> {
  return settle(
    parseView(search) === "invites"
      ? client.query(invitesQueryOptions(parseInvitesParams(search)))
      : client.query(usersQueryOptions(parseUsersParams(search))),
  );
}

export async function primeUser(client: QueryClient, userId: string): Promise<void> {
  await Promise.all([
    settle(client.query(userQueryOptions(userId))),
    settle(client.query(userSessionsQueryOptions(userId))),
  ]);
}

export function primeSettings(client: QueryClient, group: AdminSettingsGroup): Promise<void> {
  return settle(client.query(settingsQueryOptions(group)));
}

export async function primeProviders(client: QueryClient): Promise<void> {
  await Promise.all([
    settle(client.query(providersQueryOptions())),
    settle(client.query(passwordLoginQueryOptions())),
    settle(client.query(settingsQueryOptions("security"))),
  ]);
}
