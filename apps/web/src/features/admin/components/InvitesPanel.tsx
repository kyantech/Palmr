import { Alert, Button, Empty, Flex, Select, Table, Tag, theme, Typography } from "antd";
import type { ColumnsType } from "antd/es/table";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { useResendInvite, useRevokeInvite } from "../api/mutations";
import type { InvitesListParams } from "../api/params";
import { useInvites } from "../api/queries";
import { INVITE_STATUSES, type InviteItem, type InviteStatus } from "../types";
import { ConfirmDialog } from "./ConfirmDialog";
import { CursorPager } from "./CursorPager";
import { reportable } from "./feedback";
import { RoleTag } from "./UserTags";

const STATUS_COLORS: Readonly<Record<InviteStatus, string>> = {
  pending: "processing",
  accepted: "success",
  revoked: "default",
  expired: "warning",
};

interface InvitesPanelProps {
  params: InvitesListParams;
  hasPrevious: boolean;
  onFilter: (status: InviteStatus | null) => void;
  onLimit: (limit: number) => void;
  onNext: (cursor: string) => void;
  onPrevious: () => void;
  onFirst: () => void;
}

export function InvitesPanel({
  params,
  hasPrevious,
  onFilter,
  onLimit,
  onNext,
  onPrevious,
  onFirst,
}: InvitesPanelProps) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  const invites = useInvites(params);
  const resend = useResendInvite();
  const revoke = useRevokeInvite();
  const [failure, setFailure] = useState<unknown>(null);
  const [notice, setNotice] = useState<"resent" | "revoked" | null>(null);
  const [revoking, setRevoking] = useState<InviteItem | null>(null);

  const runResend = (invite: InviteItem) => {
    setFailure(null);
    setNotice(null);
    resend.mutate(invite.id, {
      onSuccess: () => {
        setNotice("resent");
      },
      onError: (error) => {
        setFailure(reportable(error));
      },
    });
  };

  const confirmRevoke = () => {
    if (revoking === null) {
      return;
    }
    setFailure(null);
    setNotice(null);
    revoke.mutate(revoking.id, {
      onSuccess: () => {
        setNotice("revoked");
        setRevoking(null);
      },
      onError: (error) => {
        setFailure(reportable(error));
        setRevoking(null);
      },
    });
  };

  const columns: ColumnsType<InviteItem> = [
    {
      key: "email",
      title: t("invites.columns.email"),
      render: (_value, invite) => (
        <Typography.Text>{invite.email ?? t("common.none")}</Typography.Text>
      ),
    },
    {
      key: "role",
      title: t("invites.columns.role"),
      render: (_value, invite) => <RoleTag role={invite.role} />,
    },
    {
      key: "status",
      title: t("invites.columns.status"),
      render: (_value, invite) => (
        <Tag
          color={STATUS_COLORS[invite.status]}
          variant="filled"
          style={{ marginInlineEnd: 0 }}
          data-testid="invite-status"
        >
          {t(`invites.status.${invite.status}`)}
        </Tag>
      ),
    },
    {
      key: "createdBy",
      title: t("invites.columns.createdBy"),
      render: (_value, invite) => <Typography.Text>{invite.createdBy.username}</Typography.Text>,
    },
    {
      key: "created",
      title: t("invites.columns.created"),
      render: (_value, invite) => formatDateTime(invite.createdAt, i18n.language),
    },
    {
      key: "expires",
      title: t("invites.columns.expires"),
      render: (_value, invite) => formatDateTime(invite.expiresAt, i18n.language),
    },
    {
      key: "lastSent",
      title: t("invites.columns.lastSent"),
      render: (_value, invite) =>
        invite.lastSentAt === null ? (
          <Typography.Text type="secondary">{t("invites.neverSent")}</Typography.Text>
        ) : (
          formatDateTime(invite.lastSentAt, i18n.language)
        ),
    },
    {
      key: "actions",
      title: t("invites.columns.actions"),
      align: "end",
      render: (_value, invite) =>
        invite.status === "pending" ? (
          <Flex gap={token.marginXS} justify="end">
            <Button
              size="small"
              loading={resend.isPending && resend.variables === invite.id}
              onClick={() => {
                runResend(invite);
              }}
            >
              {t("invites.actions.resend")}
            </Button>
            <Button
              size="small"
              danger
              onClick={() => {
                setRevoking(invite);
              }}
            >
              {t("invites.actions.revoke")}
            </Button>
          </Flex>
        ) : null,
    },
  ];

  const items = invites.data?.items ?? [];
  const nextCursor = invites.data?.nextCursor ?? null;

  return (
    <Flex vertical gap={token.margin}>
      <Flex gap={token.marginXS} wrap align="center">
        <Select<InviteStatus | "all">
          aria-label={t("invites.filters.status")}
          value={params.status ?? "all"}
          style={{ width: 180 }}
          options={[
            { value: "all", label: t("invites.filters.allStatuses") },
            ...INVITE_STATUSES.map((status) => ({
              value: status,
              label: t(`invites.status.${status}`),
            })),
          ]}
          onChange={(status) => {
            onFilter(status === "all" ? null : status);
          }}
        />
      </Flex>
      {failure === null ? null : <ErrorAlert error={failure} />}
      {notice === null ? null : (
        <Alert type="success" showIcon role="status" title={t(`invites.notice.${notice}`)} />
      )}
      {invites.isError && invites.data === undefined ? (
        <ErrorAlert error={invites.error} />
      ) : (
        <Table<InviteItem>
          rowKey="id"
          size="middle"
          columns={columns}
          dataSource={[...items]}
          loading={invites.isPending || (invites.isFetching && invites.isPlaceholderData)}
          pagination={false}
          scroll={{ x: 900 }}
          aria-label={t("invites.tableLabel")}
          locale={{
            emptyText: (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description={t(
                  params.status === null ? "invites.empty.none" : "invites.empty.filtered",
                )}
              />
            ),
          }}
        />
      )}
      {invites.data === undefined ? null : (
        <CursorPager
          shown={items.length}
          totalCount={invites.data.totalCount}
          limit={params.limit}
          atFirstPage={params.cursor === null}
          hasPrevious={hasPrevious}
          nextCursor={nextCursor}
          loading={invites.isFetching}
          onPrevious={onPrevious}
          onFirst={onFirst}
          onNext={() => {
            if (nextCursor !== null) {
              onNext(nextCursor);
            }
          }}
          onLimit={onLimit}
        />
      )}
      <ConfirmDialog
        open={revoking !== null}
        title={t("invites.revoke.title")}
        description={
          <Typography.Text>
            {t("invites.revoke.description", { email: revoking?.email ?? t("common.none") })}
          </Typography.Text>
        }
        confirmLabel={t("invites.revoke.confirm")}
        danger
        loading={revoke.isPending}
        onCancel={() => {
          setRevoking(null);
        }}
        onConfirm={confirmRevoke}
      />
    </Flex>
  );
}
