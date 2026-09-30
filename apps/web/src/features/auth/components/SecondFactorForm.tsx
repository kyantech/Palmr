import { Button, Checkbox, Flex, Form, theme, Typography } from "antd";
import { useEffect, useId, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { ErrorAlert, type ErrorCode, presentError } from "../../../shared/errors";
import { FormField } from "../../../shared/ui/FormField";
import { useLoginSecondFactor } from "../api/mutations";
import { clearMfaChallenge, type MfaChallenge, setLoginNotice } from "../store";
import { compactCode, isBackupCode, isTotpCode } from "./codeFormat";
import { type CodeKind, OneTimeCodeInput } from "./OneTimeCodeInput";

interface SecondFactorValues {
  code: string;
  rememberDevice: boolean;
}

const FIELD_CODES: readonly ErrorCode[] = [
  "AUTH_2FA_INVALID",
  "BACKUP_CODE_INVALID",
  "TOTP_CODE_REPLAYED",
];

function expireChallenge() {
  setLoginNotice("challengeExpired");
  clearMfaChallenge();
}

interface SecondFactorFormProps {
  challenge: MfaChallenge;
  onSignedIn: () => Promise<void>;
}

export function SecondFactorForm({ challenge, onSignedIn }: SecondFactorFormProps) {
  const { t } = useTranslation(["auth", "errors"]);
  const { token } = theme.useToken();
  const idPrefix = useId();
  const codeId = `${idPrefix}-code`;
  const rememberId = `${idPrefix}-remember`;
  const inFlight = useRef(false);
  const [kind, setKind] = useState<CodeKind>("totp");
  const [failure, setFailure] = useState<unknown>(null);
  const [retryBlock, setRetryBlock] = useState<{ seconds: number } | null>(null);
  const [rememberAvailable, setRememberAvailable] = useState(challenge.trustedDeviceOffered);
  const secondFactor = useLoginSecondFactor();
  const backupAllowed = challenge.methods.includes("backup_code");
  const {
    control,
    handleSubmit,
    setError,
    clearErrors,
    resetField,
    setFocus,
    setValue,
    formState: { errors, isSubmitting },
  } = useForm<SecondFactorValues>({ defaultValues: { code: "", rememberDevice: false } });

  useEffect(() => {
    setFocus("code");
  }, [setFocus, kind]);

  useEffect(() => {
    const timer = window.setTimeout(expireChallenge, Math.max(0, challenge.deadline - Date.now()));
    return () => {
      window.clearTimeout(timer);
    };
  }, [challenge.deadline]);

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

  function switchKind(next: CodeKind) {
    setKind(next);
    setFailure(null);
    clearErrors();
    resetField("code");
  }

  function validate(code: string): string | null {
    if (compactCode(code) === "") {
      return t(kind === "totp" ? "secondFactor.codeRequired" : "secondFactor.backupRequired");
    }
    const valid = kind === "totp" ? isTotpCode(code) : isBackupCode(code);
    return valid
      ? null
      : t(kind === "totp" ? "secondFactor.codeFormat" : "secondFactor.backupFormat");
  }

  function applyError(error: unknown) {
    const presented = presentError(error);
    if (presented.presentation.silent) {
      return;
    }
    switch (presented.code) {
      case "AUTH_2FA_CHALLENGE_EXPIRED":
        expireChallenge();
        return;
      case "TRUSTED_DEVICE_DISABLED":
        setRememberAvailable(false);
        setValue("rememberDevice", false);
        setFailure(error);
        return;
      default:
        break;
    }
    if (presented.code !== null && (FIELD_CODES as readonly string[]).includes(presented.code)) {
      resetField("code");
      setError("code", {
        type: "server",
        message: t(presented.presentation.i18nKey, { ns: "errors" }),
      });
      setFocus("code");
      return;
    }
    setFailure(error);
    if (presented.retryAfterSeconds !== null && presented.retryAfterSeconds > 0) {
      setRetryBlock({ seconds: presented.retryAfterSeconds });
    }
    resetField("code");
    setFocus("code");
  }

  async function submit({ code, rememberDevice }: SecondFactorValues) {
    if (inFlight.current) {
      return;
    }
    const problem = validate(code);
    if (problem !== null) {
      setError("code", { type: "validate", message: problem });
      setFocus("code");
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      let outcome;
      try {
        outcome = await secondFactor.mutateAsync({
          code: compactCode(code),
          rememberDevice: rememberAvailable && rememberDevice,
        });
      } catch (error) {
        applyError(error);
        return;
      }
      if (outcome === "challengeMissing") {
        expireChallenge();
        return;
      }
      try {
        await onSignedIn();
      } finally {
        clearMfaChallenge();
      }
    } catch (error) {
      setFailure(error);
    } finally {
      inFlight.current = false;
    }
  }

  return (
    <form
      noValidate
      aria-busy={isSubmitting}
      onSubmit={(event) => {
        if (retryBlock === null) {
          void handleSubmit(submit)(event);
        } else {
          event.preventDefault();
        }
      }}
    >
      <Form layout="vertical" component={false} requiredMark={false}>
        {failure === null ? null : (
          <div style={{ marginBottom: token.marginLG }} data-testid="second-factor-error">
            <ErrorAlert error={failure} />
          </div>
        )}
        <FormField
          id={codeId}
          label={t(kind === "totp" ? "secondFactor.codeLabel" : "secondFactor.backupLabel")}
          error={errors.code?.message}
          extra={t(kind === "totp" ? "secondFactor.codeHelp" : "secondFactor.backupHelp")}
        >
          {(aria) => (
            <Controller
              name="code"
              control={control}
              render={({ field }) => (
                <OneTimeCodeInput {...field} {...aria} kind={kind} readOnly={isSubmitting} />
              )}
            />
          )}
        </FormField>
        {rememberAvailable ? (
          <Flex
            vertical
            gap={2}
            style={{
              marginTop: errors.code === undefined ? 0 : token.marginSM,
              marginBottom: token.marginLG,
            }}
          >
            <Controller
              name="rememberDevice"
              control={control}
              render={({ field: { value, onChange, onBlur, ref } }) => (
                <Checkbox
                  ref={ref}
                  id={rememberId}
                  checked={value}
                  onChange={(event) => {
                    onChange(event.target.checked);
                  }}
                  onBlur={onBlur}
                  aria-describedby={`${rememberId}-help`}
                  disabled={isSubmitting}
                >
                  {t("secondFactor.remember")}
                </Checkbox>
              )}
            />
            <Typography.Text
              id={`${rememberId}-help`}
              type="secondary"
              style={{ fontSize: token.fontSizeSM, paddingInlineStart: token.paddingLG }}
            >
              {t("secondFactor.rememberHelp")}
            </Typography.Text>
          </Flex>
        ) : null}
        <Flex vertical gap={token.marginSM}>
          <Button
            type="primary"
            htmlType="submit"
            size="large"
            block
            loading={isSubmitting}
            disabled={retryBlock !== null}
          >
            {t("secondFactor.submit")}
          </Button>
          {backupAllowed ? (
            <Button
              type="link"
              block
              disabled={isSubmitting}
              onClick={() => {
                switchKind(kind === "totp" ? "backup" : "totp");
              }}
            >
              {t(kind === "totp" ? "secondFactor.useBackup" : "secondFactor.useAuthenticator")}
            </Button>
          ) : null}
        </Flex>
      </Form>
    </form>
  );
}
