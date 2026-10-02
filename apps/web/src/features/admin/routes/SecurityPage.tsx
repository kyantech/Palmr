import { Flex, theme } from "antd";
import { useTranslation } from "react-i18next";
import { GeneralSettingsForm } from "../components/GeneralSettingsForm";
import { PageHeader } from "../components/PageHeader";
import { PublicLinkSettingsForm } from "../components/PublicLinkSettingsForm";
import { QuotaSettingsForm } from "../components/QuotaSettingsForm";
import { SecuritySettingsForm } from "../components/SecuritySettingsForm";
import { SettingsLoader } from "../components/SettingsLoader";

const CONTENT_MAX_WIDTH = 880;

export interface AdminSecurityPageProps {
  locales: readonly string[];
}

export function AdminSecurityPage({ locales }: AdminSecurityPageProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  return (
    <section data-testid="admin-security-page">
      <PageHeader title={t("settings.title")} description={t("settings.description")} />
      <Flex vertical gap={token.margin} style={{ maxWidth: CONTENT_MAX_WIDTH }}>
        <SettingsLoader
          group="security"
          title={t("settings.security.title")}
          description={t("settings.security.description")}
          testId="settings-security"
        >
          {(settings) => <SecuritySettingsForm settings={settings} />}
        </SettingsLoader>
        <SettingsLoader
          group="quotas"
          title={t("settings.quotas.title")}
          description={t("settings.quotas.description")}
          testId="settings-quotas"
        >
          {(settings) => <QuotaSettingsForm settings={settings} />}
        </SettingsLoader>
        <SettingsLoader
          group="public-links"
          title={t("settings.publicLinks.title")}
          description={t("settings.publicLinks.description")}
          testId="settings-public-links"
        >
          {(settings) => <PublicLinkSettingsForm settings={settings} />}
        </SettingsLoader>
        <SettingsLoader
          group="general"
          title={t("settings.general.title")}
          description={t("settings.general.description")}
          testId="settings-general"
        >
          {(settings) => <GeneralSettingsForm settings={settings} locales={locales} />}
        </SettingsLoader>
      </Flex>
    </section>
  );
}
