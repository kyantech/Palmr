import { Button, Dropdown, Empty, Flex, Table, Tag, theme, Typography } from "antd";
import type { ColumnsType } from "antd/es/table";
import Ellipsis from "@gravity-ui/icons/Ellipsis";
import { type CSSProperties, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router";
import { formatDateTime } from "../../../shared/format/dateTime";
import { useActivateUser, useDeactivateUser, useUnlockUser } from "../api/mutations";
import type { UserRow } from "../types";
import { reportable } from "./feedback";
import { ConfirmDialog } from "./ConfirmDialog";
import { isOverQuota, StorageUsage } from "./StorageUsage";
import { ActiveTag, RoleTag } from "./UserTags";

const VISUALLY_HIDDEN: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
};

export function fullName(user: Pick<UserRow, "firstName" | "lastName">): string {
  return `${user.firstName} ${user.lastName}`.trim();
}

export function userPath(userId: string): string {
  return `/admin/users/${userId}`;
}

function SecurityTags({ user }: { user: UserRow }) {
  const { t } = useTranslation("admin");
  const tags: { key: string; label: string; color?: string }[] = [];
  if (user.isLockedOut) {
    tags.push({ key: "locked", label: t("tags.locked"), color: "warning" });
  }
  if (user.mustChangePassword) {
    tags.push({ key: "mustChange", label: t("tags.mustChangePassword"), color: "gold" });
  }
  if (!user.hasLocalPassword) {
    tags.push({ key: "sso", label: t("tags.ssoOnly") });
  }
  if (user.twoFactorEnabled) {
    tags.push({ key: "2fa", label: t("tags.twoFactor"), color: "success" });
  }
  if (tags.length === 0) {
    return <Typography.Text type="secondary">{t("common.none")}</Typography.Text>;
  }
  return (
    <Flex gap={4} wrap>
      {tags.map((tag) => (
        <Tag
          key={tag.key}
          variant="filled"
          {...(tag.color === undefined ? {} : { color: tag.color })}
          style={{ marginInlineEnd: 0 }}
        >
          {tag.label}
        </Tag>
      ))}
    </Flex>
  );
}

type StartAction = (
  userId: string,
  options: { onSuccess: () => void; onError: (error: Error) => void },
) => void;

interface RowActionsProps {
  user: UserRow;
  onFailure: (error: unknown) => void;
  onNotice: (notice: "activated" | "deactivated" | "unlocked") => void;
}

function RowActions({ user, onFailure, onNotice }: RowActionsProps) {
  const { t } = useTranslation("admin");
  const navigate = useNavigate();
  const activate = useActivateUser();
  const deactivate = useDeactivateUser();
  const unlock = useUnlockUser();
  const [confirming, setConfirming] = useState(false);
  const name = fullName(user);

  const run = (
    start: StartAction,
    notice: "activated" | "deactivated" | "unlocked",
    done?: () => void,
  ) => {
    start(user.id, {
      onSuccess: () => {
        onNotice(notice);
        done?.();
      },
      onError: (error) => {
        onFailure(reportable(error));
        done?.();
      },
    });
  };

  return (
    <>
      <Dropdown
        trigger={["click"]}
        menu={{
          items: [
            { key: "view", label: t("users.actions.view") },
            ...(user.isActive
              ? [{ key: "deactivate", label: t("users.actions.deactivate"), danger: true }]
              : [{ key: "activate", label: t("users.actions.activate") }]),
            ...(user.isLockedOut ? [{ key: "unlock", label: t("users.actions.unlock") }] : []),
          ],
          onClick: ({ key }) => {
            if (key === "view") {
              void navigate(userPath(user.id));
            } else if (key === "deactivate") {
              setConfirming(true);
            } else if (key === "activate") {
              run(activate.mutate, "activated");
            } else if (key === "unlock") {
              run(unlock.mutate, "unlocked");
            }
          },
        }}
      >
        <Button
          type="text"
          icon={<Ellipsis aria-hidden="true" focusable="false" width="1em" height="1em" />}
          aria-label={t("users.actions.label", { name })}
          loading={activate.isPending || unlock.isPending}
        />
      </Dropdown>
      <ConfirmDialog
        open={confirming}
        title={t("users.deactivate.title", { name })}
        description={<Typography.Text>{t("users.deactivate.description")}</Typography.Text>}
        confirmLabel={t("users.deactivate.confirm")}
        danger
        loading={deactivate.isPending}
        onCancel={() => {
          setConfirming(false);
        }}
        onConfirm={() => {
          run(deactivate.mutate, "deactivated", () => {
            setConfirming(false);
          });
        }}
      />
    </>
  );
}

