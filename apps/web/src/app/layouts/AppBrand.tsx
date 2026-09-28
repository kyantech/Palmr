import { theme, Typography } from "antd";
import { type CSSProperties, useState } from "react";
import { resolveApiUrl } from "../../shared/api/basePath";

export interface AppBrandProps {
  appName: string;
  logoUrl: string | null;
  logoSize?: number;
  showName?: boolean;
  testId?: string;
  style?: CSSProperties;
}

export function AppBrand({
  appName,
  logoUrl,
  logoSize = 32,
  showName = true,
  testId,
  style,
}: AppBrandProps) {
  const { token } = theme.useToken();
  const [logoFailed, setLogoFailed] = useState(false);
  const showLogo = logoUrl !== null && !logoFailed;
  return (
    <div
      {...(testId === undefined ? {} : { "data-testid": testId })}
      style={{
        display: "flex",
        alignItems: "center",
        gap: token.marginSM,
        minWidth: 0,
        ...style,
      }}
    >
      {showLogo ? (
        <img
          src={resolveApiUrl(logoUrl)}
          alt=""
          width={logoSize}
          height={logoSize}
          style={{ borderRadius: token.borderRadiusLG, objectFit: "contain", flex: "none" }}
          onError={() => {
            setLogoFailed(true);
          }}
        />
      ) : null}
      {showName ? (
        <Typography.Text
          strong
          style={{
            minWidth: 0,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            fontSize: token.fontSizeLG,
            lineHeight: token.lineHeightLG,
            fontWeight: 650,
            letterSpacing: "-0.015em",
          }}
        >
          {appName}
        </Typography.Text>
      ) : null}
    </div>
  );
}
