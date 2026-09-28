import { Flex, Layout, theme, Typography } from "antd";
import type { CSSProperties } from "react";
import { useBootState } from "../bootstrap/bootState";

// Decision 93: the vendor credit text and destination are fixed product
// behavior. Neither the text nor the URL is admin-editable, so both live here
// as constants instead of in the translation catalogue.
export const POWERED_BY_TEXT = "Powered by Palmr";
export const PALMR_REPOSITORY_URL = "https://github.com/kyantech/Palmr";

interface ShellFooterProps {
  style?: CSSProperties;
}

export function ShellFooter({ style }: ShellFooterProps) {
  const { bootstrap } = useBootState();
  const { token } = theme.useToken();
  const version = bootstrap.version?.trim() ?? "";
  const showVersion = version.length > 0;
  const showPoweredBy = bootstrap.poweredByVisible;

  if (!showVersion && !showPoweredBy) {
    return null;
  }

  return (
    <Layout.Footer
      data-testid="app-footer"
      style={{
        paddingBlock: token.padding,
        paddingInline: token.padding,
        background: "transparent",
        fontSize: token.fontSizeSM,
        ...style,
      }}
    >
      <Flex
        align="center"
        gap={token.marginSM}
        wrap
        style={{
          paddingTop: token.padding,
          borderTop: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
        }}
      >
        {showVersion ? (
          <Typography.Text
            type="secondary"
            data-testid="app-version"
            style={{
              fontFamily: token.fontFamilyCode,
              fontSize: token.fontSizeSM,
              paddingInline: token.paddingXS,
              paddingBlock: 2,
              borderRadius: token.borderRadiusSM,
              background: token.colorFillQuaternary,
              border: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
            }}
          >
            {version}
          </Typography.Text>
        ) : null}
        {showPoweredBy ? (
          <Typography.Link
            href={PALMR_REPOSITORY_URL}
            target="_blank"
            rel="noreferrer"
            data-testid="powered-by"
            style={{
              marginInlineStart: "auto",
              fontSize: token.fontSizeSM,
              color: token.colorTextTertiary,
            }}
          >
            {POWERED_BY_TEXT}
          </Typography.Link>
        ) : null}
      </Flex>
    </Layout.Footer>
  );
}
