import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { type RouteObject, useLocation, useNavigate, useParams } from "react-router";
import {
  clearMfaChallenge,
  EnrollTwoFactorPage,
  ForcedPasswordChangePage,
  InvitePage,
  type LoginMode,
  LoginPage,
  ResetPasswordPage,
  SecondFactorPage,
} from "../../features/auth";
import { useEffectiveSettings } from "../../features/settings";
import { SetupPage } from "../../features/setup";
import { useBootState } from "../bootstrap/bootState";
import { RequireAnonymous } from "../guards/RequireAnonymous";
import { RequireAuth } from "../guards/RequireAuth";
import { RequireMfaPending } from "../guards/RequireMfaPending";
import { RequirePending2faEnrollment } from "../guards/RequirePending2faEnrollment";
import { RequirePendingPasswordChange } from "../guards/RequirePendingPasswordChange";
import { RequireSetup } from "../guards/RequireSetup";
import { RequireSetupIncomplete } from "../guards/RequireSetupIncomplete";
import { isLocaleCode } from "../i18n/catalog";
import { AuthLayout, type AuthLayoutHandle } from "../layouts/AuthLayout";
import {
  reconcileAuthTransition,
  reconcileSetupFinished,
  reconcileSignedIn,
} from "../session/authReconciliation";
import { useSignOut } from "../session/useSignOut";
import { keepNext } from "./next";
import { PATHS } from "./paths";

interface LoginLocationState {
  authMode?: LoginMode;
}

function loginModeOf(state: unknown): LoginMode {
  return (state as LoginLocationState | null)?.authMode === "forgot" ? "forgot" : "signIn";
}

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
  const navigate = useNavigate();
  const location = useLocation();
  const { search, key } = location;
  const { bootstrap } = useBootState();
  const onSignedIn = useCallback(() => reconcileAuthTransition(client), [client]);
  const onMfaRequired = useCallback(() => {
    void navigate(keepNext(PATHS.twoFactor, search));
  }, [navigate, search]);
  useEffect(() => {
    clearMfaChallenge();
  }, []);
  return (
    <LoginPage
      key={key}
      appName={bootstrap.appName}
      passwordLoginEnabled={bootstrap.passwordLoginEnabled}
      providers={bootstrap.providers}
      onSignedIn={onSignedIn}
      onMfaRequired={onMfaRequired}
      initialMode={loginModeOf(location.state as unknown)}
    />
  );
}

function SecondFactorRoute() {
  const client = useQueryClient();
  const onSignedIn = useCallback(() => reconcileAuthTransition(client), [client]);
  return <SecondFactorPage onSignedIn={onSignedIn} />;
}

function useLoginNavigation() {
  const navigate = useNavigate();
  return useCallback(
    (mode: LoginMode = "signIn") => {
      void navigate(PATHS.login, { replace: true, state: { authMode: mode } });
    },
    [navigate],
  );
}

function ResetPasswordRoute() {
  const { token = "" } = useParams();
  const toLogin = useLoginNavigation();
  return (
    <ResetPasswordPage
      key={token}
      token={token}
      onReset={() => {
        toLogin();
      }}
      onRequestNewLink={() => {
        toLogin("forgot");
      }}
      onBackToSignIn={() => {
        toLogin();
      }}
    />
  );
}

function InviteRoute() {
  const client = useQueryClient();
  const { token = "" } = useParams();
  const { i18n } = useTranslation();
  const { bootstrap } = useBootState();
  const toLogin = useLoginNavigation();
  const onAccepted = useCallback(() => reconcileAuthTransition(client), [client]);
  return (
    <InvitePage
      key={token}
      token={token}
      appName={bootstrap.appName}
      supportedLocales={bootstrap.supportedLocales.filter(isLocaleCode)}
      defaultLocale={i18n.language}
      onAccepted={onAccepted}
      onBackToSignIn={() => {
        toLogin();
      }}
    />
  );
}

function useRestrictedSession() {
  const client = useQueryClient();
  const { me } = useBootState();
  const { signOut } = useSignOut();
  const reconcile = useCallback(() => reconcileSignedIn(client), [client]);
  return { account: me?.user.email ?? "", reconcile, signOut };
}

function ForcedPasswordChangeRoute() {
  const { account, reconcile, signOut } = useRestrictedSession();
  const settings = useEffectiveSettings();
  return (
    <ForcedPasswordChangePage
      account={account}
      passwordMinLength={settings.data?.passwordMinLength}
      onChanged={reconcile}
      onSignOut={signOut}
    />
  );
}

function EnrollTwoFactorRoute() {
  const { bootstrap } = useBootState();
  const { account, reconcile, signOut } = useRestrictedSession();
  return (
    <EnrollTwoFactorPage
      appName={bootstrap.appName}
      account={account}
      onEnrolled={reconcile}
      onSignOut={signOut}
    />
  );
}

const wideHandle: AuthLayoutHandle = { authLayoutWidth: "wide" };

export const authRoutes: RouteObject[] = [
  {
    element: <RequireSetupIncomplete />,
    children: [
      {
        element: <AuthLayout />,
        children: [{ path: PATHS.setup, handle: wideHandle, element: <SetupRoute /> }],
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
            children: [
              { path: PATHS.login, element: <LoginRoute /> },
              {
                element: <RequireMfaPending />,
                children: [{ path: PATHS.twoFactor, element: <SecondFactorRoute /> }],
              },
              { path: PATHS.resetPassword, element: <ResetPasswordRoute /> },
              { path: PATHS.invite, handle: wideHandle, element: <InviteRoute /> },
            ],
          },
        ],
      },
      {
        element: <RequireAuth />,
        children: [
          {
            element: <AuthLayout />,
            children: [
              {
                element: <RequirePendingPasswordChange />,
                children: [
                  { path: PATHS.forcedPasswordChange, element: <ForcedPasswordChangeRoute /> },
                ],
              },
              {
                element: <RequirePending2faEnrollment />,
                children: [
                  {
                    path: PATHS.enrollTwoFactor,
                    handle: wideHandle,
                    element: <EnrollTwoFactorRoute />,
                  },
                ],
              },
            ],
          },
        ],
      },
    ],
  },
];
