import { Skeleton } from "antd";
import { ErrorAlert } from "../../../shared/errors";
import { usePreferences } from "../api/queries";
import { AppearanceForm } from "../components/AppearanceForm";
import type { AccentPreset } from "../types";

export interface AppearancePageProps {
  accents: readonly AccentPreset[];
  locales: readonly string[];
}

export function AppearancePage({ accents, locales }: AppearancePageProps) {
  const preferences = usePreferences();
  if (preferences.data !== undefined) {
    return <AppearanceForm preferences={preferences.data} accents={accents} locales={locales} />;
  }
  return preferences.isError ? <ErrorAlert error={preferences.error} /> : <Skeleton active />;
}
