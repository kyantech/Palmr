import { ConfigProvider, Flex, Grid, Menu, theme, Typography, type MenuProps } from "antd";
import { useTranslation } from "react-i18next";
import { NavLink, Outlet, useLocation } from "react-router";

export const SETTINGS_SECTIONS = ["profile", "appearance", "security", "sessions"] as const;

export type SettingsSectionKey = (typeof SETTINGS_SECTIONS)[number];

const NAVIGATION_WIDTH = 220;
const CONTENT_MAX_WIDTH = 760;

function activeSection(pathname: string): SettingsSectionKey | null {
  const segments = pathname.split("/").filter((segment) => segment.length > 0);
  return SETTINGS_SECTIONS.find((section) => segments.includes(section)) ?? null;
}

function SectionMenu({ selected }: { selected: SettingsSectionKey | null }) {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  const items: MenuProps["items"] = SETTINGS_SECTIONS.map((section) => ({
    key: section,
    label: <NavLink to={section}>{t(`sections.${section}`)}</NavLink>,
  }));
  return (
    <ConfigProvider
      theme={{
        components: {
          Menu: {
            itemBg: "transparent",
            itemHeight: 38,
            itemMarginInline: 0,
            itemMarginBlock: token.marginXXS,
            itemBorderRadius: token.borderRadius + 2,
            itemColor: token.colorTextSecondary,
            itemHoverColor: token.colorText,
            itemHoverBg: token.colorFillTertiary,
            itemSelectedBg: token.colorFillSecondary,
            itemSelectedColor: token.colorText,
            activeBarBorderWidth: 0,
          },
        },
      }}
    >
      <Menu
        mode="inline"
        selectedKeys={selected === null ? [] : [selected]}
        items={items}
        style={{ background: "transparent", borderInlineEnd: "none", fontWeight: 500 }}
      />
    </ConfigProvider>
  );
}

function SectionStrip({ selected }: { selected: SettingsSectionKey | null }) {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  return (
    <Flex
      gap={token.marginXXS}
      style={{
        overflowX: "auto",
        paddingBottom: token.paddingXS,
        borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
      }}
    >
      {SETTINGS_SECTIONS.map((section) => {
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
  );
}

export function SettingsLayout() {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  const { pathname } = useLocation();
  const screens = Grid.useBreakpoint();
  const wide = screens.md === true;
  const selected = activeSection(pathname);

  return (
    <section data-testid="settings-page">
      <Typography.Title
        level={1}
        style={{
          margin: 0,
          marginBottom: wide ? token.marginXL : token.marginLG,
          fontSize: `clamp(${String(token.fontSizeHeading3)}px, 2.4vw + 12px, ${String(token.fontSizeHeading1)}px)`,
          lineHeight: 1.15,
          fontWeight: 700,
          letterSpacing: "-0.025em",
        }}
      >
        {t("title")}
      </Typography.Title>
      <Flex
        vertical={!wide}
        gap={wide ? token.marginXXL : token.marginLG}
        align={wide ? "flex-start" : "stretch"}
      >
        <nav
          aria-label={t("sections.label")}
          style={wide ? { width: NAVIGATION_WIDTH, flex: "none" } : { minWidth: 0 }}
        >
          {wide ? <SectionMenu selected={selected} /> : <SectionStrip selected={selected} />}
        </nav>
        <div style={{ flex: 1, minWidth: 0, maxWidth: CONTENT_MAX_WIDTH }}>
          <Outlet />
        </div>
      </Flex>
    </section>
  );
}
