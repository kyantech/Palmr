import { Alert, Flex, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { presentError, type PresentedError } from "./presentation";

export const ERRORS_NAMESPACE = "errors";

export function useErrorMessage(error: unknown): string {
  const { t } = useTranslation(ERRORS_NAMESPACE);
  return t(presentError(error).presentation.i18nKey);
}

export function RequestId({ requestId }: { requestId: string }) {
  const { t } = useTranslation(ERRORS_NAMESPACE);
  return (
    <Typography.Text type="secondary" copyable={{ text: requestId }} data-request-id={requestId}>
      {t("details.requestId", { requestId })}
    </Typography.Text>
  );
}

export function ErrorTechnicalDetails({ presented }: { presented: PresentedError }) {
  const { t } = useTranslation(ERRORS_NAMESPACE);
  const { presentation, known, code, requestId } = presented;
  const showCode = !known && code !== null;
  const showRequestId = presentation.showRequestId && requestId !== null;
  if (!showCode && !showRequestId) {
    return null;
  }
  return (
    <Flex vertical gap={2} style={{ fontSize: "0.8125rem" }}>
      {showCode ? (
        <Typography.Text type="secondary" data-error-code={code}>
          {t("details.errorCode", { code })}
        </Typography.Text>
      ) : null}
      {showRequestId ? <RequestId requestId={requestId} /> : null}
    </Flex>
  );
}

export function ErrorAlert({ error }: { error: unknown }) {
  const { t } = useTranslation(ERRORS_NAMESPACE);
  const presented = presentError(error);
  if (presented.presentation.silent) {
    return null;
  }
  const detailed = presented.presentation.showRequestId || !presented.known;
  return (
    <Alert
      role="alert"
      type={presented.presentation.severity}
      showIcon
      title={t(presented.presentation.i18nKey)}
      {...(detailed ? { description: <ErrorTechnicalDetails presented={presented} /> } : {})}
    />
  );
}
