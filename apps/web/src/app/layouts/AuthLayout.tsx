import { Layout, theme, Typography } from "antd";
import { type CSSProperties, useState } from "react";
import { Outlet, useMatches } from "react-router";
import { resolveApiUrl } from "../../shared/api/basePath";
import { useBootState } from "../bootstrap/bootState";

export const AUTH_BACKGROUND_SIZE = "cover";
export const AUTH_BACKGROUND_POSITION = "center center";

export interface AuthLayoutHandle {
  authLayoutWidth?: "default" | "wide";
}

const CARD_WIDTH = { default: 420, wide: 520 } as const;

export function authBackgroundStyle(url: string | null | undefined): CSSProperties {
  if (!url) {
    return {};
  }
  return {
    backgroundImage: `url(${JSON.stringify(resolveApiUrl(url))})`,
    backgroundSize: AUTH_BACKGROUND_SIZE,
    backgroundPosition: AUTH_BACKGROUND_POSITION,
    backgroundRepeat: "no-repeat",
  };
}

function px(value: number): string {
  return `${String(value)}px`;
}

function useCardWidth(): number {
  const matches = useMatches();
  const width = matches.reduce<AuthLayoutHandle["authLayoutWidth"]>(
    (current, match) => (match.handle as AuthLayoutHandle | undefined)?.authLayoutWidth ?? current,
    "default",
  );
  return CARD_WIDTH[width ?? "default"];
}

function Brand({ appName, logoUrl }: { appName: string; logoUrl: string | null }) {
  const { token } = theme.useToken();
  const [logoFailed, setLogoFailed] = useState(false);
  const showLogo = logoUrl !== null && !logoFailed;
  return (
    <div
      data-testid="auth-brand"
      style={{
        display: "flex",
        alignItems: "center",
        gap: token.marginSM,
        marginBottom: token.marginXL,
        minHeight: 36,
      }}
    >
      {showLogo ? (
        <img
          src={resolveApiUrl(logoUrl)}
          alt=""
          width={36}
          height={36}
          style={{ borderRadius: token.borderRadius, objectFit: "contain", flex: "none" }}
          onError={() => {
            setLogoFailed(true);
          }}
        />
      ) : null}
      <Typography.Text
        strong
        style={{
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          fontSize: token.fontSizeLG,
          lineHeight: token.lineHeightLG,
        }}
      >
        {appName}
      </Typography.Text>
    </div>
  );
}

interface AuthLayoutProps {
  backgroundUrl?: string | null;
}

export function AuthLayout({ backgroundUrl = null }: AuthLayoutProps) {
  const { bootstrap } = useBootState();
  const { token } = theme.useToken();
  const width = useCardWidth();
  return (
    <Layout
      data-auth-background={backgroundUrl ? "image" : "none"}
      style={{
        minHeight: "100dvh",
        background: token.colorBgLayout,
        ...authBackgroundStyle(backgroundUrl),
      }}
    >
      <main
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          paddingBlock: `clamp(${px(token.paddingLG)}, 8vh, ${px(token.paddingXL * 2)})`,
          paddingInline: `clamp(${px(token.padding)}, 4vw, ${px(token.paddingLG)})`,
        }}
      >
        <div
          style={{
            width: "100%",
            maxWidth: width,
            boxSizing: "border-box",
            background: token.colorBgContainer,
            borderWidth: token.lineWidth,
            borderStyle: token.lineType,
            borderColor: token.colorBorderSecondary,
            borderRadius: token.borderRadiusLG * 1.5,
            boxShadow: token.boxShadowTertiary,
            padding: `clamp(${px(token.paddingLG)}, 6vw, ${px(token.paddingXL + token.paddingXS)})`,
          }}
        >
          <Brand appName={bootstrap.appName} logoUrl={bootstrap.logoUrl} />
          <Outlet />
        </div>
      </main>
    </Layout>
  );
}
