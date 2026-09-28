import { Flex, theme, Typography } from "antd";

interface AuthHeadingProps {
  title: string;
  description: string;
}

export function AuthHeading({ title, description }: AuthHeadingProps) {
  const { token } = theme.useToken();
  return (
    <Flex vertical gap={token.marginXXS} style={{ marginBottom: token.marginLG }}>
      <Typography.Title
        level={1}
        style={{
          margin: 0,
          fontSize: token.fontSizeHeading3,
          lineHeight: token.lineHeightHeading3,
          letterSpacing: "-0.01em",
        }}
      >
        {title}
      </Typography.Title>
      <Typography.Text type="secondary">{description}</Typography.Text>
    </Flex>
  );
}
