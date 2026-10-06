import { Alert, Flex, Skeleton, Switch, theme, Typography } from "antd";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { useUpdateSecuritySettings } from "../api/mutations";
import { useSettings } from "../api/queries";
import { reportable } from "./feedback";

export function ProvidersGlobalToggle() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const switchId = useId();
  const settings = useSettings("security");
  const update = useUpdateSecuritySettings();

  if (settings.isPending) {
    return <Skeleton active paragraph={false} title={{ width: 240 }} />;
  }
  if (settings.isError) {
    return <ErrorAlert error={settings.error} />;
  }
  const enabled = settings.data.authProvidersEnabled;
  const failure = update.isError ? reportable(update.error) : null;
  return (
    <Flex vertical gap={token.marginXS} data-testid="providers-global-toggle">
      <Flex align="center" gap={token.marginXS}>
        <Switch
          id={switchId}
          checked={enabled}
          loading={update.isPending}
          disabled={update.isPending}
          onChange={(checked) => {
            update.mutate({ authProvidersEnabled: checked });
          }}
        />
        <label htmlFor={switchId}>{t("providers.global.label")}</label>
      </Flex>
      <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
        {t("providers.global.hint")}
      </Typography.Text>
      {enabled ? null : <Alert type="warning" showIcon title={t("providers.global.offNotice")} />}
      {failure === null ? null : <ErrorAlert error={failure} />}
    </Flex>
  );
}
