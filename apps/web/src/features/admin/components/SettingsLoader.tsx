import { Button, Flex, Skeleton } from "antd";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { AdminSettingsGroup } from "../../../shared/api/query-keys";
import { ErrorAlert } from "../../../shared/errors";
import { type SettingsByGroup, useSettings } from "../api/queries";
import { Section } from "./Section";

interface SettingsLoaderProps<Group extends AdminSettingsGroup> {
  group: Group;
  title: string;
  description: string;
  testId: string;
  children: (settings: SettingsByGroup[Group]) => ReactNode;
}

export function SettingsLoader<Group extends AdminSettingsGroup>({
  group,
  title,
  description,
  testId,
  children,
}: SettingsLoaderProps<Group>) {
  const { t } = useTranslation("admin");
  const settings = useSettings(group);
  return (
    <Section title={title} description={description} testId={testId}>
      {settings.isPending ? (
        <Skeleton active paragraph={{ rows: 4 }} />
      ) : settings.isError ? (
        <Flex vertical gap={12} align="flex-start">
          <ErrorAlert error={settings.error} />
          <Button
            onClick={() => {
              void settings.refetch();
            }}
          >
            {t("common.retry")}
          </Button>
        </Flex>
      ) : (
        children(settings.data)
      )}
    </Section>
  );
}
