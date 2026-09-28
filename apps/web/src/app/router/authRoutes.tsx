import { useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import type { RouteObject } from "react-router";
import { LoginPage } from "../../features/auth";
import { SetupPage } from "../../features/setup";
import { useBootState } from "../bootstrap/bootState";
import { RequireAnonymous } from "../guards/RequireAnonymous";
import { RequireSetup } from "../guards/RequireSetup";
import { RequireSetupIncomplete } from "../guards/RequireSetupIncomplete";
import { AuthLayout, type AuthLayoutHandle } from "../layouts/AuthLayout";
import { reconcileAuthTransition, reconcileSetupFinished } from "../session/authReconciliation";
import { PATHS } from "./paths";

function SetupRoute() {
  const client = useQueryClient();
  const { bootstrap } = useBootState();
  const onSetupFinished = useCallback(() => reconcileSetupFinished(client), [client]);
  return (
    <SetupPage
      defaultLocale={bootstrap.defaultLocale}
      supportedLocales={bootstrap.supportedLocales}
      onSetupFinished={onSetupFinished}
    />
  );
}

function LoginRoute() {
  const client = useQueryClient();
  const { bootstrap } = useBootState();
  const onSignedIn = useCallback(() => reconcileAuthTransition(client), [client]);
  return (
    <LoginPage
      appName={bootstrap.appName}
      passwordLoginEnabled={bootstrap.passwordLoginEnabled}
      providers={bootstrap.providers}
      onSignedIn={onSignedIn}
    />
  );
}

const setupHandle: AuthLayoutHandle = { authLayoutWidth: "wide" };

export const authRoutes: RouteObject[] = [
  {
    element: <RequireSetupIncomplete />,
    children: [
      {
        element: <AuthLayout />,
        children: [{ path: PATHS.setup, handle: setupHandle, element: <SetupRoute /> }],
      },
    ],
  },
  {
    element: <RequireSetup />,
    children: [
      {
        element: <RequireAnonymous />,
        children: [
          {
            element: <AuthLayout />,
            children: [{ path: PATHS.login, element: <LoginRoute /> }],
          },
        ],
      },
    ],
  },
];
