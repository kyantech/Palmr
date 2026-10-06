import { Alert, Button, Checkbox, Flex, Modal, Skeleton, Tag, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ApiError,
  detailBlockers,
  ErrorAlert,
  isApiErrorCode,
  presentErrorCode,
} from "../../../shared/errors";
import { useSetPasswordLogin } from "../api/mutations";
import { usePasswordLogin } from "../api/queries";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { Section } from "./Section";

type Notice = "disabled" | "enabled";

function BlockerList({ codes }: { codes: readonly string[] }) {
  const { t } = useTranslation("errors");
  return (
    <ul style={{ margin: 0, paddingInlineStart: 20 }} data-testid="password-login-blockers">
      {codes.map((code, index) => (
        <li key={`${code}-${String(index)}`} data-blocker={code}>
          {t(presentErrorCode(code, null).presentation.i18nKey)}
        </li>
      ))}
    </ul>
  );
}

export function PasswordLoginPanel() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const state = usePasswordLogin();
  const change = useSetPasswordLogin();
  const feedback = useActionFeedback<Notice>();
  const [confirming, setConfirming] = useState(false);
  const [understood, setUnderstood] = useState(false);

  const dialogOpen = confirming && !(change.isSuccess && !change.variables.enabled);
  const refusal =
    change.isError && change.error instanceof ApiError ? detailBlockers(change.error) : [];

  function openDialog() {
    change.reset();
    feedback.clear();
    setUnderstood(false);
    setConfirming(true);
  }

  function disable() {
    if (!understood || change.isPending) {
      return;
    }
    change.mutate(
      { enabled: false, confirm: true },
      {
        onSuccess: () => {
          setConfirming(false);
          feedback.succeed("disabled");
        },
      },
    );
  }

  function enable() {
    if (change.isPending) {
      return;
    }
    feedback.clear();
    change.mutate(
      { enabled: true, confirm: true },
      {
        onSuccess: () => {
          feedback.succeed("enabled");
        },
        onError: (error) => {
          feedback.fail(error);
        },
      },
    );
  }

  let body;
  if (state.isPending) {
    body = <Skeleton active paragraph={{ rows: 3 }} />;
  } else if (state.isError) {
    body = <ErrorAlert error={state.error} />;
  } else {
    const { passwordLoginEnabled, canDisable, blockers, safeAdminLoginPaths } = state.data;
    body = (
      <Flex vertical gap={token.margin}>
        <FeedbackAlerts
          feedback={feedback}
          noticeKey={(notice) => `passwordLogin.notice.${notice}`}
        />
        <Flex align="center" gap={token.marginXS} wrap>
          <Typography.Text strong>{t("passwordLogin.state")}</Typography.Text>
          <Tag
            color={passwordLoginEnabled ? "success" : "warning"}
            variant="filled"
            data-testid="password-login-state"
            data-enabled={passwordLoginEnabled ? "true" : "false"}
          >
            {t(passwordLoginEnabled ? "passwordLogin.on" : "passwordLogin.off")}
          </Tag>
        </Flex>
        <Typography.Text type="secondary">
          {t(passwordLoginEnabled ? "passwordLogin.explainOn" : "passwordLogin.explainOff")}
        </Typography.Text>
        {passwordLoginEnabled && blockers.length > 0 ? (
          <Alert
            type="warning"
            showIcon
            data-testid="password-login-blockers-alert"
            title={t("passwordLogin.blockers.title")}
            description={<BlockerList codes={blockers.map((blocker) => blocker.code)} />}
          />
        ) : null}
        <Flex vertical gap={token.marginXS} data-testid="password-login-paths">
          <Typography.Text strong>{t("passwordLogin.paths.title")}</Typography.Text>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t("passwordLogin.paths.description")}
          </Typography.Text>
          {safeAdminLoginPaths.length === 0 ? (
            <Typography.Text type="secondary" data-testid="password-login-paths-empty">
              {t("passwordLogin.paths.empty")}
            </Typography.Text>
          ) : (
            <ul
              style={{
                margin: 0,
                padding: 0,
                listStyle: "none",
                display: "grid",
                gap: token.marginXXS,
              }}
            >
              {safeAdminLoginPaths.map((path) => (
                <li
                  key={`${path.userId}-${path.providerSlug}`}
                  data-testid="password-login-path"
                  data-validated={path.providerValidated ? "true" : "false"}
                >
                  <Flex align="center" gap={token.marginXS} wrap>
                    <Typography.Text>
                      {t("passwordLogin.paths.entry", {
                        username: path.username,
                        provider: path.providerSlug,
                      })}
                    </Typography.Text>
                    <Tag
                      color={path.providerValidated ? "success" : "default"}
                      variant="filled"
                      style={{ marginInlineEnd: 0 }}
                    >
                      {t(
                        path.providerValidated
                          ? "passwordLogin.paths.validated"
                          : "passwordLogin.paths.notValidated",
                      )}
                    </Tag>
                  </Flex>
                </li>
              ))}
            </ul>
          )}
        </Flex>
        <div>
          {passwordLoginEnabled ? (
            <Button danger disabled={!canDisable} onClick={openDialog}>
              {t("passwordLogin.disable.action")}
            </Button>
          ) : (
            <Button type="primary" loading={change.isPending} onClick={enable}>
              {t("passwordLogin.enable.action")}
            </Button>
          )}
        </div>
      </Flex>
    );
  }

  return (
    <Section
      title={t("passwordLogin.title")}
      description={t("passwordLogin.description")}
      testId="password-login-panel"
    >
      {body}
      <Modal
        open={dialogOpen}
        title={t("passwordLogin.disable.title")}
        okText={t("passwordLogin.disable.confirm")}
        cancelText={t("common.cancel")}
        okButtonProps={{ danger: true, disabled: !understood, loading: change.isPending }}
        cancelButtonProps={{ disabled: change.isPending }}
        onOk={disable}
        onCancel={() => {
          if (!change.isPending) {
            setConfirming(false);
          }
        }}
        centered
        destroyOnHidden
        width={520}
        mask={{ closable: !change.isPending }}
        keyboard={!change.isPending}
      >
        <Flex vertical gap={token.marginSM} data-testid="password-login-disable-dialog">
          <Typography.Text>{t("passwordLogin.disable.intro")}</Typography.Text>
          <ul style={{ margin: 0, paddingInlineStart: 20, display: "grid", gap: token.marginXXS }}>
            <li>{t("passwordLogin.disable.points.unavailable")}</li>
            <li>{t("passwordLogin.disable.points.admission")}</li>
            <li>{t("passwordLogin.disable.points.standing")}</li>
            <li>{t("passwordLogin.disable.points.recovery")}</li>
          </ul>
          {change.isError && !isApiErrorCode(change.error, "AUTH_RECENT_AUTH_REQUIRED") ? (
            <Flex vertical gap={token.marginXS}>
              <ErrorAlert error={change.error} />
              {refusal.length === 0 ? null : (
                <BlockerList codes={refusal.map((item) => item.code)} />
              )}
            </Flex>
          ) : null}
          <Checkbox
            checked={understood}
            disabled={change.isPending}
            onChange={(event) => {
              setUnderstood(event.target.checked);
            }}
          >
            {t("passwordLogin.disable.understood")}
          </Checkbox>
        </Flex>
      </Modal>
    </Section>
  );
}
