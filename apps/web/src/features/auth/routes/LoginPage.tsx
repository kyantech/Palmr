import { Alert, Button, Divider, Flex, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, type ReportedError, ReportedErrorAlert } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useStartExternalLogin } from "../api/mutations";
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
  returnTo?: string | null;
  callbackError?: ReportedError | null;
  initialMode?: LoginMode;
}

const NOTICE_TYPE: Record<LoginNotice, "success" | "info" | "warning"> = {
  passwordReset: "success",
  twoFactorDisabled: "info",
  challengeExpired: "warning",
  identityUnlinked: "info",
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
  returnTo = null,
  callbackError = null,
  initialMode = "signIn",
}: LoginPageProps) {
  const { t } = useTranslation("auth");
  const notice = useLoginNotice();
  const startExternalLogin = useStartExternalLogin();
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
      {callbackError === null ? null : (
        <div data-testid="login-callback-error" style={{ marginBottom: 24 }}>
          <ReportedErrorAlert reported={callbackError} />
        </div>
      )}
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
      {hasProviders ? (
        <Flex vertical gap={16}>
          {startExternalLogin.isError ? <ErrorAlert error={startExternalLogin.error} /> : null}
          <ProviderButtons
            providers={providers}
            pendingSlug={
              startExternalLogin.isPending || startExternalLogin.isSuccess
                ? startExternalLogin.variables.slug
                : null
            }
            onSelect={(slug) => {
              startExternalLogin.mutate({ slug, returnTo });
            }}
          />
        </Flex>
      ) : null}
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
