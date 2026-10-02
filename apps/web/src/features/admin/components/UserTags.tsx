import { Tag } from "antd";
import { useTranslation } from "react-i18next";

export function RoleTag({ role }: { role: string }) {
  const { t } = useTranslation("admin");
  return (
    <Tag
      color={role === "admin" ? "processing" : "default"}
      variant="filled"
      style={{ marginInlineEnd: 0 }}
      data-testid="role-tag"
    >
      {role === "admin" ? t("roles.admin") : t("roles.user")}
    </Tag>
  );
}

export function ActiveTag({ active }: { active: boolean }) {
  const { t } = useTranslation("admin");
  return (
    <Tag
      color={active ? "success" : "default"}
      variant="filled"
      style={{ marginInlineEnd: 0 }}
      data-testid="active-tag"
    >
      {t(active ? "status.active" : "status.inactive")}
    </Tag>
  );
}
