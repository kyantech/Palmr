import { Alert, Divider } from "antd";
import { useTranslation } from "react-i18next";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useForcedPasswordChange } from "../api/mutations";
import { NewPasswordForm } from "../components/NewPasswordForm";
import { SignOutAction } from "../components/SignOutAction";

export interface ForcedPasswordChangePageProps {
  account: string;
  passwordMinLength: number | undefined;
  onChanged: () => Promise<void>;
  onSignOut: () => Promise<void>;
}

export function ForcedPasswordChangePage({
  account,
  passwordMinLength,
  onChanged,
  onSignOut,
}: ForcedPasswordChangePageProps) {
  const { t } = useTranslation("auth");
  const change = useForcedPasswordChange();
  return (
    <div data-testid="forced-password-change">
      <AuthHeading
        title={t("forcedPassword.title")}
        description={t("forcedPassword.description")}
      />
      <Alert
        type="warning"
        showIcon
        style={{ marginBottom: 24 }}
        title={t("forcedPassword.reasonTitle")}
        description={t("forcedPassword.reasonDescription")}
      />
      <NewPasswordForm
        minLength={passwordMinLength}
        submitLabel={t("forcedPassword.submit")}
        consequence={t("forcedPassword.consequence")}
        submit={async (newPassword) => {
          await change.mutateAsync({ newPassword });
          await onChanged();
        }}
      />
      <Divider style={{ marginBlock: 20 }} />
      <SignOutAction account={account} onSignOut={onSignOut} />
    </div>
  );
}
