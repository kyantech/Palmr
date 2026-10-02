import { Alert, Button, Flex, Popconfirm, Skeleton, Tag, theme, Typography } from "antd";
import Display from "@gravity-ui/icons/Display";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { useRevokeOtherSessions, useRevokeSession } from "../api/mutations";
import { useSessions } from "../api/queries";
import type { SessionItem } from "../types";
import { SettingsSection } from "../../../shared/ui/SettingsSection";
import { summarizeUserAgent } from "./userAgent";

function earliest(first: string, second: string): string {
  return Date.parse(second) < Date.parse(first) ? second : first;
}

function useDeviceLabel(userAgent: string | null): string {
  const { t } = useTranslation("settings");
  const { browser, system } = summarizeUserAgent(userAgent);
  if (browser !== null && system !== null) {
    return t("sessions.device.browserOnSystem", { browser, system });
  }
  return browser ?? system ?? t("sessions.device.unknown");
}

interface SessionRowProps {
  session: SessionItem;
  revoking: boolean;
  onRevoke: (session: SessionItem) => void;
}

function SessionRow({ session, revoking, onRevoke }: SessionRowProps) {
  const { t, i18n } = useTranslation("settings");
  const { token } = theme.useToken();
  const device = useDeviceLabel(session.userAgent);
  const locale = i18n.language;
  const facts = [
    session.ipAddress === null
      ? t("sessions.ipUnavailable")
      : t("sessions.ip", { ip: session.ipAddress }),
    t("sessions.lastActive", { date: formatDateTime(session.lastSeenAt, locale) }),
    t("sessions.signedIn", { date: formatDateTime(session.createdAt, locale) }),
    t("sessions.expires", {
      date: formatDateTime(earliest(session.expiresAt, session.absoluteExpiresAt), locale),
    }),
  ];
  return (
    <li
      data-testid="session-row"
      data-session-id={session.id}
      data-current={session.isCurrent ? "true" : "false"}
      style={{
        listStyle: "none",
        paddingBlock: token.padding,
        borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
      }}
    >
      <Flex gap={token.margin} align="flex-start" wrap>
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            flex: "none",
            width: 36,
            height: 36,
            fontSize: 18,
            borderRadius: token.borderRadius,
            color: token.colorTextSecondary,
            background: token.colorFillTertiary,
          }}
        >
          <Display aria-hidden="true" focusable="false" width="1em" height="1em" />
        </span>
        <Flex vertical gap={token.marginXXS} style={{ flex: "1 1 240px", minWidth: 0 }}>
          <Flex align="center" gap={token.marginXS} wrap>
            <Typography.Text
              strong
              {...(session.userAgent === null ? {} : { title: session.userAgent })}
            >
              {device}
            </Typography.Text>
            {session.isCurrent ? (
              <Tag color="success" variant="filled" style={{ marginInlineEnd: 0 }}>
                {t("sessions.current")}
              </Tag>
            ) : null}
            <Tag variant="filled" style={{ marginInlineEnd: 0 }}>
              {t(`sessions.origin.${session.origin}`)}
            </Tag>
          </Flex>
          <Flex vertical gap={2}>
            {facts.map((fact) => (
              <Typography.Text key={fact} type="secondary" style={{ fontSize: token.fontSizeSM }}>
                {fact}
              </Typography.Text>
            ))}
          </Flex>
        </Flex>
        <Popconfirm
          title={t(session.isCurrent ? "sessions.signOutCurrent.title" : "sessions.revoke.title")}
          description={t(
            session.isCurrent
              ? "sessions.signOutCurrent.description"
              : "sessions.revoke.description",
          )}
          okText={t(
            session.isCurrent ? "sessions.signOutCurrent.confirm" : "sessions.revoke.confirm",
          )}
          cancelText={t("sessions.cancel")}
          okButtonProps={{ danger: true }}
          onConfirm={() => {
            onRevoke(session);
          }}
        >
          <Button
            danger={!session.isCurrent}
            loading={revoking}
            aria-label={t(
              session.isCurrent ? "sessions.signOutCurrent.label" : "sessions.revoke.label",
              { device },
            )}
          >
            {t(session.isCurrent ? "sessions.signOutCurrent.action" : "sessions.revoke.action")}
          </Button>
        </Popconfirm>
      </Flex>
    </li>
  );
}

interface SessionsListProps {
  onCurrentSessionEnded: () => Promise<void>;
}

export function SessionsList({ onCurrentSessionEnded }: SessionsListProps) {
  const { t } = useTranslation("settings");
  const { token } = theme.useToken();
  const sessions = useSessions();
  const revokeSession = useRevokeSession(onCurrentSessionEnded);
  const revokeOthers = useRevokeOtherSessions();
  const [failure, setFailure] = useState<unknown>(null);
  const [notice, setNotice] = useState<"revoked" | "othersRevoked" | null>(null);

  const items = sessions.data?.pages.flatMap((page) => page.items) ?? [];
  const ordered = [
    ...items.filter((item) => item.isCurrent),
    ...items.filter((item) => !item.isCurrent),
  ];
  const hasOthers = items.some((item) => !item.isCurrent) || sessions.hasNextPage;

  const reportFailure = (error: unknown) => {
    if (error instanceof ApiError && error.code === "AUTH_RECENT_AUTH_REQUIRED") {
      return;
    }
    setFailure(error);
  };

  const revoke = (session: SessionItem) => {
    setFailure(null);
    setNotice(null);
    revokeSession.mutate(
      { id: session.id, isCurrent: session.isCurrent },
      {
        onSuccess: () => {
          if (!session.isCurrent) {
            setNotice("revoked");
          }
        },
        onError: reportFailure,
      },
    );
  };

  const signOutOthers = () => {
    setFailure(null);
    setNotice(null);
    revokeOthers.mutate(undefined, {
      onSuccess: () => {
        setNotice("othersRevoked");
      },
      onError: reportFailure,
    });
  };

  return (
    <SettingsSection
      title={t("sessions.title")}
      description={t("sessions.description")}
      testId="settings-sessions"
      extra={
        <Popconfirm
          title={t("sessions.revokeOthers.title")}
          description={t("sessions.revokeOthers.description")}
          okText={t("sessions.revokeOthers.confirm")}
          cancelText={t("sessions.cancel")}
          okButtonProps={{ danger: true }}
          onConfirm={signOutOthers}
          disabled={!hasOthers}
        >
          <Button loading={revokeOthers.isPending} disabled={!hasOthers}>
            {t("sessions.revokeOthers.action")}
          </Button>
        </Popconfirm>
      }
    >
      <Flex vertical gap={token.margin}>
        {failure === null ? null : <ErrorAlert error={failure} />}
        {notice === null ? null : (
          <Alert type="success" showIcon role="status" title={t(`sessions.notice.${notice}`)} />
        )}
        {sessions.isPending ? (
          <Skeleton active paragraph={{ rows: 3 }} />
        ) : sessions.isError ? (
          <ErrorAlert error={sessions.error} />
        ) : (
          <ul aria-label={t("sessions.listLabel")} style={{ margin: 0, padding: 0 }}>
            {ordered.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                revoking={revokeSession.isPending && revokeSession.variables.id === session.id}
                onRevoke={revoke}
              />
            ))}
          </ul>
        )}
        {sessions.hasNextPage ? (
          <div>
            <Button
              loading={sessions.isFetchingNextPage}
              onClick={() => {
                void sessions.fetchNextPage();
              }}
            >
              {t("sessions.loadMore")}
            </Button>
          </div>
        ) : null}
      </Flex>
    </SettingsSection>
  );
}
