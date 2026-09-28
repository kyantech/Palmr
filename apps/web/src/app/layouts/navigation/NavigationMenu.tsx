import { ConfigProvider, Menu, theme, type MenuProps } from "antd";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router";
import type { NavigationEntry } from "./registry";

export interface NavigationMenuProps {
  entries: readonly NavigationEntry[];
  selectedKey: string | null;
  onNavigate?: () => void;
}

export function NavigationMenu({ entries, selectedKey, onNavigate }: NavigationMenuProps) {
  const { t } = useTranslation("common");
  const { token } = theme.useToken();
  const items: MenuProps["items"] = entries.map((entry) => ({
    key: entry.key,
    icon: <span className="anticon">{entry.icon}</span>,
    label: (
      <NavLink to={entry.path} onClick={onNavigate}>
        {t(entry.labelKey)}
      </NavLink>
    ),
  }));
  return (
    <ConfigProvider
      theme={{
        components: {
          Menu: {
            itemBg: "transparent",
            itemHeight: 40,
            itemMarginInline: 0,
            itemMarginBlock: token.marginXXS,
            itemBorderRadius: token.borderRadius + 2,
            itemColor: token.colorTextSecondary,
            itemHoverColor: token.colorText,
            itemHoverBg: token.colorFillTertiary,
            itemSelectedBg: token.colorPrimaryBg,
            itemSelectedColor: token.colorText,
            iconSize: 18,
            collapsedIconSize: 18,
            iconMarginInlineEnd: token.marginSM,
            activeBarBorderWidth: 0,
            fontSize: token.fontSize,
          },
        },
      }}
    >
      <Menu
        mode="inline"
        selectedKeys={selectedKey === null ? [] : [selectedKey]}
        items={items}
        style={{ borderInlineEnd: "none", background: "transparent", fontWeight: 500 }}
      />
    </ConfigProvider>
  );
}
