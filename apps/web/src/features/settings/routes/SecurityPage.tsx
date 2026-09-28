import { useEffectiveSettings } from "../api/queries";
import { PasswordSection } from "../components/PasswordForm";

export interface SecurityPageProps {
  canChangePassword: boolean;
  hasLocalPassword: boolean;
}

export function SecurityPage({ canChangePassword, hasLocalPassword }: SecurityPageProps) {
  const settings = useEffectiveSettings();
  return (
    <PasswordSection
      canChangePassword={canChangePassword}
      hasLocalPassword={hasLocalPassword}
      passwordMinLength={settings.data?.passwordMinLength}
    />
  );
}
