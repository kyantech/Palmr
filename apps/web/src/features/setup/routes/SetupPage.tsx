import { Button, Flex, theme } from "antd";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useSetupStatus } from "../api/queries";
import { SetupForm } from "../components/SetupForm";

export interface SetupPageProps {
  defaultLocale: string;
  supportedLocales: readonly string[];
  onSetupFinished: () => Promise<void>;
}

export function SetupPage({ defaultLocale, supportedLocales, onSetupFinished }: SetupPageProps) {
  const { t } = useTranslation("setup");
  const { token } = theme.useToken();
  const status = useSetupStatus();
  const alreadyCompleted = status.data?.setupCompleted === true;

  useEffect(() => {
    if (alreadyCompleted) {
      onSetupFinished().catch(() => undefined);
    }
  }, [alreadyCompleted, onSetupFinished]);

  return (
    <>
      <AuthHeading title={t("title")} description={t("description")} />
      {status.isError ? (
        <Flex vertical gap={token.marginSM} style={{ marginBottom: token.marginLG }}>
          <ErrorAlert error={status.error} />
          <Flex justify="end">
            <Button
              loading={status.isFetching}
              onClick={() => {
                void status.refetch();
              }}
            >
              {t("retry")}
            </Button>
          </Flex>
        </Flex>
      ) : null}
      <SetupForm
        defaultLocale={defaultLocale}
        supportedLocales={supportedLocales}
        passwordMinLength={alreadyCompleted ? undefined : status.data?.passwordMinLength}
        onSetupFinished={onSetupFinished}
      />
    </>
  );
}
