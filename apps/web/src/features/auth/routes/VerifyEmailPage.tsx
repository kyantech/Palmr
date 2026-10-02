import { Alert, Button, Flex } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useVerifyEmail } from "../api/mutations";
import { AuthStatePanel } from "../components/AuthStatePanel";

const WELL_FORMED_TOKEN = /^[A-Za-z0-9_-]+$/;

type DeadState = "invalid" | "expired" | "taken";

const DEAD_STATES: Readonly<Record<string, DeadState>> = {
  EMAIL_VERIFICATION_TOKEN_INVALID: "invalid",
  EMAIL_VERIFICATION_TOKEN_EXPIRED: "expired",
  USER_EMAIL_TAKEN: "taken",
};

function deadState(error: unknown): DeadState | null {
  const { code } = presentError(error);
  return code === null ? null : (DEAD_STATES[code] ?? null);
}

export interface VerifyEmailPageProps {
  token: string;
  signedIn: boolean;
  onVerified: () => Promise<void>;
  onContinue: () => void;
}

export function VerifyEmailPage({ token, signedIn, onVerified, onContinue }: VerifyEmailPageProps) {
  const { t } = useTranslation("auth");
  const verify = useVerifyEmail(token);
  const [done, setDone] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [reconcileFailed, setReconcileFailed] = useState(false);

  if (token === "" || !WELL_FORMED_TOKEN.test(token)) {
    return (
      <div data-testid="verify-email-malformed">
        <AuthHeading
          title={t("verifyEmail.malformed.title")}
          description={t("verifyEmail.malformed.description")}
        />
        <Button type="primary" size="large" block onClick={onContinue}>
          {t("verifyEmail.toSignIn")}
        </Button>
      </div>
    );
  }

  if (done) {
    return (
      <div data-testid="verify-email-success">
        <AuthHeading
          title={t("verifyEmail.success.title")}
          description={t(
            signedIn ? "verifyEmail.success.signedIn" : "verifyEmail.success.description",
          )}
        />
        <Flex vertical gap={12}>
          {reconcileFailed ? (
            <Alert type="warning" showIcon title={t("verifyEmail.reconcileFailed")} />
          ) : null}
          <Button type="primary" size="large" block onClick={onContinue}>
            {t(signedIn ? "verifyEmail.toOverview" : "verifyEmail.toSignIn")}
          </Button>
        </Flex>
      </div>
    );
  }

  const state = failure === null ? null : deadState(failure);
  if (state !== null) {
    return (
      <AuthStatePanel
        testId="verify-email-dead"
        title={t(`verifyEmail.dead.${state}`)}
        error={failure}
        actions={
          <Button type="primary" size="large" block onClick={onContinue}>
            {t("verifyEmail.toSignIn")}
          </Button>
        }
      />
    );
  }

  const confirm = () => {
    if (verify.isPending) {
      return;
    }
    setFailure(null);
    verify.mutate(undefined, {
      onSuccess: () => {
        onVerified().then(
          () => {
            setDone(true);
          },
          () => {
            setReconcileFailed(true);
            setDone(true);
          },
        );
      },
      onError: setFailure,
    });
  };

  return (
    <div data-testid="verify-email">
      <AuthHeading title={t("verifyEmail.title")} description={t("verifyEmail.description")} />
      <Flex vertical gap={12}>
        {failure === null ? null : <ErrorAlert error={failure} />}
        <Button type="primary" size="large" block loading={verify.isPending} onClick={confirm}>
          {t("verifyEmail.confirm")}
        </Button>
      </Flex>
    </div>
  );
}
