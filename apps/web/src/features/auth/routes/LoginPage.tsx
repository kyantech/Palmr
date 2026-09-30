import { Alert, Button, Divider, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { ForgotPassword } from "../components/ForgotPassword";
import { LoginForm } from "../components/LoginForm";
import { type LoginProvider, ProviderButtons } from "../components/ProviderButtons";
import { clearLoginNotice, type LoginNotice, useLoginNotice } from "../store";

export type LoginMode = "signIn" | "forgot";

export interface LoginPageProps {
  appName: string;
  passwordLoginEnabled: boolean;
  providers: readonly LoginProvider[];
  onSignedIn: () => Promise<void>;
  onMfaRequired: () => void;
  onProviderSelect?: (slug: string) => void;
  initialMode?: LoginMode;
}

const NOTICE_TYPE: Record<LoginNotice, "success" | "info" | "warning"> = {
  passwordReset: "success",
  twoFactorDisabled: "info",
  challengeExpired: "warning",
};

function Notice({ notice }: { notice: LoginNotice }) {
  const { t } = useTranslation("auth");
  return (
    <Alert
      type={NOTICE_TYPE[notice]}
      showIcon
      role="status"
      data-testid="login-notice"
      style={{ marginBottom: 24 }}
      title={t(`notice.${notice}.title`)}
      description={t(`notice.${notice}.description`)}
    />
  );
}

export function LoginPage({
  appName,
  passwordLoginEnabled,
  providers,
  onSignedIn,
  onMfaRequired,
  onProviderSelect,
  initialMode = "signIn",
}: LoginPageProps) {
  const { t } = useTranslation("auth");
  const notice = useLoginNotice();
  const [mode, setMode] = useState<LoginMode>(passwordLoginEnabled ? initialMode : "signIn");
  const hasProviders = providers.length > 0;

  if (mode === "forgot") {
    return (
      <ForgotPassword
        onBack={() => {
          setMode("signIn");
        }}
      />
    );
  }

  return (
    <>
      <AuthHeading title={t("login.title")} description={t("login.description", { appName })} />
      {notice === null ? null : <Notice notice={notice} />}
      {passwordLoginEnabled ? (
        <LoginForm
          onSignedIn={onSignedIn}
          onMfaRequired={onMfaRequired}
          passwordAction={
            <Button
              type="link"
              size="small"
              style={{ paddingInline: 0 }}
              onClick={() => {
                clearLoginNotice();
                setMode("forgot");
              }}
            >
              {t("login.forgotPassword")}
            </Button>
          }
        />
      ) : null}
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
