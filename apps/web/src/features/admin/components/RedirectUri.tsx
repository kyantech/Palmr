import { Flex, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";

interface RedirectUriProps {
  value: string;
}

export function RedirectUri({ value }: RedirectUriProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  return (
    <Flex vertical gap={2} data-testid="provider-redirect-uri">
      <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
        {t("providers.redirectUri.label")}
      </Typography.Text>
      <Typography.Text
        code
        copyable={{
          text: value,
          tooltips: [t("providers.redirectUri.copy"), t("providers.redirectUri.copied")],
        }}
        style={{ overflowWrap: "anywhere" }}
        data-redirect-uri={value}
      >
        {value}
      </Typography.Text>
    </Flex>
  );
}
