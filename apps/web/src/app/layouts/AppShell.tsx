import { Button, ConfigProvider, Drawer, Flex, Grid, Layout, theme, Tooltip } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Outlet, useLocation } from "react-router";
import { ErrorAlert } from "../../shared/errors";
import { useBootState } from "../bootstrap/bootState";
import { useSignOut } from "../session/useSignOut";
import { AppBrand } from "./AppBrand";
import { BottomTabs } from "./navigation/BottomTabs";
import { useTextDirection } from "./navigation/direction";
import { MenuIcon, PanelIcon } from "./navigation/icons";
import { NavigationMenu } from "./navigation/NavigationMenu";
import {
  NAV_ENTRIES,
  type NavigationEntry,
  selectedNavigationKey,
  visibleNavigation,
} from "./navigation/registry";
import { ShellFooter } from "./ShellFooter";
import { UserMenu } from "./UserMenu";

const SIDER_WIDTH = 256;
const COLLAPSED_SIDER_WIDTH = 72;
const HEADER_HEIGHT = 64;
const CONTENT_MAX_WIDTH = 1280;
const BOTTOM_TABS_CLEARANCE = 88;

export interface AppShellProps {
  entries?: readonly NavigationEntry[];
}

export function AppShell({ entries = NAV_ENTRIES }: AppShellProps) {
  const { bootstrap, me } = useBootState();
  const { t } = useTranslation("common");
  const { token } = theme.useToken();
  const direction = useTextDirection();
  const { signOut, isPending, error } = useSignOut();
  const location = useLocation();
  const screens = Grid.useBreakpoint();
  const desktop = screens.lg === true;
  const compact = screens.xs === true;
  const [navigationOpen, setNavigationOpen] = useState(false);
  const [navigationCollapsed, setNavigationCollapsed] = useState(false);

  const visible = visibleNavigation(entries, me?.user.role);
  const selectedKey = selectedNavigationKey(visible, location.pathname);
  const hairline = `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`;
  const inset = token.marginXS;
  const glass = `color-mix(in srgb, ${token.colorBgContainer} 78%, transparent)`;

  const navigation = (onNavigate?: () => void) => (
    <nav aria-label={t("nav.label")}>
      <NavigationMenu
        entries={visible}
        selectedKey={selectedKey}
        {...(onNavigate ? { onNavigate } : {})}
      />
    </nav>
  );

  const collapseLabel = t(navigationCollapsed ? "nav.expand" : "nav.collapse");

  return (
    <Layout
      style={{ minHeight: "100dvh", background: token.colorBgLayout }}
      data-testid="app-shell"
    >
      {desktop ? (
        <Layout.Sider
          width={SIDER_WIDTH}
          collapsedWidth={COLLAPSED_SIDER_WIDTH}
          collapsed={navigationCollapsed}
          trigger={null}
          data-testid="app-sider"
          style={{
            position: "sticky",
            top: 0,
            height: "100dvh",
            background: "transparent",
          }}
        >
          <Flex
            vertical
            style={{
              height: "100%",
              paddingInline: token.paddingSM,
              paddingBottom: token.paddingSM,
            }}
          >
            <Flex
              align="center"
              justify={navigationCollapsed ? "center" : "flex-start"}
              style={{
                height: HEADER_HEIGHT + inset,
                paddingTop: inset,
                paddingInline: navigationCollapsed ? 0 : token.paddingXS,
              }}
            >
              <AppBrand
                appName={bootstrap.appName}
                logoUrl={bootstrap.logoUrl}
                showName={!navigationCollapsed}
              />
            </Flex>
            <div style={{ marginTop: token.marginSM, flex: 1, minHeight: 0, overflowY: "auto" }}>
              {navigation()}
            </div>
            <Flex justify={navigationCollapsed ? "center" : "flex-start"}>
              <Tooltip title={collapseLabel} placement={direction === "rtl" ? "left" : "right"}>
                <Button
                  type="text"
                  icon={<PanelIcon />}
                  aria-label={collapseLabel}
                  aria-expanded={!navigationCollapsed}
                  onClick={() => {
                    setNavigationCollapsed((value) => !value);
                  }}
                  style={{
                    width: 40,
                    height: 40,
                    fontSize: 18,
                    color: token.colorTextTertiary,
                    marginInlineStart: navigationCollapsed ? 0 : token.marginXXS,
                  }}
                />
              </Tooltip>
            </Flex>
          </Flex>
        </Layout.Sider>
      ) : null}
      <Layout
        style={{
          background: token.colorBgContainer,
          ...(desktop
            ? {
                marginBlock: inset,
                marginInlineEnd: inset,
                minHeight: `calc(100dvh - ${String(inset * 2)}px)`,
                border: hairline,
                borderRadius: token.borderRadiusLG + 4,
                boxShadow: token.boxShadowTertiary,
              }
            : {}),
        }}
      >
        <Layout.Header
          style={{
            position: "sticky",
            top: 0,
            zIndex: token.zIndexBase + 10,
            height: HEADER_HEIGHT,
            paddingInline: desktop ? token.paddingLG : token.padding,
            background: glass,
            backdropFilter: "saturate(180%) blur(14px)",
            WebkitBackdropFilter: "saturate(180%) blur(14px)",
            borderBottom: hairline,
            lineHeight: token.lineHeight,
            ...(desktop
              ? {
                  borderStartStartRadius: token.borderRadiusLG + 4,
                  borderStartEndRadius: token.borderRadiusLG + 4,
                }
              : {}),
          }}
        >
          <Flex align="center" gap={token.marginXS} style={{ height: "100%" }}>
            {desktop ? null : (
              <Button
                type="text"
                aria-label={t("nav.open")}
                aria-expanded={navigationOpen}
                icon={<MenuIcon />}
                onClick={() => {
                  setNavigationOpen(true);
                }}
                style={{ width: 40, height: 40, fontSize: 20, marginInlineStart: -token.marginXS }}
              />
            )}
            {desktop ? null : (
              <AppBrand appName={bootstrap.appName} logoUrl={bootstrap.logoUrl} logoSize={28} />
            )}
            <Flex flex={1} justify="flex-end" align="center">
              {me === null ? null : (
                <UserMenu
                  user={me.user}
                  compact={compact}
                  signingOut={isPending}
                  onSignOut={() => {
                    void signOut();
                  }}
                />
              )}
            </Flex>
          </Flex>
        </Layout.Header>
        {error === null ? null : (
          <div
            style={{
              paddingInline: desktop ? token.paddingLG : token.padding,
              paddingTop: token.padding,
            }}
          >
            <ErrorAlert error={error} />
          </div>
        )}
        <Layout.Content
          data-testid="app-main"
          style={{
            paddingInline: desktop ? token.paddingXL : token.padding,
            paddingTop: desktop ? token.paddingXL : token.paddingLG,
            paddingBottom: token.paddingXL,
          }}
        >
          <div style={{ maxWidth: CONTENT_MAX_WIDTH, marginInline: "auto" }}>
            <Outlet />
          </div>
        </Layout.Content>
        <ShellFooter style={{ paddingInline: desktop ? token.paddingXL : token.padding }} />
        {compact ? <div aria-hidden="true" style={{ height: BOTTOM_TABS_CLEARANCE }} /> : null}
      </Layout>
      {desktop ? null : (
        <ConfigProvider theme={{ components: { Drawer: { paddingLG: token.paddingSM } } }}>
          <Drawer
            open={navigationOpen}
            onClose={() => {
              setNavigationOpen(false);
            }}
            placement={direction === "rtl" ? "right" : "left"}
            size={SIDER_WIDTH + token.paddingLG}
            title={
              <AppBrand appName={bootstrap.appName} logoUrl={bootstrap.logoUrl} logoSize={28} />
            }
          >
            {navigation(() => {
              setNavigationOpen(false);
            })}
          </Drawer>
        </ConfigProvider>
      )}
      {compact ? <BottomTabs entries={visible} selectedKey={selectedKey} /> : null}
      {/* Transfer Center dock: the future Transfer Center mounts here and is empty until then. */}
      <div data-transfer-dock="" />
    </Layout>
  );
}
