import { Button, Flex, Select, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { PAGE_SIZES } from "../api/params";

interface CursorPagerProps {
  shown: number;
  totalCount: number | null;
  limit: number;
  atFirstPage: boolean;
  hasPrevious: boolean;
  nextCursor: string | null;
  loading: boolean;
  onPrevious: () => void;
  onFirst: () => void;
  onNext: () => void;
  onLimit: (limit: number) => void;
}

export function CursorPager({
  shown,
  totalCount,
  limit,
  atFirstPage,
  hasPrevious,
  nextCursor,
  loading,
  onPrevious,
  onFirst,
  onNext,
  onLimit,
}: CursorPagerProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  return (
    <Flex
      justify="space-between"
      align="center"
      gap={token.marginSM}
      wrap
      style={{ marginTop: token.margin }}
    >
      <Typography.Text type="secondary" data-testid="pager-summary">
        {totalCount === null
          ? t("pager.showing", { shown })
          : t("pager.showingOf", { shown, total: totalCount })}
      </Typography.Text>
      <Flex gap={token.marginXS} align="center" wrap>
        <Select<number>
          aria-label={t("pager.pageSize")}
          value={limit}
          style={{ width: 130 }}
          options={PAGE_SIZES.map((size) => ({
            value: size,
            label: t("pager.perPage", { size }),
          }))}
          onChange={onLimit}
        />
        {atFirstPage ? null : <Button onClick={onFirst}>{t("pager.first")}</Button>}
        <Button disabled={!hasPrevious || loading} onClick={onPrevious}>
          {t("pager.previous")}
        </Button>
        <Button disabled={nextCursor === null || loading} onClick={onNext}>
          {t("pager.next")}
        </Button>
      </Flex>
    </Flex>
  );
}
