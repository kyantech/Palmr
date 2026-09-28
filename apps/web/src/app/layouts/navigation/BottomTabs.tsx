import { Button, Drawer, Flex, theme, Typography } from "antd";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router";
import { useTextDirection } from "./direction";
import { MoreIcon } from "./icons";
import { NavigationMenu } from "./NavigationMenu";
import type { NavigationEntry } from "./registry";

export interface BottomTabsProps {
  entries: readonly NavigationEntry[];
  selectedKey: string | null;
}

function TabFace({ icon, label, selected }: { icon: ReactNode; label: string; selected: boolean }) {
  const { token } = theme.useToken();
  return (
    <>
      <span
        style={{
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          width: 56,
          height: 30,
          fontSize: 20,
          borderRadius: 999,
          color: selected ? token.colorPrimary : "inherit",
          background: selected ? token.colorPrimaryBg : "transparent",
          transition: `background ${token.motionDurationMid} ${token.motionEaseInOut}`,
        }}
      >
        {icon}
      </span>
      <Typography.Text
        style={{
          fontSize: token.fontSizeSM,
          lineHeight: 1.2,
          fontWeight: selected ? 600 : 500,
          color: "inherit",
        }}
      >
        {label}
      </Typography.Text>
    </>
  );
}

export function BottomTabs({ entries, selectedKey }: BottomTabsProps) {
  const { t } = useTranslation("common");
  const { token } = theme.useToken();
  const direction = useTextDirection();
  const [moreOpen, setMoreOpen] = useState(false);
  const tabs = entries.filter((entry) => entry.bottomBar === true);
  const overflow = entries.filter((entry) => entry.bottomBar !== true);
  const tabStyle = {
    flex: 1,
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: token.marginXXS,
    minHeight: 64,
    paddingBlock: token.paddingXS,
  } as const;

  return (
    <nav
      aria-label={t("nav.label")}
      data-testid="bottom-tabs"
      style={{
        position: "fixed",
        insetInline: 0,
        bottom: 0,
        zIndex: token.zIndexPopupBase,
        background: `color-mix(in srgb, ${token.colorBgContainer} 82%, transparent)`,
        backdropFilter: "saturate(180%) blur(16px)",
        WebkitBackdropFilter: "saturate(180%) blur(16px)",
        borderTop: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
        paddingBottom: "env(safe-area-inset-bottom)",
      }}
    >
      <Flex align="stretch">
        {tabs.map((entry) => {
          const selected = entry.key === selectedKey;
          return (
            <NavLink
              key={entry.key}
              to={entry.path}
              style={{
                ...tabStyle,
                textDecoration: "none",
                color: selected ? token.colorText : token.colorTextSecondary,
              }}
            >
              <TabFace icon={entry.icon} label={t(entry.labelKey)} selected={selected} />
            </NavLink>
          );
        })}
        {overflow.length > 0 ? (
          <Button
            type="text"
            aria-label={t("nav.more")}
            onClick={() => {
              setMoreOpen(true);
            }}
            style={{
              ...tabStyle,
              height: "auto",
              borderRadius: 0,
              color: token.colorTextSecondary,
            }}
          >
            <TabFace icon={<MoreIcon />} label={t("nav.more")} selected={false} />
          </Button>
        ) : null}
      </Flex>
      <Drawer
        open={moreOpen}
        onClose={() => {
          setMoreOpen(false);
        }}
        placement={direction === "rtl" ? "right" : "left"}
        title={t("nav.more")}
      >
        <NavigationMenu
          entries={overflow}
          selectedKey={selectedKey}
          onNavigate={() => {
            setMoreOpen(false);
          }}
        />
      </Drawer>
    </nav>
  );
}