interface UsersTableProps {
  users: readonly UserRow[];
  loading: boolean;
  filtered: boolean;
  onClearFilters: () => void;
  onFailure: (error: unknown) => void;
  onNotice: (notice: "activated" | "deactivated" | "unlocked") => void;
}

export function UsersTable({
  users,
  loading,
  filtered,
  onClearFilters,
  onFailure,
  onNotice,
}: UsersTableProps) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  const locale = i18n.language;

  const columns: ColumnsType<UserRow> = [
    {
      key: "user",
      title: t("users.columns.user"),
      render: (_value, user) => (
        <Flex vertical gap={2} style={{ minWidth: 200 }}>
          <Link to={userPath(user.id)} style={{ fontWeight: 600 }} data-testid="user-link">
            {fullName(user) === "" ? user.username : fullName(user)}
          </Link>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t("users.usernameLine", { username: user.username })}
          </Typography.Text>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {user.email}
          </Typography.Text>
          {user.pendingEmail === null ? null : (
            <Typography.Text
              type="warning"
              style={{ fontSize: token.fontSizeSM }}
              data-testid="pending-email"
            >
              {t("users.pendingEmail", { email: user.pendingEmail })}
            </Typography.Text>
          )}
        </Flex>
      ),
    },
    {
      key: "role",
      title: t("users.columns.role"),
      render: (_value, user) => <RoleTag role={user.role} />,
    },
    {
      key: "status",
      title: t("users.columns.status"),
      render: (_value, user) => <ActiveTag active={user.isActive} />,
    },
    {
      key: "security",
      title: t("users.columns.security"),
      render: (_value, user) => <SecurityTags user={user} />,
    },
    {
      key: "storage",
      title: t("users.columns.storage"),
      render: (_value, user) => (
        <StorageUsage
          usedBytes={user.usedBytes}
          effectiveQuotaBytes={user.effectiveQuotaBytes}
          overQuota={isOverQuota(user.usedBytes, user.effectiveQuotaBytes)}
        />
      ),
    },
    {
      key: "lastLogin",
      title: t("users.columns.lastSignIn"),
      render: (_value, user) =>
        user.lastLoginAt === null ? (
          <Typography.Text type="secondary">{t("users.neverSignedIn")}</Typography.Text>
        ) : (
          <Typography.Text>{formatDateTime(user.lastLoginAt, locale)}</Typography.Text>
        ),
    },
    {
      key: "actions",
      title: <span style={VISUALLY_HIDDEN}>{t("users.columns.actions")}</span>,
      align: "end",
      width: 64,
      render: (_value, user) => (
        <RowActions user={user} onFailure={onFailure} onNotice={onNotice} />
      ),
    },
  ];

  return (
    <Table<UserRow>
      rowKey="id"
      size="middle"
      columns={columns}
      dataSource={[...users]}
      loading={loading}
      pagination={false}
      scroll={{ x: 960 }}
      aria-label={t("users.tableLabel")}
      locale={{
        emptyText: (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={t(filtered ? "users.empty.filtered" : "users.empty.none")}
          >
            {filtered ? <Button onClick={onClearFilters}>{t("users.empty.clear")}</Button> : null}
          </Empty>
        ),
      }}
    />
  );
}
