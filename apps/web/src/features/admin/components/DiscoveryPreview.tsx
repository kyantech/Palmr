import { Button, Flex, Tag, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";
import type { DiscoveredProvider } from "../types";
import { FactList } from "./FactList";

interface DiscoveryPreviewProps {
  discovered: DiscoveredProvider;
  onApply: () => void;
}

const ENDPOINT_MEMBERS = ["authorization", "token", "userinfo", "jwks"] as const;

export function DiscoveryPreview({ discovered, onApply }: DiscoveryPreviewProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const facts = [
    {
      key: "issuer",
      label: t("providers.discovery.issuer"),
      value: <Typography.Text code>{discovered.issuerUrl}</Typography.Text>,
    },
    ...ENDPOINT_MEMBERS.map((member) => ({
      key: member,
      label: t(`providers.discovery.endpoints.${member}`),
      value:
        discovered.endpoints[member] === null ? (
          <Typography.Text type="secondary">{t("providers.discovery.none")}</Typography.Text>
        ) : (
          <Typography.Text code>{discovered.endpoints[member]}</Typography.Text>
        ),
    })),
    {
      key: "scopes",
      label: t("providers.discovery.scopes"),
      value: (
        <Flex gap={token.marginXXS} wrap>
          {discovered.scopesSupported.length === 0 ? (
            <Typography.Text type="secondary">{t("providers.discovery.none")}</Typography.Text>
          ) : (
            discovered.scopesSupported.map((scope) => <Tag key={scope}>{scope}</Tag>)
          )}
        </Flex>
      ),
    },
    {
      key: "authMethods",
      label: t("providers.discovery.authMethods"),
      value: (
        <Flex gap={token.marginXXS} wrap>
          {discovered.tokenEndpointAuthMethodsSupported.length === 0 ? (
            <Typography.Text type="secondary">{t("providers.discovery.none")}</Typography.Text>
          ) : (
            discovered.tokenEndpointAuthMethodsSupported.map((method) => (
              <Tag key={method}>{method}</Tag>
            ))
          )}
        </Flex>
      ),
    },
  ];
  return (
    <Flex
      vertical
      gap={token.marginSM}
      data-testid="provider-discovery"
      style={{
        padding: token.padding,
        marginBottom: token.marginSM,
        border: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
        borderRadius: token.borderRadiusLG,
        background: token.colorFillQuaternary,
      }}
    >
      <Typography.Text strong>{t("providers.discovery.title")}</Typography.Text>
      <FactList facts={facts} />
      <div>
        <Button onClick={onApply}>{t("providers.discovery.apply")}</Button>
      </div>
    </Flex>
  );
}
