import { Button, Flex, theme, Typography } from "antd";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ReportedErrorAlert } from "../../../shared/errors";
import { messageForLanding, readExternalReauthLanding } from "../externalReauthMessage";

export interface ReauthCompletePageProps {
  search: string;
  onContinue: () => void;
}

function openerWindow(): Window | null {
  const opener = (window as { opener?: Window | null }).opener;
  return opener === undefined || opener === null || opener.closed ? null : opener;
}

export function ReauthCompletePage({ search, onContinue }: ReauthCompletePageProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const landing = useMemo(() => readExternalReauthLanding(search), [search]);
  const [opener] = useState(openerWindow);
  const sent = useRef(false);
  const reporting = landing !== null && opener !== null;

  useEffect(() => {
    if (landing === null || opener === null || sent.current) {
      return;
    }
    sent.current = true;
    try {
      opener.postMessage(messageForLanding(landing), window.location.origin);
      window.close();
    } catch {
      return;
    }
  }, [landing, opener]);

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
        ) : (
          <Flex justify="end">
            <Button type="primary" onClick={onContinue}>
              {t("reauthComplete.continue")}
            </Button>
          </Flex>
        )}
      </Flex>
    </main>
  );
}
