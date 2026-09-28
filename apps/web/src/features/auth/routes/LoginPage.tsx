import { Alert, Divider, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { LoginForm } from "../components/LoginForm";
import { type LoginProvider, ProviderButtons } from "../components/ProviderButtons";

export interface LoginPageProps {
  appName: string;
  passwordLoginEnabled: boolean;
  providers: readonly LoginProvider[];
  onSignedIn: () => Promise<void>;
  onProviderSelect?: (slug: string) => void;
}

export function LoginPage({
  appName,
  passwordLoginEnabled,
  providers,
  onSignedIn,
  onProviderSelect,
}: LoginPageProps) {
  const { t } = useTranslation("auth");
  const hasProviders = providers.length > 0;
  return (
    <>
      <AuthHeading title={t("login.title")} description={t("login.description", { appName })} />
      {passwordLoginEnabled ? <LoginForm onSignedIn={onSignedIn} /> : null}
      {passwordLoginEnabled && hasProviders ? (
        <Divider plain>
          <Typography.Text type="secondary">{t("login.divider")}</Typography.Text>
        </Divider>
      ) : null}
      {hasProviders ? <ProviderButtons providers={providers} onSelect={onProviderSelect} /> : null}
      {!passwordLoginEnabled && !hasProviders ? (
        <Alert
          type="warning"
          showIcon
          title={t("login.unavailable.title")}
          description={t("login.unavailable.description")}
        />
      ) : null}
    </>
  );
}
