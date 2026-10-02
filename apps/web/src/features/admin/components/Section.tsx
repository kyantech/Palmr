import { Card, Flex, theme, Typography } from "antd";
import { type ReactNode, useId } from "react";

interface SectionProps {
  title: string;
  description?: string;
  extra?: ReactNode;
  children: ReactNode;
  testId?: string;
  tone?: "default" | "danger";
}

export function Section({
  title,
  description,
  extra,
  children,
  testId,
  tone = "default",
}: SectionProps) {
  const { token } = theme.useToken();
  const titleId = useId();
  return (
    <Card
      variant="outlined"
      styles={{ body: { padding: token.paddingLG } }}
      {...(tone === "danger" ? { style: { borderColor: token.colorErrorBorder } } : {})}
    >
      <section aria-labelledby={titleId} data-testid={testId}>
        <Flex
          justify="space-between"
          align="flex-start"
          gap={token.marginSM}
          wrap
          style={{ marginBottom: token.margin }}
        >
          <Flex vertical gap={2} style={{ minWidth: 0, flex: "1 1 240px" }}>
            <Typography.Title
              level={3}
              id={titleId}
              style={{ margin: 0, fontSize: token.fontSizeLG, fontWeight: 600 }}
            >
              {title}
            </Typography.Title>
            {description === undefined ? null : (
              <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {description}
              </Typography.Text>
            )}
          </Flex>
          {extra}
        </Flex>
        {children}
      </section>
    </Card>
  );
}
