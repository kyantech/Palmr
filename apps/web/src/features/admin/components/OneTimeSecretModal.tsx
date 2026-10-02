import { Alert, Button, Flex, Input, Modal, theme, Typography } from "antd";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";

interface OneTimeSecretModalProps {
  secret: string | null;
  title: string;
  description: string;
  fieldLabel: string;
  testId: string;
  onClose: () => void;
}

type CopyState = "idle" | "copied" | "failed";

export function OneTimeSecretModal({
  secret,
  title,
  description,
  fieldLabel,
  testId,
  onClose,
}: OneTimeSecretModalProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const fieldId = useId();
  const [copy, setCopy] = useState<CopyState>("idle");

  async function copySecret() {
    if (secret === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(secret);
      setCopy("copied");
    } catch {
      setCopy("failed");
    }
  }

  function close() {
    setCopy("idle");
    onClose();
  }

  return (
    <Modal
      open={secret !== null}
      title={title}
      onCancel={close}
      footer={
        <Button type="primary" onClick={close}>
          {t("secret.done")}
        </Button>
      }
      centered
      destroyOnHidden
      width={480}
      mask={{ closable: false }}
    >
      {secret === null ? null : (
        <Flex vertical gap={token.margin} data-testid={testId}>
          <Alert type="warning" showIcon title={t("secret.notShownAgain")} />
          <Typography.Text>{description}</Typography.Text>
          <Flex vertical gap={token.marginXXS}>
            <label htmlFor={fieldId}>
              <Typography.Text strong>{fieldLabel}</Typography.Text>
            </label>
            <Flex gap={token.marginXS}>
              <Input
                id={fieldId}
                readOnly
                value={secret}
                autoComplete="off"
                spellCheck={false}
                style={{ fontFamily: token.fontFamilyCode }}
                onFocus={(event) => {
                  event.target.select();
                }}
              />
              <Button
                onClick={() => {
                  void copySecret();
                }}
              >
                {t("secret.copy")}
              </Button>
            </Flex>
            {copy === "idle" ? null : (
              <Typography.Text
                role="status"
                type={copy === "copied" ? "success" : "warning"}
                style={{ fontSize: token.fontSizeSM }}
              >
                {t(copy === "copied" ? "secret.copied" : "secret.copyFailed")}
              </Typography.Text>
            )}
          </Flex>
        </Flex>
      )}
    </Modal>
  );
}
