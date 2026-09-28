import type { ReactNode } from "react";
import { PATHS } from "../../router/paths";
import { OverviewIcon } from "./icons";

export type NavigationRole = "admin" | "user";

export interface NavigationEntry {
  key: string;
  path: string;
  labelKey: string;
  icon: ReactNode;
  order: number;
  requiredRole?: NavigationRole;
  bottomBar?: boolean;
}

// A concept is listed here only once its feature and route exist, so the shell
// never links to a destination the product cannot serve. Future concepts are
// appended with their own order; Admin carries requiredRole: "admin".
export const NAV_ENTRIES: readonly NavigationEntry[] = [
  {
    key: "overview",
    path: PATHS.overview,
    labelKey: "nav.overview",
    icon: <OverviewIcon />,
    order: 10,
    bottomBar: true,
  },
];

// The role is presentation only; it always comes from `/auth/me` and the server
// remains the authority on every request.
export function visibleNavigation(
  entries: readonly NavigationEntry[],
  role: string | undefined,
): NavigationEntry[] {
  return entries
    .filter((entry) => entry.requiredRole === undefined || entry.requiredRole === role)
    .sort((left, right) => left.order - right.order);
}

export function selectedNavigationKey(
  entries: readonly NavigationEntry[],
  pathname: string,
): string | null {
  let selected: NavigationEntry | null = null;
  for (const entry of entries) {
    if (pathname !== entry.path && !pathname.startsWith(`${entry.path}/`)) {
      continue;
    }
    if (selected === null || entry.path.length > selected.path.length) {
      selected = entry;
    }
  }
  return selected?.key ?? null;
}
