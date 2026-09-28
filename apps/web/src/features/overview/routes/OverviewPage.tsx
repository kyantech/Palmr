import { theme, Typography } from "antd";
import { useTranslation } from "react-i18next";

export function OverviewPage() {
  const { t } = useTranslation("overview");
  const { token } = theme.useToken();
  return (
    <section data-testid="overview-page">
      <Typography.Title
        level={1}
        style={{
          margin: 0,
          fontSize: `clamp(${String(token.fontSizeHeading3)}px, 2.4vw + 12px, ${String(token.fontSizeHeading1)}px)`,
          lineHeight: 1.15,
          fontWeight: 700,
          letterSpacing: "-0.025em",
        }}
      >
        {t("title")}
      </Typography.Title>
    </section>
  );
}
