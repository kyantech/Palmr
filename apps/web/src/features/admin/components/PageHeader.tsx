import { Flex, theme, Typography } from "antd";
import { type ReactNode, useId } from "react";

interface PageHeaderProps {
  title: string;
  description?: string;
  extra?: ReactNode;
  back?: ReactNode;
}

export function PageHeader({ title, description, extra, back }: PageHeaderProps) {
  const { token } = theme.useToken();
  const titleId = useId();
  return (
    <header aria-labelledby={titleId} style={{ marginBottom: token.marginLG }}>
      {back === undefined ? null : <div style={{ marginBottom: token.marginXS }}>{back}</div>}
      <Flex justify="space-between" align="flex-start" gap={token.marginSM} wrap>
        <Flex vertical gap={token.marginXXS} style={{ minWidth: 0, flex: "1 1 280px" }}>
          <Typography.Title
            level={2}
            id={titleId}
            style={{
              margin: 0,
              fontSize: token.fontSizeHeading4,
              lineHeight: token.lineHeightHeading4,
              fontWeight: 600,
            }}
          >
            {title}
          </Typography.Title>
          {description === undefined ? null : (
            <Typography.Text type="secondary">{description}</Typography.Text>
          )}
        </Flex>
        {extra}
      </Flex>
    </header>
  );
}
