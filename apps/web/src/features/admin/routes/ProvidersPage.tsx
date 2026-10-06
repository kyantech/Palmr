import { Button, Flex, Skeleton, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { useProviders } from "../api/queries";
import { PageHeader } from "../components/PageHeader";
import { PasswordLoginPanel } from "../components/PasswordLoginPanel";
import { ProviderEditor, type ProviderEditorTarget } from "../components/ProviderEditor";
import { ProviderList } from "../components/ProviderList";
import { ProvidersGlobalToggle } from "../components/ProvidersGlobalToggle";
import { Section } from "../components/Section";

const CONTENT_MAX_WIDTH = 960;

export function ProvidersPage() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const providers = useProviders();
  const [editor, setEditor] = useState<ProviderEditorTarget | null>(null);

  let list;
  if (providers.isPending) {
    list = <Skeleton active paragraph={{ rows: 4 }} />;
  } else if (providers.isError) {
    list = <ErrorAlert error={providers.error} />;
  } else if (providers.data.items.length === 0) {
    list = (
      <Flex
        vertical
        gap={token.marginXXS}
        data-testid="providers-empty"
        style={{
          padding: token.paddingLG,
          textAlign: "center",
          border: `${String(token.lineWidth)}px dashed ${token.colorBorder}`,
          borderRadius: token.borderRadiusLG,
        }}
      >
        <Typography.Text strong>{t("providers.empty.title")}</Typography.Text>
        <Typography.Text type="secondary">{t("providers.empty.description")}</Typography.Text>
      </Flex>
    );
  } else {
    list = (
      <ProviderList
        providers={providers.data.items}
        onEdit={(provider) => {
          setEditor({ mode: "edit", provider });
        }}
      />
    );
  }

  return (
    <section data-testid="admin-providers-page">
      <PageHeader title={t("providers.title")} description={t("providers.description")} />
      <Flex vertical gap={token.margin} style={{ maxWidth: CONTENT_MAX_WIDTH }}>
        <Section
          title={t("providers.list.title")}
          testId="providers-section"
          extra={
            <Button
              type="primary"
              onClick={() => {
                setEditor({ mode: "create" });
              }}
            >
              {t("providers.add")}
            </Button>
          }
        >
          <Flex vertical gap={token.margin}>
            <ProvidersGlobalToggle />
            {list}
          </Flex>
        </Section>
        <PasswordLoginPanel />
      </Flex>
      <ProviderEditor
        target={editor}
        onClose={() => {
          setEditor(null);
        }}
      />
    </section>
  );
}
