import { Flex, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { NavLink, Outlet, useLocation } from "react-router";

export const ADMIN_SECTIONS = ["users", "security", "smtp"] as const;

export type AdminSectionKey = (typeof ADMIN_SECTIONS)[number];

function activeSection(pathname: string): AdminSectionKey | null {
  const segments = pathname.split("/").filter((segment) => segment.length > 0);
  return ADMIN_SECTIONS.find((section) => segments[1] === section) ?? null;
}

export function AdminLayout() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const { pathname } = useLocation();
  const selected = activeSection(pathname);

  return (
    <section data-testid="admin-page">
      <Typography.Title
        level={1}
        style={{
          margin: 0,
          marginBottom: token.marginMD,
          fontSize: `clamp(${String(token.fontSizeHeading3)}px, 2.4vw + 12px, ${String(token.fontSizeHeading1)}px)`,
          lineHeight: 1.15,
          fontWeight: 700,
          letterSpacing: "-0.025em",
        }}
      >
        {t("title")}
      </Typography.Title>
      <nav aria-label={t("sections.label")}>
        <Flex
          gap={token.marginXXS}
          style={{
            overflowX: "auto",
            paddingBottom: token.paddingXS,
            marginBottom: token.marginLG,
            borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
          }}
        >
          {ADMIN_SECTIONS.map((section) => {
            const active = section === selected;
            return (
              <NavLink
                key={section}
                to={section}
                style={{
                  flex: "none",
                  paddingInline: token.paddingSM,
                  paddingBlock: token.paddingXXS + 2,
                  borderRadius: 999,
                  fontWeight: 500,
                  whiteSpace: "nowrap",
                  textDecoration: "none",
                  color: active ? token.colorText : token.colorTextSecondary,
                  background: active ? token.colorFillSecondary : "transparent",
                }}
              >
                {t(`sections.${section}`)}
              </NavLink>
            );
          })}
        </Flex>
      </nav>
      <Outlet />
    </section>
  );
}
