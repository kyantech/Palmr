import { Button, Flex, Skeleton } from "antd";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { useInviteLookup } from "../api/queries";
import { AuthStatePanel } from "../components/AuthStatePanel";
import { InviteAcceptForm } from "../components/InviteAcceptForm";

type DeadInvite = "notFound" | "expired" | "used" | "revoked";

const DEAD_INVITE: Readonly<Record<string, DeadInvite>> = {
  INVITE_NOT_FOUND: "notFound",
  INVITE_EXPIRED: "expired",
  INVITE_ALREADY_USED: "used",
  INVITE_REVOKED: "revoked",
};

function deadState(error: unknown): DeadInvite | null {
  const { code } = presentError(error);
  return code === null ? null : (DEAD_INVITE[code] ?? null);
}

export interface InvitePageProps {
  token: string;
  appName: string;
  supportedLocales: readonly string[];
  defaultLocale: string;
  onAccepted: () => Promise<void>;
  onBackToSignIn: () => void;
}

export function InvitePage({
  token,
  appName,
  supportedLocales,
  defaultLocale,
  onAccepted,
  onBackToSignIn,
}: InvitePageProps) {
  const { t } = useTranslation("auth");
  const instance = useId();
  const lookup = useInviteLookup(instance, token);
  const [dead, setDead] = useState<unknown>(null);
  const failure = dead ?? (lookup.isError ? lookup.error : null);
  const state = failure === null ? null : deadState(failure);
  const backToSignIn = (
    <Button type="text" block onClick={onBackToSignIn}>
      {t("invite.backToSignIn")}
    </Button>
  );

  if (state !== null) {
    return (
      <AuthStatePanel
        testId="invite-dead"
        title={t(`invite.dead.${state}`)}
        error={failure}
        actions={
          <Button type="primary" size="large" block onClick={onBackToSignIn}>
            {t("invite.backToSignIn")}
          </Button>
        }
      />
    );
  }

  const heading = (
    <AuthHeading title={t("invite.title")} description={t("invite.description", { appName })} />
  );

  if (lookup.isPending) {
    return (
      <div aria-busy="true" data-testid="invite-loading">
        {heading}
        <Skeleton active title={false} paragraph={{ rows: 6 }} />
      </div>
    );
  }

  if (lookup.isError) {
    return (
      <>
        {heading}
        <Flex vertical gap={12}>
          <ErrorAlert error={lookup.error} />
          <Button
            type="primary"
            block
            onClick={() => {
              void lookup.refetch();
            }}
          >
            {t("invite.retry")}
          </Button>
          {backToSignIn}
        </Flex>
      </>
    );
  }

  return (
    <div data-testid="invite-form">
      {heading}
      <InviteAcceptForm
        token={token}
        invite={lookup.data}
        supportedLocales={supportedLocales}
        defaultLocale={defaultLocale}
        onAccepted={onAccepted}
        onTerminal={(error) => {
          if (deadState(error) === null) {
            return false;
          }
          setDead(error);
          return true;
        }}
      />
    </div>
  );
}
