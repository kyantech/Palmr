import { Flex, Input, Select, theme } from "antd";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { UsersListParams } from "../api/params";
import {
  USER_ROLES,
  USER_SORTS,
  USER_STATUSES,
  type UserRole,
  type UserSort,
  type UserStatus,
} from "../types";

const SEARCH_DEBOUNCE_MS = 300;

const SORT_KEYS: Readonly<Record<UserSort, string>> = {
  "createdAt:desc": "newest",
  "createdAt:asc": "oldest",
  "username:asc": "usernameAsc",
  "username:desc": "usernameDesc",
  "email:asc": "emailAsc",
  "email:desc": "emailDesc",
  "usedBytes:desc": "storageDesc",
  "usedBytes:asc": "storageAsc",
};

interface UsersToolbarProps {
  params: UsersListParams;
  onSearch: (q: string) => void;
  onChange: (change: Partial<Pick<UsersListParams, "role" | "status" | "sort" | "limit">>) => void;
}

export function UsersToolbar({ params, onSearch, onChange }: UsersToolbarProps) {
  const { t } = useTranslation("admin");
  const { token } = theme.useToken();
  const [draft, setDraft] = useState(params.q);
  const [seenQuery, setSeenQuery] = useState(params.q);
  if (params.q !== seenQuery) {
    setSeenQuery(params.q);
    if (draft.trim() !== params.q) {
      setDraft(params.q);
    }
  }

  useEffect(() => {
    const next = draft.trim();
    if (next === params.q) {
      return;
    }
    const timer = window.setTimeout(() => {
      onSearch(next);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, [draft, params.q, onSearch]);

  return (
    <Flex
      gap={token.marginXS}
      wrap
      align="center"
      role="search"
      aria-label={t("users.filters.label")}
    >
      <Input.Search
        allowClear
        value={draft}
        placeholder={t("users.filters.searchPlaceholder")}
        aria-label={t("users.filters.search")}
        style={{ width: 280, maxWidth: "100%" }}
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onSearch={(value) => {
          setDraft(value);
          onSearch(value.trim());
        }}
      />
      <Select<UserRole | "all">
        aria-label={t("users.filters.role")}
        value={params.role ?? "all"}
        style={{ width: 150 }}
        options={[
          { value: "all", label: t("users.filters.allRoles") },
          ...USER_ROLES.map((role) => ({ value: role, label: t(`roles.${role}`) })),
        ]}
        onChange={(role) => {
          onChange({ role: role === "all" ? null : role });
        }}
      />
      <Select<UserStatus | "all">
        aria-label={t("users.filters.status")}
        value={params.status ?? "all"}
        style={{ width: 160 }}
        options={[
          { value: "all", label: t("users.filters.allStatuses") },
          ...USER_STATUSES.map((status) => ({ value: status, label: t(`status.${status}`) })),
        ]}
        onChange={(status) => {
          onChange({ status: status === "all" ? null : status });
        }}
      />
      <Select<UserSort>
        aria-label={t("users.filters.sort")}
        value={params.sort}
        style={{ width: 200 }}
        options={USER_SORTS.map((sort) => ({
          value: sort,
          label: t(`users.sorts.${SORT_KEYS[sort]}`),
        }))}
        onChange={(sort) => {
          onChange({ sort });
        }}
      />
    </Flex>
  );
}
