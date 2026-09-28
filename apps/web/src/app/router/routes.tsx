import { createBrowserRouter, type RouteObject, useRouteError } from "react-router";
import { OverviewPage } from "../../features/overview";
import { RouteErrorView } from "../error/RouteErrorBoundary";
import { authenticatedRoutes } from "../guards/chain";
import { AppShell } from "../layouts/AppShell";
import { authRoutes } from "./authRoutes";
import { resolveBasename } from "./basename";
import { PATHS } from "./paths";
import { RootRedirect } from "./RootRedirect";
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
      { path: "*", element: <NotFoundPanel /> },
    ],
  },
];

export function createAppRouter(basename: string = resolveBasename()) {
  return createBrowserRouter(appRoutes, { basename });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
