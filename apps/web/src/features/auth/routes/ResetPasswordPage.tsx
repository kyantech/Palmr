import { Button, Flex, Skeleton } from "antd";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useResetPassword } from "../api/mutations";
import { useResetTokenCheck } from "../api/queries";
import { AuthStatePanel } from "../components/AuthStatePanel";
import { NewPasswordForm } from "../components/NewPasswordForm";
import { setLoginNotice } from "../store";

const DEAD_TOKEN: Readonly<Record<string, "invalid" | "expired" | "used">> = {
  RESET_TOKEN_INVALID: "invalid",
  RESET_TOKEN_EXPIRED: "expired",
  RESET_TOKEN_USED: "used",
};

function deadState(error: unknown): "invalid" | "expired" | "used" | null {
  const { code } = presentError(error);
  return code === null ? null : (DEAD_TOKEN[code] ?? null);
}

export interface ResetPasswordPageProps {
  token: string;
  onReset: () => void;
  onRequestNewLink: () => void;
  onBackToSignIn: () => void;
}

export function ResetPasswordPage({
  token,
  onReset,
  onRequestNewLink,
  onBackToSignIn,
}: ResetPasswordPageProps) {
  const { t } = useTranslation("auth");
  const instance = useId();
  const check = useResetTokenCheck(instance, token);
  const reset = useResetPassword(token);
  const [dead, setDead] = useState<unknown>(null);
  const failure = dead ?? (check.isError ? check.error : null);
  const state = failure === null ? null : deadState(failure);

  if (state !== null) {
    return (
      <AuthStatePanel
        testId="reset-password-dead"
        title={t(`reset.dead.${state}`)}
        error={failure}
        actions={
          <>
            <Button type="primary" size="large" block onClick={onRequestNewLink}>
              {t("reset.requestNewLink")}
            </Button>
            <Button type="text" block onClick={onBackToSignIn}>
              {t("reset.backToSignIn")}
            </Button>
          </>
        }
      />
    );
  }

  const heading = <AuthHeading title={t("reset.title")} description={t("reset.description")} />;

  if (check.isPending) {
    return (
      <div aria-busy="true" data-testid="reset-password-checking">
        {heading}
        <Skeleton active title={false} paragraph={{ rows: 4 }} />
      </div>
    );
  }

  if (check.isError) {
    return (
      <>
        {heading}
        <Flex vertical gap={12}>
          <ErrorAlert error={check.error} />
          <Button
            type="primary"
            block
            onClick={() => {
              void check.refetch();
            }}
          >
            {t("reset.retry")}
          </Button>
          <Button type="text" block onClick={onBackToSignIn}>
            {t("reset.backToSignIn")}
          </Button>
        </Flex>
      </>
    );
  }

  return (
    <div data-testid="reset-password-form">
      {heading}
      <NewPasswordForm
        minLength={check.data.passwordMinLength}
        submitLabel={t("reset.submit")}
        consequence={t("reset.consequence")}
        interceptError={(error) => {
          if (deadState(error) === null) {
            return false;
          }
          setDead(error);
          return true;
        }}
        submit={async (newPassword) => {
          await reset.mutateAsync({ newPassword });
          setLoginNotice("passwordReset");
          onReset();
        }}
        secondaryAction={
          <Button type="text" block onClick={onBackToSignIn}>
            {t("reset.backToSignIn")}
          </Button>
        }
      />
    </div>
  );
}
