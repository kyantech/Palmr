import { Alert, Button, Card, Flex, Segmented, Skeleton, theme } from "antd";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useLocation, useSearchParams } from "react-router";
import { ApiError, ErrorAlert } from "../../../shared/errors";
import {
  type AdminView,
  invitesSearch,
  parseInvitesParams,
  parseUsersParams,
  parseView,
  trailOf,
  type UsersListParams,
  usersSearch,
} from "../api/params";
import { useSettings, useUsers } from "../api/queries";
import { CreateInviteModal } from "../components/CreateInviteModal";
import { CreateUserModal } from "../components/CreateUserModal";
import { CursorPager } from "../components/CursorPager";
import { InvitesPanel } from "../components/InvitesPanel";
import { OneTimeSecretModal } from "../components/OneTimeSecretModal";
import { PageHeader } from "../components/PageHeader";
import { fullName, userPath, UsersTable } from "../components/UsersTable";
import { UsersToolbar } from "../components/UsersToolbar";
import type { CreatedInvite, InviteStatus, UserRow } from "../types";

interface UsersPageProps {
  locales: readonly string[];
}

type RowNotice = "activated" | "deactivated" | "unlocked";

export function UsersPage({ locales }: UsersPageProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const [search, setSearch] = useSearchParams();
  const view = parseView(search);
  const [creatingUser, setCreatingUser] = useState(false);
  const [creatingInvite, setCreatingInvite] = useState(false);
  const [createdUser, setCreatedUser] = useState<UserRow | null>(null);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const smtp = useSettings("smtp");

  const selectView = (next: AdminView) => {
    setSearch(new URLSearchParams({ view: next }), { state: { trail: [] } });
  };

  const onInviteCreated = (invite: CreatedInvite) => {
    setInviteUrl(invite.inviteUrl);
  };

  return (
    <section data-testid="admin-users-page">
      <PageHeader title={t("users.title")} description={t("users.description")} />
      <Flex
        justify="space-between"
        align="center"
        gap={token.marginSM}
        wrap
        style={{ marginBottom: token.margin }}
      >
        <Segmented<AdminView>
          aria-label={t("users.views.label")}
          value={view}
          options={[
            { value: "users", label: t("users.views.users") },
            { value: "invites", label: t("users.views.invites") },
          ]}
          onChange={selectView}
        />
        {view === "users" ? (
          <Button
            type="primary"
            onClick={() => {
              setCreatedUser(null);
              setCreatingUser(true);
            }}
          >
            {t("users.create.open")}
          </Button>
        ) : (
          <Button
            type="primary"
            onClick={() => {
              setCreatingInvite(true);
            }}
          >
            {t("invites.create.open")}
          </Button>
        )}
      </Flex>
      {createdUser === null ? null : (
        <Alert
          type="success"
          showIcon
          role="status"
          closable={{
            onClose: () => {
              setCreatedUser(null);
            },
          }}
          style={{ marginBottom: token.margin }}
          title={t("users.create.created", { name: fullName(createdUser) })}
          action={<Link to={userPath(createdUser.id)}>{t("users.create.view")}</Link>}
        />
      )}
      <Card variant="outlined" styles={{ body: { padding: token.paddingMD } }}>
        {view === "users" ? <UsersView /> : <InvitesView />}
      </Card>
      <CreateUserModal
        open={creatingUser}
        locales={locales}
        onClose={() => {
          setCreatingUser(false);
        }}
        onCreated={setCreatedUser}
      />
      <CreateInviteModal
        open={creatingInvite}
        emailEnabled={smtp.data?.enabled === true}
        onClose={() => {
          setCreatingInvite(false);
        }}
        onCreated={onInviteCreated}
      />
      <OneTimeSecretModal
        secret={inviteUrl}
        title={t("invites.link.title")}
        description={t("invites.link.description")}
        fieldLabel={t("invites.link.label")}
        testId="invite-link-result"
        onClose={() => {
          setInviteUrl(null);
        }}
      />
    </section>
  );
}

