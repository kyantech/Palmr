import { zodResolver } from "@hookform/resolvers/zod";
import { Alert, Button, Flex, Form, Input, theme } from "antd";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { z } from "zod";
import { ErrorAlert, presentError } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";
import { FormField } from "../../../shared/ui/FormField";
import { useForgotPassword } from "../api/mutations";

interface ForgotValues {
  identifier: string;
}

interface ForgotPasswordProps {
  onBack: () => void;
}

export function ForgotPassword({ onBack }: ForgotPasswordProps) {
  const { t } = useTranslation("auth");
  const { token } = theme.useToken();
  const idPrefix = useId();
  const inFlight = useRef(false);
  const [requested, setRequested] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const [retryBlock, setRetryBlock] = useState<{ seconds: number } | null>(null);
  const forgot = useForgotPassword();
  const schema = useMemo(
    () => z.object({ identifier: z.string().trim().min(1, t("forgot.identifierRequired")) }),
    [t],
  );
  const {
    control,
    handleSubmit,
    setFocus,
    formState: { errors, isSubmitting },
  } = useForm<ForgotValues>({ resolver: zodResolver(schema), defaultValues: { identifier: "" } });

  useEffect(() => {
    if (!requested) {
      setFocus("identifier");
    }
  }, [requested, setFocus]);

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

  async function submit({ identifier }: ForgotValues) {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setFailure(null);
    try {
      await forgot.mutateAsync({ identifier: identifier.trim() });
      setRequested(true);
    } catch (error) {
      const presented = presentError(error);
      if (presented.presentation.silent) {
        return;
      }
      setFailure(error);
      if (presented.retryAfterSeconds !== null && presented.retryAfterSeconds > 0) {
        setRetryBlock({ seconds: presented.retryAfterSeconds });
      }
    } finally {
      inFlight.current = false;
    }
  }

  if (requested) {
    return (
      <>
        <AuthHeading title={t("forgot.sentTitle")} description={t("forgot.sentDescription")} />
        <Flex vertical gap={token.margin}>
          <Alert
            type="info"
            showIcon
            role="status"
            data-testid="forgot-password-sent"
            title={t("forgot.sentNotice")}
          />
          <Button size="large" block onClick={onBack}>
            {t("forgot.back")}
          </Button>
        </Flex>
      </>
    );
  }

  return (
    <>
      <AuthHeading title={t("forgot.title")} description={t("forgot.description")} />
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
            <div style={{ marginBottom: token.marginLG }}>
              <ErrorAlert error={failure} />
            </div>
          )}
          <FormField
            id={`${idPrefix}-identifier`}
            label={t("forgot.identifier")}
            error={errors.identifier?.message}
          >
            {(aria) => (
              <Controller
                name="identifier"
                control={control}
                render={({ field }) => (
                  <Input
                    {...field}
                    {...aria}
                    size="large"
                    autoComplete="username"
                    autoCapitalize="none"
                    spellCheck={false}
                    readOnly={isSubmitting}
                  />
                )}
              />
            )}
          </FormField>
          <Flex vertical gap={token.marginSM} style={{ marginTop: token.marginXS }}>
            <Button
              type="primary"
              htmlType="submit"
              size="large"
              block
              loading={isSubmitting}
              disabled={retryBlock !== null}
            >
              {t("forgot.submit")}
            </Button>
            <Button type="text" block onClick={onBack} disabled={isSubmitting}>
              {t("forgot.back")}
            </Button>
          </Flex>
        </Form>
      </form>
    </>
  );
}
