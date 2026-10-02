import { Button, Flex, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useActivateUser, useDeactivateUser, useRevokeUserSessions } from "../api/mutations";
import type { UserDetail } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { ConfirmDialog } from "./ConfirmDialog";
import { Section } from "./Section";

type DangerNotice = "deactivated" | "activated" | "sessionsRevoked";
type Confirming = "deactivate" | "revoke" | null;

interface ActionRowProps {
  title: string;
  description: string;
  children: React.ReactNode;
}

function ActionRow({ title, description, children }: ActionRowProps) {
  const { token } = theme.useToken();
  return (
    <Flex justify="space-between" align="center" gap={token.margin} wrap>
      <Flex vertical gap={2} style={{ flex: "1 1 280px", minWidth: 0 }}>
        <Typography.Text strong>{title}</Typography.Text>
        <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
          {description}
        </Typography.Text>
      </Flex>
      {children}
    </Flex>
  );
}

export function DangerZone({ user }: { user: UserDetail }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const feedback = useActionFeedback<DangerNotice>();
  const activate = useActivateUser();
  const deactivate = useDeactivateUser();
  const revoke = useRevokeUserSessions();
  const [confirming, setConfirming] = useState<Confirming>(null);
  const name = `${user.firstName} ${user.lastName}`.trim();

  return (
    <Section
      title={t("detail.danger.title")}
      description={t("detail.danger.description")}
      testId="user-danger"
      tone="danger"
    >
      <Flex vertical gap={token.margin}>
        <FeedbackAlerts
          feedback={feedback}
          noticeKey={(notice) => `detail.danger.notice.${notice}`}
        />
        {user.isActive ? (
          <ActionRow
            title={t("detail.danger.deactivate.title")}
            description={t("detail.danger.deactivate.description")}
          >
            <Button
              danger
              loading={deactivate.isPending}
              onClick={() => {
                feedback.clear();
                setConfirming("deactivate");
              }}
            >
              {t("detail.danger.deactivate.action")}
            </Button>
          </ActionRow>
        ) : (
          <ActionRow
            title={t("detail.danger.activate.title")}
            description={t("detail.danger.activate.description")}
          >
            <Button
              type="primary"
              loading={activate.isPending}
              onClick={() => {
                feedback.clear();
                activate.mutate(user.id, {
                  onSuccess: () => {
                    feedback.succeed("activated");
                  },
                  onError: feedback.fail,
                });
              }}
            >
              {t("detail.danger.activate.action")}
            </Button>
          </ActionRow>
        )}
        <ActionRow
          title={t("detail.danger.revoke.title")}
          description={t("detail.danger.revoke.description")}
        >
          <Button
            danger
            loading={revoke.isPending}
            onClick={() => {
              feedback.clear();
              setConfirming("revoke");
            }}
          >
            {t("detail.danger.revoke.action")}
          </Button>
        </ActionRow>
      </Flex>
      <ConfirmDialog
        open={confirming === "deactivate"}
        title={t("detail.danger.deactivate.dialogTitle", { name })}
        description={
          <Typography.Text>{t("detail.danger.deactivate.dialogDescription")}</Typography.Text>
        }
        confirmLabel={t("detail.danger.deactivate.confirm")}
        danger
        loading={deactivate.isPending}
        onCancel={() => {
          setConfirming(null);
        }}
        onConfirm={() => {
          deactivate.mutate(user.id, {
            onSuccess: () => {
              setConfirming(null);
              feedback.succeed("deactivated");
            },
            onError: (error) => {
              setConfirming(null);
              feedback.fail(error);
            },
          });
        }}
      />
      <ConfirmDialog
        open={confirming === "revoke"}
        title={t("detail.danger.revoke.dialogTitle", { name })}
        description={
          <Typography.Text>{t("detail.danger.revoke.dialogDescription")}</Typography.Text>
        }
        confirmLabel={t("detail.danger.revoke.confirm")}
        danger
        loading={revoke.isPending}
        onCancel={() => {
          setConfirming(null);
        }}
        onConfirm={() => {
          revoke.mutate(user.id, {
            onSuccess: () => {
              setConfirming(null);
              feedback.succeed("sessionsRevoked");
            },
            onError: (error) => {
              setConfirming(null);
              feedback.fail(error);
            },
          });
        }}
      />
    </Section>
  );
}
