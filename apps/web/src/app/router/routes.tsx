import type { QueryClient } from "@tanstack/react-query";
import { createBrowserRouter, type RouteObject, useRouteError } from "react-router";
import { OverviewPage } from "../../features/overview";
import { RouteErrorView } from "../error/RouteErrorBoundary";
import { authenticatedRoutes } from "../guards/chain";
import { AppShell } from "../layouts/AppShell";
import { adminAreaRoutes } from "./adminRoutes";
import { authRoutes } from "./authRoutes";
import { resolveBasename } from "./basename";
import { PATHS } from "./paths";
import { RootRedirect } from "./RootRedirect";
import { routerContextFor } from "./routeContext";
import { settingsRoutes } from "./settingsRoutes";
import { NotFoundPanel } from "./StatusPanel";

function RouteErrorElement() {
  return <RouteErrorView error={useRouteError()} />;
}

export const appRoutes: RouteObject[] = [
  {
    errorElement: <RouteErrorElement />,
    children: [
      { path: PATHS.root, element: <RootRedirect /> },
      ...authRoutes,
      ...authenticatedRoutes([
        {
          element: <AppShell />,
          children: [{ path: PATHS.overview, element: <OverviewPage /> }, ...settingsRoutes],
        },
      ]),
      ...adminAreaRoutes,
      { path: "*", element: <NotFoundPanel /> },
    ],
  },
];

export function createAppRouter(queryClient: QueryClient, basename: string = resolveBasename()) {
  return createBrowserRouter(appRoutes, {
    basename,
    getContext: routerContextFor(queryClient),
  });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
