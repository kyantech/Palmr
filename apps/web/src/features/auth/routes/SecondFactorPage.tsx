import { Button, Divider } from "antd";
import { useTranslation } from "react-i18next";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { SecondFactorForm } from "../components/SecondFactorForm";
import { clearMfaChallenge, useMfaChallenge } from "../store";

export interface SecondFactorPageProps {
  onSignedIn: () => Promise<void>;
}

export function SecondFactorPage({ onSignedIn }: SecondFactorPageProps) {
  const { t } = useTranslation("auth");
  const challenge = useMfaChallenge();
  if (challenge === null) {
    return null;
  }
  return (
    <>
      <AuthHeading title={t("secondFactor.title")} description={t("secondFactor.description")} />
      <SecondFactorForm challenge={challenge} onSignedIn={onSignedIn} />
      <Divider style={{ marginBlock: 20 }} />
      <Button type="text" block onClick={clearMfaChallenge}>
        {t("secondFactor.cancel")}
      </Button>
    </>
  );
}
