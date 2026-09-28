import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Input, Modal, theme, Typography } from "antd";
import { Suspense, useEffect, useId, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import type { components } from "../../../shared/api/schema";
import { ErrorAlert, presentError, useErrorMessage } from "../../../shared/errors";
import { useReauthenticate } from "../api/mutations";
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

const passwordSchema = z.object({ password: z.string().min(1) });

type PasswordValues = z.infer<typeof passwordSchema>;

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
        <Typography.Text>
          {t(
            method === "password" ? "recentAuth.description" : "recentAuth.unavailableDescription",
          )}
        </Typography.Text>
        <Typography.Text type="secondary" style={{ fontSize: "0.8125rem" }}>
          {t("recentAuth.signedInAs", { account: me.user.email })}
        </Typography.Text>
      </Flex>
      {method === "password" ? (
        <PasswordChallenge challenge={challenge} onCancel={onCancel} />
      ) : (
        <UnavailableChallenge method={method} onCancel={onCancel} />
      )}
    </Flex>
  );
}

function UnavailableChallenge({
  method,
  onCancel,
}: {
  method: Exclude<RecentAuthMethod, "password">;
  onCancel: () => void;
}) {
  const { t } = useTranslation("auth");
  const copy = method === "twoFactor" ? "twoFactorUnavailable" : "externalUnavailable";
  return (
    <>
      <Alert
        type="info"
        showIcon
        data-recent-auth-method={method}
        title={t(`recentAuth.${copy}.title`)}
        description={t(`recentAuth.${copy}.description`)}
      />
      <Flex justify="end">
        <Button onClick={onCancel}>{t("recentAuth.close")}</Button>
      </Flex>
    </>
  );
}

function PasswordChallenge({
  challenge,
  onCancel,
}: {
  challenge: RecentAuthChallenge;
  onCancel: () => void;
}) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const passwordId = useId();
  const helpId = useId();
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [retryBlock, setRetryBlock] = useState<{ seconds: number } | null>(null);
  const reauthenticate = useReauthenticate();
  const {
    control,
    handleSubmit,
    resetField,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<PasswordValues>({
    resolver: zodResolver(passwordSchema),
    defaultValues: { password: "" },
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

  async function confirm({ password }: PasswordValues) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      await reauthenticate.mutateAsync({ password });
    } catch (error) {
      const presented = presentError(error);
      if (presented.presentation.silent) {
        return;
      }
      setFailure(error);
      if (presented.retryAfterSeconds !== null && presented.retryAfterSeconds > 0) {
        setRetryBlock({ seconds: presented.retryAfterSeconds });
      }
      resetField("password");
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
  const fieldHelp =
    errors.password !== undefined
      ? t("recentAuth.passwordRequired")
      : inlineFailure
        ? failureText
        : null;

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
          <Form.Item
            label={t("recentAuth.password")}
            htmlFor={passwordId}
            {...(fieldHelp === null
              ? {}
              : { validateStatus: "error", help: <span id={helpId}>{fieldHelp}</span> })}
            style={{ marginBottom: 0 }}
          >
            <Controller
              name="password"
              control={control}
              render={({ field }) => (
                <Input.Password
                  {...field}
                  id={passwordId}
                  autoComplete="current-password"
                  aria-invalid={fieldHelp !== null}
                  aria-describedby={fieldHelp === null ? undefined : helpId}
                />
              )}
            />
          </Form.Item>
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
