import { Avatar, Button, Dropdown, Flex, theme, Typography, type MenuProps } from "antd";
import ArrowLeftFromLine from "@gravity-ui/icons/ArrowLeftFromLine";
import ChevronDown from "@gravity-ui/icons/ChevronDown";
import { useTranslation } from "react-i18next";
import { resolveApiUrl } from "../../shared/api/basePath";
import type { Me } from "../bootstrap/queries";

export function userDisplayName(user: Me["user"]): string {
  const name = [user.firstName, user.lastName]
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .join(" ");
  return name.length > 0 ? name : user.username;
}

export function userInitials(user: Me["user"]): string {
  const initials = `${user.firstName.trim().slice(0, 1)}${user.lastName.trim().slice(0, 1)}`
    .trim()
    .toUpperCase();
  return initials.length > 0 ? initials : userDisplayName(user).slice(0, 1).toUpperCase();
}

export interface UserMenuProps {
  user: Me["user"];
  onSignOut: () => void;
  signingOut: boolean;
  compact?: boolean;
}

function UserAvatar({ user, size }: { user: Me["user"]; size: number }) {
  const { token } = theme.useToken();
  return (
    <Avatar
      size={size}
      alt=""
      style={{
        flex: "none",
        fontSize: size * 0.4,
        fontWeight: 600,
        color: token.colorWhite,
        background: `linear-gradient(135deg, ${token.colorPrimary}, ${token.colorPrimaryActive})`,
        boxShadow: `0 0 0 2px ${token.colorBgContainer}, 0 0 0 3px ${token.colorPrimaryBorder}`,
      }}
      {...(user.avatarUrl === null ? {} : { src: resolveApiUrl(user.avatarUrl) })}
    >
      {userInitials(user)}
    </Avatar>
  );
}

export function UserMenu({ user, onSignOut, signingOut, compact = false }: UserMenuProps) {
  const { t } = useTranslation("common");
  const { token } = theme.useToken();
  const name = userDisplayName(user);
  const items: MenuProps["items"] = [
    {
      key: "sign-out",
      label: t("user.menu.signOut"),
      icon: (
        <span className="anticon">
          <ArrowLeftFromLine aria-hidden="true" focusable="false" width="1em" height="1em" />
        </span>
      ),
      danger: true,
      disabled: signingOut,
    },
  ];
  const onClick: MenuProps["onClick"] = ({ key }) => {
    if (key === "sign-out") {
      onSignOut();
    }
  };
  return (
    <Dropdown
      menu={{ items, onClick, style: { boxShadow: "none", background: "transparent" } }}
      trigger={["click"]}
      placement="bottomRight"
      popupRender={(menu) => (
        <div
          style={{
            minWidth: 260,
            background: token.colorBgElevated,
            borderRadius: token.borderRadiusLG,
            boxShadow: token.boxShadowSecondary,
            overflow: "hidden",
          }}
        >
          <Flex
            align="center"
            gap={token.marginSM}
            style={{
              padding: token.padding,
              borderBottom: `${String(token.lineWidth)}px ${token.lineType} ${token.colorSplit}`,
            }}
          >
            <UserAvatar user={user} size={40} />
            <Flex vertical style={{ minWidth: 0 }}>
              <Typography.Text strong ellipsis>
                {name}
              </Typography.Text>
              <Typography.Text type="secondary" ellipsis style={{ fontSize: token.fontSizeSM }}>
                {user.email}
              </Typography.Text>
            </Flex>
          </Flex>
          <div style={{ padding: token.paddingXXS }}>{menu}</div>
        </div>
      )}
    >
      <Button
        type="text"
        aria-label={t("user.menu.label")}
        loading={signingOut}
        style={{
          height: 44,
          paddingInlineStart: token.paddingXXS,
          paddingInlineEnd: compact ? token.paddingXXS : token.paddingSM,
          borderRadius: 999,
        }}
      >
        <Flex align="center" gap={token.marginSM}>
          <UserAvatar user={user} size={32} />
          {compact ? null : (
            <Flex vertical align="flex-start" style={{ minWidth: 0, lineHeight: 1.25 }}>
              <Typography.Text strong style={{ maxWidth: 180 }} ellipsis>
                {name}
              </Typography.Text>
              <Typography.Text
                type="secondary"
                style={{ maxWidth: 180, fontSize: token.fontSizeSM }}
                ellipsis
              >
                {user.email}
              </Typography.Text>
            </Flex>
          )}
          {compact ? null : (
            <span style={{ display: "inline-flex", color: token.colorTextTertiary, fontSize: 16 }}>
              <ChevronDown aria-hidden="true" focusable="false" width="1em" height="1em" />
            </span>
          )}
        </Flex>
      </Button>
    </Dropdown>
  );
}
