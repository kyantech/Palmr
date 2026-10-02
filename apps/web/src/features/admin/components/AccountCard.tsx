import { Button, Flex, Select, Tag, theme, Typography } from "antd";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "../../../shared/format/dateTime";
import { useChangeRole, useResetUserPassword, useUnlockUser } from "../api/mutations";
import { USER_ROLES, type IdentityLink, type UserDetail, type UserRole } from "../types";
import { FeedbackAlerts, useActionFeedback } from "./ActionFeedback";
import { ConfirmDialog } from "./ConfirmDialog";
import { FactList } from "./FactList";
import { OneTimeSecretModal } from "./OneTimeSecretModal";
import { Section } from "./Section";
import { ActiveTag, RoleTag } from "./UserTags";

type AccountNotice = "roleChanged" | "unlocked";

function isUserRole(value: string): value is UserRole {
  return (USER_ROLES as readonly string[]).includes(value);
}

function IdentityLinks({ links }: { links: readonly IdentityLink[] }) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  if (links.length === 0) {
    return <Typography.Text type="secondary">{t("common.none")}</Typography.Text>;
  }
  return (
    <Flex vertical gap={token.marginXXS}>
      {links.map((link) => (
        <Flex key={link.id} gap={token.marginXS} align="center" wrap data-testid="identity-link">
          <Typography.Text>{link.providerName}</Typography.Text>
          <Tag
            color={link.state === "active" ? "success" : "default"}
            variant="filled"
            style={{ marginInlineEnd: 0 }}
          >
            {t(`detail.account.linkState.${link.state}`)}
          </Tag>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t(`detail.account.linkMethod.${link.linkMethod}`)}
            {link.lastLoginAt === null
              ? ""
              : ` · ${t("detail.account.linkLastLogin", {
                  date: formatDateTime(link.lastLoginAt, i18n.language),
                })}`}
          </Typography.Text>
        </Flex>
      ))}
    </Flex>
  );
}

