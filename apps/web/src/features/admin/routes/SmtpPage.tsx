import { Button, Flex, Skeleton, theme } from "antd";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { useSettings } from "../api/queries";
import { PageHeader } from "../components/PageHeader";
import { SmtpWorkspace } from "../components/SmtpWorkspace";

const CONTENT_MAX_WIDTH = 880;

export function SmtpPage() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const smtp = useSettings("smtp");
  return (
    <section data-testid="admin-smtp-page">
      <PageHeader title={t("smtp.pageTitle")} description={t("smtp.pageDescription")} />
      <div style={{ maxWidth: CONTENT_MAX_WIDTH }}>
        {smtp.isPending ? (
          <Skeleton active paragraph={{ rows: 8 }} />
        ) : smtp.isError ? (
          <Flex vertical gap={token.marginSM} align="flex-start">
            <ErrorAlert error={smtp.error} />
            <Button
              onClick={() => {
                void smtp.refetch();
              }}
            >
              {t("common.retry")}
            </Button>
          </Flex>
        ) : (
          <SmtpWorkspace settings={smtp.data} />
        )}
      </div>
    </section>
  );
}
