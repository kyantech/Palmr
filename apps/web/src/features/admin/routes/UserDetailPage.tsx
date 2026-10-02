import { Alert, Flex, Grid, Skeleton, Tag, theme } from "antd";
import { useTranslation } from "react-i18next";
import { Link, useParams } from "react-router";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import { useUser } from "../api/queries";
import { AccountCard } from "../components/AccountCard";
import { DangerZone } from "../components/DangerZone";
import { EmailCard } from "../components/EmailCard";
import { IdentityCard } from "../components/IdentityCard";
import { PageHeader } from "../components/PageHeader";
import { SessionsCard } from "../components/SessionsCard";
import { StorageCard } from "../components/StorageCard";
import { ActiveTag, RoleTag } from "../components/UserTags";

function BackLink() {
  const { t } = useTranslation("admin");
  return <Link to="/admin/users">{t("detail.back")}</Link>;
}

export function UserDetailPage() {
  const { userId = "" } = useParams();
  return <UserDetail key={userId} userId={userId} />;
}

function UserDetail({ userId }: { userId: string }) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const screens = Grid.useBreakpoint();
  const wide = screens.xl === true;
  const user = useUser(userId);

  if (user.isPending) {
    return (
      <section data-testid="admin-user-page">
        <PageHeader title={t("detail.loading")} back={<BackLink />} />
        <Skeleton active paragraph={{ rows: 8 }} />
      </section>
    );
  }
  if (user.isError) {
    const missing = user.error instanceof ApiError && user.error.code === "USER_NOT_FOUND";
    return (
      <section data-testid="admin-user-page">
        <PageHeader title={t(missing ? "detail.notFound" : "detail.failed")} back={<BackLink />} />
        <ErrorAlert error={user.error} />
      </section>
    );
  }

  const detail = user.data;
  const name = `${detail.firstName} ${detail.lastName}`.trim();

  return (
    <section data-testid="admin-user-page" data-user-id={detail.id}>
      <PageHeader
        title={name === "" ? detail.username : name}
        back={<BackLink />}
        description={t("detail.subtitle", { username: detail.username, email: detail.email })}
        extra={
          <Flex gap={token.marginXS} align="center" wrap>
            <RoleTag role={detail.role} />
            <ActiveTag active={detail.isActive} />
            {detail.isLockedOut ? (
              <Tag color="warning" variant="filled" style={{ marginInlineEnd: 0 }}>
                {t("tags.locked")}
              </Tag>
            ) : null}
            {detail.overQuota ? (
              <Tag color="warning" variant="filled" style={{ marginInlineEnd: 0 }}>
                {t("storage.overQuota")}
              </Tag>
            ) : null}
          </Flex>
        }
      />
      {detail.isActive ? null : (
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: token.margin }}
          title={t("detail.inactiveNotice")}
        />
      )}
      <Flex gap={token.margin} align="flex-start" vertical={!wide} style={{ width: "100%" }}>
        <Flex vertical gap={token.margin} style={{ flex: 1, minWidth: 0, width: "100%" }}>
          <IdentityCard user={detail} />
          <EmailCard user={detail} />
          <AccountCard user={detail} />
        </Flex>
        <Flex vertical gap={token.margin} style={{ flex: 1, minWidth: 0, width: "100%" }}>
          <StorageCard user={detail} />
          <SessionsCard userId={detail.id} />
          <DangerZone user={detail} />
        </Flex>
      </Flex>
    </section>
  );
}