export function AccountCard({ user }: { user: UserDetail }) {
  const { t, i18n } = useTranslation("admin");
  const { token } = theme.useToken();
  const feedback = useActionFeedback<AccountNotice>();
  const changeRole = useChangeRole();
  const unlock = useUnlockUser();
  const [temporaryPassword, setTemporaryPassword] = useState<string | null>(null);
  const reset = useResetUserPassword(setTemporaryPassword);
  const [pendingRole, setPendingRole] = useState<UserRole>(
    isUserRole(user.role) ? user.role : "user",
  );
  const [confirmingRole, setConfirmingRole] = useState(false);
  const [confirmingReset, setConfirmingReset] = useState(false);
  const name = `${user.firstName} ${user.lastName}`.trim();
  const roleChanged = pendingRole !== user.role;

  const lock = user.lockout;
  const lockedUntil =
    lock.lockedUntil === null ? null : formatDateTime(lock.lockedUntil, i18n.language);

  const facts = [
    {
      key: "role",
      label: t("detail.account.role"),
      value: (
        <Flex gap={token.marginXS} align="center" wrap>
          <RoleTag role={user.role} />
          <Select<UserRole>
            aria-label={t("detail.account.roleSelect")}
            size="small"
            value={pendingRole}
            style={{ width: 130 }}
            options={USER_ROLES.map((role) => ({ value: role, label: t(`roles.${role}`) }))}
            onChange={setPendingRole}
          />
          <Button
            size="small"
            disabled={!roleChanged}
            loading={changeRole.isPending}
            onClick={() => {
              setConfirmingRole(true);
            }}
          >
            {t("detail.account.roleSave")}
          </Button>
        </Flex>
      ),
    },
    {
      key: "status",
      label: t("detail.account.status"),
      value: <ActiveTag active={user.isActive} />,
    },
    {
      key: "password",
      label: t("detail.account.localPassword"),
      value: t(
        user.hasLocalPassword ? "detail.account.passwordLocal" : "detail.account.passwordSso",
      ),
    },
    {
      key: "mustChange",
      label: t("detail.account.mustChange"),
      value: t(user.mustChangePassword ? "common.yes" : "common.no"),
    },
    {
      key: "twoFactor",
      label: t("detail.account.twoFactor"),
      value: t(
        user.twoFactorEnabled ? "detail.account.twoFactorOn" : "detail.account.twoFactorOff",
      ),
    },
    {
      key: "lockout",
      label: t("detail.account.lockout"),
      value: (
        <Flex vertical gap={2} data-testid="lockout">
          <Flex gap={token.marginXS} align="center" wrap>
            {user.isLockedOut ? (
              <Tag color="warning" variant="filled" style={{ marginInlineEnd: 0 }}>
                {lockedUntil === null
                  ? t("detail.account.locked")
                  : t("detail.account.lockedUntil", { date: lockedUntil })}
              </Tag>
            ) : (
              <Typography.Text>{t("detail.account.notLocked")}</Typography.Text>
            )}
            {user.isLockedOut ? (
              <Button
                size="small"
                loading={unlock.isPending}
                onClick={() => {
                  feedback.clear();
                  unlock.mutate(user.id, {
                    onSuccess: () => {
                      feedback.succeed("unlocked");
                    },
                    onError: feedback.fail,
                  });
                }}
              >
                {t("detail.account.unlock")}
              </Button>
            ) : null}
          </Flex>
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t("detail.account.lockoutCounters", {
              failed: lock.failedCount,
              locks: lock.lockCount,
            })}
          </Typography.Text>
        </Flex>
      ),
    },
    { key: "sessions", label: t("detail.account.sessions"), value: String(user.sessionCount) },
    {
      key: "trusted",
      label: t("detail.account.trustedDevices"),
      value: String(user.trustedDeviceCount),
    },
    {
      key: "links",
      label: t("detail.account.identityLinks", { links: user.identityLinkCount }),
      value: <IdentityLinks links={user.identityLinks} />,
    },
  ];

  return (
    <Section
      title={t("detail.account.title")}
      description={t("detail.account.description")}
      testId="user-account"
      extra={
        user.hasLocalPassword ? (
          <Button
            loading={reset.isPending}
            onClick={() => {
              feedback.clear();
              setConfirmingReset(true);
            }}
          >
            {t("detail.account.resetPassword")}
          </Button>
        ) : undefined
      }
    >
      <Flex vertical gap={token.margin}>
        <FeedbackAlerts
          feedback={feedback}
          noticeKey={(notice) => `detail.account.notice.${notice}`}
        />
        <FactList facts={facts} />
        {user.hasLocalPassword ? null : (
          <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
            {t("detail.account.noLocalPasswordHint")}
          </Typography.Text>
        )}
      </Flex>
      <ConfirmDialog
        open={confirmingRole}
        title={t("detail.account.roleDialog.title", { name })}
        description={
          <Typography.Text>
            {t("detail.account.roleDialog.description", { role: t(`roles.${pendingRole}`) })}
          </Typography.Text>
        }
        confirmLabel={t("detail.account.roleDialog.confirm")}
        loading={changeRole.isPending}
        onCancel={() => {
          setConfirmingRole(false);
        }}
        onConfirm={() => {
          feedback.clear();
          changeRole.mutate(
            { userId: user.id, role: pendingRole },
            {
              onSuccess: () => {
                setConfirmingRole(false);
                feedback.succeed("roleChanged");
              },
              onError: (error) => {
                setConfirmingRole(false);
                feedback.fail(error);
              },
            },
          );
        }}
      />
      <ConfirmDialog
        open={confirmingReset}
        title={t("detail.account.resetDialog.title", { name })}
        description={
          <Typography.Text>{t("detail.account.resetDialog.description")}</Typography.Text>
        }
        confirmLabel={t("detail.account.resetDialog.confirm")}
        danger
        loading={reset.isPending}
        onCancel={() => {
          setConfirmingReset(false);
        }}
        onConfirm={() => {
          reset.mutate(user.id, {
            onSuccess: () => {
              setConfirmingReset(false);
            },
            onError: (error) => {
              setConfirmingReset(false);
              feedback.fail(error);
            },
          });
        }}
      />
      <OneTimeSecretModal
        secret={temporaryPassword}
        title={t("detail.account.resetResult.title")}
        description={t("detail.account.resetResult.description", { name })}
        fieldLabel={t("detail.account.resetResult.label")}
        testId="temporary-password-result"
        onClose={() => {
          setTemporaryPassword(null);
        }}
      />
    </Section>
  );
}
