import { Alert, Button, Flex, Form, QRCode, Skeleton, theme, Typography } from "antd";
import { type ReactNode, useCallback, useEffect, useId, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { ErrorAlert, type ErrorCode, presentError } from "../../../shared/errors";
import { formatDateTime } from "../../../shared/format/dateTime";
import { FormField } from "../../../shared/ui/FormField";
import {
  type BackupCodes,
  type Enrollment,
  useStartEnrollment,
  useVerifyEnrollment,
} from "../api/mutations";
import { BackupCodesPanel } from "./BackupCodesPanel";
import { compactCode, groupSecret, isTotpCode } from "./codeFormat";
import { OneTimeCodeInput } from "./OneTimeCodeInput";

type Phase =
  | { kind: "starting" }
  | { kind: "idle"; failure: unknown }
  | { kind: "provisioned"; enrollment: Enrollment }
  | { kind: "expired" }
  | { kind: "codes"; codes: BackupCodes }
  | { kind: "done" };

const CODE_ERRORS: readonly ErrorCode[] = ["AUTH_2FA_INVALID", "TOTP_CODE_REPLAYED"];

function isRecentAuthChallenge(error: unknown): boolean {
  return presentError(error).code === "AUTH_RECENT_AUTH_REQUIRED";
}

export interface TwoFactorSetupProps {
  appName: string;
  finishLabel: string;
  onVerified?: () => Promise<void>;
  onFinished: () => Promise<void> | void;
  onCancel?: () => void;
}

export function TwoFactorSetup({
  appName,
  finishLabel,
  onVerified,
  onFinished,
  onCancel,
}: TwoFactorSetupProps) {
  const { t } = useTranslation("auth");
  const [phase, setPhase] = useState<Phase>({ kind: "starting" });
  const started = useRef(false);
  const start = useStartEnrollment((enrollment) => {
    setPhase({ kind: "provisioned", enrollment });
  });
  const expire = useCallback(() => {
    setPhase({ kind: "expired" });
  }, []);

  const begin = () => {
    setPhase({ kind: "starting" });
    start.mutate(undefined, {
      onError: (error) => {
        setPhase({ kind: "idle", failure: isRecentAuthChallenge(error) ? null : error });
      },
    });
  };

  useEffect(() => {
    if (!started.current) {
      started.current = true;
      begin();
    }
  });

  const cancel =
    onCancel === undefined ? null : (
      <Button onClick={onCancel} disabled={start.isPending}>
        {t("setup.cancel")}
      </Button>
    );

  switch (phase.kind) {
    case "starting":
      return (
        <Flex vertical gap={16} aria-busy="true" data-testid="two-factor-setup-starting">
          <Typography.Text type="secondary">{t("setup.preparing")}</Typography.Text>
          <Skeleton active paragraph={{ rows: 4 }} title={false} />
        </Flex>
      );
    case "idle":
      return (
        <Flex vertical gap={16}>
          {phase.failure === null ? (
            <Typography.Text type="secondary">{t("setup.notStarted")}</Typography.Text>
          ) : (
            <ErrorAlert error={phase.failure} />
          )}
          <Flex gap={8} wrap>
            <Button type="primary" onClick={begin}>
              {t("setup.start")}
            </Button>
            {cancel}
          </Flex>
        </Flex>
      );
    case "expired":
      return (
        <Flex vertical gap={16}>
          <Alert
            type="warning"
            showIcon
            role="status"
            title={t("setup.expiredTitle")}
            description={t("setup.expiredDescription")}
          />
          <Flex gap={8} wrap>
            <Button type="primary" onClick={begin}>
              {t("setup.restart")}
            </Button>
            {cancel}
          </Flex>
        </Flex>
      );
    case "done":
      return <Skeleton active paragraph={{ rows: 2 }} title={false} />;
    case "codes":
      return (
        <BackupCodesPanel
          codes={phase.codes.backupCodes}
          appName={appName}
          doneLabel={finishLabel}
          onDone={() => {
            setPhase({ kind: "done" });
            void onFinished();
          }}
        />
      );
    case "provisioned":
      return (
        <ProvisionedEnrollment
          key={phase.enrollment.enrollmentId}
          enrollment={phase.enrollment}
          cancel={cancel}
          onExpired={expire}
          onVerified={(codes) => {
            setPhase({ kind: "codes", codes });
            void onVerified?.();
          }}
        />
      );
  }
}

interface ProvisionedEnrollmentProps {
  enrollment: Enrollment;
  cancel: ReactNode;
  onExpired: () => void;
  onVerified: (codes: BackupCodes) => void;
}

function ProvisionedEnrollment({
  enrollment,
  cancel,
  onExpired,
  onVerified,
}: ProvisionedEnrollmentProps) {
  const { t, i18n } = useTranslation(["auth", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const qrLabelId = `${idPrefix}-qr`;
  const codeId = `${idPrefix}-code`;
  const inFlight = useRef(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [keyCopied, setKeyCopied] = useState(false);
  const verify = useVerifyEnrollment(onVerified);
  const {
    control,
    handleSubmit,
    setError,
    resetField,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<{ code: string }>({ defaultValues: { code: "" } });

  useEffect(() => {
    const remaining = Date.parse(enrollment.expiresAt) - Date.now();
    if (!Number.isFinite(remaining) || remaining <= 0) {
      return;
    }
    const timer = window.setTimeout(onExpired, remaining);
    return () => {
      window.clearTimeout(timer);
    };
  }, [enrollment.expiresAt, onExpired]);

  useEffect(() => {
    if (!keyCopied) {
      return;
    }
    const timer = window.setTimeout(() => {
      setKeyCopied(false);
    }, 2_500);
    return () => {
      window.clearTimeout(timer);
    };
  }, [keyCopied]);

  async function submit({ code }: { code: string }) {
    if (inFlight.current) {
      return;
    }
    if (!isTotpCode(code)) {
      setError("code", {
        type: "validate",
        message: t(
          compactCode(code) === "" ? "secondFactor.codeRequired" : "secondFactor.codeFormat",
        ),
      });
      setFocus("code");
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      await verify.mutateAsync({ enrollmentId: enrollment.enrollmentId, code: compactCode(code) });
    } catch (error) {
      const presented = presentError(error);
      if (presented.presentation.silent || presented.code === "AUTH_RECENT_AUTH_REQUIRED") {
        return;
      }
      if (presented.code === "TOTP_ENROLLMENT_PENDING_MISSING") {
        onExpired();
        return;
      }
      resetField("code");
      if (presented.code !== null && (CODE_ERRORS as readonly string[]).includes(presented.code)) {
        setError("code", {
          type: "server",
          message: t(presented.presentation.i18nKey, { ns: "errors" }),
        });
      } else {
        setFailure(error);
      }
      setFocus("code");
    } finally {
      inFlight.current = false;
    }
  }

  const stepTitle = {
    level: 3 as const,
    style: {
      margin: 0,
      fontSize: token.fontSize,
      lineHeight: token.lineHeight,
      fontWeight: token.fontWeightStrong,
    },
  };

  return (
    <Flex vertical gap={token.marginLG} data-testid="two-factor-setup">
      <section aria-labelledby={`${idPrefix}-scan`}>
        <Typography.Title id={`${idPrefix}-scan`} {...stepTitle}>
          {t("setup.scanTitle")}
        </Typography.Title>
        <Typography.Paragraph type="secondary" style={{ marginBlock: token.marginXXS }}>
          {t("setup.scanDescription")}
        </Typography.Paragraph>
        <Flex gap={token.marginLG} wrap align="center" style={{ marginTop: token.marginSM }}>
          <figure
            role="img"
            aria-labelledby={qrLabelId}
            style={{ margin: 0, flex: "none", lineHeight: 0 }}
          >
            <QRCode
              value={enrollment.otpauthUri}
              type="svg"
              size={176}
              bordered
              color={token.colorText}
              bgColor={token.colorBgContainer}
            />
            <figcaption id={qrLabelId} style={{ display: "none" }}>
              {t("setup.qrLabel")}
            </figcaption>
          </figure>
          <Flex vertical gap={token.marginXS} style={{ flex: "1 1 12rem", minWidth: 0 }}>
            <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
              {t("setup.manualKeyLabel")}
            </Typography.Text>
            <Typography.Text
              data-testid="two-factor-secret"
              translate="no"
              style={{
                fontFamily: token.fontFamilyCode,
                fontSize: token.fontSize,
                letterSpacing: "0.04em",
                wordBreak: "normal",
                overflowWrap: "anywhere",
                padding: `${String(token.paddingXS)}px ${String(token.paddingSM)}px`,
                background: token.colorFillQuaternary,
                border: `${String(token.lineWidth)}px ${token.lineType} ${token.colorBorderSecondary}`,
                borderRadius: token.borderRadius,
              }}
            >
              {groupSecret(enrollment.secretBase32)}
            </Typography.Text>
            <div>
              <Button
                size="small"
                aria-label={t("setup.copyKeyLabel")}
                onClick={() => {
                  void navigator.clipboard
                    .writeText(enrollment.secretBase32)
                    .then(() => {
                      setKeyCopied(true);
                    })
                    .catch(() => undefined);
                }}
              >
                {t(keyCopied ? "setup.keyCopied" : "setup.copyKey")}
              </Button>
            </div>
          </Flex>
        </Flex>
      </section>
      <section aria-labelledby={`${idPrefix}-verify`}>
        <Typography.Title id={`${idPrefix}-verify`} {...stepTitle}>
          {t("setup.verifyTitle")}
        </Typography.Title>
        <Typography.Paragraph type="secondary" style={{ marginBlock: token.marginXXS }}>
          {t("setup.verifyDescription")}
        </Typography.Paragraph>
        <form
          noValidate
          aria-busy={isSubmitting}
          style={{ marginTop: token.marginSM }}
          onSubmit={(event) => {
            void handleSubmit(submit)(event);
          }}
        >
          <Form layout="vertical" component={false} requiredMark={false}>
            {failure === null ? null : (
              <div style={{ marginBottom: token.margin }}>
                <ErrorAlert error={failure} />
              </div>
            )}
            <FormField id={codeId} label={t("setup.codeLabel")} error={errors.code?.message}>
              {(aria) => (
                <Controller
                  name="code"
                  control={control}
                  render={({ field }) => (
                    <div style={{ maxWidth: 240 }}>
                      <OneTimeCodeInput
                        {...field}
                        {...aria}
                        kind="totp"
                        size="middle"
                        readOnly={isSubmitting}
                      />
                    </div>
                  )}
                />
              )}
            </FormField>
            <Flex gap={token.marginXS} wrap>
              <Button type="primary" htmlType="submit" loading={isSubmitting}>
                {t("setup.verify")}
              </Button>
              {cancel}
            </Flex>
          </Form>
        </form>
      </section>
      <Typography.Text type="secondary" style={{ fontSize: token.fontSizeSM }}>
        {t("setup.expiresAt", { date: formatDateTime(enrollment.expiresAt, i18n.language) })}
      </Typography.Text>
    </Flex>
  );
}