function UsersView() {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const [search, setSearch] = useSearchParams();
  const location = useLocation();
  const params = useMemo(() => parseUsersParams(search), [search]);
  const trail = trailOf(location.state);
  const users = useUsers(params);
  const [failure, setFailure] = useState<unknown>(null);
  const [notice, setNotice] = useState<RowNotice | null>(null);

  const go = (next: UsersListParams, nextTrail: readonly (string | null)[], replace = false) => {
    setSearch(usersSearch(next), { replace, state: { trail: nextTrail } });
  };
  const restart = (change: Partial<UsersListParams>, replace = false) => {
    go({ ...params, ...change, cursor: null }, [], replace);
  };

  const filtered = params.q !== "" || params.role !== null || params.status !== null;
  const nextCursor = users.data?.nextCursor ?? null;
  const items = users.data?.items ?? [];
  const cursorInvalid = users.error instanceof ApiError && users.error.code === "CURSOR_INVALID";

  return (
    <Flex vertical gap={token.margin}>
      <UsersToolbar
        params={params}
        onSearch={(q) => {
          restart({ q }, true);
        }}
        onChange={(change) => {
          restart(change);
        }}
      />
      {failure === null ? null : <ErrorAlert error={failure} />}
      {notice === null ? null : (
        <Alert type="success" showIcon role="status" title={t(`users.notice.${notice}`)} />
      )}
      {users.isError && users.data === undefined ? (
        <Flex vertical gap={token.marginXS} align="flex-start">
          <ErrorAlert error={users.error} />
          {cursorInvalid ? (
            <Button
              onClick={() => {
                restart({});
              }}
            >
              {t("pager.first")}
            </Button>
          ) : null}
        </Flex>
      ) : users.isPending ? (
        <Skeleton active paragraph={{ rows: 6 }} />
      ) : (
        <UsersTable
          users={items}
          loading={users.isFetching && users.isPlaceholderData}
          filtered={filtered}
          onClearFilters={() => {
            restart({ q: "", role: null, status: null });
          }}
          onFailure={setFailure}
          onNotice={(next) => {
            setFailure(null);
            setNotice(next);
          }}
        />
      )}
      {users.data === undefined ? null : (
        <CursorPager
          shown={items.length}
          totalCount={users.data.totalCount}
          limit={params.limit}
          atFirstPage={params.cursor === null}
          hasPrevious={trail.length > 0}
          nextCursor={nextCursor}
          loading={users.isFetching}
          onFirst={() => {
            go({ ...params, cursor: null }, []);
          }}
          onPrevious={() => {
            go({ ...params, cursor: trail.at(-1) ?? null }, trail.slice(0, -1));
          }}
          onNext={() => {
            if (nextCursor !== null) {
              go({ ...params, cursor: nextCursor }, [...trail, params.cursor]);
            }
          }}
          onLimit={(limit) => {
            restart({ limit });
          }}
        />
      )}
    </Flex>
  );
}

function InvitesView() {
  const [search, setSearch] = useSearchParams();
  const location = useLocation();
  const params = useMemo(() => parseInvitesParams(search), [search]);
  const trail = trailOf(location.state);

  const go = (next: typeof params, nextTrail: readonly (string | null)[]) => {
    setSearch(invitesSearch(next), { state: { trail: nextTrail } });
  };

  return (
    <InvitesPanel
      params={params}
      hasPrevious={trail.length > 0}
      onFilter={(status: InviteStatus | null) => {
        go({ ...params, status, cursor: null }, []);
      }}
      onLimit={(limit) => {
        go({ ...params, limit, cursor: null }, []);
      }}
      onNext={(cursor) => {
        go({ ...params, cursor }, [...trail, params.cursor]);
      }}
      onPrevious={() => {
        go({ ...params, cursor: trail.at(-1) ?? null }, trail.slice(0, -1));
      }}
      onFirst={() => {
        go({ ...params, cursor: null }, []);
      }}
    />
  );
}
