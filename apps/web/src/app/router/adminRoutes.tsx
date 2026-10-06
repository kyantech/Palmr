import type { QueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { type LoaderFunction, Navigate, type Params, type RouteObject } from "react-router";
import {
  AdminLayout,
  AdminSecurityPage,
  primeProviders,
  primeSettings,
  primeUser,
  primeUsers,
  ProvidersPage,
  SmtpPage,
  UserDetailPage,
  UsersPage,
} from "../../features/admin";
import { qk } from "../../shared/api/query-keys";
import { useBootState } from "../bootstrap/bootState";
import type { Me } from "../bootstrap/queries";
import { adminRoutes } from "../guards/chain";
import { ADMIN_ROLE } from "../guards/RequireAdmin";
import { isLocaleCode } from "../i18n/catalog";
import { AppShell } from "../layouts/AppShell";
import { PATHS } from "./paths";
import { queryClientContext } from "./routeContext";

interface PrimeArgs {
  url: URL;
  params: Params;
}

function primeAdminQueries(
  prime: (client: QueryClient, args: PrimeArgs) => Promise<void>,
): LoaderFunction {
  return ({ request, params, context }) => {
    const client = context.get(queryClientContext);
    if (client === null) {
      return null;
    }
    const me = client.getQueryData<Me | null>(qk.me.current());
    if (me?.user.role !== ADMIN_ROLE || me.restriction !== null) {
      return null;
    }
    void prime(client, { url: new URL(request.url), params });
    return null;
  };
}

function useSupportedLocales(): readonly string[] {
  const { bootstrap } = useBootState();
  return useMemo(
    () => bootstrap.supportedLocales.filter(isLocaleCode),
    [bootstrap.supportedLocales],
  );
}

function UsersRoute() {
  return <UsersPage locales={useSupportedLocales()} />;
}

function SecurityRoute() {
  return <AdminSecurityPage locales={useSupportedLocales()} />;
}

const adminChildren: RouteObject[] = [
  { index: true, element: <Navigate to={PATHS.adminUsers} replace /> },
  {
    path: "users",
    element: <UsersRoute />,
    loader: primeAdminQueries((client, { url }) => primeUsers(client, url.searchParams)),
  },
  {
    path: "users/:userId",
    element: <UserDetailPage />,
    loader: primeAdminQueries((client, { params }) =>
      params.userId === undefined ? Promise.resolve() : primeUser(client, params.userId),
    ),
  },
  {
    path: "security",
    element: <SecurityRoute />,
    loader: primeAdminQueries((client) => primeSettings(client, "security")),
  },
  {
    path: "smtp",
    element: <SmtpPage />,
    loader: primeAdminQueries((client) => primeSettings(client, "smtp")),
  },
  {
    path: "providers",
    element: <ProvidersPage />,
    loader: primeAdminQueries((client) => primeProviders(client)),
  },
];

export const adminAreaRoutes: RouteObject[] = adminRoutes([
  {
    element: <AppShell />,
    children: [{ path: PATHS.admin, element: <AdminLayout />, children: adminChildren }],
  },
]);
