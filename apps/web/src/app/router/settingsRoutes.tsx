import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import { Navigate, type RouteObject } from "react-router";
import { TrustedDevices, TwoFactorSettings } from "../../features/auth";
import {
  AppearancePage,
  ProfilePage,
  SecurityPage,
  SessionsPage,
  SETTINGS_SECTIONS,
  SettingsLayout,
} from "../../features/settings";
import { useBootState } from "../bootstrap/bootState";
import { isLocaleCode } from "../i18n/catalog";
import { reconcileSignedIn } from "../session/authReconciliation";
import { endLocalSession } from "../session/useSignOut";
import { accentSwatches } from "../theme/appearance";
import { PATHS } from "./paths";

function AppearanceRoute() {
  const { bootstrap } = useBootState();
  const accents = useMemo(() => accentSwatches(bootstrap.primaryColor), [bootstrap.primaryColor]);
  const locales = useMemo(
    () => bootstrap.supportedLocales.filter(isLocaleCode),
    [bootstrap.supportedLocales],
  );
  return <AppearancePage accents={accents} locales={locales} />;
}

function SecurityRoute() {
  const client = useQueryClient();
  const { bootstrap, me } = useBootState();
  const hasLocalPassword = me?.capabilities.hasLocalPassword ?? false;
  const onEnabled = useCallback(() => reconcileSignedIn(client), [client]);
  const onSignedOutEverywhere = useCallback(() => endLocalSession(client), [client]);
  return (
    <SecurityPage
      canChangePassword={me?.capabilities.canChangePassword ?? false}
      hasLocalPassword={hasLocalPassword}
    >
      <TwoFactorSettings
        appName={bootstrap.appName}
        hasLocalPassword={hasLocalPassword}
        onEnabled={onEnabled}
        onSignedOutEverywhere={onSignedOutEverywhere}
      />
      <TrustedDevices />
    </SecurityPage>
  );
}

function SessionsRoute() {
  const client = useQueryClient();
  const onCurrentSessionEnded = useCallback(() => endLocalSession(client), [client]);
  return <SessionsPage onCurrentSessionEnded={onCurrentSessionEnded} />;
}

const SECTION_ELEMENTS: Record<(typeof SETTINGS_SECTIONS)[number], RouteObject["element"]> = {
  profile: <ProfilePage />,
  appearance: <AppearanceRoute />,
  security: <SecurityRoute />,
  sessions: <SessionsRoute />,
};

export const settingsRoutes: RouteObject[] = [
  {
    path: PATHS.settings,
    element: <SettingsLayout />,
    children: [
      { index: true, element: <Navigate to={PATHS.settingsProfile} replace /> },
      ...SETTINGS_SECTIONS.map((section) => ({
        path: section,
        element: SECTION_ELEMENTS[section],
      })),
    ],
  },
];
