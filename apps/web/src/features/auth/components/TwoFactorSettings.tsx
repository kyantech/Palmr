import { Alert, Button, Flex, Popconfirm, Skeleton, Tag, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { SettingsSection } from "../../../shared/ui/SettingsSection";
import { type BackupCodes, useDisableTwoFactor, useRegenerateBackupCodes } from "../api/mutations";
import { type TwoFactorStatus, useTwoFactorStatus } from "../api/queries";
import { setLoginNotice } from "../store";
import { BackupCodesPanel } from "./BackupCodesPanel";
import { TwoFactorSetup } from "./TwoFactorSetup";

const LOW_BACKUP_CODES = 3;

function StatusTag({ enabled }: { enabled: boolean }) {
  const { t } = useTranslation("auth");
  return (
    <Tag
      color={enabled ? "success" : "default"}
      variant="filled"
      style={{ marginInlineEnd: 0 }}
      data-testid="two-factor-state"
      data-enabled={enabled ? "true" : "false"}
    >
      {t(enabled ? "twoFactor.on" : "twoFactor.off")}
    </Tag>
  );
}

function reportable(error: unknown): unknown {
  const presented = presentError(error);
  return presented.presentation.silent || presented.code === "AUTH_RECENT_AUTH_REQUIRED"
    ? null
    : error;
}

export interface TwoFactorSettingsProps {
  appName: string;
  hasLocalPassword: boolean;
  onEnabled: () => Promise<void>;
  onSignedOutEverywhere: () => Promise<void>;
}

export function TwoFactorSettings({
  appName,
  hasLocalPassword,
  onEnabled,
  onSignedOutEverywhere,
}: TwoFactorSettingsProps) {
  const { t } = useTranslation("auth");
  const status = useTwoFactorStatus();
  const [setupOpen, setSetupOpen] = useState(false);
  const [codes, setCodes] = useState<BackupCodes | null>(null);
  const [notice, setNotice] = useState<"enabled" | "regenerated" | null>(null);

  let body;
  if (setupOpen) {
    body = (
      <TwoFactorSetup
        appName={appName}
        finishLabel={t("twoFactor.done")}
        onVerified={onEnabled}
        onFinished={() => {
          setSetupOpen(false);
          setNotice("enabled");
        }}
        onCancel={() => {
          setSetupOpen(false);
        }}
      />
    );
  } else if (codes !== null) {
    body = (
      <BackupCodesPanel
        codes={codes.backupCodes}
        appName={appName}
        doneLabel={t("twoFactor.done")}
        onDone={() => {
          setCodes(null);
          setNotice("regenerated");
        }}
      />
    );
  } else if (status.isPending) {
    body = <Skeleton active paragraph={{ rows: 2 }} />;
  } else if (status.isError) {
    body = <ErrorAlert error={status.error} />;
  } else {
    body = (
      <TwoFactorOverview
        status={status.data}
        hasLocalPassword={hasLocalPassword}
        notice={notice}
        onEnable={() => {
          setNotice(null);
          setSetupOpen(true);
        }}
        onCodes={(next) => {
          setNotice(null);
          setCodes(next);
        }}
        onDisabled={async () => {
          setLoginNotice("twoFactorDisabled");
          await onSignedOutEverywhere();
        }}
      />
    );
  }

  return (
    <SettingsSection
      title={t("twoFactor.title")}
      description={t("twoFactor.description")}
      testId="settings-two-factor"
    >
      {body}
    </SettingsSection>
  );
}

interface TwoFactorOverviewProps {
  status: TwoFactorStatus;
  hasLocalPassword: boolean;
  notice: "enabled" | "regenerated" | null;
  onEnable: () => void;
  onCodes: (codes: BackupCodes) => void;
  onDisabled: () => Promise<void>;
}

function TwoFactorOverview({
  status,
  hasLocalPassword,
  notice,
  onEnable,
  onCodes,
  onDisabled,
}: TwoFactorOverviewProps) {
  const { t, i18n } = useTranslation("auth");
  const { token } = theme.useToken();
  const [failure, setFailure] = useState<unknown>(null);
  const regenerate = useRegenerateBackupCodes(onCodes);
  const disable = useDisableTwoFactor(onDisabled);
  const low = status.enabled && status.backupCodesRemaining <= LOW_BACKUP_CODES;

  const run = (action: typeof regenerate | typeof disable) => {
    setFailure(null);
    action.mutate(undefined, {
      onError: (error) => {
        setFailure(reportable(error));
      },
    });
  };

  return (
    <Flex vertical gap={token.margin}>
      {notice === null ? null : (
        <Alert type="success" showIcon role="status" title={t(`twoFactor.notice.${notice}`)} />
      )}
      {failure === null ? null : <ErrorAlert error={failure} />}
      <Flex vertical gap={token.marginXXS}>
        <Flex align="center" gap={token.marginXS} wrap>
          <Typography.Text strong>{t("twoFactor.authenticatorApp")}</Typography.Text>
          <StatusTag enabled={status.enabled} />
        </Flex>
        {status.enabled ? (
          <Flex vertical gap={2}>
            {status.enrolledAt === null ? null : (
              <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {t("twoFactor.enrolledAt", {
                  date: formatDateTime(status.enrolledAt, i18n.language),
                })}
              </Typography.Text>
            )}
            <Typography.Text
              type={low ? "warning" : "secondary"}
              style={{ fontSize: token.fontSizeSM }}
              data-testid="backup-codes-remaining"
            >
              {t("twoFactor.backupCodesRemaining", { remaining: status.backupCodesRemaining })}
            </Typography.Text>
            {low ? (
              <Typography.Text type="warning" style={{ fontSize: token.fontSizeSM }}>
                {t("twoFactor.backupCodesLow")}
              </Typography.Text>
            ) : null}
          </Flex>
        ) : (
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t(hasLocalPassword ? "twoFactor.offDescription" : "twoFactor.external")}
          </Typography.Text>
        )}
      </Flex>
      {status.requiredByPolicy ? (
        <Alert
          type="info"
          showIcon
          data-testid="two-factor-policy"
          title={t("twoFactor.policy.title")}
          description={t(
            status.enabled ? "twoFactor.policy.enabled" : "twoFactor.policy.description",
          )}
        />
      ) : null}
      {status.enabled ? (
        <Flex gap={token.marginXS} wrap>
          <Popconfirm
            title={t("twoFactor.regenerate.title")}
            description={t("twoFactor.regenerate.description")}
            okText={t("twoFactor.regenerate.confirm")}
            cancelText={t("twoFactor.cancel")}
            onConfirm={() => {
              run(regenerate);
            }}
          >
            <Button loading={regenerate.isPending}>{t("twoFactor.regenerate.action")}</Button>
          </Popconfirm>
          {status.canDisable ? (
            <Popconfirm
              title={t("twoFactor.disable.title")}
              description={t("twoFactor.disable.description")}
              okText={t("twoFactor.disable.confirm")}
              cancelText={t("twoFactor.cancel")}
              okButtonProps={{ danger: true }}
              onConfirm={() => {
                run(disable);
              }}
            >
              <Button danger loading={disable.isPending}>
                {t("twoFactor.disable.action")}
              </Button>
            </Popconfirm>
          ) : null}
        </Flex>
      ) : hasLocalPassword ? (
        <div>
          <Button type="primary" onClick={onEnable}>
            {t("twoFactor.enable")}
          </Button>
        </div>
      ) : null}
    </Flex>
  );
}
