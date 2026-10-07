import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Input, Modal, theme, Typography } from "antd";
import { Suspense, useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import type { components } from "../../../shared/api/schema";
import {
  ErrorAlert,
  presentError,
  type ReportedError,
  ReportedErrorAlert,
  RequestId,
  useErrorMessage,
} from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useReauthenticate } from "../api/mutations";
import { compactCode, isTotpCode } from "./codeFormat";
import { OneTimeCodeInput } from "./OneTimeCodeInput";
import { useExternalReauth } from "./useExternalReauth";
import {
  discardRecentAuthChallenge,
  type RecentAuthChallenge,
  takeRecentAuthReplay,
  useRecentAuthChallenge,
} from "../store";

type Me = components["schemas"]["MeResponse"];

type RecentAuthMethod = "password" | "twoFactor" | "external";

function recentAuthMethodOf({ capabilities }: Pick<Me, "capabilities">): RecentAuthMethod {
  if (!capabilities.hasLocalPassword) {
    return "external";
  }
  return capabilities.twoFactorEnabled ? "twoFactor" : "password";
}

interface ChallengeValues {
  password: string;
  totpCode: string;
}

const CODE_ERRORS: readonly string[] = ["AUTH_2FA_INVALID", "TOTP_CODE_REPLAYED"];

interface RecentAuthModalProps {
  me: Me;
}

export function RecentAuthModal({ me }: RecentAuthModalProps) {
  const challenge = useRecentAuthChallenge();
  const [engaged, setEngaged] = useState(false);
  if (challenge !== null && !engaged) {
    setEngaged(true);
  }
  if (!engaged) {
    return null;
  }
  return (
    <Suspense fallback={null}>
      <RecentAuthDialog me={me} challenge={challenge} />
    </Suspense>
  );
}

interface RecentAuthDialogProps {
  me: Me;
  challenge: RecentAuthChallenge | null;
}

function RecentAuthDialog({ me, challenge }: RecentAuthDialogProps) {
  const { t } = useTranslation("auth");
  const cancel = () => {
    if (challenge !== null) {
      discardRecentAuthChallenge(challenge.id);
    }
  };
  return (
    <Modal
      open={challenge !== null}
      onCancel={cancel}
      title={t("recentAuth.title")}
      footer={null}
      width={420}
      centered
      mask={{ closable: false }}
      keyboard
      destroyOnHidden
      styles={{ body: { paddingBlockStart: 4 } }}
    >
      {challenge === null ? null : (
        <RecentAuthBody key={challenge.id} me={me} challenge={challenge} onCancel={cancel} />
      )}
    </Modal>
  );
}

interface RecentAuthBodyProps {
  me: Me;
  challenge: RecentAuthChallenge;
  onCancel: () => void;
}

function RecentAuthBody({ me, challenge, onCancel }: RecentAuthBodyProps) {
  const { t } = useTranslation("auth");
  const method = recentAuthMethodOf(me);
  return (
    <Flex vertical gap={16}>
      <Flex vertical gap={4}>
        <Typography.Text>{t(`recentAuth.descriptions.${method}`)}</Typography.Text>
        <Typography.Text type="secondary" style={{ fontSize: "0.8125rem" }}>
          {t("recentAuth.signedInAs", { account: me.user.email })}
        </Typography.Text>
      </Flex>
      {method === "external" ? (
        <ExternalChallenge challenge={challenge} onCancel={onCancel} />
      ) : (
        <CredentialChallenge
          challenge={challenge}
          requireCode={method === "twoFactor"}
          onCancel={onCancel}
        />
      )}
    </Flex>
  );
}

function ExternalFailure({ reported }: { reported: ReportedError }) {
  const { t } = useTranslation("auth");
  if (reported.code !== "AUTH_RECENT_AUTH_REQUIRED") {
    return <ReportedErrorAlert reported={reported} />;
  }
  return (
    <Alert
      type="warning"
      showIcon
      role="alert"
      data-testid="recent-auth-external-unconfirmed"
      title={t("recentAuth.external.unconfirmed")}
      {...(reported.requestId === null
        ? {}
        : { description: <RequestId requestId={reported.requestId} /> })}
    />
  );
}

function ExternalChallenge({
  challenge,
  onCancel,
}: {
  challenge: RecentAuthChallenge;
  onCancel: () => void;
}) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const { phase, notice, failure, start } = useExternalReauth(challenge);
  const busy = phase === "starting" || phase === "verifying";
  return (
    <Flex vertical gap={16} data-recent-auth-method="external">
      {failure?.kind === "api" ? <ErrorAlert error={failure.error} /> : null}
      {failure?.kind === "reported" ? <ExternalFailure reported={failure.reported} /> : null}
      {notice === null ? null : (
        <Alert
          type={notice === "popupBlocked" ? "warning" : "info"}
          showIcon
          role="status"
          data-testid="recent-auth-external-notice"
          data-notice={notice}
          title={t(`recentAuth.external.notice.${notice}.title`)}
          description={t(`recentAuth.external.notice.${notice}.description`)}
        />
      )}
      {phase === "waiting" || phase === "verifying" ? (
        <Alert
          type="info"
          showIcon
          role="status"
          data-testid="recent-auth-external-progress"
          title={t(`recentAuth.external.progress.${phase}`)}
        />
      ) : null}
      <Flex justify="end" gap={token.marginXS}>
        <Button onClick={onCancel}>{t("recentAuth.cancel")}</Button>
        {phase === "waiting" ? (
          <Button type="primary" onClick={start}>
            {t("recentAuth.external.tryAgain")}
          </Button>
        ) : (
          <Button type="primary" loading={busy} disabled={busy} onClick={start}>
            {t("recentAuth.external.continue")}
          </Button>
        )}
      </Flex>
    </Flex>
  );
}

