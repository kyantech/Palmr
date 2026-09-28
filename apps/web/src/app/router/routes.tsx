import { createBrowserRouter, type RouteObject, useRouteError } from "react-router";
import { RouteErrorView } from "../error/RouteErrorBoundary";
import { authRoutes } from "./authRoutes";
import { resolveBasename } from "./basename";
import { PATHS } from "./paths";
import { RootRedirect } from "./RootRedirect";
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
      { path: "*", element: <NotFoundPanel /> },
    ],
  },
];

export function createAppRouter(basename: string = resolveBasename()) {
  return createBrowserRouter(appRoutes, { basename });
}

export type AppRouter = ReturnType<typeof createAppRouter>;
