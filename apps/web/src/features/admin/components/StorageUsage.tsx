import { Flex, Progress, Tag, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { formatBytes } from "../../../shared/format/bytes";

export function isOverQuota(usedBytes: number, effectiveQuotaBytes: number | null): boolean {
  return effectiveQuotaBytes !== null && usedBytes > effectiveQuotaBytes;
}

interface StorageUsageProps {
  usedBytes: number;
  effectiveQuotaBytes: number | null;
  overQuota: boolean;
  width?: number;
}

export function StorageUsage({
  usedBytes,
  effectiveQuotaBytes,
  overQuota,
  width = 180,
}: StorageUsageProps) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  const used = formatBytes(usedBytes, i18n.language);
  const percent =
    effectiveQuotaBytes === null || effectiveQuotaBytes === 0
      ? overQuota
        ? 100
        : 0
      : Math.min(100, (usedBytes / effectiveQuotaBytes) * 100);
  return (
    <Flex
      vertical
      gap={2}
      style={{ minWidth: width }}
      data-testid="storage-usage"
      data-over-quota={overQuota ? "true" : "false"}
    >
      <Flex align="center" gap={token.marginXS} wrap>
        <Typography.Text>
          {effectiveQuotaBytes === null
            ? t("storage.usedOfUnlimited", { used })
            : t("storage.usedOf", {
                used,
                quota: formatBytes(effectiveQuotaBytes, i18n.language),
              })}
        </Typography.Text>
        {overQuota ? (
          <Tag color="warning" variant="filled" style={{ marginInlineEnd: 0 }}>
            {t("storage.overQuota")}
          </Tag>
        ) : null}
      </Flex>
      {effectiveQuotaBytes === null ? null : (
        <Progress
          percent={percent}
          showInfo={false}
          size="small"
          aria-label={t("storage.usageLabel")}
          strokeColor={overQuota ? token.colorWarning : token.colorPrimary}
        />
      )}
    </Flex>
  );
}