function CredentialChallenge({
  challenge,
  requireCode,
  onCancel,
}: {
  challenge: RecentAuthChallenge;
  requireCode: boolean;
  onCancel: () => void;
}) {
  const { t } = useTranslation(["auth", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const passwordId = `${idPrefix}-password`;
  const codeId = `${idPrefix}-code`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [retryBlock, setRetryBlock] = useState<{ seconds: number } | null>(null);
  const reauthenticate = useReauthenticate();
  const schema = useMemo(
    () =>
      z.object({
        password: z.string().min(1, t("recentAuth.passwordRequired")),
        totpCode: requireCode
          ? z.string().refine(isTotpCode, {
              message: t("recentAuth.codeRequired"),
            })
          : z.string(),
      }),
    [t, requireCode],
  );
  const {
    control,
    handleSubmit,
    resetField,
    setError,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<ChallengeValues>({
    resolver: zodResolver(schema),
    defaultValues: { password: "", totpCode: "" },
  });
  const failureText = useErrorMessage(failure);

  useEffect(() => {
    setFocus("password");
  }, [setFocus]);

  useEffect(() => {
    if (retryBlock === null) {
      return;
    }
    const timer = window.setTimeout(() => {
      setRetryBlock(null);
    }, retryBlock.seconds * 1_000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [retryBlock]);

  async function confirm({ password, totpCode }: ChallengeValues) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      await reauthenticate.mutateAsync(
        requireCode ? { password, totpCode: compactCode(totpCode) } : { password },
      );
    } catch (error) {
      const presented = presentError(error);
      if (presented.presentation.silent) {
        return;
      }
      if (retryAfter(presented.retryAfterSeconds)) {
        setRetryBlock({ seconds: presented.retryAfterSeconds ?? 0 });
      }
      if (requireCode && presented.code !== null && CODE_ERRORS.includes(presented.code)) {
        resetField("totpCode");
        setError("totpCode", {
          type: "server",
          message: t(presented.presentation.i18nKey, { ns: "errors" }),
        });
        setFocus("totpCode");
        return;
      }
      setFailure(error);
      resetField("password");
      if (requireCode) {
        resetField("totpCode");
      }
      setFocus("password");
      return;
    } finally {
      inFlight.current = false;
    }
    const replay = takeRecentAuthReplay(challenge.id);
    if (replay !== null) {
      replay().catch(() => undefined);
    }
  }

  const presentedFailure = failure === null ? null : presentError(failure);
  const inlineFailure = presentedFailure?.presentation.surface === "inline";
  const passwordHelp = errors.password?.message ?? (inlineFailure ? failureText : undefined);

  return (
    <form
      noValidate
      onSubmit={(event) => {
        if (retryBlock === null) {
          void handleSubmit(confirm)(event);
        } else {
          event.preventDefault();
        }
      }}
    >
      <Flex vertical gap={16}>
        {failure !== null && !inlineFailure ? <ErrorAlert error={failure} /> : null}
        <Form layout="vertical" component={false} requiredMark={false}>
          <div>
            <FormField
              id={passwordId}
              label={t("recentAuth.password")}
              error={passwordHelp}
              {...(requireCode ? {} : { style: { marginBottom: 0 } })}
            >
              {(aria) => (
                <Controller
                  name="password"
                  control={control}
                  render={({ field }) => (
                    <Input.Password {...field} {...aria} autoComplete="current-password" />
                  )}
                />
              )}
            </FormField>
            {requireCode ? (
              <FormField
                id={codeId}
                label={t("recentAuth.code")}
                error={errors.totpCode?.message}
                extra={t("recentAuth.codeHelp")}
                style={{ marginBottom: 0 }}
              >
                {(aria) => (
                  <Controller
                    name="totpCode"
                    control={control}
                    render={({ field }) => (
                      <OneTimeCodeInput {...field} {...aria} kind="totp" size="middle" />
                    )}
                  />
                )}
              </FormField>
            ) : null}
          </div>
        </Form>
        <Flex justify="end" gap={token.marginXS} style={{ marginTop: token.marginXS }}>
          <Button onClick={onCancel}>{t("recentAuth.cancel")}</Button>
          <Button
            type="primary"
            htmlType="submit"
            loading={isSubmitting}
            disabled={retryBlock !== null}
          >
            {t("recentAuth.confirm")}
          </Button>
        </Flex>
      </Flex>
    </form>
  );
}

function retryAfter(seconds: number | null): boolean {
  return seconds !== null && seconds > 0;
}
