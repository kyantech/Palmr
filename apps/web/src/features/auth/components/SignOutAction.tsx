import { Button, Flex, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";

interface SignOutActionProps {
  account: string;
  onSignOut: () => Promise<void>;
}

export function SignOutAction({ account, onSignOut }: SignOutActionProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);

  const signOut = async () => {
    setPending(true);
    setFailure(null);
    try {
      await onSignOut();
    } catch (error) {
      setFailure(error);
      setPending(false);
    }
  };

  return (
    <Flex vertical gap={token.marginXS} align="center" data-testid="restricted-sign-out">
      {failure === null ? null : <ErrorAlert error={failure} />}
      <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
        {t("restricted.signedInAs", { account })}
      </Typography.Text>
      <Button
        type="text"
        loading={pending}
        onClick={() => {
          void signOut();
        }}
      >
        {t("restricted.signOut")}
      </Button>
    </Flex>
  );
}
