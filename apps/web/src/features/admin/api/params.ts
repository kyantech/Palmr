import {
  INVITE_STATUSES,
  type InviteStatus,
  USER_ROLES,
  USER_SORTS,
  USER_STATUSES,
  type UserRole,
  type UserSort,
  type UserStatus,
} from "../types";

export const PAGE_SIZES = [25, 50, 100] as const;
export const DEFAULT_PAGE_SIZE = 25;
export const DEFAULT_USER_SORT: UserSort = "createdAt:desc";

export type AdminView = "users" | "invites";

export interface UsersListParams {
  q: string;
  role: UserRole | null;
  status: UserStatus | null;
  sort: UserSort;
  cursor: string | null;
  limit: number;
}

export interface InvitesListParams {
  status: InviteStatus | null;
  cursor: string | null;
  limit: number;
}

function oneOf<Value extends string>(values: readonly Value[], raw: string | null): Value | null {
  return values.find((value) => value === raw) ?? null;
}

function limitOf(raw: string | null): number {
  const parsed = Number(raw);
  return PAGE_SIZES.find((size) => size === parsed) ?? DEFAULT_PAGE_SIZE;
}

function cursorOf(raw: string | null): string | null {
  return raw === null || raw === "" ? null : raw;
}

export function parseView(search: URLSearchParams): AdminView {
  return search.get("view") === "invites" ? "invites" : "users";
}

export function parseUsersParams(search: URLSearchParams): UsersListParams {
  return {
    q: (search.get("q") ?? "").trim(),
    role: oneOf(USER_ROLES, search.get("role")),
    status: oneOf(USER_STATUSES, search.get("status")),
    sort: oneOf(USER_SORTS, search.get("sort")) ?? DEFAULT_USER_SORT,
    cursor: cursorOf(search.get("cursor")),
    limit: limitOf(search.get("limit")),
  };
}

export function parseInvitesParams(search: URLSearchParams): InvitesListParams {
  return {
    status: oneOf(INVITE_STATUSES, search.get("status")),
    cursor: cursorOf(search.get("cursor")),
    limit: limitOf(search.get("limit")),
  };
}

export function usersSearch(params: UsersListParams): URLSearchParams {
  const search = new URLSearchParams({ view: "users" });
  if (params.q !== "") {
    search.set("q", params.q);
  }
  if (params.role !== null) {
    search.set("role", params.role);
  }
  if (params.status !== null) {
    search.set("status", params.status);
  }
  if (params.sort !== DEFAULT_USER_SORT) {
    search.set("sort", params.sort);
  }
  if (params.cursor !== null) {
    search.set("cursor", params.cursor);
  }
  if (params.limit !== DEFAULT_PAGE_SIZE) {
    search.set("limit", String(params.limit));
  }
  return search;
}

export function invitesSearch(params: InvitesListParams): URLSearchParams {
  const search = new URLSearchParams({ view: "invites" });
  if (params.status !== null) {
    search.set("status", params.status);
  }
  if (params.cursor !== null) {
    search.set("cursor", params.cursor);
  }
  if (params.limit !== DEFAULT_PAGE_SIZE) {
    search.set("limit", String(params.limit));
  }
  return search;
}

export function usersRequestQuery(params: UsersListParams) {
  return {
    ...(params.q === "" ? {} : { q: params.q }),
    ...(params.role === null ? {} : { role: params.role }),
    ...(params.status === null ? {} : { status: params.status }),
    sort: params.sort,
    limit: params.limit,
    ...(params.cursor === null ? {} : { cursor: params.cursor }),
  };
}

export function invitesRequestQuery(params: InvitesListParams) {
  return {
    ...(params.status === null ? {} : { status: params.status }),
    sort: "createdAt:desc",
    limit: params.limit,
    ...(params.cursor === null ? {} : { cursor: params.cursor }),
  };
}

export interface CursorTrail {
  trail: readonly (string | null)[];
}

export function trailOf(state: unknown): readonly (string | null)[] {
  if (typeof state !== "object" || state === null || !("trail" in state)) {
    return [];
  }
  const { trail } = state;
  return Array.isArray(trail)
    ? trail.filter((entry): entry is string | null => entry === null || typeof entry === "string")
    : [];
}
