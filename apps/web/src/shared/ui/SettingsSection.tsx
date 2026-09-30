import { Flex, theme, Typography } from "antd";
import { type ReactNode, useId } from "react";

interface SettingsSectionProps {
  title: string;
  description?: string;
  extra?: ReactNode;
  children: ReactNode;
  testId?: string;
}

export function SettingsSection({
  title,
  description,
  extra,
  children,
  testId,
}: SettingsSectionProps) {
  const { token } = theme.useToken();
  const titleId = useId();
  return (
    <section aria-labelledby={titleId} data-testid={testId}>
      <Flex
        justify="space-between"
        align="flex-start"
        gap={token.marginSM}
        wrap
        style={{ marginBottom: token.marginLG }}
      >
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
      {children}
    </section>
  );
}
