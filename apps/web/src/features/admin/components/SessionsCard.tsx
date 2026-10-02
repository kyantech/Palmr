import { Flex, Skeleton, Tag, theme, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { useUserSessions } from "../api/queries";
import type { UserSession } from "../types";
import { Section } from "./Section";

function SessionRow({ session }: { session: UserSession }) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  return (
    <li
      data-testid="user-session-row"
      style={{
        listStyle: "none",
        paddingBlock: token.paddingSM,
        borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
      }}
    >
      <Flex vertical gap={2}>
        <Flex align="center" gap={token.marginXS} wrap>
          <Typography.Text
            strong
            ellipsis
            style={{ maxWidth: 360 }}
            {...(session.userAgent === null ? {} : { title: session.userAgent })}
          >
            {session.userAgent ?? t("detail.sessions.unknownDevice")}
          </Typography.Text>
          <Tag variant="filled" style={{ marginInlineEnd: 0 }}>
            {t(`detail.sessions.origin.${session.origin}`)}
          </Tag>
        </Flex>
        <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
          {session.ipAddress === null
            ? t("detail.sessions.ipUnavailable")
            : t("detail.sessions.ip", { ip: session.ipAddress })}
          {" · "}
          {t("detail.sessions.lastActive", {
            date: formatDateTime(session.lastSeenAt, i18n.language),
          })}
          {" · "}
          {t("detail.sessions.signedIn", {
            date: formatDateTime(session.createdAt, i18n.language),
          })}
        </Typography.Text>
      </Flex>
    </li>
  );
}

export function SessionsCard({ userId }: { userId: string }) {
  const { t } = useTranslation("admin");
  const sessions = useUserSessions(userId);
  const items = sessions.data?.items ?? [];
  const total = sessions.data?.totalCount ?? items.length;
  return (
    <Section
      title={t("detail.sessions.title")}
      description={t("detail.sessions.description")}
      testId="user-sessions"
    >
      {sessions.isPending ? (
        <Skeleton active paragraph={{ rows: 2 }} />
      ) : sessions.isError ? (
        <ErrorAlert error={sessions.error} />
      ) : items.length === 0 ? (
        <Typography.Text type="secondary">{t("detail.sessions.none")}</Typography.Text>
      ) : (
        <>
          <ul aria-label={t("detail.sessions.listLabel")} style={{ margin: 0, padding: 0 }}>
            {items.map((session) => (
              <SessionRow key={session.id} session={session} />
            ))}
          </ul>
          {total > items.length ? (
            <Typography.Text type="secondary">
              {t("detail.sessions.more", { more: total - items.length })}
            </Typography.Text>
          ) : null}
        </>
      )}
    </Section>
  );
}
