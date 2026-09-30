import { Divider } from "antd";
import { useTranslation } from "react-i18next";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { SignOutAction } from "../components/SignOutAction";
import { TwoFactorSetup } from "../components/TwoFactorSetup";

export interface EnrollTwoFactorPageProps {
  appName: string;
  account: string;
  onEnrolled: () => Promise<void>;
  onSignOut: () => Promise<void>;
}

export function EnrollTwoFactorPage({
  appName,
  account,
  onEnrolled,
  onSignOut,
}: EnrollTwoFactorPageProps) {
  const { t } = useTranslation("auth");
  return (
    <div data-testid="mandatory-two-factor">
      <AuthHeading title={t("enroll.title")} description={t("enroll.description")} />
      <TwoFactorSetup
        appName={appName}
        finishLabel={t("enroll.continue")}
        onFinished={onEnrolled}
      />
      <Divider style={{ marginBlock: 20 }} />
      <SignOutAction account={account} onSignOut={onSignOut} />
    </div>
  );
}
