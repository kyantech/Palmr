import { Button, Flex, theme, Typography } from "antd";
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { ReportedErrorAlert } from "../../../shared/errors";
import { openExternalReauthChannel } from "../externalReauthChannel";
import {
  type ExternalReauthCompletion,
  messageForLanding,
  readExternalReauthCompletion,
} from "../externalReauthMessage";

export interface ReauthCompletePageProps {
  search: string;
  onContinue: () => void;
}

function broadcast({ channel, landing }: ExternalReauthCompletion): boolean {
  try {
    const target = openExternalReauthChannel(channel);
    target.postMessage(messageForLanding(landing));
    target.close();
    return true;
  } catch {
    return false;
  }
}

export function ReauthCompletePage({ search, onContinue }: ReauthCompletePageProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const completion = useMemo(() => readExternalReauthCompletion(search), [search]);
  const landing = completion?.landing ?? null;
  const sent = useRef(false);
  const reporting = completion !== null;

  useEffect(() => {
    if (completion === null || sent.current) {
      return;
    }
    sent.current = true;
    if (broadcast(completion)) {
      try {
        window.close();
      } catch {
        return;
      }
    }
  }, [completion]);

  const outcome = landing?.status ?? "invalid";
  return (
    <main
      data-testid="reauth-complete"
      data-outcome={outcome}
      data-reporting={reporting ? "true" : "false"}
      style={{
        minHeight: "100dvh",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: token.paddingLG,
        background: token.colorBgLayout,
      }}
    >
      <Flex
        vertical
        gap={token.margin}
        style={{
          width: "100%",
          maxWidth: 420,
          padding: token.paddingLG,
          background: token.colorBgContainer,
          border: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
          borderRadius: token.borderRadiusLG,
        }}
      >
        <Typography.Title level={1} style={{ margin: 0, fontSize: token.fontSizeHeading4 }}>
          {t(`reauthComplete.${outcome}.title`)}
        </Typography.Title>
        {landing?.status === "error" ? (
          <ReportedErrorAlert reported={landing.reported} />
        ) : (
          <Typography.Text type="secondary">
            {t(`reauthComplete.${outcome}.description`)}
          </Typography.Text>
        )}
        {reporting ? (
          <Typography.Text type="secondary" role="status">
            {t("reauthComplete.closeHint")}
          </Typography.Text>
        ) : null}
        <Flex justify="end">
          <Button type={reporting ? "default" : "primary"} onClick={onContinue}>
            {t("reauthComplete.continue")}
          </Button>
        </Flex>
      </Flex>
    </main>
  );
}
